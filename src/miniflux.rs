use crate::app::FeedUpdate;
use crate::config::BackendConfig;
use crate::feeds::{build_client, Feed, FeedEntry, RemoteFeedRef};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::mpsc;

const PAGE_LIMIT: i64 = 100;

#[derive(Debug, Deserialize)]
struct MfCategory {
  title: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MfFeed {
  id: i64,
  title: String,
  feed_url: String,
  category: MfCategory,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MfEntry {
  id: i64,
  feed_id: i64,
  title: String,
  url: String,
  content: String,
  published_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MfEntriesResponse {
  total: i64,
  entries: Vec<MfEntry>,
}

pub struct MinifluxClient {
  base_url: String,
  token: String,
  client: Client,
}

impl MinifluxClient {
  pub fn new(base_url: String, token: String) -> Self {
    Self {
      base_url: base_url.trim_end_matches('/').to_string(),
      token,
      client: build_client(),
    }
  }

  fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
    self
      .client
      .request(method, format!("{}{path}", self.base_url))
      .header("X-Auth-Token", &self.token)
  }

  pub(crate) async fn fetch_feeds(&self) -> Result<Vec<MfFeed>, String> {
    let resp = self
      .request(reqwest::Method::GET, "/v1/feeds")
      .send()
      .await
      .map_err(|e| format!("failed to fetch feeds: {e}"))?;
    let body = resp
      .text()
      .await
      .map_err(|e| format!("failed to read feeds response: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("failed to parse feeds response: {e}"))
  }

  /// Fetch every entry across all feeds, paginating until the server's
  /// reported `total` is reached. Does not filter by status — a read-filtered
  /// fetch would never clear previously-synced entries, since `save_feed`'s
  /// upsert has no "missing from this sync ⇒ delete" semantics.
  pub(crate) async fn fetch_all_entries(&self) -> Result<Vec<MfEntry>, String> {
    let mut all = Vec::new();
    let mut offset = 0i64;
    loop {
      let resp = self
        .request(
          reqwest::Method::GET,
          &format!("/v1/entries?limit={PAGE_LIMIT}&offset={offset}"),
        )
        .send()
        .await
        .map_err(|e| format!("failed to fetch entries: {e}"))?;
      let body = resp
        .text()
        .await
        .map_err(|e| format!("failed to read entries response: {e}"))?;
      let page: MfEntriesResponse = serde_json::from_str(&body)
        .map_err(|e| format!("failed to parse entries response: {e}"))?;
      let got = page.entries.len() as i64;
      all.extend(page.entries);
      offset += got;
      if got < PAGE_LIMIT || offset >= page.total {
        break;
      }
    }
    Ok(all)
  }

  /// Push a read/unread status update for the given remote entry ids.
  pub async fn update_entries_status(&self, ids: &[String], status: &str) -> Result<(), String> {
    if ids.is_empty() {
      return Ok(());
    }
    let ids: Vec<i64> = ids.iter().filter_map(|id| id.parse().ok()).collect();
    let body = serde_json::json!({ "entry_ids": ids, "status": status }).to_string();
    let resp = self
      .request(reqwest::Method::PUT, "/v1/entries")
      .header("Content-Type", "application/json")
      .body(body)
      .send()
      .await
      .map_err(|e| format!("failed to update entry status: {e}"))?;
    if !resp.status().is_success() {
      return Err(format!(
        "miniflux returned {} updating entry status",
        resp.status()
      ));
    }
    Ok(())
  }
}

/// Position bucket reserved for remote-backend feeds, chosen well above any
/// realistic feeds.toml length so remote feeds always sort after local ones.
/// Multiple configured backend instances currently share this one bucket
/// (their feeds can tie and fall back to arbitrary relative order against
/// each other), but ordering within one backend's own feed list is stable
/// across syncs — see `build_feeds`'s sort below.
const REMOTE_POSITION_BASE: usize = 1_000_000;

/// Group `entries` by Miniflux feed id and build one `feeds::Feed` per
/// Miniflux feed (paired with its intended cache `position`), tagging each
/// with its category and the given `backend_id`.
fn build_feeds(
  backend_id: &str,
  mf_feeds: Vec<MfFeed>,
  mf_entries: Vec<MfEntry>,
) -> Vec<(Feed, usize)> {
  let mut by_feed: std::collections::HashMap<i64, Vec<MfEntry>> = std::collections::HashMap::new();
  for entry in mf_entries {
    by_feed.entry(entry.feed_id).or_default().push(entry);
  }

  let mut feeds: Vec<(String, Feed)> = mf_feeds
    .into_iter()
    .map(|mf_feed| {
      let category_title = mf_feed.category.title.clone();
      let entries = by_feed
        .remove(&mf_feed.id)
        .unwrap_or_default()
        .into_iter()
        .map(|e| {
          // Miniflux's `content` is always sanitized HTML (unlike local
          // RSS/Atom bodies, which are checked with `looks_like_html` since
          // they're sometimes plain text) — always run it through the same
          // HTML→markdown conversion `parse_single_feed` uses, resolving
          // relative links against the entry's own URL.
          let converted = crate::content::html::html_to_markdown(&e.content, Some(&e.url));
          let mut links = vec![e.url];
          for link in converted.links {
            if !links.contains(&link) {
              links.push(link);
            }
          }
          FeedEntry {
            title: e.title,
            published: e.published_at,
            text: converted.markdown,
            links,
            remote_id: Some(e.id.to_string()),
            ..Default::default()
          }
        })
        .collect();

      let feed = Feed {
        url: mf_feed.feed_url,
        title: mf_feed.title,
        entries,
        tags: Some(vec![category_title.clone()]),
        backend: Some(RemoteFeedRef {
          backend_id: backend_id.to_string(),
          remote_feed_id: mf_feed.id.to_string(),
        }),
      };
      (category_title, feed)
    })
    .collect();

  // Stable sort by (category, title) so feed order doesn't shuffle across
  // syncs just because Miniflux's own array order isn't stable.
  feeds.sort_by(|(cat_a, feed_a), (cat_b, feed_b)| {
    cat_a
      .cmp(cat_b)
      .then_with(|| feed_a.title.cmp(&feed_b.title))
  });
  feeds
    .into_iter()
    .enumerate()
    .map(|(ordinal, (_, feed))| (feed, REMOTE_POSITION_BASE + ordinal))
    .collect()
}

/// Spawn one background sync task per enabled, configured backend instance.
/// Each task streams its own `FeedUpdate`s (including its own
/// `FetchComplete`) through `tx`; `App::handle_feed_update` already treats
/// repeated `FetchComplete`s as idempotent. This is the single dispatch
/// point that would grow a `match` arm for a future backend type — call
/// sites (`main.rs` startup, `cmd_refresh`, `App::refresh_feeds`) never need
/// to change when one is added.
pub fn spawn_backend_syncs(backends: &[BackendConfig], tx: mpsc::UnboundedSender<FeedUpdate>) {
  for backend in backends.iter().filter(|b| b.enabled).cloned() {
    let tx = tx.clone();
    tokio::spawn(async move {
      match &backend.kind {
        crate::config::BackendKind::Miniflux { .. } => {
          sync_with_progress(&backend, tx).await;
        }
      }
    });
  }
}

/// Sync every feed/entry for one configured Miniflux backend instance,
/// streaming results through `tx` exactly like the local RSS fetch path:
/// one `FeedUpdate::UpdateSingle` per feed, always ending `FetchComplete`.
/// Any HTTP/auth/parse failure becomes a single `FeedUpdate::FeedError`
/// (one HTTP origin, not per-feed).
pub async fn sync_with_progress(backend: &BackendConfig, tx: mpsc::UnboundedSender<FeedUpdate>) {
  let backend_id = backend.id.clone();
  let crate::config::BackendKind::Miniflux { url, .. } = &backend.kind;
  let url = url.clone();

  let _ = tx.send(FeedUpdate::FetchingFeed(backend_id.clone()));

  let token = match backend.resolve_token() {
    Ok(t) => t,
    Err(err) => {
      let _ = tx.send(FeedUpdate::FeedError {
        name: backend_id,
        error: err,
      });
      let _ = tx.send(FeedUpdate::FetchComplete);
      return;
    }
  };

  let client = MinifluxClient::new(url, token);

  let mf_feeds = match client.fetch_feeds().await {
    Ok(f) => f,
    Err(err) => {
      let _ = tx.send(FeedUpdate::FeedError {
        name: backend_id,
        error: err,
      });
      let _ = tx.send(FeedUpdate::FetchComplete);
      return;
    }
  };

  let _ = tx.send(FeedUpdate::BackendFeedList {
    backend_id: backend_id.clone(),
    remote_feed_ids: mf_feeds.iter().map(|f| f.id.to_string()).collect(),
  });

  let mf_entries = match client.fetch_all_entries().await {
    Ok(e) => e,
    Err(err) => {
      let _ = tx.send(FeedUpdate::FeedError {
        name: backend_id,
        error: err,
      });
      let _ = tx.send(FeedUpdate::FetchComplete);
      return;
    }
  };

  for (feed, position) in build_feeds(&backend_id, mf_feeds, mf_entries) {
    let _ = tx.send(FeedUpdate::UpdateSingleRemote { feed, position });
  }
  let _ = tx.send(FeedUpdate::FetchComplete);
}

#[cfg(test)]
mod tests {
  use super::*;

  fn mf_feed(id: i64, title: &str, feed_url: &str, category: &str) -> MfFeed {
    MfFeed {
      id,
      title: title.to_string(),
      feed_url: feed_url.to_string(),
      category: MfCategory {
        title: category.to_string(),
      },
    }
  }

  fn mf_entry(id: i64, feed_id: i64, title: &str) -> MfEntry {
    MfEntry {
      id,
      feed_id,
      title: title.to_string(),
      url: format!("https://example.com/{id}"),
      content: "<p>content</p>".to_string(),
      published_at: Some("2024-01-01T00:00:00Z".to_string()),
    }
  }

  #[test]
  fn test_build_feeds_groups_entries_by_feed_id() {
    let feeds = vec![
      mf_feed(1, "Feed A", "https://a.example.com/rss", "Tech"),
      mf_feed(2, "Feed B", "https://b.example.com/rss", "News"),
    ];
    let entries = vec![
      mf_entry(10, 1, "A1"),
      mf_entry(11, 1, "A2"),
      mf_entry(12, 2, "B1"),
    ];

    let result = build_feeds("home-miniflux", feeds, entries);
    assert_eq!(result.len(), 2);

    let (feed_a, pos_a) = result.iter().find(|(f, _)| f.title == "Feed A").unwrap();
    assert_eq!(feed_a.entries.len(), 2);
    assert_eq!(feed_a.tags.as_deref(), Some(&["Tech".to_string()][..]));
    assert_eq!(feed_a.backend.as_ref().unwrap().remote_feed_id, "1");
    assert_eq!(feed_a.backend.as_ref().unwrap().backend_id, "home-miniflux");
    assert_eq!(feed_a.entries[0].remote_id.as_deref(), Some("10"));
    assert!(*pos_a >= REMOTE_POSITION_BASE);
    // `mf_entry`'s content is "<p>content</p>" — must be converted to
    // markdown, not stored as raw HTML (the reader renders this as markdown).
    assert!(!feed_a.entries[0].text.contains('<'));
    assert!(feed_a.entries[0].text.contains("content"));

    let (feed_b, pos_b) = result.iter().find(|(f, _)| f.title == "Feed B").unwrap();
    assert_eq!(feed_b.entries.len(), 1);
    assert_ne!(pos_a, pos_b);
  }

  #[test]
  fn test_build_feeds_feed_with_no_entries() {
    let feeds = vec![mf_feed(
      1,
      "Empty Feed",
      "https://a.example.com/rss",
      "Tech",
    )];
    let result = build_feeds("home-miniflux", feeds, vec![]);
    assert_eq!(result.len(), 1);
    assert!(result[0].0.entries.is_empty());
  }

  #[test]
  fn test_build_feeds_sorted_by_category_then_title() {
    let feeds = vec![
      mf_feed(1, "Zebra", "https://a.example.com/rss", "News"),
      mf_feed(2, "Apple", "https://b.example.com/rss", "News"),
      mf_feed(3, "Middle", "https://c.example.com/rss", "Blogs"),
    ];
    let result = build_feeds("home-miniflux", feeds, vec![]);
    let titles: Vec<&str> = result.iter().map(|(f, _)| f.title.as_str()).collect();
    assert_eq!(titles, vec!["Middle", "Apple", "Zebra"]);
    // Positions are assigned in the same sorted order, strictly increasing.
    let positions: Vec<usize> = result.iter().map(|(_, p)| *p).collect();
    assert_eq!(positions, vec![1_000_000, 1_000_001, 1_000_002]);
  }
}

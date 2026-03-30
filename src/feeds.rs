use crate::app::FeedUpdate;
use crate::config::Feed as FeedConfig;
use feed_rs::parser;
use reqwest::{Error as ReqError, get};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct Feed {
  pub url: String,
  pub title: String,
  pub entries: Vec<FeedEntry>,
  pub tags: Option<Vec<String>>,
}

// ─── Entry ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FeedEntry {
  pub title: String,
  pub published: Option<String>,
  pub text: String,
  pub links: Vec<String>,
  pub media: Option<String>,      // Media attachment URL if present
  pub feed_title: Option<String>, // Source feed title (for query feeds)
  pub read: bool,                 // Whether this entry has been read
}

// ─── Fetching ─────────────────────────────────────────────────────────────────

/// Fetch multiple feeds concurrently with progress reporting
pub async fn fetch_feed_with_progress(
  feeds: Vec<FeedConfig>,
  tx: mpsc::UnboundedSender<FeedUpdate>,
) {
  let mut tasks = Vec::new();

  for feed_config in feeds {
    let tx_clone = tx.clone();
    let task = tokio::spawn(async move {
      let feed_name = feed_config
        .name
        .clone()
        .unwrap_or_else(|| feed_config.link.clone());
      let _ = tx_clone.send(FeedUpdate::FetchingFeed(feed_name.clone()));

      match fetch_single_feed(&feed_config.link).await {
        Ok(body) => match parse_single_feed(feed_config.clone(), &body) {
          Some(feed) => Some(feed),
          None => {
            let _ = tx_clone.send(FeedUpdate::FeedError {
              name: feed_name,
              error: "Failed to parse feed".to_string(),
            });
            None
          }
        },
        Err(err) => {
          let _ = tx_clone.send(FeedUpdate::FeedError {
            name: feed_name,
            error: format!("Failed to fetch: {}", err),
          });
          None
        }
      }
    });
    tasks.push(task);
  }

  let mut feed_list = Vec::new();
  for task in tasks {
    if let Ok(Some(feed)) = task.await {
      feed_list.push(feed);
    }
  }

  let _ = tx.send(FeedUpdate::Replace(feed_list));
  let _ = tx.send(FeedUpdate::FetchComplete);
}

/// Fetch a single feed from URL
async fn fetch_single_feed(url: &str) -> Result<String, ReqError> {
  get(url).await?.text().await
}

/// Fetch a single feed with progress reporting
/// Fetch a subset of feeds concurrently, sending UpdateSingle for each result.
pub async fn fetch_feeds_subset_with_progress(
  feeds: Vec<FeedConfig>,
  tx: mpsc::UnboundedSender<FeedUpdate>,
) {
  let mut tasks = Vec::new();

  for feed_config in feeds {
    let tx_clone = tx.clone();
    let task = tokio::spawn(async move {
      let feed_name = feed_config
        .name
        .clone()
        .unwrap_or_else(|| feed_config.link.clone());
      let _ = tx_clone.send(FeedUpdate::FetchingFeed(feed_name.clone()));

      match fetch_single_feed(&feed_config.link).await {
        Ok(body) => match parse_single_feed(feed_config, &body) {
          Some(feed) => {
            let _ = tx_clone.send(FeedUpdate::UpdateSingle(feed));
          }
          None => {
            let _ = tx_clone.send(FeedUpdate::FeedError {
              name: feed_name,
              error: "Failed to parse feed".to_string(),
            });
          }
        },
        Err(err) => {
          let _ = tx_clone.send(FeedUpdate::FeedError {
            name: feed_name,
            error: format!("Failed to fetch: {}", err),
          });
        }
      }
    });
    tasks.push(task);
  }

  for task in tasks {
    let _ = task.await;
  }
  let _ = tx.send(FeedUpdate::FetchComplete);
}

// ─── Parsing ──────────────────────────────────────────────────────────────────
/// Parse a single feed from its body content
pub fn parse_single_feed(feed_config: FeedConfig, body: &str) -> Option<Feed> {
  let parsed = parser::parse(body.as_bytes()).ok()?;

  let rss_title = parsed.title.map(|t| t.content);
  let title = feed_config
    .name
    .or(rss_title)
    .unwrap_or_else(|| feed_config.link.clone());

  let entries = parsed
    .entries
    .into_iter()
    .map(|e| {
      let entry_title = e
        .title
        .map(|t| t.content)
        .unwrap_or_else(|| "Untitled".to_string());

      let published = e.published.or(e.updated).map(|dt| dt.to_rfc3339());

      let html_content = e
        .content
        .as_ref()
        .and_then(|c| c.body.clone())
        .or_else(|| {
          e.media
            .first()
            .and_then(|m| m.description.as_ref().map(|d| d.content.clone()))
        })
        .or_else(|| e.summary.as_ref().map(|s| s.content.clone()))
        .unwrap_or_default();

      let text = html2text::from_read(html_content.as_bytes(), usize::MAX)
        .unwrap_or_else(|_| String::from("Failed to parse content"));

      let mut links: Vec<_> = e.links.into_iter().map(|l| l.href).collect();

      links.extend(extract_html_links(&html_content));

      let media = e
        .media
        .first()
        .and_then(|m| m.content.first())
        .and_then(|c| c.url.as_ref())
        .map(|u| u.to_string());

      FeedEntry {
        title: entry_title,
        published,
        text,
        links,
        media,
        feed_title: None,
        read: false,
      }
    })
    .collect();

  Some(Feed {
    url: feed_config.link,
    title,
    entries,
    tags: feed_config.tags,
  })
}

fn extract_html_links(html: &str) -> Vec<String> {
  let dom = tl::parse(html, tl::ParserOptions::default()).ok();
  let Some(dom) = dom else { return vec![] };
  let parser = dom.parser();
  dom
    .query_selector("a[href]")
    .into_iter()
    .flatten()
    .filter_map(|h| h.get(parser)?.as_tag())
    .filter_map(|tag| tag.attributes().get("href"))
    .map(|b| b.unwrap().as_utf8_str().into_owned())
    .filter(|href| href.starts_with("http"))
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_extract_html_links_basic() {
    let html = r#"<p>Check <a href="https://example.com">this</a> and <a href="https://other.com/page">that</a></p>"#;
    let links = extract_html_links(html);
    assert_eq!(links.len(), 2);
    assert_eq!(links[0], "https://example.com");
    assert_eq!(links[1], "https://other.com/page");
  }

  #[test]
  fn test_extract_html_links_filters_non_http() {
    let html = r#"<a href="https://example.com">ok</a><a href="mailto:test@test.com">email</a><a href="/relative">rel</a>"#;
    let links = extract_html_links(html);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0], "https://example.com");
  }

  #[test]
  fn test_extract_html_links_empty_html() {
    let links = extract_html_links("");
    assert!(links.is_empty());
  }

  #[test]
  fn test_extract_html_links_no_anchors() {
    let html = "<p>No links here</p>";
    let links = extract_html_links(html);
    assert!(links.is_empty());
  }

  #[test]
  fn test_extract_html_links_anchor_without_href() {
    let html = r#"<a name="anchor">no href</a>"#;
    let links = extract_html_links(html);
    assert!(links.is_empty());
  }

  #[test]
  fn test_parse_single_feed_atom() {
    let atom_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Test Blog</title>
  <entry>
    <title>First Post</title>
    <published>2024-01-15T10:00:00Z</published>
    <content type="html">&lt;p&gt;Hello world&lt;/p&gt;</content>
    <link href="https://example.com/first-post"/>
  </entry>
  <entry>
    <title>Second Post</title>
    <updated>2024-02-20T12:00:00Z</updated>
    <summary type="html">&lt;p&gt;Summary text&lt;/p&gt;</summary>
    <link href="https://example.com/second-post"/>
  </entry>
</feed>"#;

    let config = FeedConfig {
      link: "https://example.com/feed.xml".to_string(),
      name: None,
      tags: Some(vec!["blog".to_string()]),
    };

    let feed = parse_single_feed(config, atom_xml).unwrap();
    assert_eq!(feed.title, "Test Blog");
    assert_eq!(feed.url, "https://example.com/feed.xml");
    assert_eq!(feed.tags.as_ref().unwrap(), &vec!["blog".to_string()]);
    assert_eq!(feed.entries.len(), 2);
    assert_eq!(feed.entries[0].title, "First Post");
    assert!(feed.entries[0].published.is_some());
    assert!(!feed.entries[0].links.is_empty());
  }

  #[test]
  fn test_parse_single_feed_rss() {
    let rss_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>RSS Feed</title>
    <item>
      <title>RSS Post</title>
      <pubDate>Mon, 15 Jan 2024 10:00:00 +0000</pubDate>
      <description>&lt;p&gt;RSS content&lt;/p&gt;</description>
      <link>https://example.com/rss-post</link>
    </item>
  </channel>
</rss>"#;

    let config = FeedConfig {
      link: "https://example.com/rss".to_string(),
      name: None,
      tags: None,
    };

    let feed = parse_single_feed(config, rss_xml).unwrap();
    assert_eq!(feed.title, "RSS Feed");
    assert_eq!(feed.entries.len(), 1);
    assert_eq!(feed.entries[0].title, "RSS Post");
  }

  #[test]
  fn test_parse_single_feed_custom_name_overrides_title() {
    let atom_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Original Title</title>
</feed>"#;

    let config = FeedConfig {
      link: "https://example.com/feed.xml".to_string(),
      name: Some("My Custom Name".to_string()),
      tags: None,
    };

    let feed = parse_single_feed(config, atom_xml).unwrap();
    assert_eq!(feed.title, "My Custom Name");
  }

  #[test]
  fn test_parse_single_feed_falls_back_to_url_when_no_title() {
    // A minimal valid Atom feed without a <title>
    let atom_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>urn:uuid:fake</id>
</feed>"#;

    let config = FeedConfig {
      link: "https://example.com/notitle".to_string(),
      name: None,
      tags: None,
    };

    let feed = parse_single_feed(config, atom_xml).unwrap();
    assert_eq!(feed.title, "https://example.com/notitle");
  }

  #[test]
  fn test_parse_single_feed_invalid_xml() {
    let config = FeedConfig {
      link: "https://example.com/bad".to_string(),
      name: None,
      tags: None,
    };

    let result = parse_single_feed(config, "not xml at all");
    assert!(result.is_none());
  }

  #[test]
  fn test_parse_single_feed_entry_defaults() {
    let atom_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Feed</title>
  <entry>
    <id>urn:uuid:entry1</id>
  </entry>
</feed>"#;

    let config = FeedConfig {
      link: "https://example.com/feed.xml".to_string(),
      name: None,
      tags: None,
    };

    let feed = parse_single_feed(config, atom_xml).unwrap();
    assert_eq!(feed.entries.len(), 1);
    assert_eq!(feed.entries[0].title, "Untitled");
    assert!(feed.entries[0].published.is_none());
    assert!(!feed.entries[0].read);
    assert!(feed.entries[0].feed_title.is_none());
    assert!(feed.entries[0].media.is_none());
  }
}

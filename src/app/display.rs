use super::types::DisplayFeed;
use super::App;
use crate::feeds::FeedEntry;
use crate::query;

impl App {
  /// Build a sorted list of unique tags with their feed counts.
  pub(super) fn build_tag_list(feeds: &[crate::feeds::Feed]) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    let mut tag_counts: HashMap<String, usize> = HashMap::new();

    for feed in feeds {
      if let Some(tags) = &feed.tags {
        for tag in tags {
          *tag_counts.entry(tag.clone()).or_insert(0) += 1;
        }
      }
    }

    let mut tags: Vec<(String, usize)> = tag_counts.into_iter().collect();
    tags.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    tags
  }

  /// Build display feeds from the canonical feed list.
  ///
  /// Regular feeds are stored as plain indices — no Feed data is cloned.
  /// Query feeds must own their aggregated entry list (cross-feed aggregation).
  pub(super) fn build_display_feeds(
    feeds: &[crate::feeds::Feed],
    query_config: &[crate::config::QueryFeed],
  ) -> Vec<DisplayFeed> {
    let mut display_feeds: Vec<DisplayFeed> = query_config
      .iter()
      .map(|qf| DisplayFeed::Query {
        name: qf.name.clone(), // owned String required by the enum variant
        entries: query::apply_query(feeds, &qf.query),
      })
      .collect();

    for i in 0..feeds.len() {
      display_feeds.push(DisplayFeed::Regular(i));
    }

    display_feeds
  }

  /// Build display feeds filtered by a tag query.
  pub(super) fn build_display_feeds_with_tag(
    feeds: &[crate::feeds::Feed],
    _query_config: &[crate::config::QueryFeed],
    tag_query: &str,
  ) -> Vec<DisplayFeed> {
    let filter = query::parse_query(tag_query);
    let matching_feeds: Vec<usize> = feeds
      .iter()
      .enumerate()
      .filter(|(_, f)| {
        if let Some(tags) = &f.tags {
          tags.iter().any(|t| {
            if let query::QueryFilter::Tags(query_tags) = &filter {
              query_tags.iter().any(|qt| qt == t)
            } else {
              false
            }
          })
        } else {
          false
        }
      })
      .map(|(i, _)| i)
      .collect();

    let mut entries: Vec<FeedEntry> = matching_feeds
      .iter()
      .flat_map(|i| {
        feeds
          .get(*i)
          .map(|f| {
            f.entries
              .iter()
              .map(|e| {
                let mut e = e.clone();
                e.feed_title = Some(f.title.clone());
                e
              })
              .collect::<Vec<_>>()
          })
          .unwrap_or_default()
      })
      .collect();

    entries.sort_by(|a, b| match (&b.published, &a.published) {
      (Some(b_date), Some(a_date)) => b_date.cmp(a_date),
      (Some(_), None) => std::cmp::Ordering::Less,
      (None, Some(_)) => std::cmp::Ordering::Greater,
      (None, None) => std::cmp::Ordering::Equal,
    });

    let tag_name = tag_query
      .strip_prefix("tags:")
      .map(|s| s.to_string())
      .unwrap_or_else(|| tag_query.to_string());

    let mut display_feeds: Vec<DisplayFeed> = vec![DisplayFeed::Query {
      name: tag_name,
      entries,
    }];

    for i in 0..feeds.len() {
      display_feeds.push(DisplayFeed::Regular(i));
    }

    display_feeds
  }

  /// Rebuild display feeds, freeing the old allocation before building the new one.
  ///
  /// If a tag filter is currently active, the tag-filtered view is rebuilt
  /// instead of the standard view so that background feed refreshes do not
  /// destroy the user's current tag feed.
  pub(super) fn rebuild_display_feeds(&mut self) {
    self.display_feeds.clear();
    self.tag_list = Self::build_tag_list(&self.feeds);
    if let Some(ref tag_query) = self.active_tag_query {
      self.display_feeds =
        Self::build_display_feeds_with_tag(&self.feeds, &self.query_config, tag_query);
    } else {
      self.display_feeds = Self::build_display_feeds(&self.feeds, &self.query_config);
    }
    self.invalidate_visible_indices();
  }
}

#[cfg(test)]
mod tests {
  use super::super::types::DisplayFeed;
  use super::App;
  use crate::config::QueryFeed;
  use crate::feeds::{Feed, FeedEntry};

  fn make_entry(title: &str, published: Option<&str>, read: bool) -> FeedEntry {
    FeedEntry {
      title: title.to_string(),
      published: published.map(|s| s.to_string()),
      text: String::new(),
      links: vec![],
      media: None,
      feed_title: None,
      read,
    }
  }

  fn make_feed(url: &str, title: &str, entries: Vec<FeedEntry>) -> Feed {
    Feed {
      url: url.to_string(),
      title: title.to_string(),
      entries,
      tags: None,
    }
  }

  #[test]
  fn test_build_display_feeds_no_queries() {
    let feeds = vec![
      make_feed("http://a.com", "Feed A", vec![]),
      make_feed("http://b.com", "Feed B", vec![]),
    ];
    let queries: Vec<QueryFeed> = vec![];
    let display = App::build_display_feeds(&feeds, &queries);
    assert_eq!(display.len(), 2);
    assert!(matches!(display[0], DisplayFeed::Regular(0)));
    assert!(matches!(display[1], DisplayFeed::Regular(1)));
  }

  #[test]
  fn test_build_display_feeds_with_queries() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];
    let mut feed_with_tags = feeds[0].clone();
    feed_with_tags.tags = Some(vec!["blog".to_string()]);
    let feeds = vec![feed_with_tags];

    let queries = vec![QueryFeed {
      name: "All Blogs".to_string(),
      query: "tags:blog".to_string(),
    }];

    let display = App::build_display_feeds(&feeds, &queries);
    // Query feeds come first, then regular feeds
    assert_eq!(display.len(), 2);
    assert!(display[0].is_query());
    assert_eq!(display[0].title(&feeds), "All Blogs");
    assert!(!display[1].is_query());
  }

  #[test]
  fn test_build_display_feeds_empty() {
    let feeds: Vec<Feed> = vec![];
    let queries: Vec<QueryFeed> = vec![];
    let display = App::build_display_feeds(&feeds, &queries);
    assert!(display.is_empty());
  }
}

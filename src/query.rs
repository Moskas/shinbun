use crate::feeds::{Feed, FeedEntry};

/// Represents a parsed query filter
#[derive(Debug, Clone)]
pub enum QueryFilter {
  /// Match feeds with any of the specified tags
  Tags(Vec<String>),
  /// Match all feeds (for testing/debugging)
  All,
}

/// Parse a query string into a filter
pub fn parse_query(query: &str) -> QueryFilter {
  let query = query.trim();

  if query.is_empty() || query == "*" {
    return QueryFilter::All;
  }

  // Parse "tags:tag1,tag2,tag3" format
  if let Some(tags_part) = query.strip_prefix("tags:") {
    let tags: Vec<String> = tags_part
      .split(',')
      .map(|s| s.trim().to_string())
      .filter(|s| !s.is_empty())
      .collect();

    return QueryFilter::Tags(tags);
  }

  // Default to empty tags (matches nothing)
  QueryFilter::Tags(Vec::new())
}

/// Check if a feed matches a query filter
pub fn feed_matches(feed: &Feed, filter: &QueryFilter) -> bool {
  match filter {
    QueryFilter::All => true,
    QueryFilter::Tags(query_tags) => {
      if query_tags.is_empty() {
        return false;
      }

      // Check if feed has any of the query tags (case-insensitive)
      if let Some(feed_tags) = &feed.tags {
        query_tags
          .iter()
          .any(|qt| feed_tags.iter().any(|ft| ft.eq_ignore_ascii_case(qt)))
      } else {
        false
      }
    }
  }
}

/// Apply a query filter to a list of feeds and return aggregated entries
pub fn apply_query(feeds: &[Feed], query: &str) -> Vec<FeedEntry> {
  let filter = parse_query(query);

  let mut entries: Vec<FeedEntry> = feeds
    .iter()
    .filter(|feed| feed_matches(feed, &filter))
    .flat_map(|feed| {
      let title = feed.title.clone();
      feed.entries.iter().map(move |entry| {
        let mut entry = entry.clone();
        entry.feed_title = Some(title.clone());
        entry
      })
    })
    .collect();

  entries.sort_by(|a, b| match (&b.published, &a.published) {
    (Some(b_date), Some(a_date)) => b_date.cmp(a_date),
    (Some(_), None) => std::cmp::Ordering::Less,
    (None, Some(_)) => std::cmp::Ordering::Greater,
    (None, None) => std::cmp::Ordering::Equal,
  });

  entries
}

/// Check if a FeedConfig matches a query filter.
/// Used by the targeted refresh to find which feeds to re-fetch.
pub fn config_feed_matches(fc: &crate::config::Feed, filter: &QueryFilter) -> bool {
  match filter {
    QueryFilter::All => true,
    QueryFilter::Tags(query_tags) => {
      if query_tags.is_empty() {
        return false;
      }
      fc.tags.as_ref().is_some_and(|feed_tags| {
        query_tags
          .iter()
          .any(|qt| feed_tags.iter().any(|ft| ft.eq_ignore_ascii_case(qt)))
      })
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_query_tags() {
    let filter = parse_query("tags:blog,tech");
    match filter {
      QueryFilter::Tags(tags) => {
        assert_eq!(tags, vec!["blog", "tech"]);
      }
      _ => panic!("Expected Tags filter"),
    }
  }

  #[test]
  fn test_parse_query_all() {
    let filter = parse_query("*");
    assert!(matches!(filter, QueryFilter::All));
  }

  #[test]
  fn test_parse_query_empty() {
    let filter = parse_query("");
    assert!(matches!(filter, QueryFilter::All));
  }

  #[test]
  fn test_parse_query_whitespace() {
    let filter = parse_query("   ");
    assert!(matches!(filter, QueryFilter::All));
  }

  #[test]
  fn test_parse_query_star_with_whitespace() {
    let filter = parse_query("  *  ");
    assert!(matches!(filter, QueryFilter::All));
  }

  #[test]
  fn test_parse_query_tags_single() {
    let filter = parse_query("tags:rust");
    match filter {
      QueryFilter::Tags(tags) => assert_eq!(tags, vec!["rust"]),
      _ => panic!("Expected Tags filter"),
    }
  }

  #[test]
  fn test_parse_query_tags_trims_whitespace() {
    let filter = parse_query("tags: blog , tech ");
    match filter {
      QueryFilter::Tags(tags) => assert_eq!(tags, vec!["blog", "tech"]),
      _ => panic!("Expected Tags filter"),
    }
  }

  #[test]
  fn test_parse_query_tags_empty_segments() {
    let filter = parse_query("tags:,,blog,,");
    match filter {
      QueryFilter::Tags(tags) => assert_eq!(tags, vec!["blog"]),
      _ => panic!("Expected Tags filter"),
    }
  }

  #[test]
  fn test_parse_query_unknown_defaults_to_empty_tags() {
    let filter = parse_query("foobar");
    match filter {
      QueryFilter::Tags(tags) => assert!(tags.is_empty()),
      _ => panic!("Expected empty Tags filter"),
    }
  }

  #[test]
  fn test_feed_matches_tag() {
    let feed = Feed {
      url: "test".to_string(),
      title: "Test".to_string(),
      entries: Vec::new(),
      tags: Some(vec!["blog".to_string(), "tech".to_string()]),
    };

    let filter = QueryFilter::Tags(vec!["blog".to_string()]);
    assert!(feed_matches(&feed, &filter));

    let filter = QueryFilter::Tags(vec!["news".to_string()]);
    assert!(!feed_matches(&feed, &filter));
  }

  #[test]
  fn test_feed_matches_all() {
    let feed = Feed {
      url: "test".to_string(),
      title: "Test".to_string(),
      entries: Vec::new(),
      tags: None,
    };
    assert!(feed_matches(&feed, &QueryFilter::All));
  }

  #[test]
  fn test_feed_matches_empty_tags_returns_false() {
    let feed = Feed {
      url: "test".to_string(),
      title: "Test".to_string(),
      entries: Vec::new(),
      tags: Some(vec!["blog".to_string()]),
    };
    let filter = QueryFilter::Tags(vec![]);
    assert!(!feed_matches(&feed, &filter));
  }

  #[test]
  fn test_feed_matches_no_feed_tags_returns_false() {
    let feed = Feed {
      url: "test".to_string(),
      title: "Test".to_string(),
      entries: Vec::new(),
      tags: None,
    };
    let filter = QueryFilter::Tags(vec!["blog".to_string()]);
    assert!(!feed_matches(&feed, &filter));
  }

  #[test]
  fn test_feed_matches_case_insensitive() {
    let feed = Feed {
      url: "test".to_string(),
      title: "Test".to_string(),
      entries: Vec::new(),
      tags: Some(vec!["Blog".to_string()]),
    };
    let filter = QueryFilter::Tags(vec!["blog".to_string()]);
    assert!(feed_matches(&feed, &filter));
    let filter = QueryFilter::Tags(vec!["BLOG".to_string()]);
    assert!(feed_matches(&feed, &filter));
  }

  #[test]
  fn test_apply_query_filters_and_sorts() {
    let feeds = vec![
      Feed {
        url: "a".to_string(),
        title: "Feed A".to_string(),
        entries: vec![
          FeedEntry {
            title: "Entry 1".to_string(),
            published: Some("2024-01-01T00:00:00Z".to_string()),
            text: String::new(),
            links: vec![],
            media: None,
            feed_title: None,
            read: false,
          },
          FeedEntry {
            title: "Entry 3".to_string(),
            published: Some("2024-03-01T00:00:00Z".to_string()),
            text: String::new(),
            links: vec![],
            media: None,
            feed_title: None,
            read: false,
          },
        ],
        tags: Some(vec!["blog".to_string()]),
      },
      Feed {
        url: "b".to_string(),
        title: "Feed B".to_string(),
        entries: vec![FeedEntry {
          title: "Entry 2".to_string(),
          published: Some("2024-02-01T00:00:00Z".to_string()),
          text: String::new(),
          links: vec![],
          media: None,
          feed_title: None,
          read: false,
        }],
        tags: Some(vec!["tech".to_string()]),
      },
    ];

    // Match only blog feeds
    let entries = apply_query(&feeds, "tags:blog");
    assert_eq!(entries.len(), 2);
    // Should be sorted newest-first
    assert_eq!(entries[0].title, "Entry 3");
    assert_eq!(entries[1].title, "Entry 1");
    // feed_title should be set
    assert_eq!(entries[0].feed_title.as_deref(), Some("Feed A"));

    // Match all
    let entries = apply_query(&feeds, "*");
    assert_eq!(entries.len(), 3);
    // Sorted newest-first
    assert_eq!(entries[0].title, "Entry 3");
    assert_eq!(entries[1].title, "Entry 2");
    assert_eq!(entries[2].title, "Entry 1");
  }

  #[test]
  fn test_apply_query_no_match() {
    let feeds = vec![Feed {
      url: "a".to_string(),
      title: "Feed A".to_string(),
      entries: vec![FeedEntry {
        title: "Entry 1".to_string(),
        published: None,
        text: String::new(),
        links: vec![],
        media: None,
        feed_title: None,
        read: false,
      }],
      tags: Some(vec!["blog".to_string()]),
    }];

    let entries = apply_query(&feeds, "tags:nonexistent");
    assert!(entries.is_empty());
  }

  #[test]
  fn test_apply_query_entries_without_dates_sorted_first() {
    let feeds = vec![Feed {
      url: "a".to_string(),
      title: "Feed A".to_string(),
      entries: vec![
        FeedEntry {
          title: "Dated".to_string(),
          published: Some("2024-01-01T00:00:00Z".to_string()),
          text: String::new(),
          links: vec![],
          media: None,
          feed_title: None,
          read: false,
        },
        FeedEntry {
          title: "Undated".to_string(),
          published: None,
          text: String::new(),
          links: vec![],
          media: None,
          feed_title: None,
          read: false,
        },
      ],
      tags: None,
    }];

    let entries = apply_query(&feeds, "*");
    // None-dated entries sort before dated ones due to the comparator:
    // (Some(_), None) => Less means the Some-dated entry is "less" so None comes first.
    assert_eq!(entries[0].title, "Undated");
    assert_eq!(entries[1].title, "Dated");
  }

  #[test]
  fn test_config_feed_matches_all() {
    let fc = crate::config::Feed {
      link: "https://example.com".to_string(),
      name: None,
      tags: None,
    };
    assert!(config_feed_matches(&fc, &QueryFilter::All));
  }

  #[test]
  fn test_config_feed_matches_tags() {
    let fc = crate::config::Feed {
      link: "https://example.com".to_string(),
      name: None,
      tags: Some(vec!["rust".to_string(), "dev".to_string()]),
    };
    let filter = QueryFilter::Tags(vec!["rust".to_string()]);
    assert!(config_feed_matches(&fc, &filter));

    let filter = QueryFilter::Tags(vec!["python".to_string()]);
    assert!(!config_feed_matches(&fc, &filter));
  }

  #[test]
  fn test_config_feed_matches_no_tags() {
    let fc = crate::config::Feed {
      link: "https://example.com".to_string(),
      name: None,
      tags: None,
    };
    let filter = QueryFilter::Tags(vec!["rust".to_string()]);
    assert!(!config_feed_matches(&fc, &filter));
  }

  #[test]
  fn test_config_feed_matches_empty_query_tags() {
    let fc = crate::config::Feed {
      link: "https://example.com".to_string(),
      name: None,
      tags: Some(vec!["rust".to_string()]),
    };
    let filter = QueryFilter::Tags(vec![]);
    assert!(!config_feed_matches(&fc, &filter));
  }
}

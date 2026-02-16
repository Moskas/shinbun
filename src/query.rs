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

            // Check if feed has any of the query tags
            if let Some(feed_tags) = &feed.tags {
                query_tags
                    .iter()
                    .any(|qt| feed_tags.iter().any(|ft| ft == qt))
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
            // Clone entries and set the feed_title for each one
            feed.entries.iter().map(|entry| {
                let mut entry = entry.clone();
                entry.feed_title = Some(feed.title.clone());
                entry
            })
        })
        .collect();

    // Sort by published date (most recent first)
    entries.sort_by(|a, b| match (&b.published, &a.published) {
        (Some(b_date), Some(a_date)) => b_date.cmp(a_date),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    entries
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
}

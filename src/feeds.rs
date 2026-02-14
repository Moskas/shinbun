use crate::config::Feed as FeedConfig;
use feed_rs::parser;
use futures::future::join_all;
use reqwest::{get, Error as ReqError};

#[derive(Debug)]
pub struct Feed {
  pub url: String,
  pub title: String,
  pub entries: Vec<FeedEntry>,
  pub tags: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct FeedEntry {
  pub title: String,
  pub published: Option<String>,
  pub text: String,
  pub links: Vec<String>,
  pub media: String,
}

/// Fetch multiple feeds concurrently
pub async fn fetch_feed(feeds: Vec<FeedConfig>) -> Vec<(FeedConfig, Result<String, ReqError>)> {
  let futures = feeds.into_iter().map(|feed| async move {
    let result = fetch_single_feed(&feed.link).await;
    (feed, result)
  });

  join_all(futures).await
}

/// Fetch a single feed from URL
async fn fetch_single_feed(url: &str) -> Result<String, ReqError> {
  match get(url).await {
    Ok(response) => response.text().await.map_err(|err| {
      eprintln!("Failed to read response body from {}: {}", url, err);
      err
    }),
    Err(err) => {
      eprintln!("Failed to fetch feed {}: {}", url, err);
      Err(err)
    }
  }
}

/// Parse feed results into structured Feed objects
pub fn parse_feed(results: Vec<(FeedConfig, Result<String, ReqError>)>) -> Vec<Feed> {
  results
    .into_iter()
    .filter_map(|(feed_config, body_result)| {
      let body = body_result.ok()?;
      parse_single_feed(feed_config, &body)
    })
    .collect()
}

/// Parse a single feed from XML/RSS content
fn parse_single_feed(feed_config: FeedConfig, content: &str) -> Option<Feed> {
  let parsed_feed = match parser::parse(content.as_bytes()) {
    Ok(feed) => feed,
    Err(err) => {
      eprintln!("Failed to parse feed: {}", feed_config.link);
      eprintln!("Error: {}", err);
      return None;
    }
  };

  // Determine the feed title
  let title = feed_config
    .name
    .clone()
    .or_else(|| parsed_feed.title.as_ref().map(|t| t.content.clone()))
    .unwrap_or_else(|| feed_config.link.clone());

  // Parse all entries
  let entries = parsed_feed
    .entries
    .into_iter()
    .map(|entry| parse_feed_entry(entry))
    .collect();

  Some(Feed {
    url: feed_config.link,
    title,
    entries,
    tags: feed_config.tags,
  })
}

/// Parse a single feed entry
fn parse_feed_entry(entry: feed_rs::model::Entry) -> FeedEntry {
  // Extract the main content (prefer content over summary)
  let html_content = entry
    .content
    .as_ref()
    .and_then(|c| c.body.clone())
    .or_else(|| entry.summary.as_ref().map(|s| s.content.clone()))
    .unwrap_or_default();

  // Convert HTML to plain text with no wrapping
  let text = html2text::from_read(html_content.as_bytes(), usize::MAX)
    .unwrap_or_else(|_| String::from("Failed to parse content"));

  // Extract links
  let links = entry.links.iter().map(|link| link.href.clone()).collect();

  // Extract media URL
  let media = entry
    .media
    .first()
    .and_then(|m| m.content.first())
    .and_then(|c| c.url.as_ref().map(|u| u.to_string()))
    .unwrap_or_default();

  // Format date: prefer published, fallback to updated
  // This handles aggregator feeds that only have <updated> tags
  let published = entry.published.or(entry.updated).map(|dt| dt.to_rfc3339());

  FeedEntry {
    title: entry
      .title
      .map(|t| t.content)
      .unwrap_or_else(|| String::from("No title")),
    published,
    text,
    links,
    media,
  }
}

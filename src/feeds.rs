use feed_rs::{
  model::{Feed as Feedrs, Person},
  parser,
};
use reqwest::get;

#[derive(Debug)]
pub struct Feed {
  authors: Vec<Person>,
  pub url: String,
  pub title: String,
  pub entries: Vec<String>,
}

pub struct _FeedList {}

pub async fn fetch_feed(feeds: Vec<String>) -> Vec<String> {
  let mut raw_feeds: Vec<String> = Vec::new();
  for url in feeds {
    let response = get(&url).await.expect("Failed to fetch feed");
    raw_feeds.push(response.text().await.expect("Failed to read response body"));
  }
  raw_feeds
}

pub fn parse_feed(feed: Vec<String>, feed_urls: Vec<String>) -> Vec<Feed> {
  let mut all_feeds: Vec<Feed> = Vec::new();

  for (index, raw) in feed.into_iter().enumerate() {
    let feed_from_xml = parser::parse(raw.as_bytes()).expect("Failed to parse feed");
    let title = feed_from_xml.title.unwrap().content;
    let authors = feed_from_xml.authors;

    let mut entries: Vec<String> = Vec::new();
    for entry in feed_from_xml.entries {
      entries.push(entry.title.unwrap().content);
    }

    let feed = Feed {
      url: feed_urls[index].clone(),
      authors,
      title,
      entries,
    };

    all_feeds.push(feed);
  }

  all_feeds
}

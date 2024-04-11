//use config::Feeds;
use crate::Feeds;
use feed_rs::parser;
use reqwest::get;

#[derive(Debug)]
pub struct Feed {
  //authors: Vec<Person>,
  pub url: String,
  pub title: String,
  pub entries: Vec<String>,
}

pub async fn fetch_feed(feeds: Vec<Feeds>) -> Vec<String> {
  let mut raw_feeds: Vec<String> = Vec::new();
  for entry in feeds {
    let response = get(entry.link).await.expect("Failed to fetch feed");
    raw_feeds.push(response.text().await.expect("Failed to read response body"));
  }
  raw_feeds
}

pub fn parse_feed(links: Vec<String>, feeds: Vec<Feeds>) -> Vec<Feed> {
  let mut all_feeds: Vec<Feed> = Vec::new();
  for (index, raw) in links.into_iter().enumerate() {
    let feed_from_xml = parser::parse(raw.as_bytes()).expect("Failed to parse the feed");
    let title = feed_from_xml.title.unwrap().content;

    let mut entries: Vec<String> = Vec::new();
    for entry in feed_from_xml.entries {
      entries.push(entry.title.unwrap().content);
    }

    let feed = Feed {
      url: feeds[index].link.clone(),
      title,
      entries,
    };

    all_feeds.push(feed);
  }
  all_feeds
}

//pub fn parse_feed(feed: Vec<String>, feed_urls: Vec<Feeds>) -> Vec<Feed> {
//  let mut all_feeds: Vec<Feed> = Vec::new();
//
//  for (index, raw) in feed.into_iter().enumerate() {
//    //for element in feed.into_iter() {
//    let feed_from_xml = parser::parse(raw.as_bytes()).expect("Failed to parse feed");
//    let title = feed_from_xml.title.unwrap().content;
//    let authors = feed_from_xml.authors;
//
//    let mut entries: Vec<String> = Vec::new();
//    for entry in feed_from_xml.entries {
//      entries.push(entry.title.unwrap().content);
//    }
//
//    let feed = Feed {
//      url: feed[index],
//      authors,
//      title,
//      entries,
//    };
//
//    all_feeds.push(feed);
//  }
//
//  all_feeds
//}

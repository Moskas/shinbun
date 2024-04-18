//use config::Feeds;
use crate::Feeds;
use feed_rs::{model::Entry, parser};
use reqwest::{get, Error as reqError};

#[derive(Debug)]
pub struct Feed {
  //authors: Vec<Person>,
  pub url: String,
  pub title: String,
  pub entries: Vec<Entry>,
}

pub async fn fetch_feed(feeds: Vec<Feeds>) -> Result<Vec<String>, reqError> {
  let mut raw_feeds: Vec<String> = Vec::new();
  for entry in feeds {
    match get(entry.link).await {
      Ok(response) => match response.text().await {
        Ok(body) => {
          raw_feeds.push(body);
        }
        Err(e) => {
          eprintln!("Failed to read response body: {}", e);
        }
      },
      Err(e) => {
        eprintln!("Failed to fetch feed: {}", e);
      }
    }
  }
  Ok::<Vec<String>, reqError>(raw_feeds)
}

pub fn parse_feed(links: Vec<String>, feeds: Vec<Feeds>) -> Vec<Feed> {
  let mut all_feeds: Vec<Feed> = Vec::new();
  for (index, raw) in links.into_iter().enumerate() {
    let feed_from_xml = parser::parse(raw.as_bytes()).expect("Failed to parse the feed");
    let title = if feeds[index].name.is_some() {
      feeds[index].name.clone().unwrap()
    } else {
      feed_from_xml.title.unwrap().content
    };

    let mut entries: Vec<Entry> = Vec::new();
    for entry in feed_from_xml.entries {
      entries.push(entry);
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

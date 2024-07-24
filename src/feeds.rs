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
  pub tags: Option<Vec<String>>,
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
    let feed_from_xml = match parser::parse(raw.as_bytes()) {
      Ok(feed) => feed,
      Err(e) => {
        eprintln!("Failed to parse the feed: {}", feeds[index].link);
        eprintln!("Details: {}", e);
        std::process::exit(-1)
      }
    };
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
      tags: feeds[index].tags.clone(),
    };

    all_feeds.push(feed);
  }
  all_feeds
}

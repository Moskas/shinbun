use feed_rs::parser;
use reqwest::get;

#[derive(Debug, Default)]
pub struct Feed {
  pub title: String,
  pub entries: Vec<String>,
}

pub async fn fetch_feed(feeds: Vec<String>) -> Vec<String> {
  let mut raw_feeds: Vec<String> = Vec::new();
  for url in feeds {
    println!("Fetching: {}", url);
    let response = get(url).await;
    raw_feeds.push(response.unwrap().text().await.unwrap());
  }
  raw_feeds
}

pub fn parse_feed(feed: Vec<String>) -> Feed {
  let mut all_entries: Vec<String> = Vec::new();
  let mut feed_title = String::new();

  for raw in feed {
    let feed_from_xml = parser::parse(raw.as_bytes()).unwrap();
    let title = feed_from_xml.title.unwrap().content;
    if feed_title.is_empty() {
      feed_title = title.clone(); // Assuming you want to use the title from the first feed
    }
    for entry in feed_from_xml.entries {
      all_entries.push(entry.title.unwrap().content);
    }
  }

  Feed {
    title: feed_title,
    entries: all_entries,
  }
}

use feed_rs::parser;
use reqwest::get;

pub async fn fetch_feed(feeds: Vec<String>) -> Vec<String> {
  let mut raw_feeds: Vec<String> = Vec::new();
  for url in feeds {
    let response = get(url).await;
    raw_feeds.push(response.unwrap().text().await.unwrap());
  }
  raw_feeds
}

pub fn parse_feed(feed: Vec<String>) -> () {
  for raw in feed {
    let feed_from_xml = parser::parse(raw.as_bytes()).unwrap();
    println!("{}:", feed_from_xml.title.unwrap().content);
    for entry in feed_from_xml.entries {
      //dbg!(&entry);
      println!("- {}", entry.title.expect("No title found").content);
    }
  }
}

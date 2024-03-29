mod config;
mod feeds;

#[tokio::main]
async fn main() {
  let feeds_urls = config::parse_feed_urls;
  let xml = feeds::fetch_feed(feeds_urls());
  feeds::parse_feed(xml.await);
}

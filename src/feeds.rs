//use config::Feeds;
use crate::Feeds;
use feed_rs::parser;
use reqwest::{get, Error as reqError};

use futures::future::join_all;


#[derive(Debug)]
pub struct Feed {
    pub url: String,
    pub title: String,
    pub entries: Vec<FeedEntry>, // Use a custom `FeedEntry` struct with plain text content
    pub tags: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct FeedEntry {
    pub title: String,
    pub published: Option<String>, // Optional published date
    pub plain_text: String,        // Store preprocessed plain text here
    pub links: Vec<String>,        // Store any relevant links
    pub media: String,             // Store any relevant links
}


pub async fn fetch_feed(feeds: Vec<Feeds>) -> Result<Vec<String>, reqError> {
    let futs = feeds.into_iter().map(|entry| async move {
        match get(entry.link).await {
            Ok(resp) => resp.text().await.map_err(|e| {
                eprintln!("Failed to read response body: {}", e);
                e
            }),
            Err(e) => {
                eprintln!("Failed to fetch feed: {}", e);
                Err(e)
            }
        }
    });
    // Run all requests concurrently
    let results = join_all(futs).await;

    // Collect only successful bodies; you could return a Vec<Result<String, reqError>> if you prefer
    let mut out = Vec::new();
    for res in results {
        if let Ok(body) = res {
            out.push(body);
        }
    }
    Ok(out)
}

pub fn parse_feed(links: Vec<String>, feeds: Vec<Feeds>, area_width: usize) -> Vec<Feed> {
    let mut all_feeds: Vec<Feed> = Vec::new();

    for (index, raw) in links.into_iter().enumerate() {
        let feed_from_xml = match parser::parse(raw.as_bytes()) {
            Ok(feed) => feed,
            Err(e) => {
                eprintln!("Failed to parse the feed: {}", feeds[index].link);
                eprintln!("Details: {}", e);
                std::process::exit(-1);
            }
        };

        let title = feeds[index]
            .name
            .clone()
            .unwrap_or_else(|| feed_from_xml.title.unwrap().content);

        let mut entries: Vec<FeedEntry> = Vec::new();

        for entry in feed_from_xml.entries {
            // Convert HTML content to plain text once
            let main_content = entry
                .content
                .as_ref()
                .and_then(|c| c.body.clone()) // Extract the HTML content
                .unwrap_or_else(|| "".to_string()); // Use empty string if none

            // Use the dynamic width from the area
            let plain_text = html2text::config::plain()
                .lines_from_read(main_content.as_bytes(), area_width - 15)
                .expect("Failed to parse HTML")
                .into_iter()
                .map(|line| line.chars().collect::<String>())
                .collect::<Vec<String>>()
                .join("\n");

            // Collect links or other metadata
            let links = entry.links.iter().map(|l| l.href.clone()).collect();
            let media = entry
                .media
                .first()
                .and_then(|media| media.content.first())
                .map(|content_item| content_item.url.as_ref().map(|l| l.to_string()))
                .unwrap_or_default()
                .unwrap_or_default();

            let feed_entry = FeedEntry {
                title: entry.title.map_or("No title".to_string(), |t| t.content),
                published: entry.published.map(|p| p.to_string()),
                plain_text, // Store preprocessed plain text
                links,
                media,
            };

            entries.push(feed_entry);
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

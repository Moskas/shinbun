use crate::Feeds;
use feed_rs::parser;
use futures::future::join_all;
use reqwest::{get, Error as reqError};

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
    pub text: String,         // <-- unwrapped plain text
    pub links: Vec<String>,
    pub media: String,
}

/// Fetch while preserving association with the original `Feeds` item.
/// We return one tuple per input feed; each has the original `Feeds` and a body or error.
pub async fn fetch_feed(feeds: Vec<Feeds>) -> Vec<(Feeds, Result<String, reqError>)> {
    let futs = feeds.into_iter().map(|f| async move {
        let res = match get(&f.link).await {
            Ok(resp) => resp.text().await.map_err(|e| {
                eprintln!("Failed to read response body from {}: {}", f.link, e);
                e
            }),
            Err(e) => {
                eprintln!("Failed to fetch feed {}: {}", f.link, e);
                Err(e)
            }
        };
        (f, res)
    });

    join_all(futs).await
}

/// Parse feeds. We skip failed downloads and failed XML parses (log & continue).
/// No static wrapping: we convert HTML to plain text with effectively infinite width.
pub fn parse_feed(results: Vec<(Feeds, Result<String, reqError>)>) -> Vec<Feed> {
    let mut all_feeds = Vec::with_capacity(results.len());

    for (feed_cfg, body_res) in results {
        let raw = match body_res {
            Ok(b) => b,
            Err(_) => {
                // Skip (or create an empty feed if you prefer).
                continue;
            }
        };

        let feed_from_xml = match parser::parse(raw.as_bytes()) {
            Ok(feed) => feed,
            Err(e) => {
                eprintln!("Failed to parse the feed: {}", feed_cfg.link);
                eprintln!("Details: {}", e);
                continue;
            }
        };

        let title = feed_cfg
            .name
            .clone()
            .or_else(|| feed_from_xml.title.as_ref().map(|t| t.content.clone()))
            .unwrap_or_else(|| feed_cfg.link.clone());

        let mut entries = Vec::with_capacity(feed_from_xml.entries.len());

        for entry in feed_from_xml.entries {
            // Prefer the main content; fall back to summary if needed
            let main_html = entry
                .content
                .as_ref()
                .and_then(|c| c.body.clone())
                .or_else(|| entry.summary.as_ref().map(|s| s.content.clone()))
                .unwrap_or_default();

            let text = html2text::from_read(main_html.as_bytes(), usize::MAX).unwrap();

            let links = entry.links.iter().map(|l| l.href.clone()).collect::<Vec<_>>();
            let media = entry
                .media
                .first()
                .and_then(|m| m.content.first())
                .and_then(|c| c.url.as_ref().map(|u| u.to_string()))
                .unwrap_or_default();

            entries.push(FeedEntry {
                title: entry.title.map_or_else(|| "No title".to_string(), |t| t.content),
                published: entry.published.map(|p| p.to_string()),
                text,
                links,
                media,
            });
        }

        all_feeds.push(Feed {
            url: feed_cfg.link,
            title,
            entries,
            tags: feed_cfg.tags,
        });
    }

    all_feeds
}
use crossterm::event::{self, poll, Event, KeyEventKind};
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;

mod app;
mod cache;
mod config;
mod feeds;
mod ui;
mod views;

use app::{App, FeedUpdate};
use cache::FeedCache;

#[tokio::main]
async fn main() -> io::Result<()> {
  let mut terminal = ui::init()?;

  // Parse configuration
  let config = config::parse_config();

  // Initialize cache
  let cache_path = config::get_cache_path();
  let cache = FeedCache::new(cache_path).expect("Failed to initialize cache");

  // Load cached feeds
  let cached_feeds = cache.load_all_feeds().unwrap_or_default();

  // Determine which feeds need to be fetched
  let feeds_to_fetch: Vec<_> = config
    .feeds
    .iter()
    .filter(|feed_config| {
      // Fetch if not in cache
      !cache.has_feed(&feed_config.link).unwrap_or(false)
    })
    .cloned()
    .collect();

  // Create channel for feed updates
  let (feed_tx, feed_rx) = mpsc::unbounded_channel();

  // Start background fetch for missing feeds
  if !feeds_to_fetch.is_empty() {
    let tx = feed_tx.clone();
    tokio::spawn(async move {
      feeds::fetch_feed_with_progress(feeds_to_fetch, tx).await;
    });
  } else {
    // No feeds to fetch, send FetchComplete immediately
    let _ = feed_tx.send(FeedUpdate::FetchComplete);
  }

  // Create and run the app with cached feeds
  let mut app = App::new(cached_feeds, config.ui, config.feeds, feed_tx, cache);
  let result = run_app(&mut terminal, &mut app, feed_rx);

  // Restore terminal
  ui::restore()?;
  result
}

fn run_app(
  terminal: &mut ui::Tui,
  app: &mut App,
  mut feed_rx: mpsc::UnboundedReceiver<FeedUpdate>,
) -> io::Result<()> {
  while !app.should_exit() {
    terminal.draw(|frame| app.render(frame))?;

    // Check for feed updates (non-blocking)
    while let Ok(update) = feed_rx.try_recv() {
      app.handle_feed_update(update);
    }

    // Poll for key events with a short timeout to allow:
    // - Feed updates to be processed
    // - Spinner animation to update smoothly
    if poll(Duration::from_millis(50))? {
      if let Event::Key(key) = event::read()? {
        if key.kind == KeyEventKind::Press {
          app.handle_key(key);
        }
      }
    }
  }
  Ok(())
}

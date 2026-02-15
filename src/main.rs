use crossterm::event::{self, poll, Event, KeyEventKind};
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;

mod app;
mod config;
mod feeds;
mod ui;
mod views;

use app::{App, FeedUpdate};

#[tokio::main]
async fn main() -> io::Result<()> {
  let mut terminal = ui::init()?;

  // Parse configuration
  let config = config::parse_config();

  // Create channel for feed updates
  let (feed_tx, feed_rx) = mpsc::unbounded_channel();

  // Start initial feed fetch in background
  let initial_feeds = config.feeds.clone();
  let tx = feed_tx.clone();
  tokio::spawn(async move {
    feeds::fetch_feed_with_progress(initial_feeds, tx).await;
  });

  // Create and run the app with empty feeds initially
  let mut app = App::new(vec![], config.ui, config.feeds, feed_tx);
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

use crossterm::event::{self, Event, KeyEventKind};
use std::io;

mod app;
mod config;
mod feeds;
mod ui;
mod views;

use app::App;

#[tokio::main]
async fn main() -> io::Result<()> {
  let mut terminal = ui::init()?;

  // Parse configuration
  let config = config::parse_config();

  // Fetch and parse feeds
  let results = feeds::fetch_feed(config.feeds).await;
  let feed_list = feeds::parse_feed(results);

  // Create and run the app
  let mut app = App::new(feed_list, config.ui);
  let result = run_app(&mut terminal, &mut app);

  // Restore terminal
  ui::restore()?;
  result
}

fn run_app(terminal: &mut ui::Tui, app: &mut App) -> io::Result<()> {
  while !app.should_exit() {
    terminal.draw(|frame| app.render(frame))?;
    if let Event::Key(key) = event::read()? {
      if key.kind == KeyEventKind::Press {
        app.handle_key(key);
      }
    }
  }
  Ok(())
}

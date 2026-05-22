use clap::{Parser, Subcommand};
use crossterm::event::{self, poll, Event, KeyEventKind};
use crossterm::{
  execute,
  style::{Color, Print, ResetColor, SetForegroundColor},
};
use std::io;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

mod app;
mod cache;
mod config;
mod feeds;
mod opml;
mod query;
mod theme;
mod ui;
mod views;

use app::{App, FeedUpdate};
use cache::FeedCache;

#[derive(Parser)]
#[command(name = "shinbun", about = "Terminal RSS reader")]
struct Cli {
  #[command(subcommand)]
  command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
  /// Export feed subscriptions to OPML
  Export {
    /// Write to file instead of stdout
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,
  },
  /// Import feed subscriptions from an OPML file
  Import {
    /// OPML file to read
    file: PathBuf,
    /// Replace existing feeds instead of merging (default: merge, skip duplicates)
    #[arg(long)]
    force: bool,
    /// Preview changes without writing to feeds.toml
    #[arg(long)]
    dry_run: bool,
  },
  /// Fetch all feeds and update the cache without launching the TUI
  Refresh {
    /// Suppress per-feed output; only print the final summary
    #[arg(short, long)]
    quiet: bool,
  },
  /// Remove cached feeds that are no longer listed in feeds.toml
  Clean {
    /// Show what would be removed without modifying the cache
    #[arg(long)]
    dry_run: bool,
  },
  /// Show cache statistics (feed counts, read/unread entries)
  Stats,
}

#[tokio::main]
async fn main() -> io::Result<()> {
  let cli = Cli::parse();

  match cli.command {
    Some(Commands::Export { output }) => {
      return cmd_export(output);
    }
    Some(Commands::Import {
      file,
      force,
      dry_run,
    }) => {
      return cmd_import(file, force, dry_run);
    }
    Some(Commands::Refresh { quiet }) => {
      return cmd_refresh(quiet).await;
    }
    Some(Commands::Clean { dry_run }) => {
      return cmd_clean(dry_run);
    }
    Some(Commands::Stats) => {
      return cmd_stats();
    }
    None => {}
  }

  let mut terminal = ui::init()?;

  // Restore terminal on panic so the shell isn't left in raw mode.
  let original_hook = std::panic::take_hook();
  std::panic::set_hook(Box::new(move |panic_info| {
    let _ = ui::restore();
    original_hook(panic_info);
  }));

  // Parse configuration
  let config = match config::parse_config() {
    Ok(c) => c,
    Err(e) => {
      ui::restore()?;
      eprintln!("{}", e);
      std::process::exit(1);
    }
  };

  // Initialize cache
  let cache_path = config::get_cache_path();
  let cache = match FeedCache::new(cache_path) {
    Ok(c) => c,
    Err(e) => {
      ui::restore()?;
      eprintln!("Failed to initialize cache: {}", e);
      std::process::exit(1);
    }
  };

  // Remove any feeds that are in the cache but no longer listed in feeds.toml.
  // Their entries are cascade-deleted automatically.
  {
    let active_urls: Vec<&str> = config.feeds.iter().map(|f| f.link.as_str()).collect();
    // Errors here are non-fatal; stale feeds will simply remain in the cache.
    let _ = cache.remove_dead_feeds(&active_urls);
  }

  // Load cached feeds (already sorted by position)
  let cached_feeds = cache.load_all_feeds().unwrap_or_default();

  // Determine which feeds need to be fetched:
  // - new feeds not yet in the cache
  // - feeds whose configured refresh interval has elapsed since last fetch
  let now = chrono::Utc::now().timestamp();
  let feeds_to_fetch: Vec<_> = config
    .feeds
    .iter()
    .filter(|feed_config| {
      if !cache.has_feed(&feed_config.link).unwrap_or(false) {
        return true;
      }
      if let Some(interval_secs) = feed_config
        .refresh
        .as_deref()
        .and_then(config::parse_refresh_interval)
      {
        if let Ok(Some(last_fetched)) = cache.get_last_fetched(&feed_config.link) {
          return (now - last_fetched) >= interval_secs as i64;
        }
      }
      false
    })
    .cloned()
    .collect();

  // Create channel for feed updates
  let (feed_tx, feed_rx) = mpsc::unbounded_channel();

  // Notify the TUI about feeds intentionally skipped (have a refresh interval that hasn't elapsed)
  for feed_config in &config.feeds {
    if feed_config.refresh.is_none() {
      continue;
    }
    let in_cache = cache.has_feed(&feed_config.link).unwrap_or(false);
    let is_being_fetched = feeds_to_fetch.iter().any(|f| f.link == feed_config.link);
    if in_cache && !is_being_fetched {
      let name = feed_config
        .name
        .clone()
        .unwrap_or_else(|| feed_config.link.clone());
      let _ = feed_tx.send(FeedUpdate::SkippedFeed(name));
    }
  }

  // Start background fetch for missing/due feeds
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
  let mut app = App::new(
    cached_feeds,
    config.general,
    config.ui,
    config.feeds,
    config.queries,
    feed_tx,
    cache,
  );
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
  // Tracks whether the previous iteration showed the loading popup, so we can
  // force one final render after the 3-second linger expires — otherwise the
  // popup would remain visible on screen forever.
  let mut prev_popup_showing = true;

  while !app.should_exit() {
    // Process pending feed updates first (may set app.dirty)
    while let Ok(update) = feed_rx.try_recv() {
      app.handle_feed_update(update);
    }

    let popup_showing = app.loading_state.should_show_popup();

    // Only render when:
    //   - state changed (keypress, feed update, resize)
    //   - a time-based animation is running (spinner, 3-second linger)
    //   - the linger popup just expired and needs a cleanup frame
    let needs_render = app.dirty || popup_showing || prev_popup_showing;
    if needs_render {
      terminal.draw(|frame| app.render(frame))?;
      app.dirty = false;
    }
    prev_popup_showing = popup_showing;

    // Poll for key events.  Shorter timeout during animation so the spinner
    // updates smoothly; longer timeout when idle to reduce wake-ups.
    let timeout = if popup_showing {
      Duration::from_millis(50)
    } else {
      Duration::from_millis(200)
    };
    if poll(timeout)? {
      match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
          app.handle_key(key);
        }
        Event::Resize(_, _) => {
          // Terminal resized — need a redraw on the next iteration.
          app.dirty = true;
        }
        _ => {}
      }
    }
  }
  Ok(())
}

fn cmd_export(output: Option<PathBuf>) -> io::Result<()> {
  let feeds = match config::parse_config() {
    Ok(c) => c.feeds,
    Err(e) => {
      eprintln!("Error reading config: {}", e);
      std::process::exit(1);
    }
  };

  if let Some(path) = output {
    let file = std::fs::File::create(&path)
      .map_err(|e| io::Error::new(e.kind(), format!("{}: {}", path.display(), e)))?;
    opml::export_opml(&feeds, file).map_err(|e| io::Error::other(e.to_string()))
  } else {
    opml::export_opml(&feeds, io::stdout().lock()).map_err(|e| io::Error::other(e.to_string()))
  }
}

fn cmd_import(file: PathBuf, force: bool, dry_run: bool) -> io::Result<()> {
  use std::io::IsTerminal;
  let color = io::stdout().is_terminal();

  let f = std::fs::File::open(&file)
    .map_err(|e| io::Error::new(e.kind(), format!("{}: {}", file.display(), e)))?;
  let imported = opml::import_opml(io::BufReader::new(f))
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

  if imported.is_empty() {
    eprintln!("No feeds found in OPML file.");
    return Ok(());
  }

  // Fast path: --force without dry-run just overwrites.
  if force && !dry_run {
    println!("Imported {} feed(s).", imported.len());
    return config::write_feeds(&imported);
  }

  let existing = config::parse_config().map(|c| c.feeds).unwrap_or_default();
  let existing_urls: std::collections::HashSet<String> =
    existing.iter().map(|f| f.link.clone()).collect();

  if force {
    // --force --dry-run: show full diff against current config.
    let imported_urls: std::collections::HashSet<String> =
      imported.iter().map(|f| f.link.clone()).collect();

    let mut n_added = 0usize;
    let mut n_kept = 0usize;
    let mut n_removed = 0usize;

    for feed in &existing {
      if !imported_urls.contains(&feed.link) {
        let label = feed.name.as_deref().unwrap_or(&feed.link);
        colored_out(&format!("  - {}", label), Color::DarkRed, color);
        n_removed += 1;
      }
    }
    for feed in &imported {
      let label = feed.name.as_deref().unwrap_or(&feed.link);
      let tags = feed_tags(feed);
      if existing_urls.contains(&feed.link) {
        colored_out(&format!("  = {}{}", label, tags), Color::DarkCyan, color);
        n_kept += 1;
      } else {
        colored_out(&format!("  + {}{}", label, tags), Color::DarkGreen, color);
        n_added += 1;
      }
    }
    println!();
    colored_segment(&format!("{} addition(s)", n_added), Color::DarkGreen, color);
    print!(", ");
    colored_segment(&format!("{} unchanged", n_kept), Color::DarkCyan, color);
    print!(", ");
    colored_segment(&format!("{} removal(s)", n_removed), Color::DarkRed, color);
    println!();
    println!("(dry run — feeds.toml not modified)");
    return Ok(());
  }

  // Merge path: partition imported into new vs duplicate.
  let (to_add, to_skip): (Vec<config::Feed>, Vec<config::Feed>) = imported
    .into_iter()
    .partition(|feed| !existing_urls.contains(&feed.link));

  if dry_run {
    for feed in &to_add {
      let label = feed.name.as_deref().unwrap_or(&feed.link);
      colored_out(
        &format!("  + {}{}", label, feed_tags(feed)),
        Color::DarkGreen,
        color,
      );
    }
    for feed in &to_skip {
      let label = feed.name.as_deref().unwrap_or(&feed.link);
      colored_out(
        &format!("  = {} (already exists)", label),
        Color::DarkCyan,
        color,
      );
    }
    println!();
    colored_segment(
      &format!("{} addition(s)", to_add.len()),
      Color::DarkGreen,
      color,
    );
    print!(", ");
    colored_segment(
      &format!("{} unchanged", to_skip.len()),
      Color::DarkCyan,
      color,
    );
    println!();
    println!("(dry run — feeds.toml not modified)");
    return Ok(());
  }

  let added = to_add.len();
  let skipped = to_skip.len();
  let mut merged = existing;
  merged.extend(to_add);
  println!("Added {} feed(s), skipped {} duplicate(s).", added, skipped);
  config::write_feeds(&merged)
}

async fn cmd_refresh(quiet: bool) -> io::Result<()> {
  use std::io::IsTerminal;
  let color = io::stdout().is_terminal();

  let config = match config::parse_config() {
    Ok(c) => c,
    Err(e) => {
      eprintln!("Error reading config: {}", e);
      std::process::exit(1);
    }
  };

  if config.feeds.is_empty() {
    println!("No feeds configured.");
    return Ok(());
  }

  let cache_path = config::get_cache_path();
  let cache = match FeedCache::new(cache_path) {
    Ok(c) => c,
    Err(e) => {
      eprintln!("Failed to initialize cache: {}", e);
      std::process::exit(1);
    }
  };

  {
    let active_urls: Vec<&str> = config.feeds.iter().map(|f| f.link.as_str()).collect();
    let _ = cache.remove_dead_feeds(&active_urls);
  }

  // Split feeds into those due for refresh and those whose interval has not elapsed
  let now = chrono::Utc::now().timestamp();
  let (feeds_to_fetch, feeds_to_skip): (Vec<_>, Vec<_>) =
    config.feeds.iter().partition(|feed_config| {
      if !cache.has_feed(&feed_config.link).unwrap_or(false) {
        return true;
      }
      if let Some(interval_secs) = feed_config
        .refresh
        .as_deref()
        .and_then(config::parse_refresh_interval)
      {
        if let Ok(Some(last_fetched)) = cache.get_last_fetched(&feed_config.link) {
          return (now - last_fetched) >= interval_secs as i64;
        }
      }
      true // no interval set: always refresh
    });

  if !quiet {
    for feed_config in &feeds_to_skip {
      let name = feed_config.name.as_deref().unwrap_or(&feed_config.link);
      colored_out(&format!("Skipped: {}", name), Color::DarkYellow, color);
    }
  }

  let total = feeds_to_fetch.len();
  let feed_config: Vec<_> = feeds_to_fetch.into_iter().cloned().collect();
  let (tx, mut rx) = mpsc::unbounded_channel::<FeedUpdate>();

  tokio::spawn(async move {
    feeds::fetch_feeds_subset_with_progress(feed_config, tx).await;
  });

  let mut updated = 0usize;
  let mut errors = 0usize;

  while let Some(update) = rx.recv().await {
    match update {
      FeedUpdate::FetchingFeed(name) if !quiet => {
        colored_out(&format!("Fetching: {}", name), Color::DarkCyan, color);
      }
      FeedUpdate::UpdateSingle(feed) => {
        let (pos, interval) = config
          .feeds
          .iter()
          .enumerate()
          .find(|(_, fc)| fc.link == feed.url)
          .map(|(pos, fc)| {
            (
              pos,
              fc
                .refresh
                .as_deref()
                .and_then(config::parse_refresh_interval),
            )
          })
          .unwrap_or((0, None));
        match cache.save_feed(&feed, pos, interval) {
          Ok(_) => {
            if !quiet {
              colored_out(&format!("  OK: {}", feed.title), Color::DarkGreen, color);
            }
            updated += 1;
          }
          Err(e) => {
            colored_out(
              &format!("  Cache error: {}: {}", feed.title, e),
              Color::DarkRed,
              color,
            );
            errors += 1;
          }
        }
      }
      FeedUpdate::FeedError { name, error } => {
        colored_out(
          &format!("  Error: {}: {}", name, error),
          Color::DarkRed,
          color,
        );
        errors += 1;
      }
      FeedUpdate::FetchComplete => break,
      _ => {}
    }
  }

  if !quiet {
    println!();
  }
  colored_segment(
    &format!("{}/{} feeds updated", updated, total),
    Color::DarkGreen,
    color,
  );
  if !feeds_to_skip.is_empty() {
    print!(", ");
    colored_segment(
      &format!("{} skipped", feeds_to_skip.len()),
      Color::DarkYellow,
      color,
    );
  }
  if errors > 0 {
    print!(", ");
    colored_segment(&format!("{} error(s)", errors), Color::DarkRed, color);
  }
  println!();

  if errors > 0 {
    std::process::exit(1);
  }

  Ok(())
}

fn cmd_clean(dry_run: bool) -> io::Result<()> {
  use std::io::IsTerminal;
  let color = io::stdout().is_terminal();

  let config = match config::parse_config() {
    Ok(c) => c,
    Err(e) => {
      eprintln!("Error reading config: {}", e);
      std::process::exit(1);
    }
  };

  let cache_path = config::get_cache_path();
  let cache = match FeedCache::new(cache_path) {
    Ok(c) => c,
    Err(e) => {
      eprintln!("Failed to initialize cache: {}", e);
      std::process::exit(1);
    }
  };

  let active_urls: Vec<&str> = config.feeds.iter().map(|f| f.link.as_str()).collect();
  let dead = cache
    .list_dead_feeds(&active_urls)
    .map_err(|e| io::Error::other(e.to_string()))?;

  if dead.is_empty() {
    println!("Cache is clean. Nothing to remove.");
    return Ok(());
  }

  for (url, title) in &dead {
    colored_out(&format!("  - {} ({})", title, url), Color::DarkRed, color);
  }
  println!();

  if dry_run {
    println!(
      "{} orphaned feed(s) would be removed. (dry run — cache not modified)",
      dead.len()
    );
  } else {
    cache
      .remove_dead_feeds(&active_urls)
      .map_err(|e| io::Error::other(e.to_string()))?;
    println!("Removed {} orphaned feed(s).", dead.len());
  }

  Ok(())
}

fn cmd_stats() -> io::Result<()> {
  use std::io::IsTerminal;
  let color = io::stdout().is_terminal();

  let config = match config::parse_config() {
    Ok(c) => c,
    Err(e) => {
      eprintln!("Error reading config: {}", e);
      std::process::exit(1);
    }
  };

  let cache_path = config::get_cache_path();
  let cache = match FeedCache::new(cache_path) {
    Ok(c) => c,
    Err(e) => {
      eprintln!("Failed to initialize cache: {}", e);
      std::process::exit(1);
    }
  };

  let active_urls: Vec<&str> = config.feeds.iter().map(|f| f.link.as_str()).collect();
  let stats = cache
    .get_stats()
    .map_err(|e| io::Error::other(e.to_string()))?;
  let orphaned = cache
    .list_dead_feeds(&active_urls)
    .map_err(|e| io::Error::other(e.to_string()))?
    .len();

  // ── Feeds section ──────────────────────────────────────────────────────
  println!("Feeds");
  print!("  In config: ");
  colored_segment(
    &format!("{:>6}", config.feeds.len()),
    Color::DarkCyan,
    color,
  );
  println!();
  print!("  In cache:  ");
  colored_segment(&format!("{:>6}", stats.feed_count), Color::DarkCyan, color);
  println!();
  if orphaned > 0 {
    print!("  Orphaned:  ");
    colored_segment(&format!("{:>6}", orphaned), Color::DarkYellow, color);
    print!("  (run 'shinbun clean' to remove)");
    println!();
  }

  // ── Entries section ────────────────────────────────────────────────────
  println!();
  println!("Entries");
  print!("  Total:     ");
  colored_segment(&format!("{:>6}", stats.entry_count), Color::DarkCyan, color);
  println!();

  if stats.entry_count > 0 {
    let read_pct = stats.read_count as f64 / stats.entry_count as f64 * 100.0;
    let unread_pct = stats.unread_count as f64 / stats.entry_count as f64 * 100.0;

    print!("  Read:      ");
    colored_segment(&format!("{:>6}", stats.read_count), Color::DarkGreen, color);
    colored_segment(&format!("  ({:.1}%)", read_pct), Color::DarkGreen, color);
    println!();

    print!("  Unread:    ");
    let unread_color = if stats.unread_count > 0 {
      Color::DarkYellow
    } else {
      Color::DarkCyan
    };
    colored_segment(&format!("{:>6}", stats.unread_count), unread_color, color);
    colored_segment(&format!("  ({:.1}%)", unread_pct), unread_color, color);
    println!();
  }

  Ok(())
}

fn feed_tags(feed: &config::Feed) -> String {
  feed
    .tags
    .as_deref()
    .filter(|t| !t.is_empty())
    .map(|t| format!(" [{}]", t.join(", ")))
    .unwrap_or_default()
}

/// Print `text` in `color` followed by a newline, falling back to plain text
/// when `use_color` is false (non-TTY or piped output).
fn colored_out(text: &str, color: Color, use_color: bool) {
  if use_color {
    let _ = execute!(
      io::stdout(),
      SetForegroundColor(color),
      Print(text),
      ResetColor,
      Print("\n"),
    );
  } else {
    println!("{}", text);
  }
}

/// Print `text` in `color` without a trailing newline (for building summary lines).
fn colored_segment(text: &str, color: Color, use_color: bool) {
  if use_color {
    let _ = execute!(
      io::stdout(),
      SetForegroundColor(color),
      Print(text),
      ResetColor,
    );
  } else {
    print!("{}", text);
  }
}

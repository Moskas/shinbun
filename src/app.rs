use crate::cache::FeedCache;
use crate::config::{Feed as FeedConfig, QueryFeed, UiConfig};
use crate::feeds::{self, Feed, FeedEntry};
use crate::query;
use crate::views::{entry_view, feeds_list_view};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::ListState;
use std::time::Instant;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
  BrowsingFeeds,
  BrowsingEntries,
  ViewingEntry,
}

/// Represents a feed or query feed in the display list
#[derive(Debug, Clone)]
pub enum DisplayFeed {
  /// A regular RSS feed
  Regular(Feed),
  /// A query feed with its name, query string, and aggregated entries
  Query {
    name: String,
    query: String,
    entries: Vec<FeedEntry>,
  },
}

impl DisplayFeed {
  /// Get the display title of the feed
  pub fn title(&self) -> &str {
    match self {
      DisplayFeed::Regular(feed) => &feed.title,
      DisplayFeed::Query { name, .. } => name,
    }
  }

  /// Get the entries for this feed
  pub fn entries(&self) -> &[FeedEntry] {
    match self {
      DisplayFeed::Regular(feed) => &feed.entries,
      DisplayFeed::Query { entries, .. } => entries,
    }
  }

  /// Get mutable entries for this feed
  pub fn entries_mut(&mut self) -> &mut Vec<FeedEntry> {
    match self {
      DisplayFeed::Regular(feed) => &mut feed.entries,
      DisplayFeed::Query { entries, .. } => entries,
    }
  }

  /// Check if this is a query feed
  pub fn is_query(&self) -> bool {
    matches!(self, DisplayFeed::Query { .. })
  }
}

/// Messages sent from background tasks to update feeds
#[derive(Clone)]
pub enum FeedUpdate {
  /// Replace all feeds with new data
  Replace(Vec<Feed>),
  /// Update a specific feed
  UpdateFeed(usize, Feed),
  /// Report progress on a specific feed
  FetchingFeed(String),
  /// Report a feed that failed to fetch or parse
  FeedError { name: String, error: String },
  /// All feeds finished fetching
  FetchComplete,
}

#[derive(Debug, Clone)]
pub struct FeedError {
  pub name: String,
  pub error: String,
}

#[derive(Debug, Clone, Copy)]
pub struct LoadingState {
  pub is_loading: bool,
  pub start_time: Instant,
}

impl LoadingState {
  pub fn new() -> Self {
    Self {
      is_loading: true,
      start_time: Instant::now(),
    }
  }

  pub fn start(&mut self) {
    self.is_loading = true;
    self.start_time = Instant::now();
  }

  pub fn stop(&mut self) {
    self.is_loading = false;
  }

  pub fn elapsed_secs(&self) -> u64 {
    self.start_time.elapsed().as_secs()
  }

  pub fn spinner_frame(&self) -> &'static str {
    if !self.is_loading {
      return "";
    }
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let index = (self.start_time.elapsed().as_millis() / 80) as usize % frames.len();
    frames[index]
  }
}

pub struct App {
  feeds: Vec<Feed>,
  display_feeds: Vec<DisplayFeed>,
  feed_config: Vec<FeedConfig>,
  query_config: Vec<QueryFeed>,
  feed_index: usize,
  feed_list_state: ListState,
  entry_list_state: ListState,
  state: AppState,
  entry_scroll: usize,
  ui_config: UiConfig,
  exit: bool,
  feed_tx: mpsc::UnboundedSender<FeedUpdate>,
  loading_state: LoadingState,
  current_feed: Option<String>,
  feed_errors: Vec<FeedError>,
  show_error_popup: bool,
  cache: FeedCache,
}

impl App {
  pub fn new(
    feeds: Vec<Feed>,
    ui_config: UiConfig,
    feed_config: Vec<FeedConfig>,
    query_config: Vec<QueryFeed>,
    feed_tx: mpsc::UnboundedSender<FeedUpdate>,
    cache: FeedCache,
  ) -> Self {
    let is_loading = feeds.is_empty();
    let display_feeds = Self::build_display_feeds(&feeds, &query_config);

    Self {
      feeds,
      display_feeds,
      feed_config,
      query_config,
      feed_index: 0,
      feed_list_state: ListState::default().with_selected(Some(0)),
      entry_list_state: ListState::default(),
      state: AppState::BrowsingFeeds,
      entry_scroll: 0,
      ui_config,
      exit: false,
      feed_tx,
      loading_state: if is_loading {
        LoadingState::new()
      } else {
        let mut state = LoadingState::new();
        state.stop();
        state
      },
      current_feed: None,
      feed_errors: Vec::new(),
      show_error_popup: false,
      cache,
    }
  }

  /// Build display feeds by combining query feeds and regular feeds
  fn build_display_feeds(feeds: &[Feed], query_config: &[QueryFeed]) -> Vec<DisplayFeed> {
    let mut display_feeds: Vec<DisplayFeed> = query_config
      .iter()
      .map(|qf| DisplayFeed::Query {
        name: qf.name.clone(),
        query: qf.query.clone(),
        entries: query::apply_query(feeds, &qf.query),
      })
      .collect();

    for feed in feeds {
      display_feeds.push(DisplayFeed::Regular(feed.clone()));
    }

    display_feeds
  }

  /// Rebuild display feeds after feeds change
  fn rebuild_display_feeds(&mut self) {
    let query_config = self.query_config.clone();
    self.display_feeds = Self::build_display_feeds(&self.feeds, &query_config);
  }

  pub fn should_exit(&self) -> bool {
    self.exit
  }

  pub fn handle_feed_update(&mut self, update: FeedUpdate) {
    match update {
      FeedUpdate::Replace(new_feeds) => {
        // Cache each feed
        for (i, feed) in new_feeds.iter().enumerate() {
          if let Err(e) = self.cache.save_feed(feed, i) {
            eprintln!("Failed to cache feed {}: {}", feed.title, e);
          }
        }
        self.feeds = new_feeds;
        self.rebuild_display_feeds();
      }
      FeedUpdate::UpdateFeed(_, feed) => {
        let position = self
          .feeds
          .iter()
          .position(|f| f.url == feed.url)
          .unwrap_or(self.feeds.len());

        if let Err(e) = self.cache.save_feed(&feed, position) {
          eprintln!("Failed to cache feed {}: {}", feed.title, e);
        }

        if let Some(existing) = self.feeds.iter_mut().find(|f| f.url == feed.url) {
          *existing = feed;
          self.rebuild_display_feeds();
        }
      }
      FeedUpdate::FetchingFeed(name) => {
        self.current_feed = Some(name);
      }
      FeedUpdate::FeedError { name, error } => {
        self.feed_errors.push(FeedError { name, error });
      }
      FeedUpdate::FetchComplete => {
        self.loading_state.stop();
        self.current_feed = None;
      }
    }
  }

  /// Trigger a refresh of all feeds
  pub fn refresh_feeds(&mut self) {
    if self.loading_state.is_loading {
      return;
    }

    self.loading_state.start();
    self.feed_errors.clear();
    self.show_error_popup = false;
    let feeds = self.feed_config.clone();
    let tx = self.feed_tx.clone();

    tokio::spawn(async move {
      feeds::fetch_feed_with_progress(feeds, tx).await;
    });
  }

  pub fn render(&mut self, frame: &mut Frame) {
    let area = frame.area();

    match self.state {
      AppState::ViewingEntry => {
        if let Some(display_feed) = self.display_feeds.get(self.feed_index) {
          if let Some(entry_idx) = self.entry_list_state.selected() {
            if let Some(entry) = display_feed.entries().get(entry_idx) {
              entry_view::render(
                frame,
                area,
                display_feed.title(),
                entry,
                &mut self.entry_scroll,
                self.ui_config.show_borders,
              );
              return;
            }
          }
        }
      }
      _ => {
        feeds_list_view::render(
          frame,
          area,
          &self.display_feeds,
          &mut self.feed_list_state,
          &mut self.entry_list_state,
          self.state,
          self.ui_config.split_view,
          self.ui_config.show_borders,
          &self.loading_state,
          self.current_feed.as_deref(),
          &self.feed_errors,
          self.show_error_popup,
        );
      }
    }
  }

  pub fn handle_key(&mut self, key: KeyEvent) {
    if self.show_error_popup {
      match key.code {
        KeyCode::Esc | KeyCode::Char('e') | KeyCode::Char('E') | KeyCode::Enter => {
          self.show_error_popup = false;
          return;
        }
        _ => return,
      }
    }

    match key.code {
      KeyCode::Char('q') | KeyCode::Char('Q') => self.exit = true,
      KeyCode::Char('r') | KeyCode::Char('R') => self.refresh_feeds(),
      KeyCode::Char('e') | KeyCode::Char('E') => {
        if !self.feed_errors.is_empty() {
          self.show_error_popup = !self.show_error_popup;
        }
      }
      KeyCode::Up | KeyCode::Char('k') => self.handle_up(),
      KeyCode::Down | KeyCode::Char('j') => self.handle_down(),
      KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => self.handle_enter(),
      KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => self.handle_back(),
      _ => {}
    }
  }

  fn handle_up(&mut self) {
    match self.state {
      AppState::BrowsingFeeds => {
        if self.feed_index > 0 {
          self.feed_index -= 1;
          self.feed_list_state.select(Some(self.feed_index));
        }
      }
      AppState::BrowsingEntries => {
        if let Some(selected) = self.entry_list_state.selected() {
          if selected > 0 {
            self.entry_list_state.select(Some(selected - 1));
          }
        }
      }
      AppState::ViewingEntry => {
        self.entry_scroll = self.entry_scroll.saturating_sub(1);
      }
    }
  }

  fn handle_down(&mut self) {
    match self.state {
      AppState::BrowsingFeeds => {
        if self.feed_index + 1 < self.display_feeds.len() {
          self.feed_index += 1;
          self.feed_list_state.select(Some(self.feed_index));
        }
      }
      AppState::BrowsingEntries => {
        if let Some(selected) = self.entry_list_state.selected() {
          if let Some(display_feed) = self.display_feeds.get(self.feed_index) {
            if selected + 1 < display_feed.entries().len() {
              self.entry_list_state.select(Some(selected + 1));
            }
          }
        }
      }
      AppState::ViewingEntry => {
        self.entry_scroll = self.entry_scroll.saturating_add(1);
      }
    }
  }

  fn handle_enter(&mut self) {
    match self.state {
      AppState::BrowsingFeeds => {
        self.state = AppState::BrowsingEntries;
        self.entry_list_state.select(Some(0));
      }
      AppState::BrowsingEntries => {
        // Mark the selected entry as read before viewing it
        if let Some(entry_idx) = self.entry_list_state.selected() {
          self.mark_selected_entry_read(entry_idx);
        }
        self.state = AppState::ViewingEntry;
        self.entry_scroll = 0;
      }
      AppState::ViewingEntry => {}
    }
  }

  fn handle_back(&mut self) {
    match self.state {
      AppState::ViewingEntry => {
        self.state = AppState::BrowsingEntries;
      }
      AppState::BrowsingEntries => {
        self.state = AppState::BrowsingFeeds;
      }
      AppState::BrowsingFeeds => {}
    }
  }

  /// Mark the entry at `entry_idx` in the current feed as read,
  /// both in-memory and in the database.
  fn mark_selected_entry_read(&mut self, entry_idx: usize) {
    let feed_idx = self.feed_index;

    // Collect the info we need before mutably borrowing display_feeds
    let info = self
      .display_feeds
      .get(feed_idx)
      .and_then(|df| df.entries().get(entry_idx))
      .map(|e| {
        (
          e.title.clone(),
          e.published.clone(),
          e.feed_title.clone(),
          e.read,
        )
      });

    let (title, published, feed_title_opt, already_read) = match info {
      Some(i) => i,
      None => return,
    };

    if already_read {
      return; // Nothing to do
    }

    // Persist to DB and update in-memory for Regular feeds
    match self.display_feeds.get(feed_idx) {
      Some(DisplayFeed::Regular(feed)) => {
        let url = feed.url.clone();
        let _ = self
          .cache
          .mark_entry_read(&url, &title, published.as_deref());

        // Update in-memory: display_feeds
        if let Some(DisplayFeed::Regular(feed)) = self.display_feeds.get_mut(feed_idx) {
          if let Some(entry) = feed.entries.get_mut(entry_idx) {
            entry.read = true;
          }
        }
        // Mirror into self.feeds
        if let Some(raw) = self.feeds.iter_mut().find(|f| f.url == url) {
          if let Some(entry) = raw.entries.get_mut(entry_idx) {
            entry.read = true;
          }
        }
      }
      Some(DisplayFeed::Query { .. }) => {
        // For query feeds, find the source feed by feed_title
        if let Some(source_title) = feed_title_opt {
          if let Some(raw) = self.feeds.iter_mut().find(|f| f.title == source_title) {
            let url = raw.url.clone();
            // Find the entry in the raw feed
            if let Some(raw_entry) = raw
              .entries
              .iter_mut()
              .find(|e| e.title == title && e.published == published)
            {
              raw_entry.read = true;
              let _ =
                self
                  .cache
                  .mark_entry_read(&url, &raw_entry.title, raw_entry.published.as_deref());
            }
          }
        }
        // Update the query feed entry in-memory
        if let Some(DisplayFeed::Query { entries, .. }) = self.display_feeds.get_mut(feed_idx) {
          if let Some(entry) = entries.get_mut(entry_idx) {
            entry.read = true;
          }
        }
      }
      None => {}
    }
  }
}

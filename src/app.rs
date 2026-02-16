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
  FetchingFeed(String), // Feed name/URL being fetched
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
  feeds: Vec<Feed>,                  // Raw feeds from cache/fetch
  display_feeds: Vec<DisplayFeed>,   // Combined query + regular feeds for display
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
    // Set initial loading state based on whether we have cached feeds
    let is_loading = feeds.is_empty();
    
    // Build initial display feeds
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
    let mut display_feeds = Vec::new();

    // Add query feeds first (they appear at the top)
    for query in query_config {
      let entries = query::apply_query(feeds, &query.query);
      display_feeds.push(DisplayFeed::Query {
        name: query.name.clone(),
        query: query.query.clone(),
        entries,
      });
    }

    // Add regular feeds
    for feed in feeds {
      display_feeds.push(DisplayFeed::Regular(feed.clone()));
    }

    display_feeds
  }

  /// Rebuild display feeds after feeds change
  fn rebuild_display_feeds(&mut self) {
    self.display_feeds = Self::build_display_feeds(&self.feeds, &self.query_config);
  }

  pub fn should_exit(&self) -> bool {
    self.exit
  }

  pub fn loading_state(&self) -> &LoadingState {
    &self.loading_state
  }

  pub fn current_feed(&self) -> Option<&str> {
    self.current_feed.as_deref()
  }

  pub fn feed_errors(&self) -> &[FeedError] {
    &self.feed_errors
  }

  pub fn show_error_popup(&self) -> bool {
    self.show_error_popup
  }

  /// Handle feed updates from background tasks
  pub fn handle_feed_update(&mut self, update: FeedUpdate) {
    match update {
      FeedUpdate::Replace(new_feeds) => {
        // Save each feed to cache with its position from config
        for feed in &new_feeds {
          // Find the position of this feed in the original config
          let position = self
            .feed_config
            .iter()
            .position(|config| config.link == feed.url)
            .unwrap_or(0);

          if let Err(e) = self.cache.save_feed(feed, position) {
            eprintln!("Failed to cache feed {}: {}", feed.title, e);
          }
        }

        // Merge new feeds with existing feeds (don't replace, merge by URL)
        let mut merged_feeds = self.feeds.clone();
        
        for new_feed in new_feeds {
          // Find if this feed already exists
          if let Some(existing) = merged_feeds.iter_mut().find(|f| f.url == new_feed.url) {
            // Update existing feed
            *existing = new_feed;
          } else {
            // Add new feed
            merged_feeds.push(new_feed);
          }
        }

        // Sort feeds by their config order before displaying
        merged_feeds.sort_by_key(|feed| {
          self
            .feed_config
            .iter()
            .position(|config| config.link == feed.url)
            .unwrap_or(usize::MAX)
        });

        self.feeds = merged_feeds;
        self.rebuild_display_feeds(); // Rebuild display feeds with new data
        self.loading_state.stop();
        self.current_feed = None;

        // Reset selection if current index is out of bounds
        if self.feed_index >= self.display_feeds.len() && !self.display_feeds.is_empty() {
          self.feed_index = 0;
          self.feed_list_state.select(Some(0));
        }

        // Show error popup if there were errors
        if !self.feed_errors.is_empty() {
          self.show_error_popup = true;
        }
      }
      FeedUpdate::UpdateFeed(index, feed) => {
        // Find the position of this feed in the original config
        let position = self
          .feed_config
          .iter()
          .position(|config| config.link == feed.url)
          .unwrap_or(0);

        // Save to cache
        if let Err(e) = self.cache.save_feed(&feed, position) {
          eprintln!("Failed to cache feed {}: {}", feed.title, e);
        }

        // Update in the feeds list
        if let Some(existing) = self.feeds.iter_mut().find(|f| f.url == feed.url) {
          *existing = feed;
          self.rebuild_display_feeds(); // Rebuild to update query feeds too
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
      return; // Already refreshing
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

    // Render based on current state
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
    // Close error popup if open
    if self.show_error_popup {
      match key.code {
        KeyCode::Esc | KeyCode::Char('e') | KeyCode::Char('E') | KeyCode::Enter => {
          self.show_error_popup = false;
          return;
        }
        _ => return, // Consume all other keys when popup is open
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
}

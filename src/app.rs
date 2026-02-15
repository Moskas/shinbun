use crate::config::{Feed as FeedConfig, UiConfig};
use crate::feeds::{self, Feed};
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
  feeds: Vec<Feed>,
  feed_config: Vec<FeedConfig>,
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
}

impl App {
  pub fn new(
    feeds: Vec<Feed>,
    ui_config: UiConfig,
    feed_config: Vec<FeedConfig>,
    feed_tx: mpsc::UnboundedSender<FeedUpdate>,
  ) -> Self {
    Self {
      feeds,
      feed_config,
      feed_index: 0,
      feed_list_state: ListState::default().with_selected(Some(0)),
      entry_list_state: ListState::default(),
      state: AppState::BrowsingFeeds,
      entry_scroll: 0,
      ui_config,
      exit: false,
      feed_tx,
      loading_state: LoadingState::new(),
      current_feed: None,
      feed_errors: Vec::new(),
      show_error_popup: false,
    }
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
        self.feeds = new_feeds;
        self.loading_state.stop();
        self.current_feed = None;

        // Reset selection if current index is out of bounds
        if self.feed_index >= self.feeds.len() && !self.feeds.is_empty() {
          self.feed_index = 0;
          self.feed_list_state.select(Some(0));
        }

        // Show error popup if there were errors
        if !self.feed_errors.is_empty() {
          self.show_error_popup = true;
        }
      }
      FeedUpdate::UpdateFeed(index, feed) => {
        if index < self.feeds.len() {
          self.feeds[index] = feed;
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
        if let Some(feed) = self.feeds.get(self.feed_index) {
          if let Some(entry_idx) = self.entry_list_state.selected() {
            if let Some(entry) = feed.entries.get(entry_idx) {
              entry_view::render(
                frame,
                area,
                feed,
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
          &self.feeds,
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
        if self.feed_index + 1 < self.feeds.len() {
          self.feed_index += 1;
          self.feed_list_state.select(Some(self.feed_index));
        }
      }
      AppState::BrowsingEntries => {
        if let Some(selected) = self.entry_list_state.selected() {
          if let Some(feed) = self.feeds.get(self.feed_index) {
            if selected + 1 < feed.entries.len() {
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

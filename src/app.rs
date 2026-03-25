use crate::cache::FeedCache;
use crate::config::{Feed as FeedConfig, GeneralConfig, QueryFeed, UiConfig};
use crate::feeds::{self, Feed, FeedEntry};
use crate::query;
use crate::views::{entry_view, feeds_list_view, help_view, links_view};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::TableState;
use std::process::Command;
use std::time::Instant;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
  BrowsingFeeds,
  BrowsingEntries,
  ViewingEntry,
}

/// Represents a feed or query feed in the display list.
///
/// `Regular` stores an *index* into `App::feeds` rather than a clone of the
/// feed data.  This means feed content lives in exactly one place in memory
/// (`App::feeds`) and the display list is just a thin index + query layer.
#[derive(Debug, Clone)]
pub enum DisplayFeed {
  /// Index into App::feeds — zero extra allocation.
  Regular(usize),
  /// A query feed with aggregated entries (cross-feed, so must own its data).
  Query {
    name: String,
    entries: Vec<FeedEntry>,
  },
}

impl DisplayFeed {
  /// Resolve the display title, borrowing from `feeds` for Regular variants.
  pub fn title<'a>(&'a self, feeds: &'a [Feed]) -> &'a str {
    match self {
      DisplayFeed::Regular(i) => feeds.get(*i).map(|f| f.title.as_str()).unwrap_or(""),
      DisplayFeed::Query { name, .. } => name,
    }
  }

  /// Resolve the entry slice, borrowing from `feeds` for Regular variants.
  pub fn entries<'a>(&'a self, feeds: &'a [Feed]) -> &'a [FeedEntry] {
    match self {
      DisplayFeed::Regular(i) => feeds.get(*i).map(|f| f.entries.as_slice()).unwrap_or(&[]),
      DisplayFeed::Query { entries, .. } => entries,
    }
  }

  pub fn is_query(&self) -> bool {
    matches!(self, DisplayFeed::Query { .. })
  }
}

/// Messages sent from background tasks to update feeds
#[derive(Clone)]
pub enum FeedUpdate {
  /// Replace all feeds with new data
  Replace(Vec<Feed>),
  UpdateSingle(Feed),
  /// Report progress on a specific feed
  FetchingFeed(String),
  /// Report a feed that failed to fetch or parse
  FeedError {
    name: String,
    error: String,
  },
  /// All feeds finished fetching — reload from cache now
  FetchComplete,
}

#[derive(Debug, Clone)]
pub struct FeedError {
  pub name: String,
  pub error: String,
}

#[derive(Debug, Clone)]
pub struct LoadingState {
  pub is_loading: bool,
  pub start_time: Instant,
  pub is_initial_load: bool,
  pub finish_time: Option<Instant>,
  pub updated_feeds: Vec<String>,
}

impl LoadingState {
  pub fn new() -> Self {
    Self {
      is_loading: true,
      is_initial_load: true,
      start_time: Instant::now(),
      finish_time: None,
      updated_feeds: Vec::new(),
    }
  }

  /// Create a loading state that starts in the idle (not loading) position.
  pub fn idle() -> Self {
    let mut state = Self::new();
    state.stop();
    state
  }

  pub fn start(&mut self) {
    self.is_loading = true;
    self.is_initial_load = false;
    self.start_time = Instant::now();
    self.finish_time = None;
    self.updated_feeds.clear();
  }

  pub fn stop(&mut self) {
    self.is_loading = false;
    self.finish_time = Some(Instant::now());
  }

  pub fn elapsed_secs(&self) -> u64 {
    self.start_time.elapsed().as_secs()
  }

  /// Returns true while loading, and for 3 seconds after loading finishes.
  pub fn should_show_popup(&self) -> bool {
    if self.is_loading {
      return true;
    }
    if let Some(finish) = self.finish_time {
      return finish.elapsed().as_secs() < 3;
    }
    false
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

/// Tracks all state that is created and mutated directly by keypresses.
#[derive(Debug, Default)]
pub struct InputState {
  /// True while waiting for a second 'g' to complete the `gg` (go-to-top) sequence.
  pub vim_g: bool,
  /// When true, read entries are hidden from the entry list.
  pub hide_read: bool,
  /// The real (unfiltered) entry index captured the moment Enter is pressed to
  /// open an entry. Stored before the entry is marked read so the visible→real
  /// mapping stays stable for the entire duration of ViewingEntry.
  pub current_entry_relative_index: Option<usize>,
}

impl InputState {
  /// Clear any multi-key sequence in progress.
  pub fn cancel_sequence(&mut self) {
    self.vim_g = false;
  }
}

pub struct App {
  feeds: Vec<Feed>,
  display_feeds: Vec<DisplayFeed>,
  feed_config: Vec<FeedConfig>,
  query_config: Vec<QueryFeed>,
  feed_index: usize,
  feed_list_state: TableState,
  entry_list_state: TableState,
  state: AppState,
  entry_scroll: usize,
  general_config: GeneralConfig,
  ui_config: UiConfig,
  exit: bool,
  feed_tx: mpsc::UnboundedSender<FeedUpdate>,
  loading_state: LoadingState,
  current_feed: Option<String>,
  feed_errors: Vec<FeedError>,
  show_error_popup: bool,
  error_scroll: usize,
  show_help_popup: bool,
  help_scroll: usize,
  show_links_popup: bool,
  links_scroll: usize,
  links_selected: usize,
  show_confirm_popup: bool,
  confirm_feed_name: String,
  input: InputState,
  cache: FeedCache,
  /// Cached visible (unfiltered) entry indices for the currently selected feed.
  /// Invalidated whenever `hide_read`, `feed_index`, or entry data changes.
  visible_indices_cache: Vec<usize>,
}

impl App {
  pub fn new(
    feeds: Vec<Feed>,
    general_config: GeneralConfig,
    ui_config: UiConfig,
    feed_config: Vec<FeedConfig>,
    query_config: Vec<QueryFeed>,
    feed_tx: mpsc::UnboundedSender<FeedUpdate>,
    cache: FeedCache,
  ) -> Self {
    let is_loading = feeds.is_empty();
    let display_feeds = Self::build_display_feeds(&feeds, &query_config);
    let hide_read = !ui_config.show_read_entries;

    Self {
      feeds,
      display_feeds,
      feed_config,
      query_config,
      feed_index: 0,
      feed_list_state: TableState::default().with_selected(Some(0)),
      entry_list_state: TableState::default(),
      state: AppState::BrowsingFeeds,
      entry_scroll: 0,
      general_config,
      ui_config,
      exit: false,
      feed_tx,
      loading_state: if is_loading {
        LoadingState::new()
      } else {
        LoadingState::idle()
      },
      current_feed: None,
      feed_errors: Vec::new(),
      show_error_popup: false,
      error_scroll: 0,
      show_help_popup: false,
      help_scroll: 0,
      show_links_popup: false,
      links_scroll: 0,
      links_selected: 0,
      show_confirm_popup: false,
      confirm_feed_name: String::new(),
      input: InputState {
        hide_read,
        ..InputState::default()
      },
      cache,
      visible_indices_cache: Vec::new(),
    }
  }

  /// Build display feeds from the canonical feed list.
  ///
  /// Regular feeds are stored as plain indices — no Feed data is cloned.
  /// Query feeds must own their aggregated entry list (cross-feed aggregation).
  fn build_display_feeds(feeds: &[Feed], query_config: &[QueryFeed]) -> Vec<DisplayFeed> {
    let mut display_feeds: Vec<DisplayFeed> = query_config
      .iter()
      .map(|qf| DisplayFeed::Query {
        name: qf.name.clone(), // owned String required by the enum variant
        entries: query::apply_query(feeds, &qf.query),
      })
      .collect();

    for i in 0..feeds.len() {
      display_feeds.push(DisplayFeed::Regular(i));
    }

    display_feeds
  }

  /// Rebuild display feeds, freeing the old allocation before building the new one.
  fn rebuild_display_feeds(&mut self) {
    self.display_feeds.clear();
    self.display_feeds = Self::build_display_feeds(&self.feeds, &self.query_config);
    self.invalidate_visible_indices();
  }

  pub fn should_exit(&self) -> bool {
    self.exit
  }

  /// Record an internal error so the user can see it in the TUI error popup.
  fn push_error(&mut self, name: impl Into<String>, error: impl Into<String>) {
    self.feed_errors.push(FeedError {
      name: name.into(),
      error: error.into(),
    });
  }

  pub fn handle_feed_update(&mut self, update: FeedUpdate) {
    match update {
      FeedUpdate::Replace(new_feeds) => {
        for feed in new_feeds.iter() {
          self.loading_state.updated_feeds.push(feed.title.clone());
        }
        for (i, feed) in new_feeds.iter().enumerate() {
          if let Err(e) = self.cache.save_feed(feed, i) {
            self.push_error(&feed.title, format!("Failed to cache: {}", e));
          }
        }
        drop(new_feeds);
        self.feeds.clear();
        self.feeds = self.cache.load_all_feeds().unwrap_or_default();
        self.rebuild_display_feeds();
      }

      FeedUpdate::FetchingFeed(name) => {
        self.current_feed = Some(name);
      }

      FeedUpdate::FeedError { name, error } => {
        self.feed_errors.push(FeedError { name, error });
      }

      FeedUpdate::UpdateSingle(feed) => {
        let pos = self
          .feed_config
          .iter()
          .position(|fc| fc.link == feed.url)
          .unwrap_or(0);
        self.loading_state.updated_feeds.push(feed.title.clone());
        if let Err(e) = self.cache.save_feed(&feed, pos) {
          self.push_error(&feed.title, format!("Failed to cache: {}", e));
        }
        self.feeds = self.cache.load_all_feeds().unwrap_or_default();
        self.rebuild_display_feeds();
      }

      FeedUpdate::FetchComplete => {
        self.feeds.clear();
        self.feeds = self.cache.load_all_feeds().unwrap_or_default();
        if self.loading_state.is_initial_load {
          self.loading_state.updated_feeds = self.feeds.iter().map(|f| f.title.clone()).collect();
        }
        self.rebuild_display_feeds();
        self.loading_state.stop();
        self.current_feed = None;
      }
    }
  }

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

  pub fn refresh_selected_feed(&mut self) {
    if self.loading_state.is_loading {
      return;
    }

    let selected = self.feed_list_state.selected().unwrap_or(0);

    let feeds_to_refresh: Vec<FeedConfig> = match self.display_feeds.get(selected) {
      Some(DisplayFeed::Regular(i)) => {
        // Single feed — find its config by URL
        self
          .feeds
          .get(*i)
          .and_then(|f| self.feed_config.iter().find(|fc| fc.link == f.url))
          .cloned()
          .map(|fc| vec![fc])
          .unwrap_or_default()
      }

      Some(DisplayFeed::Query { name, .. }) => {
        // Find the query string for this named query feed, then filter
        // feed_config to only the entries whose tags match.
        let query_str = self
          .query_config
          .iter()
          .find(|qf| qf.name == *name)
          .map(|qf| qf.query.clone());

        match query_str {
          Some(q) => {
            let filter = query::parse_query(&q);
            self
              .feed_config
              .iter()
              .filter(|fc| query::config_feed_matches(fc, &filter))
              .cloned()
              .collect()
          }
          None => vec![],
        }
      }

      None => vec![],
    };

    if feeds_to_refresh.is_empty() {
      return;
    }

    self.loading_state.start();
    self.feed_errors.clear();
    self.show_error_popup = false;

    let tx = self.feed_tx.clone();
    tokio::spawn(async move {
      feeds::fetch_feeds_subset_with_progress(feeds_to_refresh, tx).await;
    });
  }

  pub fn render(&mut self, frame: &mut Frame) {
    let area = frame.area();

    match self.state {
      AppState::ViewingEntry => {
        if let Some(real_idx) = self.input.current_entry_relative_index {
          if let Some(display_feed) = self.display_feeds.get(self.feed_index) {
            if let Some(entry) = display_feed.entries(&self.feeds).get(real_idx) {
              entry_view::render(
                frame,
                area,
                display_feed.title(&self.feeds),
                entry,
                &mut self.entry_scroll,
                self.ui_config.show_borders,
              );
            }
          }
        }
      }
      _ => {
        feeds_list_view::render(
          frame,
          area,
          &mut feeds_list_view::FeedsViewState {
            raw_feeds: &self.feeds,
            display_feeds: &self.display_feeds,
            feed_state: &mut self.feed_list_state,
            entry_state: &mut self.entry_list_state,
            app_state: self.state,
            show_borders: self.ui_config.show_borders,
            loading_state: &self.loading_state,
            current_feed: self.current_feed.as_deref(),
            feed_errors: &self.feed_errors,
            show_error_popup: self.show_error_popup,
            error_scroll: &mut self.error_scroll,
            hide_read: self.input.hide_read,
          },
        );
      }
    }

    if self.show_help_popup {
      help_view::render_help_popup(frame, area, &mut self.help_scroll);
    }

    if self.show_links_popup {
      if let Some(real_idx) = self.input.current_entry_relative_index {
        if let Some(display_feed) = self.display_feeds.get(self.feed_index) {
          if let Some(entry) = display_feed.entries(&self.feeds).get(real_idx) {
            links_view::render_links_popup(
              frame,
              area,
              &entry.links,
              &mut self.links_selected,
              &mut self.links_scroll,
            );
          }
        }
      }
    }

    if self.show_confirm_popup {
      feeds_list_view::render_confirm_popup(frame, area, &self.confirm_feed_name);
    }
  }

  pub fn handle_key(&mut self, key: KeyEvent) {
    if self.show_help_popup {
      match key.code {
        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter => {
          self.show_help_popup = false;
          self.help_scroll = 0;
          return;
        }
        KeyCode::Down | KeyCode::Char('j') => {
          self.help_scroll = self.help_scroll.saturating_add(1);
          return;
        }
        KeyCode::Up | KeyCode::Char('k') => {
          self.help_scroll = self.help_scroll.saturating_sub(1);
          return;
        }
        KeyCode::Home | KeyCode::Char('g') => {
          self.help_scroll = 0;
          return;
        }
        KeyCode::End | KeyCode::Char('G') => {
          self.help_scroll = usize::MAX;
          return;
        }
        _ => return,
      }
    }

    if self.show_error_popup {
      match key.code {
        KeyCode::Esc | KeyCode::Char('e') | KeyCode::Char('E') | KeyCode::Enter => {
          self.show_error_popup = false;
          self.error_scroll = 0;
          return;
        }
        KeyCode::Down | KeyCode::Char('j') => {
          self.error_scroll = self.error_scroll.saturating_add(1);
          return;
        }
        KeyCode::Up | KeyCode::Char('k') => {
          self.error_scroll = self.error_scroll.saturating_sub(1);
          return;
        }
        KeyCode::Home | KeyCode::Char('g') => {
          self.error_scroll = 0;
          return;
        }
        KeyCode::End | KeyCode::Char('G') => {
          self.error_scroll = usize::MAX;
          return;
        }
        _ => return,
      }
    }

    if self.show_links_popup {
      match key.code {
        KeyCode::Esc | KeyCode::Char('L') => {
          self.show_links_popup = false;
          self.links_scroll = 0;
          self.links_selected = 0;
          return;
        }
        KeyCode::Down | KeyCode::Char('j') => {
          self.links_selected = self.links_selected.saturating_add(1);
          // Clamping happens in the render function
          return;
        }
        KeyCode::Up | KeyCode::Char('k') => {
          self.links_selected = self.links_selected.saturating_sub(1);
          return;
        }
        KeyCode::Home | KeyCode::Char('g') => {
          self.links_selected = 0;
          self.links_scroll = 0;
          return;
        }
        KeyCode::End | KeyCode::Char('G') => {
          self.links_selected = usize::MAX;
          return;
        }
        KeyCode::Enter | KeyCode::Char('o') | KeyCode::Char('O') => {
          self.open_selected_link();
          return;
        }
        _ => return,
      }
    }

    if self.show_confirm_popup {
      match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
          self.show_confirm_popup = false;
          self.mark_current_feed_read();
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
          self.show_confirm_popup = false;
          self.confirm_feed_name.clear();
        }
        _ => {} // ignore all other keys while popup is open
      }
      return;
    }

    match key.code {
      KeyCode::Char('q') | KeyCode::Char('Q') => self.exit = true,
      KeyCode::Char('?') => {
        self.show_help_popup = !self.show_help_popup;
        self.help_scroll = 0;
      }
      KeyCode::Char('r') => self.refresh_selected_feed(),
      KeyCode::Char('R') => self.refresh_feeds(),
      KeyCode::Char('e') | KeyCode::Char('E') => {
        if !self.feed_errors.is_empty() {
          self.show_error_popup = !self.show_error_popup;
        }
      }
      KeyCode::Char('u') | KeyCode::Char('U') => {
        self.input.cancel_sequence();
        self.input.hide_read = !self.input.hide_read;
        self.invalidate_visible_indices();
        // Reset selection so it doesn't point out-of-bounds in the filtered list
        self.entry_list_state.select(Some(0));
      }
      KeyCode::Char('m') | KeyCode::Char('M') => match self.state {
        AppState::BrowsingEntries => {
          if let Some(visible_idx) = self.entry_list_state.selected() {
            let real_idx = self
              .visible_to_real_entry_idx(visible_idx)
              .unwrap_or(visible_idx);
            self.toggle_selected_entry_read(real_idx);
          }
        }
        AppState::ViewingEntry => {
          if let Some(real_idx) = self.input.current_entry_relative_index {
            self.toggle_selected_entry_read(real_idx);
          }
        }
        _ => {}
      },
      KeyCode::Char('A') => match self.state {
        AppState::BrowsingFeeds | AppState::BrowsingEntries | AppState::ViewingEntry => {
          self.request_mark_feed_read();
        }
        #[allow(unreachable_patterns)]
        _ => {}
      },
      KeyCode::Char('o') | KeyCode::Char('O') => match self.state {
        AppState::BrowsingEntries | AppState::ViewingEntry => {
          self.open_current_entry_in_browser();
        }
        _ => {}
      },
      KeyCode::Char('p') | KeyCode::Char('P') => match self.state {
        AppState::BrowsingEntries | AppState::ViewingEntry => {
          self.open_media_in_player();
        }
        _ => {}
      },
      KeyCode::Char('L') => match self.state {
        AppState::ViewingEntry => {
          self.show_links_popup = !self.show_links_popup;
          self.links_scroll = 0;
          self.links_selected = 0;
        }
        _ => {}
      },
      KeyCode::Up | KeyCode::Char('k') => {
        self.input.cancel_sequence();
        self.handle_up();
      }
      KeyCode::Down | KeyCode::Char('j') => {
        self.input.cancel_sequence();
        self.handle_down();
      }
      KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
        self.input.cancel_sequence();
        self.handle_enter();
      }
      KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => {
        self.input.cancel_sequence();
        self.handle_back();
      }
      KeyCode::Home => {
        self.input.cancel_sequence();
        self.handle_go_top();
      }
      KeyCode::End => {
        self.input.cancel_sequence();
        self.handle_go_bottom();
      }
      KeyCode::Char('G') => {
        self.input.cancel_sequence();
        self.handle_go_bottom();
      }
      KeyCode::Char('g') => {
        if self.input.vim_g {
          self.input.vim_g = false;
          self.handle_go_top();
        } else {
          self.input.vim_g = true;
        }
      }
      _ => {
        self.input.cancel_sequence();
      }
    }
  }

  // ─── Visible entry helpers ────────────────────────────────────────────────

  /// Rebuild the cached visible-entry-indices vector.
  /// Must be called whenever `hide_read`, `feed_index`, or entry read-state changes.
  fn invalidate_visible_indices(&mut self) {
    self.visible_indices_cache.clear();
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    self.visible_indices_cache = df
      .entries(&self.feeds)
      .iter()
      .enumerate()
      .filter(|(_, e)| !self.input.hide_read || !e.read)
      .map(|(i, _)| i)
      .collect();
  }

  /// Returns the cached visible (unfiltered) entry indices for the current feed.
  fn visible_entry_indices(&self) -> &[usize] {
    &self.visible_indices_cache
  }

  /// Maps a visible (filtered) list position to the real entry index in the feed.
  fn visible_to_real_entry_idx(&self, visible_idx: usize) -> Option<usize> {
    self.visible_indices_cache.get(visible_idx).copied()
  }

  /// Resolve the real entry index for the currently focused entry,
  /// regardless of whether we are in BrowsingEntries or ViewingEntry.
  fn resolve_current_entry_idx(&self) -> Option<usize> {
    match self.state {
      AppState::ViewingEntry => self.input.current_entry_relative_index,
      _ => {
        let visible_idx = self.entry_list_state.selected()?;
        Some(
          self
            .visible_to_real_entry_idx(visible_idx)
            .unwrap_or(visible_idx),
        )
      }
    }
  }

  // ─── Navigation helpers ───────────────────────────────────────────────────

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
          let len = self.visible_entry_indices().len();
          if selected + 1 < len {
            self.entry_list_state.select(Some(selected + 1));
          }
        }
      }
      AppState::ViewingEntry => {
        self.entry_scroll = self.entry_scroll.saturating_add(1);
      }
    }
  }

  fn handle_go_top(&mut self) {
    match self.state {
      AppState::BrowsingFeeds => {
        self.feed_index = 0;
        self.feed_list_state.select(Some(0));
      }
      AppState::BrowsingEntries => {
        self.entry_list_state.select(Some(0));
      }
      AppState::ViewingEntry => {
        self.entry_scroll = 0;
      }
    }
  }

  fn handle_go_bottom(&mut self) {
    match self.state {
      AppState::BrowsingFeeds => {
        let last = self.display_feeds.len().saturating_sub(1);
        self.feed_index = last;
        self.feed_list_state.select(Some(last));
      }
      AppState::BrowsingEntries => {
        let last = self.visible_entry_indices().len().saturating_sub(1);
        self.entry_list_state.select(Some(last));
      }
      AppState::ViewingEntry => {
        // Set to usize::MAX; the render fn clamps it to max_scroll automatically
        self.entry_scroll = usize::MAX;
      }
    }
  }

  fn handle_enter(&mut self) {
    match self.state {
      AppState::BrowsingFeeds => {
        self.state = AppState::BrowsingEntries;
        self.invalidate_visible_indices();
        self.entry_list_state.select(Some(0));
      }
      AppState::BrowsingEntries => {
        if let Some(visible_idx) = self.entry_list_state.selected() {
          let real_idx = self
            .visible_to_real_entry_idx(visible_idx)
            .unwrap_or(visible_idx);
          // Store the real index BEFORE marking as read — once read, this entry
          // may vanish from the visible list and the mapping would shift.
          self.input.current_entry_relative_index = Some(real_idx);
          self.mark_selected_entry_read(real_idx);
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
        self.input.current_entry_relative_index = None;
        self.show_links_popup = false;
        self.links_scroll = 0;
        self.links_selected = 0;
        self.state = AppState::BrowsingEntries;
      }
      AppState::BrowsingEntries => self.state = AppState::BrowsingFeeds,
      AppState::BrowsingFeeds => {}
    }
  }

  // ─── Browser / player helpers ─────────────────────────────────────────────

  fn spawn_cmd(cmd: &str, url: &str) -> Result<(), String> {
    let mut parts = cmd.split_whitespace();
    if let Some(bin) = parts.next() {
      let args: Vec<&str> = parts.collect();
      Command::new(bin)
        .args(args)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to launch '{}': {}", cmd, e))?;
    }
    Ok(())
  }

  /// Open the first link of the currently selected entry in a browser.
  fn open_current_entry_in_browser(&mut self) {
    let Some(real_idx) = self.resolve_current_entry_idx() else {
      return;
    };
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let Some(entry) = df.entries(&self.feeds).get(real_idx) else {
      return;
    };
    let Some(url) = entry.links.first() else {
      return;
    };

    let result = if let Some(ref cmd) = self.general_config.browser {
      Self::spawn_cmd(cmd, url)
    } else {
      open::that(url).map_err(|e| format!("Failed to open URL in default browser: {}", e))
    };
    if let Err(e) = result {
      self.push_error("Browser", e);
    }
  }

  /// Open the currently selected link from the links popup in a browser.
  fn open_selected_link(&mut self) {
    let Some(real_idx) = self.resolve_current_entry_idx() else {
      return;
    };
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let Some(entry) = df.entries(&self.feeds).get(real_idx) else {
      return;
    };
    let selected = self.links_selected.min(entry.links.len().saturating_sub(1));
    let Some(url) = entry.links.get(selected) else {
      return;
    };

    let result = if let Some(ref cmd) = self.general_config.browser {
      Self::spawn_cmd(cmd, url)
    } else {
      open::that(url).map_err(|e| format!("Failed to open URL in default browser: {}", e))
    };
    if let Err(e) = result {
      self.push_error("Browser", e);
    }
  }

  /// Open the media attachment of the currently selected entry.
  fn open_media_in_player(&mut self) {
    let Some(real_idx) = self.resolve_current_entry_idx() else {
      return;
    };
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let Some(entry) = df.entries(&self.feeds).get(real_idx) else {
      return;
    };
    let Some(url) = &entry.media else { return };

    let result = if let Some(ref cmd) = self.general_config.media_player {
      Self::spawn_cmd(cmd, url)
    } else {
      open::that(url).map_err(|e| format!("Failed to open media URL with OS default: {}", e))
    };
    if let Err(e) = result {
      self.push_error("Media player", e);
    }
  }

  // ─── Read-state helpers ───────────────────────────────────────────────────

  /// Resolve a display entry to `(feed_vec_idx, entry_vec_idx, currently_read)`.
  ///
  /// Returns only `Copy` values so all borrows on `self` are fully released
  /// before the caller proceeds to mutate — eliminating the need to clone any
  /// string data.
  ///
  /// For `Regular` feeds the display-list entry index equals the raw entry index.
  /// For `Query` feeds the function locates the matching canonical entry in
  /// `self.feeds` by title + published date.  Because `self.display_feeds` and
  /// `self.feeds` are distinct struct fields, the borrow checker allows both to
  /// be borrowed simultaneously without conflict.
  fn resolve_entry(
    &self,
    display_feed_idx: usize,
    entry_idx: usize,
  ) -> Option<(usize, usize, bool)> {
    match self.display_feeds.get(display_feed_idx)? {
      DisplayFeed::Regular(i) => {
        let i = *i;
        let currently_read = self.feeds.get(i)?.entries.get(entry_idx)?.read;
        Some((i, entry_idx, currently_read))
      }

      DisplayFeed::Query { entries, .. } => {
        let entry = entries.get(entry_idx)?;
        let currently_read = entry.read;

        // Locate the source feed by its title.
        // self.feeds is a different field from self.display_feeds → simultaneous
        // immutable borrows are permitted by the borrow checker.
        let feed_vec_idx = entry
          .feed_title
          .as_deref()
          .and_then(|ft| self.feeds.iter().position(|f| f.title == ft))?;

        // Locate the raw entry by title + published date.
        let entry_title = entry.title.as_str();
        let entry_published = entry.published.as_deref();
        let entry_vec_idx = self.feeds[feed_vec_idx]
          .entries
          .iter()
          .position(|e| e.title == entry_title && e.published.as_deref() == entry_published)?;

        // Return only Copy values — all borrows on self are released here.
        Some((feed_vec_idx, entry_vec_idx, currently_read))
      }
    }
  }

  /// Update the `read` flag on the canonical entry and mirror the change into
  /// any query `DisplayFeed`s that reference the same entry.
  ///
  /// Takes indices instead of string keys.  This allows:
  ///   • a mutable borrow of `self.feeds` (to update the canonical entry), then
  ///   • an immutable borrow of `self.feeds` (for title/published match data), and
  ///   • a simultaneous mutable borrow of `self.display_feeds` (to update query copies),
  /// all without allocating — Rust's borrow checker permits simultaneous borrows
  /// of distinct struct fields.
  fn sync_read_state(&mut self, feed_vec_idx: usize, entry_vec_idx: usize, read: bool) {
    // 1. Update the one canonical copy in self.feeds.
    if let Some(entry) = self
      .feeds
      .get_mut(feed_vec_idx)
      .and_then(|f| f.entries.get_mut(entry_vec_idx))
    {
      entry.read = read;
    }

    // 2. Borrow title/published from self.feeds for query-entry matching.
    //    self.display_feeds is a different field → the mutable borrow below is allowed.
    let Some(feed) = self.feeds.get(feed_vec_idx) else {
      return;
    };
    let Some(entry) = feed.entries.get(entry_vec_idx) else {
      return;
    };
    let feed_title = feed.title.as_str();
    let entry_title = entry.title.as_str();
    let entry_published = entry.published.as_deref();

    // 3. Mirror into query DisplayFeeds (different field from self.feeds).
    for df in self.display_feeds.iter_mut() {
      if let DisplayFeed::Query { entries, .. } = df {
        for qe in entries.iter_mut() {
          if qe.feed_title.as_deref() == Some(feed_title)
            && qe.title == entry_title
            && qe.published.as_deref() == entry_published
          {
            qe.read = read;
          }
        }
      }
    }
  }

  fn toggle_selected_entry_read(&mut self, entry_idx: usize) {
    let Some((feed_vec_idx, entry_vec_idx, currently_read)) =
      self.resolve_entry(self.feed_index, entry_idx)
    else {
      return;
    };

    let new_read = !currently_read;

    // self.cache is a distinct struct field from self.feeds, so &mut self.cache
    // and &self.feeds[..] can coexist — no string clones needed for the arguments.
    let db_result = if new_read {
      self.cache.mark_entry_read(
        &self.feeds[feed_vec_idx].url,
        &self.feeds[feed_vec_idx].entries[entry_vec_idx].title,
        self.feeds[feed_vec_idx].entries[entry_vec_idx]
          .published
          .as_deref(),
      )
    } else {
      self.cache.mark_entry_unread(
        &self.feeds[feed_vec_idx].url,
        &self.feeds[feed_vec_idx].entries[entry_vec_idx].title,
        self.feeds[feed_vec_idx].entries[entry_vec_idx]
          .published
          .as_deref(),
      )
    };

    if let Err(e) = db_result {
      self.push_error("Cache", format!("Failed to toggle entry read state: {}", e));
      return;
    }

    self.sync_read_state(feed_vec_idx, entry_vec_idx, new_read);
    self.invalidate_visible_indices();
  }

  fn mark_selected_entry_read(&mut self, entry_idx: usize) {
    let Some((feed_vec_idx, entry_vec_idx, already_read)) =
      self.resolve_entry(self.feed_index, entry_idx)
    else {
      return;
    };

    if already_read {
      return;
    }

    // Same field-borrow trick — no clone needed.
    if let Err(e) = self.cache.mark_entry_read(
      &self.feeds[feed_vec_idx].url,
      &self.feeds[feed_vec_idx].entries[entry_vec_idx].title,
      self.feeds[feed_vec_idx].entries[entry_vec_idx]
        .published
        .as_deref(),
    ) {
      self.push_error("Cache", format!("Failed to mark entry read: {}", e));
      return;
    }

    self.sync_read_state(feed_vec_idx, entry_vec_idx, true);
    self.invalidate_visible_indices();
  }

  /// Show the confirmation popup for marking the currently focused feed as read.
  fn request_mark_feed_read(&mut self) {
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let name = df.title(&self.feeds).to_string();
    if df.entries(&self.feeds).is_empty() {
      return;
    }
    self.confirm_feed_name = name;
    self.show_confirm_popup = true;
  }

  /// Actually mark every entry of the focused feed as read.
  /// Called after the user confirms with 'y' in the popup.
  fn mark_current_feed_read(&mut self) {
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      self.confirm_feed_name.clear();
      return;
    };

    match df {
      DisplayFeed::Regular(i) => {
        let i = *i;
        let Some(feed) = self.feeds.get(i) else {
          self.confirm_feed_name.clear();
          return;
        };
        if let Err(e) = self.cache.mark_feed_read(&feed.url) {
          self.push_error("Cache", format!("Failed to mark feed as read: {}", e));
          self.confirm_feed_name.clear();
          return;
        }
        // Update in-memory state for every entry
        if let Some(feed) = self.feeds.get_mut(i) {
          for entry in feed.entries.iter_mut() {
            entry.read = true;
          }
        }
      }
      DisplayFeed::Query { .. } => {
        // For query feeds, mark each source feed entry individually
        let entries: Vec<(String, String, Option<String>)> = df
          .entries(&self.feeds)
          .iter()
          .filter_map(|e| {
            let feed_title = e.feed_title.as_deref()?;
            let feed_url = self
              .feeds
              .iter()
              .find(|f| f.title == feed_title)
              .map(|f| f.url.clone())?;
            Some((feed_url, e.title.clone(), e.published.clone()))
          })
          .collect();

        for (feed_url, entry_title, published) in &entries {
          if let Err(e) = self
            .cache
            .mark_entry_read(feed_url, entry_title, published.as_deref())
          {
            self.push_error("Cache", format!("Failed to mark entry read: {}", e));
          }
        }

        // Update in-memory state
        for (feed_url, entry_title, published) in &entries {
          if let Some(feed) = self.feeds.iter_mut().find(|f| f.url == *feed_url) {
            if let Some(entry) = feed
              .entries
              .iter_mut()
              .find(|e| e.title == *entry_title && e.published.as_deref() == published.as_deref())
            {
              entry.read = true;
            }
          }
        }
      }
    }

    // Mirror changes into query display feeds
    self.rebuild_display_feeds();
    self.invalidate_visible_indices();
    self.confirm_feed_name.clear();
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn make_entry(title: &str, published: Option<&str>, read: bool) -> FeedEntry {
    FeedEntry {
      title: title.to_string(),
      published: published.map(|s| s.to_string()),
      text: String::new(),
      links: vec![],
      media: None,
      feed_title: None,
      read,
    }
  }

  fn make_feed(url: &str, title: &str, entries: Vec<FeedEntry>) -> Feed {
    Feed {
      url: url.to_string(),
      title: title.to_string(),
      entries,
      tags: None,
    }
  }

  // ─── DisplayFeed tests ─────────────────────────────────────────────────

  #[test]
  fn test_display_feed_regular_title() {
    let feeds = vec![make_feed("http://a.com", "Feed A", vec![])];
    let df = DisplayFeed::Regular(0);
    assert_eq!(df.title(&feeds), "Feed A");
  }

  #[test]
  fn test_display_feed_regular_out_of_bounds() {
    let feeds: Vec<Feed> = vec![];
    let df = DisplayFeed::Regular(5);
    assert_eq!(df.title(&feeds), "");
    assert!(df.entries(&feeds).is_empty());
  }

  #[test]
  fn test_display_feed_query_title() {
    let feeds: Vec<Feed> = vec![];
    let df = DisplayFeed::Query {
      name: "All Blogs".to_string(),
      entries: vec![],
    };
    assert_eq!(df.title(&feeds), "All Blogs");
  }

  #[test]
  fn test_display_feed_query_entries() {
    let feeds: Vec<Feed> = vec![];
    let entries = vec![make_entry("Post 1", None, false)];
    let df = DisplayFeed::Query {
      name: "Q".to_string(),
      entries,
    };
    assert_eq!(df.entries(&feeds).len(), 1);
    assert_eq!(df.entries(&feeds)[0].title, "Post 1");
  }

  #[test]
  fn test_display_feed_is_query() {
    assert!(!DisplayFeed::Regular(0).is_query());
    assert!(DisplayFeed::Query {
      name: "Q".to_string(),
      entries: vec![]
    }
    .is_query());
  }

  #[test]
  fn test_display_feed_regular_entries() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];
    let df = DisplayFeed::Regular(0);
    assert_eq!(df.entries(&feeds).len(), 1);
    assert_eq!(df.entries(&feeds)[0].title, "Post 1");
  }

  // ─── LoadingState tests ────────────────────────────────────────────────

  #[test]
  fn test_loading_state_new() {
    let state = LoadingState::new();
    assert!(state.is_loading);
    assert!(state.is_initial_load);
    assert!(state.finish_time.is_none());
    assert!(state.updated_feeds.is_empty());
  }

  #[test]
  fn test_loading_state_idle() {
    let state = LoadingState::idle();
    assert!(!state.is_loading);
    assert!(state.finish_time.is_some());
  }

  #[test]
  fn test_loading_state_start_stop() {
    let mut state = LoadingState::idle();
    state.start();
    assert!(state.is_loading);
    assert!(!state.is_initial_load);
    assert!(state.finish_time.is_none());
    assert!(state.updated_feeds.is_empty());

    state.stop();
    assert!(!state.is_loading);
    assert!(state.finish_time.is_some());
  }

  #[test]
  fn test_loading_state_should_show_popup_while_loading() {
    let state = LoadingState::new();
    assert!(state.should_show_popup());
  }

  #[test]
  fn test_loading_state_should_show_popup_after_stop() {
    let mut state = LoadingState::new();
    state.stop();
    // Should still show for up to 3 seconds
    assert!(state.should_show_popup());
  }

  #[test]
  fn test_loading_state_spinner_frame_while_loading() {
    let state = LoadingState::new();
    let frame = state.spinner_frame();
    assert!(!frame.is_empty());
  }

  #[test]
  fn test_loading_state_spinner_frame_when_not_loading() {
    let state = LoadingState::idle();
    assert_eq!(state.spinner_frame(), "");
  }

  #[test]
  fn test_loading_state_start_clears_updated_feeds() {
    let mut state = LoadingState::new();
    state.updated_feeds.push("Feed A".to_string());
    state.start();
    assert!(state.updated_feeds.is_empty());
  }

  // ─── InputState tests ─────────────────────────────────────────────────

  #[test]
  fn test_input_state_default() {
    let input = InputState::default();
    assert!(!input.vim_g);
    assert!(!input.hide_read);
    assert!(input.current_entry_relative_index.is_none());
  }

  #[test]
  fn test_input_state_cancel_sequence() {
    let mut input = InputState::default();
    input.vim_g = true;
    input.cancel_sequence();
    assert!(!input.vim_g);
  }

  // ─── build_display_feeds tests ─────────────────────────────────────────

  #[test]
  fn test_build_display_feeds_no_queries() {
    let feeds = vec![
      make_feed("http://a.com", "Feed A", vec![]),
      make_feed("http://b.com", "Feed B", vec![]),
    ];
    let queries: Vec<QueryFeed> = vec![];
    let display = App::build_display_feeds(&feeds, &queries);
    assert_eq!(display.len(), 2);
    assert!(matches!(display[0], DisplayFeed::Regular(0)));
    assert!(matches!(display[1], DisplayFeed::Regular(1)));
  }

  #[test]
  fn test_build_display_feeds_with_queries() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];
    let mut feed_with_tags = feeds[0].clone();
    feed_with_tags.tags = Some(vec!["blog".to_string()]);
    let feeds = vec![feed_with_tags];

    let queries = vec![QueryFeed {
      name: "All Blogs".to_string(),
      query: "tags:blog".to_string(),
    }];

    let display = App::build_display_feeds(&feeds, &queries);
    // Query feeds come first, then regular feeds
    assert_eq!(display.len(), 2);
    assert!(display[0].is_query());
    assert_eq!(display[0].title(&feeds), "All Blogs");
    assert!(!display[1].is_query());
  }

  #[test]
  fn test_build_display_feeds_empty() {
    let feeds: Vec<Feed> = vec![];
    let queries: Vec<QueryFeed> = vec![];
    let display = App::build_display_feeds(&feeds, &queries);
    assert!(display.is_empty());
  }

  // ─── App integration tests (with in-memory cache) ─────────────────────

  fn make_test_app() -> App {
    let (tx, _rx) = mpsc::unbounded_channel();
    let cache = crate::cache::FeedCache::new_in_memory().unwrap();
    App::new(
      vec![],
      GeneralConfig::default(),
      UiConfig {
        show_borders: true,
        show_read_entries: true,
      },
      vec![],
      vec![],
      tx,
      cache,
    )
  }

  fn make_app_with_feeds(feeds: Vec<Feed>) -> App {
    let (tx, _rx) = mpsc::unbounded_channel();
    let cache = crate::cache::FeedCache::new_in_memory().unwrap();
    App::new(
      feeds,
      GeneralConfig::default(),
      UiConfig {
        show_borders: true,
        show_read_entries: true,
      },
      vec![],
      vec![],
      tx,
      cache,
    )
  }

  #[test]
  fn test_app_initial_state() {
    let app = make_test_app();
    assert_eq!(app.state, AppState::BrowsingFeeds);
    assert!(!app.exit);
    assert!(!app.show_help_popup);
    assert!(!app.show_error_popup);
  }

  #[test]
  fn test_app_quit() {
    let mut app = make_test_app();
    assert!(!app.should_exit());
    app.handle_key(KeyEvent::from(KeyCode::Char('q')));
    assert!(app.should_exit());
  }

  #[test]
  fn test_app_toggle_help() {
    let mut app = make_test_app();
    assert!(!app.show_help_popup);
    app.handle_key(KeyEvent::from(KeyCode::Char('?')));
    assert!(app.show_help_popup);
    app.handle_key(KeyEvent::from(KeyCode::Char('?')));
    assert!(!app.show_help_popup);
  }

  #[test]
  fn test_app_help_popup_scroll() {
    let mut app = make_test_app();
    app.handle_key(KeyEvent::from(KeyCode::Char('?')));
    assert!(app.show_help_popup);

    // Scroll down
    app.handle_key(KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(app.help_scroll, 1);

    // Scroll up
    app.handle_key(KeyEvent::from(KeyCode::Char('k')));
    assert_eq!(app.help_scroll, 0);

    // Can't scroll past 0
    app.handle_key(KeyEvent::from(KeyCode::Char('k')));
    assert_eq!(app.help_scroll, 0);

    // Close with Esc resets scroll
    app.handle_key(KeyEvent::from(KeyCode::Char('j')));
    app.handle_key(KeyEvent::from(KeyCode::Char('j')));
    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert!(!app.show_help_popup);
    assert_eq!(app.help_scroll, 0);
  }

  #[test]
  fn test_app_help_popup_blocks_other_keys() {
    let mut app = make_test_app();
    app.handle_key(KeyEvent::from(KeyCode::Char('?')));
    // 'q' should NOT quit while help is open
    app.handle_key(KeyEvent::from(KeyCode::Char('q')));
    assert!(!app.should_exit());
    assert!(app.show_help_popup);
  }

  #[test]
  fn test_app_navigation_feeds() {
    let feeds = vec![
      make_feed("http://a.com", "A", vec![]),
      make_feed("http://b.com", "B", vec![]),
      make_feed("http://c.com", "C", vec![]),
    ];
    let mut app = make_app_with_feeds(feeds);
    assert_eq!(app.feed_index, 0);

    // Move down
    app.handle_key(KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(app.feed_index, 1);

    app.handle_key(KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(app.feed_index, 2);

    // Can't go past last
    app.handle_key(KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(app.feed_index, 2);

    // Move up
    app.handle_key(KeyEvent::from(KeyCode::Char('k')));
    assert_eq!(app.feed_index, 1);

    // Go to bottom with G
    app.handle_key(KeyEvent::from(KeyCode::Char('G')));
    assert_eq!(app.feed_index, 2);

    // Go to top with gg
    app.handle_key(KeyEvent::from(KeyCode::Char('g')));
    app.handle_key(KeyEvent::from(KeyCode::Char('g')));
    assert_eq!(app.feed_index, 0);
  }

  #[test]
  fn test_app_enter_entries_and_back() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];
    let mut app = make_app_with_feeds(feeds);

    // Enter entries
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.state, AppState::BrowsingEntries);

    // Go back to feeds
    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    assert_eq!(app.state, AppState::BrowsingFeeds);
  }

  #[test]
  fn test_app_enter_entry_view() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];
    let mut app = make_app_with_feeds(feeds);

    // Navigate to entries
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.state, AppState::BrowsingEntries);

    // Open entry
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.state, AppState::ViewingEntry);
    assert_eq!(app.input.current_entry_relative_index, Some(0));

    // Go back
    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    assert_eq!(app.state, AppState::BrowsingEntries);
    assert!(app.input.current_entry_relative_index.is_none());
  }

  #[test]
  fn test_app_toggle_hide_read() {
    let mut app = make_test_app();
    assert!(!app.input.hide_read);
    app.handle_key(KeyEvent::from(KeyCode::Char('u')));
    assert!(app.input.hide_read);
    app.handle_key(KeyEvent::from(KeyCode::Char('u')));
    assert!(!app.input.hide_read);
  }

  #[test]
  fn test_app_error_popup_toggle() {
    let mut app = make_test_app();
    // No errors → 'e' does nothing
    app.handle_key(KeyEvent::from(KeyCode::Char('e')));
    assert!(!app.show_error_popup);

    // Add an error and try again
    app.feed_errors.push(FeedError {
      name: "Test".to_string(),
      error: "error".to_string(),
    });
    app.handle_key(KeyEvent::from(KeyCode::Char('e')));
    assert!(app.show_error_popup);

    // Close with 'e'
    app.handle_key(KeyEvent::from(KeyCode::Char('e')));
    assert!(!app.show_error_popup);
  }

  #[test]
  fn test_app_error_popup_scroll() {
    let mut app = make_test_app();
    app.feed_errors.push(FeedError {
      name: "Test".to_string(),
      error: "error".to_string(),
    });
    app.handle_key(KeyEvent::from(KeyCode::Char('e')));
    assert!(app.show_error_popup);

    app.handle_key(KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(app.error_scroll, 1);

    app.handle_key(KeyEvent::from(KeyCode::Char('k')));
    assert_eq!(app.error_scroll, 0);

    // Close resets scroll
    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert!(!app.show_error_popup);
    assert_eq!(app.error_scroll, 0);
  }

  #[test]
  fn test_app_vim_g_cancelled_by_other_keys() {
    let mut app = make_test_app();
    app.handle_key(KeyEvent::from(KeyCode::Char('g')));
    assert!(app.input.vim_g);

    // Any key other than 'g' should cancel the sequence
    app.handle_key(KeyEvent::from(KeyCode::Char('j')));
    assert!(!app.input.vim_g);
  }

  #[test]
  fn test_app_handle_feed_update_fetching() {
    let mut app = make_test_app();
    app.handle_feed_update(FeedUpdate::FetchingFeed("Feed A".to_string()));
    assert_eq!(app.current_feed.as_deref(), Some("Feed A"));
  }

  #[test]
  fn test_app_handle_feed_update_error() {
    let mut app = make_test_app();
    app.handle_feed_update(FeedUpdate::FeedError {
      name: "Bad Feed".to_string(),
      error: "timeout".to_string(),
    });
    assert_eq!(app.feed_errors.len(), 1);
    assert_eq!(app.feed_errors[0].name, "Bad Feed");
    assert_eq!(app.feed_errors[0].error, "timeout");
  }

  #[test]
  fn test_app_handle_feed_update_fetch_complete() {
    let mut app = make_test_app();
    app.loading_state.is_loading = true;
    app.handle_feed_update(FeedUpdate::FetchComplete);
    assert!(!app.loading_state.is_loading);
    assert!(app.current_feed.is_none());
  }

  #[test]
  fn test_app_scroll_in_entry_view() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];
    let mut app = make_app_with_feeds(feeds);

    // Navigate to entry view
    app.handle_key(KeyEvent::from(KeyCode::Enter)); // → BrowsingEntries
    app.handle_key(KeyEvent::from(KeyCode::Enter)); // → ViewingEntry

    assert_eq!(app.entry_scroll, 0);
    app.handle_key(KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(app.entry_scroll, 1);
    app.handle_key(KeyEvent::from(KeyCode::Char('k')));
    assert_eq!(app.entry_scroll, 0);
  }

  #[test]
  fn test_app_push_error() {
    let mut app = make_test_app();
    app.push_error("Test", "something failed");
    assert_eq!(app.feed_errors.len(), 1);
    assert_eq!(app.feed_errors[0].name, "Test");
    assert_eq!(app.feed_errors[0].error, "something failed");
  }

  // ─── Mark feed as read / confirmation popup tests ─────────────────────

  #[test]
  fn test_app_mark_feed_read_shows_confirmation() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![
        make_entry("Post 1", None, false),
        make_entry("Post 2", None, false),
      ],
    )];
    let mut app = make_app_with_feeds(feeds);

    // Press 'A' while browsing feeds
    app.handle_key(KeyEvent::from(KeyCode::Char('A')));
    assert!(app.show_confirm_popup);
    assert_eq!(app.confirm_feed_name, "Feed A");
  }

  #[test]
  fn test_app_mark_feed_read_no_popup_for_empty_feed() {
    let feeds = vec![make_feed("http://a.com", "Empty Feed", vec![])];
    let mut app = make_app_with_feeds(feeds);

    app.handle_key(KeyEvent::from(KeyCode::Char('A')));
    assert!(!app.show_confirm_popup);
  }

  #[test]
  fn test_app_mark_feed_read_confirm_yes() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![
        make_entry("Post 1", Some("2024-01-01T00:00:00Z"), false),
        make_entry("Post 2", Some("2024-02-01T00:00:00Z"), false),
      ],
    )];
    let (tx, _rx) = mpsc::unbounded_channel();
    let cache = crate::cache::FeedCache::new_in_memory().unwrap();

    // Save the feed to the cache so mark_feed_read works
    cache.save_feed(&feeds[0], 0).unwrap();

    let mut app = App::new(
      feeds,
      GeneralConfig::default(),
      UiConfig {
        show_borders: true,
        show_read_entries: true,
      },
      vec![],
      vec![],
      tx,
      cache,
    );

    // Press 'A' to request mark-as-read
    app.handle_key(KeyEvent::from(KeyCode::Char('A')));
    assert!(app.show_confirm_popup);

    // Confirm with 'y'
    app.handle_key(KeyEvent::from(KeyCode::Char('y')));
    assert!(!app.show_confirm_popup);

    // All entries should now be read
    assert!(app.feeds[0].entries.iter().all(|e| e.read));
  }

  #[test]
  fn test_app_mark_feed_read_confirm_no() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];
    let mut app = make_app_with_feeds(feeds);

    app.handle_key(KeyEvent::from(KeyCode::Char('A')));
    assert!(app.show_confirm_popup);

    // Cancel with 'n'
    app.handle_key(KeyEvent::from(KeyCode::Char('n')));
    assert!(!app.show_confirm_popup);

    // Entry should still be unread
    assert!(!app.feeds[0].entries[0].read);
  }

  #[test]
  fn test_app_mark_feed_read_confirm_esc() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];
    let mut app = make_app_with_feeds(feeds);

    app.handle_key(KeyEvent::from(KeyCode::Char('A')));
    assert!(app.show_confirm_popup);

    // Cancel with Esc
    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert!(!app.show_confirm_popup);
    assert!(!app.feeds[0].entries[0].read);
  }

  #[test]
  fn test_app_confirm_popup_blocks_other_keys() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];
    let mut app = make_app_with_feeds(feeds);

    app.handle_key(KeyEvent::from(KeyCode::Char('A')));
    assert!(app.show_confirm_popup);

    // 'q' should NOT quit while confirm popup is open
    app.handle_key(KeyEvent::from(KeyCode::Char('q')));
    assert!(!app.should_exit());
    assert!(app.show_confirm_popup);
  }

  #[test]
  fn test_app_mark_feed_read_from_entry_list() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![
        make_entry("Post 1", Some("2024-01-01T00:00:00Z"), false),
        make_entry("Post 2", Some("2024-02-01T00:00:00Z"), false),
      ],
    )];
    let (tx, _rx) = mpsc::unbounded_channel();
    let cache = crate::cache::FeedCache::new_in_memory().unwrap();
    cache.save_feed(&feeds[0], 0).unwrap();

    let mut app = App::new(
      feeds,
      GeneralConfig::default(),
      UiConfig {
        show_borders: true,
        show_read_entries: true,
      },
      vec![],
      vec![],
      tx,
      cache,
    );

    // Navigate into the entry list
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.state, AppState::BrowsingEntries);

    // Press 'A' from the entry list
    app.handle_key(KeyEvent::from(KeyCode::Char('A')));
    assert!(app.show_confirm_popup);
    assert_eq!(app.confirm_feed_name, "Feed A");

    // Confirm
    app.handle_key(KeyEvent::from(KeyCode::Char('y')));
    assert!(!app.show_confirm_popup);
    assert!(app.feeds[0].entries.iter().all(|e| e.read));
  }

  #[test]
  fn test_app_mark_feed_read_from_entry_view() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![
        make_entry("Post 1", Some("2024-01-01T00:00:00Z"), false),
        make_entry("Post 2", Some("2024-02-01T00:00:00Z"), false),
      ],
    )];
    let (tx, _rx) = mpsc::unbounded_channel();
    let cache = crate::cache::FeedCache::new_in_memory().unwrap();
    cache.save_feed(&feeds[0], 0).unwrap();

    let mut app = App::new(
      feeds,
      GeneralConfig::default(),
      UiConfig {
        show_borders: true,
        show_read_entries: true,
      },
      vec![],
      vec![],
      tx,
      cache,
    );

    // Navigate into entry view
    app.handle_key(KeyEvent::from(KeyCode::Enter)); // BrowsingEntries
    app.handle_key(KeyEvent::from(KeyCode::Enter)); // ViewingEntry
    assert_eq!(app.state, AppState::ViewingEntry);

    // Press 'A' from within entry view
    app.handle_key(KeyEvent::from(KeyCode::Char('A')));
    assert!(app.show_confirm_popup);

    app.handle_key(KeyEvent::from(KeyCode::Char('y')));
    assert!(app.feeds[0].entries.iter().all(|e| e.read));
  }
}

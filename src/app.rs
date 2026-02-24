use crate::cache::FeedCache;
use crate::config::{Feed as FeedConfig, GeneralConfig, QueryFeed, UiConfig};
use crate::feeds::{self, Feed, FeedEntry};
use crate::query;
use crate::views::{entry_view, feeds_list_view};
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
  /// Report progress on a specific feed
  FetchingFeed(String),
  /// Report a feed that failed to fetch or parse
  FeedError { name: String, error: String },
  /// All feeds finished fetching — reload from cache now
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
  pub finish_time: Option<Instant>,
}

impl LoadingState {
  pub fn new() -> Self {
    Self {
      is_loading: true,
      start_time: Instant::now(),
      finish_time: None,
    }
  }

  pub fn start(&mut self) {
    self.is_loading = true;
    self.start_time = Instant::now();
    self.finish_time = None;
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
  input: InputState,
  cache: FeedCache,
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
        let mut state = LoadingState::new();
        state.stop();
        state
      },
      current_feed: None,
      feed_errors: Vec::new(),
      show_error_popup: false,
      input: InputState::default(),
      cache,
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
    // Drop old display data before allocating the new set so they never coexist.
    drop(std::mem::take(&mut self.display_feeds));
    // Pass &self.query_config directly — no clone needed now that display_feeds
    // is empty and no longer holds a borrow conflict.
    self.display_feeds = Self::build_display_feeds(&self.feeds, &self.query_config);
  }

  pub fn should_exit(&self) -> bool {
    self.exit
  }

  pub fn handle_feed_update(&mut self, update: FeedUpdate) {
    match update {
      FeedUpdate::Replace(new_feeds) => {
        for (i, feed) in new_feeds.iter().enumerate() {
          if let Err(e) = self.cache.save_feed(feed, i) {
            eprintln!("Failed to cache feed {}: {}", feed.title, e);
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

      FeedUpdate::FetchComplete => {
        self.feeds.clear();
        self.feeds = self.cache.load_all_feeds().unwrap_or_default();
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
    // These two clones are genuinely required: the spawned task needs ownership
    // of the config vec, and UnboundedSender is designed to be cloned for sharing.
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
          self.input.hide_read,
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
      KeyCode::Char('u') | KeyCode::Char('U') => {
        self.input.cancel_sequence();
        self.input.hide_read = !self.input.hide_read;
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

  /// Returns the real (unfiltered) entry indices that are currently visible
  /// given the `hide_read` setting. When `hide_read` is false this is simply
  /// every index; when true only unread entries are included.
  fn visible_entry_indices(&self) -> Vec<usize> {
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return vec![];
    };
    df.entries(&self.feeds)
      .iter()
      .enumerate()
      .filter(|(_, e)| !self.input.hide_read || !e.read)
      .map(|(i, _)| i)
      .collect()
  }

  /// Maps a visible (filtered) list position to the real entry index in the feed.
  fn visible_to_real_entry_idx(&self, visible_idx: usize) -> Option<usize> {
    self.visible_entry_indices().get(visible_idx).copied()
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
        self.state = AppState::BrowsingEntries;
      }
      AppState::BrowsingEntries => self.state = AppState::BrowsingFeeds,
      AppState::BrowsingFeeds => {}
    }
  }

  // ─── Browser / player helpers ─────────────────────────────────────────────

  fn spawn_cmd(cmd: &str, url: &str) {
    let mut parts = cmd.split_whitespace();
    if let Some(bin) = parts.next() {
      let args: Vec<&str> = parts.collect();
      if let Err(e) = Command::new(bin)
        .args(args)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
      {
        eprintln!("Failed to launch '{}': {}", cmd, e);
      }
    }
  }

  /// Open the first link of the currently selected entry in a browser.
  fn open_current_entry_in_browser(&self) {
    let real_idx = match self.state {
      AppState::ViewingEntry => match self.input.current_entry_relative_index {
        Some(i) => i,
        None => return,
      },
      _ => {
        let visible_idx = match self.entry_list_state.selected() {
          Some(i) => i,
          None => return,
        };
        self
          .visible_to_real_entry_idx(visible_idx)
          .unwrap_or(visible_idx)
      }
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

    if let Some(ref cmd) = self.general_config.browser {
      Self::spawn_cmd(cmd, url);
    } else if let Err(e) = open::that(url) {
      eprintln!("Failed to open URL in default browser: {}", e);
    }
  }

  /// Open the media attachment of the currently selected entry.
  fn open_media_in_player(&self) {
    let real_idx = match self.state {
      AppState::ViewingEntry => match self.input.current_entry_relative_index {
        Some(i) => i,
        None => return,
      },
      _ => {
        let visible_idx = match self.entry_list_state.selected() {
          Some(i) => i,
          None => return,
        };
        self
          .visible_to_real_entry_idx(visible_idx)
          .unwrap_or(visible_idx)
      }
    };
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let Some(entry) = df.entries(&self.feeds).get(real_idx) else {
      return;
    };
    let Some(url) = &entry.media else { return };

    if let Some(ref cmd) = self.general_config.media_player {
      Self::spawn_cmd(cmd, url);
    } else if let Err(e) = open::that(url) {
      eprintln!("Failed to open media URL with OS default: {}", e);
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
      eprintln!("Failed to toggle entry read state: {}", e);
      return;
    }

    self.sync_read_state(feed_vec_idx, entry_vec_idx, new_read);
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
      eprintln!("Failed to mark entry read: {}", e);
      return;
    }

    self.sync_read_state(feed_vec_idx, entry_vec_idx, true);
  }
}

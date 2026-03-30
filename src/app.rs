use crate::cache::FeedCache;
use crate::config::{Feed as FeedConfig, GeneralConfig, QueryFeed, UiConfig};
use crate::feeds::{self, Feed, FeedEntry};
use crate::query;
use crate::theme::Theme;
use crate::views::{entry_view, feeds_list_view, help_view, links_view};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListPane {
  Feeds,
  Tags,
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
  /// True when the fuzzy search bar is active (triggered by '/').
  pub search_active: bool,
  /// The current search query typed by the user.
  pub search_query: String,
  /// Indices of items matching the current search query, sorted best-match-first.
  /// For BrowsingFeeds: indices into `display_feeds`.
  /// For BrowsingEntries: indices into the visible entry list.
  pub search_matches: Vec<usize>,
  /// Current position within `search_matches` (for n/N cycling).
  pub search_match_cursor: usize,
}

impl InputState {
  /// Clear any multi-key sequence in progress.
  pub fn cancel_sequence(&mut self) {
    self.vim_g = false;
  }

  /// Deactivate search and clear the query.
  pub fn clear_search(&mut self) {
    self.search_active = false;
    self.search_query.clear();
    self.search_matches.clear();
    self.search_match_cursor = 0;
  }
}

/// Scored fuzzy matching.  Returns `None` for no match, or `Some(score)` where
/// a **lower** score is a **better** match.
///
/// Scoring tiers (lower = better):
///   0 — exact match (case-insensitive)
///   1 — pattern is a contiguous substring (case-insensitive)
///   2 — pattern matches at word boundaries (e.g. initials)
///   3 — subsequence match (characters in order, but not contiguous)
pub fn fuzzy_score(text: &str, pattern: &str) -> Option<u32> {
  if pattern.is_empty() {
    return Some(0);
  }
  let text_lower = text.to_lowercase();
  let pattern_lower = pattern.to_lowercase();

  // Tier 0 — exact
  if text_lower == pattern_lower {
    return Some(0);
  }

  // Tier 1 — contiguous substring
  if text_lower.contains(&pattern_lower) {
    return Some(1);
  }

  // Tier 2 — word-boundary / initials match
  // Each pattern char must match the start of a word (or continue within the
  // same word as the previous matched char).
  if word_boundary_match(&text_lower, &pattern_lower) {
    return Some(2);
  }

  // Tier 3 — subsequence (characters in order, gaps allowed)
  if subsequence_match(&text_lower, &pattern_lower) {
    return Some(3);
  }

  None
}

/// Check if every character of `pattern` appears in `text` in order.
fn subsequence_match(text: &str, pattern: &str) -> bool {
  let mut pattern_chars = pattern.chars();
  let mut current = pattern_chars.next();
  for ch in text.chars() {
    if let Some(p) = current {
      if ch == p {
        current = pattern_chars.next();
      }
    } else {
      break;
    }
  }
  current.is_none()
}

/// Check if the pattern matches at word boundaries in the text.
/// A "word boundary" is the first character of the text or a character following
/// a non-alphanumeric separator (space, dash, underscore, etc.).
fn word_boundary_match(text: &str, pattern: &str) -> bool {
  let text_chars: Vec<char> = text.chars().collect();
  let pattern_chars: Vec<char> = pattern.chars().collect();
  if pattern_chars.is_empty() {
    return true;
  }

  let mut pi = 0; // index into pattern_chars
  let mut i = 0; // index into text_chars

  while i < text_chars.len() && pi < pattern_chars.len() {
    let is_boundary = i == 0 || !text_chars[i - 1].is_alphanumeric();
    if is_boundary && text_chars[i] == pattern_chars[pi] {
      // Start matching from this word boundary
      pi += 1;
      i += 1;
      // Continue matching consecutive chars within the same word
      while i < text_chars.len() && pi < pattern_chars.len() && text_chars[i] == pattern_chars[pi] {
        pi += 1;
        i += 1;
      }
    } else {
      i += 1;
    }
  }

  pi == pattern_chars.len()
}

/// Backwards-compatible boolean wrapper: returns true if the pattern matches at any tier.
#[cfg(test)]
pub fn fuzzy_match(text: &str, pattern: &str) -> bool {
  fuzzy_score(text, pattern).is_some()
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
  list_pane: ListPane,
  previous_list_pane: ListPane,
  tag_list: Vec<(String, usize)>,
  tag_index: usize,
  tag_list_state: TableState,
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
  theme: Theme,
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
    let theme = Theme::from_config(&ui_config.theme);

    let tag_list = Self::build_tag_list(&feeds);

    Self {
      feeds,
      display_feeds,
      feed_config,
      query_config,
      feed_index: 0,
      feed_list_state: TableState::default().with_selected(Some(0)),
      entry_list_state: TableState::default(),
      state: AppState::BrowsingFeeds,
      list_pane: ListPane::Feeds,
      previous_list_pane: ListPane::Feeds,
      tag_list,
      tag_index: 0,
      tag_list_state: TableState::default().with_selected(Some(0)),
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
      theme,
      visible_indices_cache: Vec::new(),
    }
  }

  /// Build a sorted list of unique tags with their feed counts.
  fn build_tag_list(feeds: &[Feed]) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    let mut tag_counts: HashMap<String, usize> = HashMap::new();

    for feed in feeds {
      if let Some(tags) = &feed.tags {
        for tag in tags {
          *tag_counts.entry(tag.clone()).or_insert(0) += 1;
        }
      }
    }

    let mut tags: Vec<(String, usize)> = tag_counts.into_iter().collect();
    tags.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    tags
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

  /// Build display feeds filtered by a tag query.
  fn build_display_feeds_with_tag(
    feeds: &[Feed],
    _query_config: &[QueryFeed],
    tag_query: &str,
  ) -> Vec<DisplayFeed> {
    let filter = query::parse_query(tag_query);
    let matching_feeds: Vec<usize> = feeds
      .iter()
      .enumerate()
      .filter(|(_, f)| {
        if let Some(tags) = &f.tags {
          tags.iter().any(|t| {
            if let query::QueryFilter::Tags(query_tags) = &filter {
              query_tags.iter().any(|qt| qt == t)
            } else {
              false
            }
          })
        } else {
          false
        }
      })
      .map(|(i, _)| i)
      .collect();

    let mut entries: Vec<FeedEntry> = matching_feeds
      .iter()
      .flat_map(|i| {
        feeds
          .get(*i)
          .map(|f| {
            f.entries
              .iter()
              .map(|e| {
                let mut e = e.clone();
                e.feed_title = Some(f.title.clone());
                e
              })
              .collect::<Vec<_>>()
          })
          .unwrap_or_default()
      })
      .collect();

    entries.sort_by(|a, b| match (&b.published, &a.published) {
      (Some(b_date), Some(a_date)) => b_date.cmp(a_date),
      (Some(_), None) => std::cmp::Ordering::Less,
      (None, Some(_)) => std::cmp::Ordering::Greater,
      (None, None) => std::cmp::Ordering::Equal,
    });

    let tag_name = tag_query
      .strip_prefix("tags:")
      .map(|s| s.to_string())
      .unwrap_or_else(|| tag_query.to_string());

    let mut display_feeds: Vec<DisplayFeed> = vec![DisplayFeed::Query {
      name: tag_name,
      entries,
    }];

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
                &entry_view::EntryViewConfig {
                  show_borders: self.ui_config.show_borders,
                  show_scrollbar: self.ui_config.show_scrollbar,
                  theme: &self.theme,
                },
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
            list_pane: self.list_pane,
            tag_list: &self.tag_list,
            tag_state: &mut self.tag_list_state,
            show_borders: self.ui_config.show_borders,
            show_scrollbar: self.ui_config.show_scrollbar,
            loading_state: &self.loading_state,
            current_feed: self.current_feed.as_deref(),
            feed_errors: &self.feed_errors,
            show_error_popup: self.show_error_popup,
            error_scroll: &mut self.error_scroll,
            hide_read: self.input.hide_read,
            search_active: self.input.search_active,
            search_query: &self.input.search_query,
            search_matches: &self.input.search_matches,
            search_match_cursor: self.input.search_match_cursor,
            theme: &self.theme,
          },
        );
      }
    }

    if self.show_help_popup {
      help_view::render_help_popup(
        frame,
        area,
        &mut self.help_scroll,
        self.ui_config.show_scrollbar,
        &self.theme,
      );
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
              self.ui_config.show_scrollbar,
              &self.theme,
            );
          }
        }
      }
    }

    if self.show_confirm_popup {
      feeds_list_view::render_confirm_popup(frame, area, &self.confirm_feed_name, &self.theme);
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

    // ── Fuzzy search input mode ──────────────────────────────────────────
    if self.input.search_active {
      match key.code {
        KeyCode::Esc => {
          // Cancel search — clear query and restore original position
          self.input.clear_search();
        }
        KeyCode::Enter => {
          // Confirm search — keep current selection, close search bar
          self.input.clear_search();
        }
        KeyCode::Backspace => {
          self.input.search_query.pop();
          self.update_search_matches();
        }
        // Next match: Ctrl+n or Tab
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
          self.search_next_match();
        }
        KeyCode::Tab => {
          self.search_next_match();
        }
        // Previous match: Ctrl+p or Shift+Tab
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
          self.search_prev_match();
        }
        KeyCode::BackTab => {
          self.search_prev_match();
        }
        KeyCode::Char(c) => {
          self.input.search_query.push(c);
          self.update_search_matches();
        }
        _ => {} // ignore other keys while searching
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
        if !self.visible_entry_indices().is_empty() {
          self.entry_list_state.select(Some(0));
        }
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
      KeyCode::Char('L') => {
        if self.state == AppState::ViewingEntry {
          self.show_links_popup = !self.show_links_popup;
          self.links_scroll = 0;
          self.links_selected = 0;
        }
      }
      KeyCode::Char('t') | KeyCode::Char('T') => {
        if self.state == AppState::BrowsingFeeds {
          self.input.cancel_sequence();
          self.list_pane = match self.list_pane {
            ListPane::Feeds => ListPane::Tags,
            ListPane::Tags => ListPane::Feeds,
          };
          self.tag_list_state.select(Some(0));
          self.tag_index = 0;
        }
      }
      KeyCode::Char('/') => match self.state {
        AppState::BrowsingFeeds | AppState::BrowsingEntries => {
          self.input.cancel_sequence();
          self.input.search_active = true;
          self.input.search_query.clear();
          self.input.search_matches.clear();
          self.input.search_match_cursor = 0;
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

    // Deselect the entry table when there are no visible entries so that
    // the "No entries" placeholder row cannot be "opened" by pressing Enter.
    if self.visible_indices_cache.is_empty() && self.state == AppState::BrowsingEntries {
      self.entry_list_state.select(None);
    }
  }

  /// Returns the cached visible (unfiltered) entry indices for the current feed.
  fn visible_entry_indices(&self) -> &[usize] {
    &self.visible_indices_cache
  }

  /// Maps a visible (filtered) list position to the real entry index in the feed.
  fn visible_to_real_entry_idx(&self, visible_idx: usize) -> Option<usize> {
    self.visible_indices_cache.get(visible_idx).copied()
  }

  /// Rebuild `input.search_matches` based on the current search query.
  /// Matches are sorted by score (best first), and the cursor selects the best match.
  fn update_search_matches(&mut self) {
    self.input.search_matches.clear();
    self.input.search_match_cursor = 0;
    if self.input.search_query.is_empty() {
      // Empty query — reset selection to 0 (show all, nothing filtered)
      match self.state {
        AppState::BrowsingFeeds => match self.list_pane {
          ListPane::Feeds => {
            self.feed_index = 0;
            self.feed_list_state.select(Some(0));
          }
          ListPane::Tags => {
            self.tag_index = 0;
            self.tag_list_state.select(Some(0));
          }
        },
        AppState::BrowsingEntries => {
          self.entry_list_state.select(Some(0));
        }
        _ => {}
      }
      return;
    }

    // Collect (index, score) pairs, then sort by score (best first), breaking
    // ties by original index to preserve list order among equal-score matches.
    let mut scored: Vec<(usize, u32)> = Vec::new();

    match self.state {
      AppState::BrowsingFeeds => match self.list_pane {
        ListPane::Feeds => {
          for (i, df) in self.display_feeds.iter().enumerate() {
            let title = df.title(&self.feeds);
            if let Some(score) = fuzzy_score(title, &self.input.search_query) {
              scored.push((i, score));
            }
          }
        }
        ListPane::Tags => {
          for (i, (tag_name, _)) in self.tag_list.iter().enumerate() {
            if let Some(score) = fuzzy_score(tag_name, &self.input.search_query) {
              scored.push((i, score));
            }
          }
        }
      },
      AppState::BrowsingEntries => {
        let visible = self.visible_entry_indices().to_vec();
        for (visible_idx, &real_idx) in visible.iter().enumerate() {
          if let Some(df) = self.display_feeds.get(self.feed_index) {
            if let Some(entry) = df.entries(&self.feeds).get(real_idx) {
              if let Some(score) = fuzzy_score(&entry.title, &self.input.search_query) {
                scored.push((visible_idx, score));
              }
            }
          }
        }
      }
      _ => {}
    }

    scored.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    self.input.search_matches = scored.into_iter().map(|(idx, _)| idx).collect();

    // Select the best match (first in the sorted list)
    self.select_current_search_match();
  }

  /// Move the selection to whichever item `search_match_cursor` points at.
  fn select_current_search_match(&mut self) {
    let Some(&idx) = self
      .input
      .search_matches
      .get(self.input.search_match_cursor)
    else {
      return;
    };
    match self.state {
      AppState::BrowsingFeeds => match self.list_pane {
        ListPane::Feeds => {
          self.feed_index = idx;
          self.feed_list_state.select(Some(idx));
        }
        ListPane::Tags => {
          self.tag_index = idx;
          self.tag_list_state.select(Some(idx));
        }
      },
      AppState::BrowsingEntries => {
        self.entry_list_state.select(Some(idx));
      }
      _ => {}
    }
  }

  /// Advance to the next search match (wrapping around).
  fn search_next_match(&mut self) {
    if self.input.search_matches.is_empty() {
      return;
    }
    self.input.search_match_cursor =
      (self.input.search_match_cursor + 1) % self.input.search_matches.len();
    self.select_current_search_match();
  }

  /// Go to the previous search match (wrapping around).
  fn search_prev_match(&mut self) {
    if self.input.search_matches.is_empty() {
      return;
    }
    if self.input.search_match_cursor == 0 {
      self.input.search_match_cursor = self.input.search_matches.len() - 1;
    } else {
      self.input.search_match_cursor -= 1;
    }
    self.select_current_search_match();
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
      AppState::BrowsingFeeds => match self.list_pane {
        ListPane::Feeds => {
          if self.feed_index > 0 {
            self.feed_index -= 1;
            self.feed_list_state.select(Some(self.feed_index));
          }
        }
        ListPane::Tags => {
          if self.tag_index > 0 {
            self.tag_index -= 1;
            self.tag_list_state.select(Some(self.tag_index));
          }
        }
      },
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
      AppState::BrowsingFeeds => match self.list_pane {
        ListPane::Feeds => {
          if self.feed_index + 1 < self.display_feeds.len() {
            self.feed_index += 1;
            self.feed_list_state.select(Some(self.feed_index));
          }
        }
        ListPane::Tags => {
          if self.tag_index + 1 < self.tag_list.len() {
            self.tag_index += 1;
            self.tag_list_state.select(Some(self.tag_index));
          }
        }
      },
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
      AppState::BrowsingFeeds => match self.list_pane {
        ListPane::Feeds => {
          self.feed_index = 0;
          self.feed_list_state.select(Some(0));
        }
        ListPane::Tags => {
          self.tag_index = 0;
          self.tag_list_state.select(Some(0));
        }
      },
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
      AppState::BrowsingFeeds => match self.list_pane {
        ListPane::Feeds => {
          let last = self.display_feeds.len().saturating_sub(1);
          self.feed_index = last;
          self.feed_list_state.select(Some(last));
        }
        ListPane::Tags => {
          let last = self.tag_list.len().saturating_sub(1);
          self.tag_index = last;
          self.tag_list_state.select(Some(last));
        }
      },
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
      AppState::BrowsingFeeds => match self.list_pane {
        ListPane::Feeds => {
          self.input.clear_search();
          self.previous_list_pane = ListPane::Feeds;
          self.state = AppState::BrowsingEntries;
          self.invalidate_visible_indices();
          if !self.visible_entry_indices().is_empty() {
            self.entry_list_state.select(Some(0));
          }
        }
        ListPane::Tags => {
          if let Some((tag_name, _)) = self.tag_list.get(self.tag_index) {
            let tag_query = format!("tags:{}", tag_name);
            self.input.clear_search();
            self.previous_list_pane = ListPane::Tags;
            self.display_feeds =
              Self::build_display_feeds_with_tag(&self.feeds, &self.query_config, &tag_query);
            self.tag_list_state.select(Some(self.tag_index));
            self.feed_index = 0;
            self.feed_list_state.select(Some(0));
            self.state = AppState::BrowsingEntries;
            self.list_pane = ListPane::Feeds;
            self.invalidate_visible_indices();
            if !self.visible_entry_indices().is_empty() {
              self.entry_list_state.select(Some(0));
            }
          }
        }
      },
      AppState::BrowsingEntries => {
        self.input.clear_search();
        if self.visible_entry_indices().is_empty() {
          return;
        }
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
      AppState::BrowsingEntries => {
        self.input.clear_search();
        self.rebuild_display_feeds();
        self.list_pane = self.previous_list_pane;
        self.state = AppState::BrowsingFeeds;
      }
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

  // ─── fuzzy_match / fuzzy_score tests ─────────────────────────────────────

  #[test]
  fn test_fuzzy_match_empty_pattern() {
    assert!(fuzzy_match("anything", ""));
  }

  #[test]
  fn test_fuzzy_match_exact() {
    assert!(fuzzy_match("hello", "hello"));
  }

  #[test]
  fn test_fuzzy_match_subsequence() {
    assert!(fuzzy_match("hello world", "hlo"));
  }

  #[test]
  fn test_fuzzy_match_case_insensitive() {
    assert!(fuzzy_match("Hello World", "hw"));
    assert!(fuzzy_match("hello world", "HW"));
  }

  #[test]
  fn test_fuzzy_match_no_match() {
    assert!(!fuzzy_match("hello", "xyz"));
  }

  #[test]
  fn test_fuzzy_match_pattern_longer_than_text() {
    assert!(!fuzzy_match("hi", "hello"));
  }

  #[test]
  fn test_fuzzy_match_empty_text() {
    assert!(!fuzzy_match("", "a"));
    assert!(fuzzy_match("", ""));
  }

  #[test]
  fn test_fuzzy_score_exact_is_best() {
    assert_eq!(fuzzy_score("Tech", "Tech"), Some(0));
    assert_eq!(fuzzy_score("tech", "Tech"), Some(0)); // case-insensitive exact
  }

  #[test]
  fn test_fuzzy_score_substring_beats_subsequence() {
    // "Tech" as substring in "TechCrunch" should score better than
    // a subsequence match in "The Electric Church"
    let substring_score = fuzzy_score("TechCrunch", "Tech").unwrap();
    let subsequence_score = fuzzy_score("The Electric Church", "Tech").unwrap();
    assert!(
      substring_score < subsequence_score,
      "substring {} should be < subsequence {}",
      substring_score,
      subsequence_score
    );
  }

  #[test]
  fn test_fuzzy_score_check_does_not_match_tech() {
    // "Check" does NOT contain "tech" as a subsequence:
    // c-h-e-c-k has no 't', so it should not match at all
    assert_eq!(fuzzy_score("Check", "tech"), None);
  }

  #[test]
  fn test_fuzzy_score_word_boundary_beats_subsequence() {
    // "hw" matching "Hello World" at word boundaries should score better
    // than matching "shower" as a subsequence
    let boundary_score = fuzzy_score("Hello World", "hw").unwrap();
    let subsequence_score = fuzzy_score("show her", "shr").unwrap();
    assert!(boundary_score <= subsequence_score);
  }

  #[test]
  fn test_fuzzy_score_no_match_returns_none() {
    assert_eq!(fuzzy_score("hello", "xyz"), None);
  }

  // ─── Search activation tests ────────────────────────────────────────────

  #[test]
  fn test_search_activated_by_slash() {
    let feeds = vec![make_feed("http://a.com", "Feed A", vec![])];
    let mut app = make_app_with_feeds(feeds);
    assert!(!app.input.search_active);

    app.handle_key(KeyEvent::from(KeyCode::Char('/')));
    assert!(app.input.search_active);
    assert!(app.input.search_query.is_empty());
  }

  #[test]
  fn test_search_typing_updates_query() {
    let feeds = vec![
      make_feed("http://a.com", "Alpha Feed", vec![]),
      make_feed("http://b.com", "Beta Feed", vec![]),
    ];
    let mut app = make_app_with_feeds(feeds);
    app.handle_key(KeyEvent::from(KeyCode::Char('/')));

    app.handle_key(KeyEvent::from(KeyCode::Char('a')));
    assert_eq!(app.input.search_query, "a");
    assert!(app.input.search_matches.contains(&0)); // "Alpha Feed" matches "a"
  }

  #[test]
  fn test_search_escape_clears() {
    let feeds = vec![make_feed("http://a.com", "Feed A", vec![])];
    let mut app = make_app_with_feeds(feeds);
    app.handle_key(KeyEvent::from(KeyCode::Char('/')));
    app.handle_key(KeyEvent::from(KeyCode::Char('x')));
    assert!(app.input.search_active);

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert!(!app.input.search_active);
    assert!(app.input.search_query.is_empty());
  }

  #[test]
  fn test_search_enter_confirms_and_clears() {
    let feeds = vec![
      make_feed("http://a.com", "Alpha", vec![]),
      make_feed("http://b.com", "Beta", vec![]),
    ];
    let mut app = make_app_with_feeds(feeds);
    app.handle_key(KeyEvent::from(KeyCode::Char('/')));
    app.handle_key(KeyEvent::from(KeyCode::Char('b')));

    // "Beta" should be selected (index 1)
    assert_eq!(app.feed_index, 1);

    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(!app.input.search_active);
    // Selection should remain on "Beta"
    assert_eq!(app.feed_index, 1);
  }

  #[test]
  fn test_search_backspace_removes_char() {
    let feeds = vec![make_feed("http://a.com", "Feed A", vec![])];
    let mut app = make_app_with_feeds(feeds);
    app.handle_key(KeyEvent::from(KeyCode::Char('/')));
    app.handle_key(KeyEvent::from(KeyCode::Char('a')));
    app.handle_key(KeyEvent::from(KeyCode::Char('b')));
    assert_eq!(app.input.search_query, "ab");

    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    assert_eq!(app.input.search_query, "a");
  }

  #[test]
  fn test_search_not_available_in_entry_view() {
    let feeds = vec![make_feed(
      "http://a.com",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];
    let mut app = make_app_with_feeds(feeds);
    app.handle_key(KeyEvent::from(KeyCode::Enter)); // BrowsingEntries
    app.handle_key(KeyEvent::from(KeyCode::Enter)); // ViewingEntry

    app.handle_key(KeyEvent::from(KeyCode::Char('/')));
    assert!(!app.input.search_active); // Should not activate in ViewingEntry
  }

  #[test]
  fn test_search_blocks_normal_keys() {
    let feeds = vec![make_feed("http://a.com", "Feed A", vec![])];
    let mut app = make_app_with_feeds(feeds);
    app.handle_key(KeyEvent::from(KeyCode::Char('/')));

    // 'q' should type into search, not quit
    app.handle_key(KeyEvent::from(KeyCode::Char('q')));
    assert!(!app.should_exit());
    assert_eq!(app.input.search_query, "q");
  }

  // ─── Search match ordering tests ──────────────────────────────────────

  #[test]
  fn test_search_exact_match_ranked_first() {
    // "Tech" should rank "Tech" (exact) above "TechCrunch" (substring)
    // above "The Electric Church" (subsequence)
    let feeds = vec![
      make_feed("http://a.com", "The Electric Church", vec![]),
      make_feed("http://b.com", "TechCrunch", vec![]),
      make_feed("http://c.com", "Tech", vec![]),
    ];
    let mut app = make_app_with_feeds(feeds);
    app.handle_key(KeyEvent::from(KeyCode::Char('/')));
    for c in "Tech".chars() {
      app.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }

    // The best match ("Tech", index 2) should be selected
    assert_eq!(app.feed_index, 2);
    // And should be first in matches
    assert_eq!(app.input.search_matches[0], 2); // exact
    assert_eq!(app.input.search_matches[1], 1); // substring
  }

  // ─── Search navigation tests (Tab / Shift+Tab) ─────────────────────────

  #[test]
  fn test_search_tab_cycles_next_match() {
    let feeds = vec![
      make_feed("http://a.com", "Alpha", vec![]),
      make_feed("http://b.com", "Alphabet", vec![]),
      make_feed("http://c.com", "Zeta", vec![]),
    ];
    let mut app = make_app_with_feeds(feeds);
    app.handle_key(KeyEvent::from(KeyCode::Char('/')));
    for c in "alph".chars() {
      app.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }

    // "Alpha" (0) and "Alphabet" (1) match; "Zeta" does not
    assert_eq!(app.input.search_matches.len(), 2);
    assert_eq!(app.input.search_match_cursor, 0);
    let first_selected = app.feed_index;

    // Tab to next match
    app.handle_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(app.input.search_match_cursor, 1);
    let second_selected = app.feed_index;
    assert_ne!(first_selected, second_selected);

    // Tab again wraps back to first
    app.handle_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(app.input.search_match_cursor, 0);
    assert_eq!(app.feed_index, first_selected);
  }

  #[test]
  fn test_search_backtab_cycles_prev_match() {
    let feeds = vec![
      make_feed("http://a.com", "Alpha", vec![]),
      make_feed("http://b.com", "Alphabet", vec![]),
      make_feed("http://c.com", "Zeta", vec![]),
    ];
    let mut app = make_app_with_feeds(feeds);
    app.handle_key(KeyEvent::from(KeyCode::Char('/')));
    for c in "alph".chars() {
      app.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }

    // Shift+Tab from first match wraps to last
    app.handle_key(KeyEvent::from(KeyCode::BackTab));
    assert_eq!(
      app.input.search_match_cursor,
      app.input.search_matches.len() - 1
    );
  }

  #[test]
  fn test_search_navigation_with_no_matches() {
    let feeds = vec![make_feed("http://a.com", "Alpha", vec![])];
    let mut app = make_app_with_feeds(feeds);
    app.handle_key(KeyEvent::from(KeyCode::Char('/')));
    // Search for something that doesn't match
    for c in "zzz".chars() {
      app.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }
    assert!(app.input.search_matches.is_empty());

    // Tab / BackTab should not panic with no matches
    app.handle_key(KeyEvent::from(KeyCode::Tab));
    app.handle_key(KeyEvent::from(KeyCode::BackTab));
    assert_eq!(app.input.search_match_cursor, 0);
  }

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
    assert!(
      DisplayFeed::Query {
        name: "Q".to_string(),
        entries: vec![]
      }
      .is_query()
    );
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
        show_scrollbar: true,
        ..Default::default()
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
        show_scrollbar: true,
        ..Default::default()
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
        show_scrollbar: true,
        ..Default::default()
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
        show_scrollbar: true,
        ..Default::default()
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
        show_scrollbar: true,
        ..Default::default()
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

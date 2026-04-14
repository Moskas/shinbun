pub mod actions;
pub mod display;
pub mod input;
pub mod loading;
pub mod navigation;
pub mod read_state;
pub mod search;
pub mod types;

pub use input::InputState;
pub use loading::LoadingState;
pub use types::*;

use crate::cache::FeedCache;
use crate::config::{Feed as FeedConfig, GeneralConfig, QueryFeed, UiConfig};
use crate::feeds::{self, Feed};
use crate::query;
use crate::theme::Theme;
use crate::views::{entry_view, feeds_list_view, help_view, links_view};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::TableState;
use tokio::sync::mpsc;

pub struct App {
  pub(crate) feeds: Vec<Feed>,
  pub(crate) display_feeds: Vec<DisplayFeed>,
  pub(crate) feed_config: Vec<FeedConfig>,
  pub(crate) query_config: Vec<QueryFeed>,
  pub(crate) feed_index: usize,
  pub(crate) feed_list_state: TableState,
  pub(crate) entry_list_state: TableState,
  pub(crate) state: AppState,
  pub(crate) list_pane: ListPane,
  pub(crate) previous_list_pane: ListPane,
  pub(crate) tag_list: Vec<(String, usize)>,
  pub(crate) tag_index: usize,
  pub(crate) tag_list_state: TableState,
  pub(crate) entry_scroll: usize,
  pub(crate) general_config: GeneralConfig,
  pub(crate) ui_config: UiConfig,
  pub(crate) exit: bool,
  pub(crate) feed_tx: mpsc::UnboundedSender<FeedUpdate>,
  pub(crate) loading_state: LoadingState,
  pub(crate) current_feed: Option<String>,
  pub(crate) feed_errors: Vec<FeedError>,
  pub(crate) show_error_popup: bool,
  pub(crate) error_scroll: usize,
  pub(crate) show_help_popup: bool,
  pub(crate) help_scroll: usize,
  pub(crate) show_links_popup: bool,
  pub(crate) links_scroll: usize,
  pub(crate) links_selected: usize,
  pub(crate) show_confirm_popup: bool,
  pub(crate) confirm_feed_name: String,
  pub(crate) input: InputState,
  pub(crate) cache: FeedCache,
  pub(crate) theme: Theme,
  /// When the user is viewing a tag-filtered feed, this stores the tag query
  /// string (e.g. "tags:tech") so that `rebuild_display_feeds` can preserve
  /// the tag view across background feed refreshes.
  pub(crate) active_tag_query: Option<String>,
  /// Cached visible (unfiltered) entry indices for the currently selected feed.
  /// Invalidated whenever `hide_read`, `feed_index`, or entry data changes.
  pub(crate) visible_indices_cache: Vec<usize>,
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
      active_tag_query: None,
      visible_indices_cache: Vec::new(),
    }
  }

  pub fn should_exit(&self) -> bool {
    self.exit
  }

  /// Record an internal error so the user can see it in the TUI error popup.
  pub(crate) fn push_error(&mut self, name: impl Into<String>, error: impl Into<String>) {
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
        KeyCode::Char('y') => {
          self.yank_selected_link();
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
      KeyCode::Char('y') => match self.state {
        AppState::BrowsingEntries | AppState::ViewingEntry => {
          self.yank_entry_link();
        }
        _ => {}
      },
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
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::feeds::FeedEntry;

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

  // ─── Search tests ──────────────────────────────────────────────────────

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

  #[test]
  fn test_search_exact_match_ranked_first() {
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

    assert_eq!(app.feed_index, 2);
    assert_eq!(app.input.search_matches[0], 2); // exact
    assert_eq!(app.input.search_matches[1], 1); // substring
  }

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

    assert_eq!(app.input.search_matches.len(), 2);
    assert_eq!(app.input.search_match_cursor, 0);
    let first_selected = app.feed_index;

    app.handle_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(app.input.search_match_cursor, 1);
    let second_selected = app.feed_index;
    assert_ne!(first_selected, second_selected);

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
    for c in "zzz".chars() {
      app.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }
    assert!(app.input.search_matches.is_empty());

    app.handle_key(KeyEvent::from(KeyCode::Tab));
    app.handle_key(KeyEvent::from(KeyCode::BackTab));
    assert_eq!(app.input.search_match_cursor, 0);
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
}

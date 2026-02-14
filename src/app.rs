use crate::config::UiConfig;
use crate::feeds::Feed;
use crate::views::{entry_view, feeds_list_view};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::ListState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
  BrowsingFeeds,
  BrowsingEntries,
  ViewingEntry,
}

pub struct App {
  feeds: Vec<Feed>,
  feed_index: usize,
  feed_list_state: ListState,
  entry_list_state: ListState,
  state: AppState,
  entry_scroll: usize,
  ui_config: UiConfig,
  exit: bool,
}

impl App {
  pub fn new(feeds: Vec<Feed>, ui_config: UiConfig) -> Self {
    Self {
      feeds,
      feed_index: 0,
      feed_list_state: ListState::default().with_selected(Some(0)),
      entry_list_state: ListState::default(),
      state: AppState::BrowsingFeeds,
      entry_scroll: 0,
      ui_config,
      exit: false,
    }
  }

  pub fn should_exit(&self) -> bool {
    self.exit
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
        );
      }
    }
  }

  pub fn handle_key(&mut self, key: KeyEvent) {
    match key.code {
      KeyCode::Char('q') | KeyCode::Char('Q') => self.exit = true,
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

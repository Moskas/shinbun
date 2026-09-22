use super::types::{AppState, ListPane};
use super::App;

impl App {
  // ─── Visible entry helpers ────────────────────────────────────────────────

  /// Rebuild the cached visible-entry-indices vector.
  /// Must be called whenever `hide_read`, `feed_index`, or entry read-state changes.
  pub(super) fn invalidate_visible_indices(&mut self) {
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
  pub(super) fn visible_entry_indices(&self) -> &[usize] {
    &self.visible_indices_cache
  }

  /// Maps a visible (filtered) list position to the real entry index in the feed.
  pub(super) fn visible_to_real_entry_idx(&self, visible_idx: usize) -> Option<usize> {
    self.visible_indices_cache.get(visible_idx).copied()
  }

  /// Resolve the real entry index for the currently focused entry,
  /// regardless of whether we are in BrowsingEntries or ViewingEntry.
  pub(super) fn resolve_current_entry_idx(&self) -> Option<usize> {
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

  pub(super) fn handle_up(&mut self) {
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

  pub(super) fn handle_down(&mut self) {
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

  pub(super) fn handle_go_top(&mut self) {
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

  pub(super) fn handle_go_bottom(&mut self) {
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

  pub(super) fn handle_enter(&mut self) {
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
            self.active_tag_query = Some(tag_query.clone());
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
          self.input.current_entry_relative_index = Some(real_idx);
        }
        self.state = AppState::ViewingEntry;
        // Resume at the entry's persisted scroll position rather than the top.
        self.entry_scroll = self
          .input
          .current_entry_relative_index
          .and_then(|idx| {
            self
              .display_feeds
              .get(self.feed_index)?
              .entries(&self.feeds)
              .get(idx)
          })
          .map(|e| e.scroll_position)
          .unwrap_or(0);
        // Recomputed by the next render before any bottom-of-entry check runs.
        self.entry_max_scroll = 0;
        // Kick off background fetches for any inline images in this entry.
        if self.show_images {
          self.queue_entry_images();
        }
      }
      AppState::ViewingEntry => {}
    }
  }

  pub(super) fn handle_back(&mut self) {
    match self.state {
      AppState::ViewingEntry => {
        // Must run before clearing current_entry_relative_index — it's
        // needed to resolve which entry to persist the scroll position for.
        self.persist_current_entry_scroll();
        self.input.current_entry_relative_index = None;
        self.show_links_popup = false;
        self.links_scroll = 0;
        self.links_selected = 0;
        self.state = AppState::BrowsingEntries;
        // Entries are no longer marked read on open — bottom-detection while
        // viewing may have just marked one read, which can shrink the
        // visible list under hide_read. Re-clamp the selection.
        self.invalidate_visible_indices();
        let len = self.visible_entry_indices().len();
        if let Some(sel) = self.entry_list_state.selected() {
          if len == 0 {
            self.entry_list_state.select(None);
          } else if sel >= len {
            self.entry_list_state.select(Some(len - 1));
          }
        }
      }
      AppState::BrowsingEntries => {
        self.input.clear_search();
        self.active_tag_query = None;
        self.rebuild_display_feeds();
        self.list_pane = self.previous_list_pane;
        self.state = AppState::BrowsingFeeds;
      }
      AppState::BrowsingFeeds => {}
    }
  }
}

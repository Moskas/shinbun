use super::types::DisplayFeed;
use super::App;

impl App {
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
  pub(super) fn resolve_entry(
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
  pub(super) fn sync_read_state(&mut self, feed_vec_idx: usize, entry_vec_idx: usize, read: bool) {
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

  pub(super) fn toggle_selected_entry_read(&mut self, entry_idx: usize) {
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

  pub(super) fn mark_selected_entry_read(&mut self, entry_idx: usize) {
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
  pub(super) fn request_mark_feed_read(&mut self) {
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
  pub(super) fn mark_current_feed_read(&mut self) {
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

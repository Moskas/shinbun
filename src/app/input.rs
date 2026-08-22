#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AddFeedField {
  #[default]
  Url,
  Name,
  Tags,
  Refresh,
}

impl AddFeedField {
  pub fn next(self) -> Self {
    match self {
      Self::Url => Self::Name,
      Self::Name => Self::Tags,
      Self::Tags => Self::Refresh,
      Self::Refresh => Self::Url,
    }
  }

  pub fn prev(self) -> Self {
    match self {
      Self::Url => Self::Refresh,
      Self::Name => Self::Url,
      Self::Tags => Self::Name,
      Self::Refresh => Self::Tags,
    }
  }
}

#[derive(Debug, Default)]
pub struct AddFeedForm {
  pub url: String,
  pub name: String,
  pub tags: String,
  pub refresh: String,
  pub focus: AddFeedField,
  pub error: Option<String>,
}

impl AddFeedForm {
  pub fn clear(&mut self) {
    *self = Self::default();
  }

  pub fn active_buffer(&mut self) -> &mut String {
    match self.focus {
      AddFeedField::Url => &mut self.url,
      AddFeedField::Name => &mut self.name,
      AddFeedField::Tags => &mut self.tags,
      AddFeedField::Refresh => &mut self.refresh,
    }
  }
}

#[derive(Debug, Default)]
pub struct SaveEntryForm {
  pub path: String,
  /// Cursor position in `path`, counted in chars (not bytes).
  pub cursor: usize,
  /// Index (in chars) of the first character currently scrolled into view.
  /// Adjusted by `visible_window` to keep the cursor on screen.
  pub scroll: usize,
  pub error: Option<String>,
}

impl SaveEntryForm {
  pub fn clear(&mut self) {
    *self = Self::default();
  }

  /// Set `path` and place the cursor at its end.
  pub fn set_path(&mut self, path: String) {
    self.cursor = path.chars().count();
    self.scroll = 0;
    self.path = path;
  }

  fn byte_index(&self) -> usize {
    self
      .path
      .char_indices()
      .nth(self.cursor)
      .map(|(i, _)| i)
      .unwrap_or(self.path.len())
  }

  pub fn insert_char(&mut self, c: char) {
    let idx = self.byte_index();
    self.path.insert(idx, c);
    self.cursor += 1;
  }

  /// Delete the character before the cursor, like a text-editor backspace.
  pub fn backspace(&mut self) {
    if self.cursor == 0 {
      return;
    }
    let idx = self.byte_index();
    let prev_idx = self.path[..idx]
      .char_indices()
      .next_back()
      .map(|(i, _)| i)
      .unwrap_or(0);
    self.path.drain(prev_idx..idx);
    self.cursor -= 1;
  }

  pub fn move_left(&mut self) {
    self.cursor = self.cursor.saturating_sub(1);
  }

  pub fn move_right(&mut self) {
    let len = self.path.chars().count();
    self.cursor = (self.cursor + 1).min(len);
  }

  /// Display column (accounting for double-width characters like CJK) where
  /// char index `idx` starts, relative to the start of `path`.
  fn col_of(&self, idx: usize) -> usize {
    self
      .path
      .chars()
      .take(idx)
      .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(1))
      .sum()
  }

  /// Scroll just enough to keep the cursor within a `visible_width`-column
  /// window, then return the substring of `path` that fits in that window
  /// (unpadded) and the cursor's column offset within it.
  pub fn visible_window(&mut self, visible_width: usize) -> (String, u16) {
    if self.cursor < self.scroll {
      self.scroll = self.cursor;
    }
    while self.scroll < self.cursor
      && self.col_of(self.cursor) - self.col_of(self.scroll) >= visible_width
    {
      self.scroll += 1;
    }

    let start_col = self.col_of(self.scroll);
    let total_chars = self.path.chars().count();
    let mut end = self.scroll;
    while end < total_chars && self.col_of(end + 1) - start_col <= visible_width {
      end += 1;
    }
    end = end.max(self.cursor);

    let visible: String = self
      .path
      .chars()
      .skip(self.scroll)
      .take(end - self.scroll)
      .collect();
    let cursor_col = (self.col_of(self.cursor) - start_col) as u16;
    (visible, cursor_col)
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn cursor_column_accounts_for_double_width_chars() {
    let mut form = SaveEntryForm::default();
    form.set_path("~/日本語.md".to_string());
    // 8 chars: ~ / 日 本 語 . m d — but 日本語 are double-width, so the
    // cursor's screen column is wider than its char-index would suggest.
    let (_, cursor_col) = form.visible_window(100);
    assert_eq!(cursor_col, "~/日本語.md".chars().count() as u16 + 3);
  }

  #[test]
  fn visible_window_fits_within_width_with_wide_chars() {
    let mut form = SaveEntryForm::default();
    form.set_path("~/日本語のフィード - タイトル.md".to_string());
    let (visible, cursor_col) = form.visible_window(10);
    assert!(unicode_width::UnicodeWidthStr::width(visible.as_str()) <= 10);
    assert!(cursor_col <= 10);
  }

  #[test]
  fn visible_window_scrolls_left_when_cursor_moves_before_view() {
    let mut form = SaveEntryForm::default();
    form.set_path("abcdefghij".to_string());
    // Force scroll to the right end first.
    form.visible_window(4);
    assert!(form.scroll > 0);

    // Move the cursor back to the very start; the window must scroll with it.
    form.cursor = 0;
    let (visible, cursor_col) = form.visible_window(4);
    assert_eq!(form.scroll, 0);
    assert_eq!(cursor_col, 0);
    assert!(visible.starts_with('a'));
  }

  #[test]
  fn insert_and_backspace_are_byte_safe_with_wide_chars() {
    let mut form = SaveEntryForm::default();
    form.set_path("日本語".to_string());
    form.cursor = 1; // between 日 and 本
    form.insert_char('X');
    assert_eq!(form.path, "日X本語");
    form.backspace();
    assert_eq!(form.path, "日本語");
  }
}

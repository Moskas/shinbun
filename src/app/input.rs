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

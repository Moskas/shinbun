use crate::feeds::{Feed, FeedEntry};

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

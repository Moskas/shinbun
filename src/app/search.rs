use super::types::{AppState, ListPane};
use super::App;

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

impl App {
  /// Rebuild `input.search_matches` based on the current search query.
  /// Matches are sorted by score (best first), and the cursor selects the best match.
  pub(super) fn update_search_matches(&mut self) {
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
  pub(super) fn select_current_search_match(&mut self) {
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
  pub(super) fn search_next_match(&mut self) {
    if self.input.search_matches.is_empty() {
      return;
    }
    self.input.search_match_cursor =
      (self.input.search_match_cursor + 1) % self.input.search_matches.len();
    self.select_current_search_match();
  }

  /// Go to the previous search match (wrapping around).
  pub(super) fn search_prev_match(&mut self) {
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
}

#[cfg(test)]
mod tests {
  use super::*;

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
}

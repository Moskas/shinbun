use crate::app::{AppState, DisplayFeed, FeedError, LoadingState};
use crate::feeds::{Feed, FeedEntry};
use ratatui::{
  prelude::*,
  symbols::border,
  widgets::{
    Block, Borders, Cell, Clear, Padding, Paragraph, Row, Scrollbar, ScrollbarOrientation,
    ScrollbarState, StatefulWidget, Table, TableState, Wrap,
  },
};

// ─── Date formatting ──────────────────────────────────────────────────────────

fn format_entry_date(date_str: Option<&str>) -> String {
  let date_str = match date_str {
    Some(s) if !s.is_empty() => s,
    _ => return "     ".to_string(),
  };

  if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(date_str) {
    return dt.format("%d %b").to_string();
  }
  if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(date_str) {
    return dt.format("%d %b").to_string();
  }

  use chrono::NaiveDate;
  if let Ok(d) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
    return d.format("%d %b").to_string();
  }
  if let Ok(d) = NaiveDate::parse_from_str(date_str, "%d/%m/%Y") {
    return d.format("%d %b").to_string();
  }
  if let Ok(d) = NaiveDate::parse_from_str(date_str, "%m/%d/%Y") {
    return d.format("%d %b").to_string();
  }

  "     ".to_string()
}

// ─── Row builders ─────────────────────────────────────────────────────────────

/// Build a Table Row for a feed.
/// `raw_feeds` is the canonical feed list used to resolve Regular indices.
fn feed_row(feed: &DisplayFeed, raw_feeds: &[Feed]) -> Row<'static> {
  let entries = feed.entries(raw_feeds);
  let total = entries.len();
  let unread = entries.iter().filter(|e| !e.read).count();

  let count_str = format!("{}/{}", unread, total);
  let title = feed.title(raw_feeds).to_string();

  let style = if unread == 0 {
    Style::default().fg(Color::DarkGray)
  } else {
    Style::default()
  };

  Row::new(vec![
    Cell::from(
      Text::from(count_str)
        .alignment(Alignment::Right)
        .fg(Color::Blue),
    ),
    Cell::from(title),
  ])
  .style(style)
}

/// Build a Table Row for an entry.
fn entry_row(entry: &FeedEntry, is_query: bool) -> Row<'static> {
  let date = format_entry_date(entry.published.as_deref());

  let date_style = if entry.read {
    Style::default().fg(Color::DarkGray)
  } else {
    Style::default().fg(Color::Cyan)
  };

  let source_style = if entry.read {
    Style::default().fg(Color::DarkGray)
  } else {
    Style::default().fg(Color::Yellow)
  };

  let title_style = if entry.read {
    Style::default().fg(Color::DarkGray)
  } else {
    Style::default().bold()
  };

  if is_query {
    let source = entry.feed_title.clone().unwrap_or_default();
    Row::new(vec![
      Cell::from(
        Text::from(date)
          .alignment(Alignment::Right)
          .style(date_style),
      ),
      Cell::from(Text::from(source).alignment(Alignment::Left)).style(source_style),
      Cell::from(entry.title.clone()).style(title_style),
    ])
  } else {
    Row::new(vec![
      Cell::from(
        Text::from(date)
          .alignment(Alignment::Right)
          .style(date_style),
      ),
      Cell::from(entry.title.clone()).style(title_style),
    ])
  }
}

// ─── Public render entry-point ────────────────────────────────────────────────

/// Bundled parameters for the feeds list view (everything except `Frame`).
pub struct FeedsViewState<'a> {
  pub raw_feeds: &'a [Feed],
  pub display_feeds: &'a [DisplayFeed],
  pub feed_state: &'a mut TableState,
  pub entry_state: &'a mut TableState,
  pub app_state: AppState,
  pub show_borders: bool,
  pub loading_state: &'a LoadingState,
  pub current_feed: Option<&'a str>,
  pub feed_errors: &'a [FeedError],
  pub show_error_popup: bool,
  pub error_scroll: &'a mut usize,
  pub hide_read: bool,
}

pub fn render(frame: &mut Frame, area: Rect, s: &mut FeedsViewState) {
  let title = " Shinbun ".bold().yellow();

  let mut instruction_spans = vec![" Help ".into(), "<?>".bold()];
  if !s.feed_errors.is_empty() {
    instruction_spans.push(" Errors ".into());
    instruction_spans.push("<e> ".bold().red());
  }
  let instructions = instruction_spans;

  let outer_block = if s.show_borders {
    Block::default()
      .title(title)
      .title_bottom(instructions)
      .borders(Borders::ALL)
      .border_style(Style::new().blue())
      .border_set(border::PLAIN)
  } else {
    Block::default().title(title).title_bottom(instructions)
  };

  let inner_area = outer_block.inner(area);
  outer_block.render(area, frame.buffer_mut());

  render_main_pane(
    frame,
    inner_area,
    s.raw_feeds,
    s.display_feeds,
    s.feed_state,
    s.entry_state,
    s.app_state,
    s.show_borders,
    s.loading_state,
    s.hide_read,
  );

  if s.show_error_popup {
    render_error_popup(frame, area, s.feed_errors, s.error_scroll);
  }
  if s.loading_state.should_show_popup() {
    render_loading_popup(frame, area, s.loading_state, s.current_feed);
  }
}

#[allow(clippy::too_many_arguments)]
fn render_main_pane(
  frame: &mut Frame,
  area: Rect,
  raw_feeds: &[Feed],
  display_feeds: &[DisplayFeed],
  feed_state: &mut TableState,
  entry_state: &mut TableState,
  app_state: AppState,
  show_borders: bool,
  loading_state: &LoadingState,
  hide_read: bool,
) {
  match app_state {
    AppState::BrowsingFeeds => {
      let feed_rows: Vec<Row> = if display_feeds.is_empty() {
        let msg = if loading_state.is_loading {
          format!(" {} Loading feeds...", loading_state.spinner_frame())
        } else {
          " No feeds configured. Press 'r' to load.".to_string()
        };
        vec![Row::new(vec![Cell::from(""), Cell::from(msg)])]
      } else {
        display_feeds
          .iter()
          .map(|f| feed_row(f, raw_feeds))
          .collect()
      };

      let count_width = display_feeds
        .iter()
        .map(|f| {
          let entries = f.entries(raw_feeds);
          let total = entries.len();
          let unread = entries.iter().filter(|e| !e.read).count();
          format!("{}/{}", unread, total).len() as u16
        })
        .max()
        .unwrap_or(5)
        .max(5);

      let feed_widths = [Constraint::Length(count_width), Constraint::Fill(1)];

      let feeds_table = Table::new(feed_rows, feed_widths)
        .block(create_feed_block(display_feeds.len(), show_borders))
        .column_spacing(2)
        .row_highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black));

      StatefulWidget::render(feeds_table, area, frame.buffer_mut(), feed_state);
    }

    AppState::BrowsingEntries | AppState::ViewingEntry => {
      let selected_feed_idx = feed_state.selected().unwrap_or(0);
      let (entry_rows, is_query, source_width) =
        build_entry_rows(display_feeds, raw_feeds, selected_feed_idx, hide_read);
      let entry_widths = entry_column_widths(is_query, source_width);
      let entry_count = entry_rows.len();

      let entries_table = Table::new(entry_rows, entry_widths)
        .block(create_entry_block(entry_count, show_borders))
        .column_spacing(2)
        .row_highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black).bold());

      StatefulWidget::render(entries_table, area, frame.buffer_mut(), entry_state);
    }
  }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Width of the date column in the entries table.
const DATE_COL_WIDTH: u16 = 8;

/// Build entry rows for the selected feed, returning (rows, is_query, max_source_col_width).
/// When `hide_read` is true, entries marked as read are excluded from the result.
fn build_entry_rows(
  display_feeds: &[DisplayFeed],
  raw_feeds: &[Feed],
  selected_feed_idx: usize,
  hide_read: bool,
) -> (Vec<Row<'static>>, bool, u16) {
  if let Some(feed) = display_feeds.get(selected_feed_idx) {
    let all_entries = feed.entries(raw_feeds);

    // Apply the hide-read filter
    let entries: Vec<&FeedEntry> = all_entries
      .iter()
      .filter(|e| !hide_read || !e.read)
      .collect();

    if entries.is_empty() {
      return (
        vec![Row::new(vec![Cell::from(""), Cell::from(" No entries")])],
        false,
        0,
      );
    }

    let is_query = feed.is_query();

    let source_width: u16 = if is_query {
      entries
        .iter()
        .map(|e| e.feed_title.as_deref().map(|t| t.len() as u16).unwrap_or(0))
        .max()
        .unwrap_or(0)
    } else {
      0
    };

    let rows = entries.iter().map(|e| entry_row(e, is_query)).collect();

    (rows, is_query, source_width)
  } else {
    (vec![], false, 0)
  }
}

/// Column width constraints for the entries table.
fn entry_column_widths(is_query: bool, source_width: u16) -> Vec<Constraint> {
  if is_query && source_width > 0 {
    vec![
      Constraint::Length(DATE_COL_WIDTH),
      Constraint::Length(source_width),
      Constraint::Fill(1),
    ]
  } else {
    vec![Constraint::Length(DATE_COL_WIDTH), Constraint::Fill(1)]
  }
}

// ─── Blocks ───────────────────────────────────────────────────────────────────

fn create_feed_block(count: usize, show_borders: bool) -> Block<'static> {
  let title = " Feeds ".green();
  let count_title = format!(" {} ", count).yellow();
  if show_borders {
    Block::default()
      .title(title)
      .title(count_title)
      .borders(Borders::ALL)
      .border_style(Style::new().blue())
      .border_set(border::PLAIN)
      .padding(Padding::horizontal(1))
  } else {
    Block::default()
      .title(title)
      .title(count_title)
      .padding(Padding::horizontal(1))
  }
}

fn create_entry_block(count: usize, show_borders: bool) -> Block<'static> {
  let title = " Entries ".green();
  let count_title = format!(" {} ", count).yellow();
  if show_borders {
    Block::default()
      .title(title)
      .title(count_title)
      .borders(Borders::ALL)
      .border_style(Style::new().blue())
      .border_set(border::PLAIN)
      .padding(Padding::horizontal(1))
  } else {
    Block::default()
      .title(title)
      .title(count_title)
      .padding(Padding::horizontal(1))
  }
}

// ─── Popups ───────────────────────────────────────────────────────────────────

fn render_error_popup(
  frame: &mut Frame,
  area: Rect,
  feed_errors: &[FeedError],
  scroll: &mut usize,
) {
  let popup_width = area.width.saturating_sub(10).min(80);
  let popup_height = (feed_errors.len() as u16 + 4).min(area.height.saturating_sub(4));

  let popup_area = Rect {
    x: area.x + (area.width.saturating_sub(popup_width)) / 2,
    y: area.y + (area.height.saturating_sub(popup_height)) / 2,
    width: popup_width,
    height: popup_height,
  };

  Clear.render(popup_area, frame.buffer_mut());

  let error_text: Vec<Line> = feed_errors
    .iter()
    .map(|e| Line::from(format!(" {} : {}", e.name, e.error)).red())
    .collect();

  let content_len = error_text.len();

  let block = Block::default()
    .title(" Feed Errors ".red().bold())
    .title_bottom(" <e> or <Esc> to close ".gray())
    .borders(Borders::ALL)
    .border_style(Style::new().red())
    .border_set(border::PLAIN);

  let inner_height = block.inner(popup_area).height as usize;
  let max_scroll = content_len.saturating_sub(inner_height);
  *scroll = (*scroll).min(max_scroll);

  let popup = Paragraph::new(error_text)
    .block(block)
    .scroll((*scroll as u16, 0))
    .wrap(Wrap { trim: false });

  popup.render(popup_area, frame.buffer_mut());

  if content_len > inner_height {
    let scrollbar_area = Rect {
      x: popup_area.x + popup_area.width.saturating_sub(1),
      y: popup_area.y + 1,
      width: 1,
      height: popup_area.height.saturating_sub(2),
    };

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
      .begin_symbol(Some("▲"))
      .end_symbol(Some("▼"));

    let mut scrollbar_state = ScrollbarState::new(max_scroll + 1).position(*scroll);
    scrollbar.render(scrollbar_area, frame.buffer_mut(), &mut scrollbar_state);
  }
}

fn render_loading_popup(
  frame: &mut Frame,
  area: Rect,
  loading_state: &LoadingState,
  current_feed: Option<&str>,
) {
  let status_line = if loading_state.is_loading {
    let spinner = loading_state.spinner_frame();
    if let Some(feed_name) = current_feed {
      let display_name = if feed_name.len() > 28 {
        let truncated: String = feed_name.chars().take(25).collect();
        format!("{}...", truncated)
      } else {
        feed_name.to_string()
      };
      format!(" {} Fetching: {} ", spinner, display_name)
    } else {
      let elapsed = loading_state.elapsed_secs();
      if elapsed > 0 {
        format!(" {} Loading... ({}s) ", spinner, elapsed)
      } else {
        format!(" {} Loading... ", spinner)
      }
    }
  } else if loading_state.is_initial_load {
    format!(" ✓ {} feeds loaded ", loading_state.updated_feeds.len())
  } else {
    match loading_state.updated_feeds.len() {
      0 => " ✓ Updated ".to_string(),
      1 => format!(" ✓ {} updated ", loading_state.updated_feeds[0]),
      n => format!(" ✓ {} feeds updated ", n),
    }
  };
  let popup_width = (status_line.len() as u16 + 2).min(area.width.saturating_sub(2));
  let popup_height = 3u16;
  let popup_x = area.x + area.width.saturating_sub(popup_width + 1);
  let popup_y = area.y + 1;

  let popup_area = Rect {
    x: popup_x,
    y: popup_y,
    width: popup_width,
    height: popup_height,
  };

  Clear.render(popup_area, frame.buffer_mut());

  let (border_style, text_style) = if loading_state.is_loading {
    (Style::new().cyan(), Style::new().cyan())
  } else {
    (Style::new().green(), Style::new().green())
  };

  let popup = Paragraph::new(Line::from(status_line).style(text_style)).block(
    Block::default()
      .borders(Borders::ALL)
      .border_style(border_style)
      .border_set(border::PLAIN),
  );

  popup.render(popup_area, frame.buffer_mut());
}

#[cfg(test)]
mod tests {
  use super::*;

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

  // ─── format_entry_date tests ─────────────────────────────────────────────

  #[test]
  fn test_format_entry_date_rfc3339() {
    let result = format_entry_date(Some("2024-01-15T10:30:00+00:00"));
    assert_eq!(result, "15 Jan");
  }

  #[test]
  fn test_format_entry_date_rfc2822() {
    let result = format_entry_date(Some("Mon, 15 Jan 2024 10:30:00 +0000"));
    assert_eq!(result, "15 Jan");
  }

  #[test]
  fn test_format_entry_date_iso_date() {
    let result = format_entry_date(Some("2024-01-15"));
    assert_eq!(result, "15 Jan");
  }

  #[test]
  fn test_format_entry_date_dd_mm_yyyy() {
    let result = format_entry_date(Some("15/01/2024"));
    assert_eq!(result, "15 Jan");
  }

  #[test]
  fn test_format_entry_date_mm_dd_yyyy() {
    let result = format_entry_date(Some("01/15/2024"));
    assert_eq!(result, "15 Jan");
  }

  #[test]
  fn test_format_entry_date_none() {
    let result = format_entry_date(None);
    assert_eq!(result, "     ");
  }

  #[test]
  fn test_format_entry_date_empty() {
    let result = format_entry_date(Some(""));
    assert_eq!(result, "     ");
  }

  #[test]
  fn test_format_entry_date_invalid() {
    let result = format_entry_date(Some("not a date"));
    assert_eq!(result, "     ");
  }

  // ─── entry_column_widths tests ───────────────────────────────────────────

  #[test]
  fn test_entry_column_widths_no_query() {
    let widths = entry_column_widths(false, 0);
    assert_eq!(widths.len(), 2);
    assert_eq!(widths[0], Constraint::Length(DATE_COL_WIDTH));
  }

  #[test]
  fn test_entry_column_widths_query() {
    let widths = entry_column_widths(true, 15);
    assert_eq!(widths.len(), 3);
    assert_eq!(widths[0], Constraint::Length(DATE_COL_WIDTH));
    assert_eq!(widths[1], Constraint::Length(15));
  }

  #[test]
  fn test_entry_column_widths_query_zero_source() {
    // Query with zero source width falls back to non-query layout
    let widths = entry_column_widths(true, 0);
    assert_eq!(widths.len(), 2);
  }

  // ─── build_entry_rows tests ──────────────────────────────────────────────

  #[test]
  fn test_build_entry_rows_regular_feed() {
    let feeds = vec![make_feed(
      "https://example.com/rss",
      "Feed",
      vec![
        make_entry("Post 1", Some("2024-01-01T00:00:00Z"), false),
        make_entry("Post 2", Some("2024-02-01T00:00:00Z"), true),
      ],
    )];
    let display = vec![DisplayFeed::Regular(0)];

    let (rows, is_query, source_width) = build_entry_rows(&display, &feeds, 0, false);
    assert_eq!(rows.len(), 2);
    assert!(!is_query);
    assert_eq!(source_width, 0);
  }

  #[test]
  fn test_build_entry_rows_hide_read() {
    let feeds = vec![make_feed(
      "https://example.com/rss",
      "Feed",
      vec![
        make_entry("Unread Post", None, false),
        make_entry("Read Post", None, true),
      ],
    )];
    let display = vec![DisplayFeed::Regular(0)];

    let (rows, _, _) = build_entry_rows(&display, &feeds, 0, true);
    assert_eq!(rows.len(), 1); // Only unread entry
  }

  #[test]
  fn test_build_entry_rows_empty_feed() {
    let feeds = vec![make_feed("https://example.com/rss", "Feed", vec![])];
    let display = vec![DisplayFeed::Regular(0)];

    let (rows, is_query, _) = build_entry_rows(&display, &feeds, 0, false);
    assert_eq!(rows.len(), 1); // "No entries" placeholder
    assert!(!is_query);
  }

  #[test]
  fn test_build_entry_rows_query_feed() {
    let feeds = vec![make_feed(
      "https://example.com/rss",
      "Feed A",
      vec![make_entry("Post 1", None, false)],
    )];

    let mut query_entry = make_entry("Post 1", None, false);
    query_entry.feed_title = Some("Feed A".to_string());
    let display = vec![DisplayFeed::Query {
      name: "All Blogs".to_string(),
      entries: vec![query_entry],
    }];

    let (rows, is_query, source_width) = build_entry_rows(&display, &feeds, 0, false);
    assert_eq!(rows.len(), 1);
    assert!(is_query);
    assert!(source_width > 0);
  }

  #[test]
  fn test_build_entry_rows_out_of_bounds() {
    let feeds: Vec<Feed> = vec![];
    let display: Vec<DisplayFeed> = vec![];

    let (rows, is_query, _) = build_entry_rows(&display, &feeds, 5, false);
    assert!(rows.is_empty());
    assert!(!is_query);
  }

  #[test]
  fn test_build_entry_rows_all_read_hidden() {
    let feeds = vec![make_feed(
      "https://example.com/rss",
      "Feed",
      vec![
        make_entry("Read 1", None, true),
        make_entry("Read 2", None, true),
      ],
    )];
    let display = vec![DisplayFeed::Regular(0)];

    let (rows, _, _) = build_entry_rows(&display, &feeds, 0, true);
    // When all entries are read and hide_read is on, should show "No entries"
    assert_eq!(rows.len(), 1);
  }
}

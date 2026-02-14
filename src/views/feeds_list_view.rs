use crate::app::AppState;
use crate::feeds::Feed;
use ratatui::{
  prelude::*,
  symbols::border,
  widgets::{
    block::{Position, Title},
    Block, Borders, List, ListItem, ListState, StatefulWidget,
  },
};

/// Format a date string for display in entry list
/// Returns formatted date like "02 May" or empty string if parsing fails
fn format_entry_date(date_str: Option<&str>) -> String {
  let date_str = match date_str {
    Some(s) if !s.is_empty() => s,
    _ => return "     ".to_string(),
  };

  // Try RFC3339/ISO8601 (most common, and what we store)
  if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(date_str) {
    return dt.format("%d %b").to_string();
  }

  // Try RFC2822 (traditional RSS format)
  if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(date_str) {
    return dt.format("%d %b").to_string();
  }

  // Try common date-only formats
  use chrono::NaiveDate;

  // YYYY-MM-DD
  if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
    return date.format("%d %b").to_string();
  }

  // DD/MM/YYYY
  if let Ok(date) = NaiveDate::parse_from_str(date_str, "%d/%m/%Y") {
    return date.format("%d %b").to_string();
  }

  // MM/DD/YYYY
  if let Ok(date) = NaiveDate::parse_from_str(date_str, "%m/%d/%Y") {
    return date.format("%d %b").to_string();
  }

  // If all parsing fails, log and return spaces
  eprintln!("Warning: Could not parse date: {}", date_str);
  "     ".to_string()
}

/// Render the feeds and entries list view
pub fn render(
  frame: &mut Frame,
  area: Rect,
  feeds: &[Feed],
  feed_state: &mut ListState,
  entry_state: &mut ListState,
  app_state: AppState,
  split_view: bool,
  show_borders: bool,
) {
  // Create the outer container
  let title = Title::from(" Shinbun ".bold().yellow());
  let instructions = Title::from(Line::from(vec![" Quit ".into(), "<q> ".bold()]));

  let outer_block = if show_borders {
    Block::default()
      .title(title.alignment(Alignment::Left))
      .title(
        instructions
          .alignment(Alignment::Left)
          .position(Position::Bottom),
      )
      .title_bottom(Line::from(" Help <?> ").right_aligned())
      .borders(Borders::ALL)
      .border_style(Style::new().blue())
      .border_set(border::PLAIN)
  } else {
    Block::default()
      .title(title.alignment(Alignment::Left))
      .title(
        instructions
          .alignment(Alignment::Left)
          .position(Position::Bottom),
      )
      .title_bottom(Line::from(" Help <?> ").right_aligned())
  };

  let inner_area = outer_block.inner(area);
  outer_block.render(area, frame.buffer_mut());

  // Use split view only if explicitly enabled in config
  if split_view {
    render_dual_pane(
      frame,
      inner_area,
      feeds,
      feed_state,
      entry_state,
      app_state,
      show_borders,
    );
  } else {
    // Default: newsboat-style single pane navigation
    render_single_pane(
      frame,
      inner_area,
      feeds,
      feed_state,
      entry_state,
      app_state,
      show_borders,
    );
  }
}

/// Render dual-pane layout (feeds on left, entries on right)
fn render_dual_pane(
  frame: &mut Frame,
  area: Rect,
  feeds: &[Feed],
  feed_state: &mut ListState,
  entry_state: &mut ListState,
  app_state: AppState,
  show_borders: bool,
) {
  let chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
    .split(area);

  // Render feeds list
  let feed_items: Vec<ListItem> = feeds
    .iter()
    .map(|feed| ListItem::new(format!(" {}", feed.title)))
    .collect();

  let feed_highlight = match app_state {
    AppState::BrowsingFeeds => Style::default().bg(Color::Yellow).fg(Color::Black),
    _ => Style::default().yellow(),
  };

  let feeds_list = List::new(feed_items)
    .block(create_feed_block(feeds.len(), show_borders))
    .highlight_style(feed_highlight);

  StatefulWidget::render(feeds_list, chunks[0], frame.buffer_mut(), feed_state);

  // Render entries list
  let selected_feed_idx = feed_state.selected().unwrap_or(0);
  let entry_items: Vec<ListItem> = if let Some(feed) = feeds.get(selected_feed_idx) {
    feed
      .entries
      .iter()
      .map(|entry| {
        let date = format_entry_date(entry.published.as_deref());
        ListItem::new(format!(" {} {}", date, entry.title))
      })
      .collect()
  } else {
    vec![]
  };

  let entry_highlight = match app_state {
    AppState::BrowsingEntries => Style::default().bg(Color::Yellow).fg(Color::Black).bold(),
    _ => Style::default(),
  };

  let entries_list = List::new(entry_items.clone())
    .block(create_entry_block(entry_items.len(), show_borders))
    .highlight_style(entry_highlight);

  StatefulWidget::render(entries_list, chunks[1], frame.buffer_mut(), entry_state);
}

/// Render single-pane layout (one list at a time)
fn render_single_pane(
  frame: &mut Frame,
  area: Rect,
  feeds: &[Feed],
  feed_state: &mut ListState,
  entry_state: &mut ListState,
  app_state: AppState,
  show_borders: bool,
) {
  match app_state {
    AppState::BrowsingFeeds => {
      let feed_items: Vec<ListItem> = feeds
        .iter()
        .map(|feed| ListItem::new(format!(" {}", feed.title)))
        .collect();

      let feeds_list = List::new(feed_items)
        .block(create_feed_block(feeds.len(), show_borders))
        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black));

      StatefulWidget::render(feeds_list, area, frame.buffer_mut(), feed_state);
    }
    AppState::BrowsingEntries | AppState::ViewingEntry => {
      let selected_feed_idx = feed_state.selected().unwrap_or(0);
      let entry_items: Vec<ListItem> = if let Some(feed) = feeds.get(selected_feed_idx) {
        feed
          .entries
          .iter()
          .map(|entry| {
            let date = format_entry_date(entry.published.as_deref());
            ListItem::new(format!(" {} {}", date, entry.title))
          })
          .collect()
      } else {
        vec![]
      };

      let entries_list = List::new(entry_items.clone())
        .block(create_entry_block(entry_items.len(), show_borders))
        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black).bold());

      StatefulWidget::render(entries_list, area, frame.buffer_mut(), entry_state);
    }
  }
}

/// Create a styled block for the feeds list
fn create_feed_block(count: usize, show_borders: bool) -> Block<'static> {
  let title = Title::from(" Feeds ".green());
  let count_title = Title::from(format!(" {} ", count).yellow())
    .alignment(Alignment::Right)
    .position(Position::Top);

  if show_borders {
    Block::default()
      .title(title)
      .title(count_title)
      .borders(Borders::ALL)
      .border_style(Style::new().blue())
      .border_set(border::PLAIN)
  } else {
    Block::default().title(title).title(count_title)
  }
}

/// Create a styled block for the entries list
fn create_entry_block(count: usize, show_borders: bool) -> Block<'static> {
  let title = Title::from(" Entries ".green());
  let count_title = Title::from(format!(" {} ", count).yellow())
    .alignment(Alignment::Right)
    .position(Position::Top);

  if show_borders {
    Block::default()
      .title(title)
      .title(count_title)
      .borders(Borders::ALL)
      .border_style(Style::new().blue())
      .border_set(border::PLAIN)
  } else {
    Block::default().title(title).title(count_title)
  }
}

use crate::app::{AppState, DisplayFeed, FeedError, LoadingState};
use ratatui::{
  prelude::*,
  symbols::border,
  widgets::{
    block::{Position, Title},
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Wrap,
  },
};

/// Format a date string for display in entry list
/// Returns formatted date like "02 May" or empty string if parsing fails
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

  if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
    return date.format("%d %b").to_string();
  }

  if let Ok(date) = NaiveDate::parse_from_str(date_str, "%d/%m/%Y") {
    return date.format("%d %b").to_string();
  }

  if let Ok(date) = NaiveDate::parse_from_str(date_str, "%m/%d/%Y") {
    return date.format("%d %b").to_string();
  }

  "     ".to_string()
}

/// Build a ListItem for a feed, showing (unread/total) counts
fn feed_list_item(feed: &DisplayFeed) -> ListItem<'static> {
  let total = feed.entries().len();
  let unread = feed.entries().iter().filter(|e| !e.read).count();

  let label = if feed.is_query() {
    format!(" ({}/{}) | 🔍 {}", unread, total, feed.title())
  } else {
    format!(" ({}/{}) | {}", unread, total, feed.title())
  };

  if unread == 0 {
    ListItem::new(label).style(Style::default().fg(Color::DarkGray))
  } else {
    ListItem::new(label)
  }
}

/// Build a ListItem for an entry, applying gray style if already read
fn entry_list_item(entry: &crate::feeds::FeedEntry, is_query: bool) -> ListItem<'static> {
  let date = format_entry_date(entry.published.as_deref());

  let text = if is_query {
    if let Some(feed_title) = &entry.feed_title {
      format!(" {} [{}] {}", date, feed_title, entry.title)
    } else {
      format!(" {} {}", date, entry.title)
    }
  } else {
    format!(" {} {}", date, entry.title)
  };

  if entry.read {
    ListItem::new(text).style(Style::default().fg(Color::DarkGray))
  } else {
    ListItem::new(text)
  }
}

/// Render the feeds and entries list view
pub fn render(
  frame: &mut Frame,
  area: Rect,
  feeds: &[DisplayFeed],
  feed_state: &mut ListState,
  entry_state: &mut ListState,
  app_state: AppState,
  split_view: bool,
  show_borders: bool,
  loading_state: &LoadingState,
  current_feed: Option<&str>,
  feed_errors: &[FeedError],
  show_error_popup: bool,
) {
  let title = Title::from(" Shinbun ".bold().yellow());

  let mut instruction_spans = vec![
    " Quit ".into(),
    "<q> ".bold(),
    " Refresh ".into(),
    "<r> ".bold(),
    " Mark read/unread ".into(),
    "<m> ".bold(),
  ];

  if !feed_errors.is_empty() {
    instruction_spans.push(" Errors ".into());
    instruction_spans.push("<e> ".bold().red());
  }

  let instructions = Title::from(Line::from(instruction_spans));

  // Outer block no longer shows status in the border — that goes in the popup
  let outer_block = if show_borders {
    Block::default()
      .title(title.alignment(Alignment::Left))
      .title(
        instructions
          .alignment(Alignment::Left)
          .position(Position::Bottom),
      )
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
  };

  let inner_area = outer_block.inner(area);
  outer_block.render(area, frame.buffer_mut());

  if split_view {
    render_dual_pane(
      frame,
      inner_area,
      feeds,
      feed_state,
      entry_state,
      app_state,
      show_borders,
      loading_state,
    );
  } else {
    render_single_pane(
      frame,
      inner_area,
      feeds,
      feed_state,
      entry_state,
      app_state,
      show_borders,
      loading_state,
    );
  }

  if show_error_popup {
    render_error_popup(frame, area, feed_errors);
  }

  // Show loading popup in top-right while fetching and briefly after completion
  if loading_state.should_show_popup() {
    render_loading_popup(frame, area, loading_state, current_feed, feeds);
  }
}

/// Render dual-pane layout (feeds on left, entries on right)
fn render_dual_pane(
  frame: &mut Frame,
  area: Rect,
  feeds: &[DisplayFeed],
  feed_state: &mut ListState,
  entry_state: &mut ListState,
  app_state: AppState,
  show_borders: bool,
  loading_state: &LoadingState,
) {
  let chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
    .split(area);

  // Render feeds list
  let feed_items: Vec<ListItem> = if feeds.is_empty() {
    if loading_state.is_loading {
      let spinner = loading_state.spinner_frame();
      vec![ListItem::new(format!(" {} Loading feeds...", spinner))]
    } else {
      vec![ListItem::new(
        " No feeds configured. Press 'r' to load.".to_string(),
      )]
    }
  } else {
    feeds.iter().map(|feed| feed_list_item(feed)).collect()
  };

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
    if feed.entries().is_empty() {
      vec![ListItem::new(" No entries".to_string())]
    } else {
      let is_query = feed.is_query();
      feed
        .entries()
        .iter()
        .map(|entry| entry_list_item(entry, is_query))
        .collect()
    }
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
  feeds: &[DisplayFeed],
  feed_state: &mut ListState,
  entry_state: &mut ListState,
  app_state: AppState,
  show_borders: bool,
  loading_state: &LoadingState,
) {
  match app_state {
    AppState::BrowsingFeeds => {
      let feed_items: Vec<ListItem> = if feeds.is_empty() {
        if loading_state.is_loading {
          let spinner = loading_state.spinner_frame();
          vec![ListItem::new(format!(" {} Loading feeds...", spinner))]
        } else {
          vec![ListItem::new(
            " No feeds configured. Press 'r' to load.".to_string(),
          )]
        }
      } else {
        feeds.iter().map(|feed| feed_list_item(feed)).collect()
      };

      let feeds_list = List::new(feed_items)
        .block(create_feed_block(feeds.len(), show_borders))
        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black));

      StatefulWidget::render(feeds_list, area, frame.buffer_mut(), feed_state);
    }
    AppState::BrowsingEntries | AppState::ViewingEntry => {
      let selected_feed_idx = feed_state.selected().unwrap_or(0);
      let entry_items: Vec<ListItem> = if let Some(feed) = feeds.get(selected_feed_idx) {
        if feed.entries().is_empty() {
          vec![ListItem::new(" No entries".to_string())]
        } else {
          let is_query = feed.is_query();
          feed
            .entries()
            .iter()
            .map(|entry| entry_list_item(entry, is_query))
            .collect()
        }
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
  let count_title = Title::from(format!(" {} ", count).yellow()).position(Position::Top);

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
  let count_title = Title::from(format!(" {} ", count).yellow()).position(Position::Top);

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

/// Render the error popup
fn render_error_popup(frame: &mut Frame, area: Rect, feed_errors: &[FeedError]) {
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

  let popup = Paragraph::new(error_text)
    .block(
      Block::default()
        .title(" Feed Errors ".red().bold())
        .title(
          Title::from(" <e> or <Esc> to close ".gray())
            .position(Position::Bottom)
            .alignment(Alignment::Right),
        )
        .borders(Borders::ALL)
        .border_style(Style::new().red())
        .border_set(border::PLAIN),
    )
    .wrap(Wrap { trim: false });

  popup.render(popup_area, frame.buffer_mut());
}

/// Render the loading status popup in the top-right corner.
/// Shown while fetching is in progress and for a few seconds after completion.
fn render_loading_popup(
  frame: &mut Frame,
  area: Rect,
  loading_state: &LoadingState,
  current_feed: Option<&str>,
  feeds: &[DisplayFeed],
) {
  let status_line = if loading_state.is_loading {
    let spinner = loading_state.spinner_frame();
    if let Some(feed_name) = current_feed {
      let display_name = if feed_name.len() > 28 {
        format!("{}...", &feed_name[..25])
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
  } else {
    // Just finished — show loaded count
    format!(" ✓ {} feeds loaded ", feeds.len())
  };

  // Size the popup to fit its content (title border takes 2 cols of width, 2 rows of height)
  let popup_width = (status_line.len() as u16 + 2).min(area.width.saturating_sub(2));
  let popup_height = 3u16; // top border + 1 content row + bottom border

  // Position: top-right corner with a 1-cell margin
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

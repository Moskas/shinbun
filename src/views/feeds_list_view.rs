use crate::app::{AppState, FeedError, LoadingState};
use crate::feeds::Feed;
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

  // If all parsing fails, return spaces
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
  loading_state: &LoadingState,
  current_feed: Option<&str>,
  feed_errors: &[FeedError],
  show_error_popup: bool,
) {
  // Create the outer container
  let title = Title::from(" Shinbun ".bold().yellow());

  // Build instructions with error indicator
  let mut instruction_spans = vec![
    " Quit ".into(),
    "<q> ".bold(),
    " Refresh ".into(),
    "<r> ".bold(),
  ];

  if !feed_errors.is_empty() {
    instruction_spans.push(" Errors ".into());
    instruction_spans.push("<e> ".bold().red());
  }

  let instructions = Title::from(Line::from(instruction_spans));

  // Create status message at the TOP (right side)
  let status_msg = if loading_state.is_loading {
    let spinner = loading_state.spinner_frame();
    if let Some(feed_name) = current_feed {
      // Truncate feed name if too long
      let display_name = if feed_name.len() > 30 {
        format!("{}...", &feed_name[..27])
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
  } else if feeds.is_empty() {
    " No feeds loaded - Press 'r' ".to_string()
  } else {
    format!(" {} feeds loaded ", feeds.len())
  };

  let status_title = Title::from(status_msg.cyan())
    .alignment(Alignment::Right)
    .position(Position::Top);

  let outer_block = if show_borders {
    Block::default()
      .title(title.alignment(Alignment::Left))
      .title(status_title)
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
      .title(status_title)
      .title(
        instructions
          .alignment(Alignment::Left)
          .position(Position::Bottom),
      )
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
      loading_state,
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
      loading_state,
    );
  }

  // Render error popup if requested
  if show_error_popup {
    render_error_popup(frame, area, feed_errors);
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
    feeds
      .iter()
      .map(|feed| ListItem::new(format!(" {}", feed.title)))
      .collect()
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
    if feed.entries.is_empty() {
      vec![ListItem::new(" No entries".to_string())]
    } else {
      feed
        .entries
        .iter()
        .map(|entry| {
          let date = format_entry_date(entry.published.as_deref());
          ListItem::new(format!(" {} {}", date, entry.title))
        })
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
  feeds: &[Feed],
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
        feeds
          .iter()
          .map(|feed| ListItem::new(format!(" {}", feed.title)))
          .collect()
      };

      let feeds_list = List::new(feed_items)
        .block(create_feed_block(feeds.len(), show_borders))
        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black));

      StatefulWidget::render(feeds_list, area, frame.buffer_mut(), feed_state);
    }
    AppState::BrowsingEntries | AppState::ViewingEntry => {
      let selected_feed_idx = feed_state.selected().unwrap_or(0);
      let entry_items: Vec<ListItem> = if let Some(feed) = feeds.get(selected_feed_idx) {
        if feed.entries.is_empty() {
          vec![ListItem::new(" No entries".to_string())]
        } else {
          feed
            .entries
            .iter()
            .map(|entry| {
              let date = format_entry_date(entry.published.as_deref());
              ListItem::new(format!(" {} {}", date, entry.title))
            })
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

/// Render a popup showing feed errors
fn render_error_popup(frame: &mut Frame, area: Rect, errors: &[FeedError]) {
  // Calculate popup size (centered, 60% width, 50% height)
  let popup_width = (area.width as f32 * 0.6) as u16;
  let popup_height = (area.height as f32 * 0.5) as u16;
  let popup_x = (area.width - popup_width) / 2;
  let popup_y = (area.height - popup_height) / 2;

  let popup_area = Rect {
    x: area.x + popup_x,
    y: area.y + popup_y,
    width: popup_width,
    height: popup_height,
  };

  // Clear the area behind the popup
  frame.render_widget(Clear, popup_area);

  // Create the popup block
  let title = format!(" Feed Errors ({}) ", errors.len());
  let popup_block = Block::default()
    .title(Title::from(title.red().bold()))
    .title_bottom(Line::from(" Press <e> or <Enter> to close ".cyan()))
    .borders(Borders::ALL)
    .border_style(Style::default().red())
    .border_set(border::THICK);

  let inner_area = popup_block.inner(popup_area);
  frame.render_widget(popup_block, popup_area);

  // Build error list
  let error_items: Vec<Line> = errors
    .iter()
    .flat_map(|error| {
      vec![
        Line::from(vec!["• ".red(), error.name.clone().yellow().bold()]),
        Line::from(vec!["  ".into(), error.error.clone().white()]),
        Line::from(""), // Empty line between errors
      ]
    })
    .collect();

  let error_list = Paragraph::new(error_items)
    .wrap(Wrap { trim: true })
    .style(Style::default());

  frame.render_widget(error_list, inner_area);
}

use crate::app::{AppState, DisplayFeed, FeedError, LoadingState};
use crate::feeds::Feed;
use ratatui::{
  prelude::*,
  symbols::border,
  widgets::{
    block::{Position, Title},
    Block, Borders, Cell, Clear, Padding, Paragraph, Row, StatefulWidget, Table, TableState, Wrap,
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
  let title = format!("{}", feed.title(raw_feeds));

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
fn entry_row(entry: &crate::feeds::FeedEntry, is_query: bool) -> Row<'static> {
  let date = format_entry_date(entry.published.as_deref());

  let date_cell = Cell::from(Text::from(format!(" {} ", date)).alignment(Alignment::Right))
    .style(Style::default().fg(Color::Cyan));

  let title_style = if entry.read {
    Style::default().fg(Color::DarkGray)
  } else {
    Style::default().bold()
  };

  if is_query {
    let source = entry.feed_title.clone().unwrap_or_default();
    Row::new(vec![
      date_cell,
      Cell::from(Text::from(source)).style(Style::default().fg(Color::Yellow)),
      Cell::from(entry.title.clone()).style(title_style),
    ])
  } else {
    Row::new(vec![
      date_cell,
      Cell::from(String::new()),
      Cell::from(entry.title.clone()).style(title_style),
    ])
  }
}

// ─── Public render entry-point ────────────────────────────────────────────────

pub fn render(
  frame: &mut Frame,
  area: Rect,
  raw_feeds: &[Feed],
  display_feeds: &[DisplayFeed],
  feed_state: &mut TableState,
  entry_state: &mut TableState,
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
      raw_feeds,
      display_feeds,
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
      raw_feeds,
      display_feeds,
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
  if loading_state.should_show_popup() {
    render_loading_popup(frame, area, loading_state, current_feed, display_feeds);
  }
}

// ─── Dual-pane ────────────────────────────────────────────────────────────────

fn render_dual_pane(
  frame: &mut Frame,
  area: Rect,
  raw_feeds: &[Feed],
  display_feeds: &[DisplayFeed],
  feed_state: &mut TableState,
  entry_state: &mut TableState,
  app_state: AppState,
  show_borders: bool,
  loading_state: &LoadingState,
) {
  let chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
    .split(area);

  // ── Feeds table ──────────────────────────────────────────────────────────
  let feed_highlight = match app_state {
    AppState::BrowsingFeeds => Style::default().bg(Color::Yellow).fg(Color::Black),
    _ => Style::default().yellow(),
  };

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
    .highlight_style(feed_highlight);

  StatefulWidget::render(feeds_table, chunks[0], frame.buffer_mut(), feed_state);

  // ── Entries table ────────────────────────────────────────────────────────
  let selected_feed_idx = feed_state.selected().unwrap_or(0);

  let entry_highlight = match app_state {
    AppState::BrowsingEntries => Style::default().bg(Color::Yellow).fg(Color::Black).bold(),
    _ => Style::default(),
  };

  let (entry_rows, is_query, source_width) =
    build_entry_rows(display_feeds, raw_feeds, selected_feed_idx);
  let entry_widths = entry_column_widths(is_query, source_width);
  let entry_count = entry_rows.len();

  let entries_table = Table::new(entry_rows, entry_widths)
    .block(create_entry_block(entry_count, show_borders))
    .column_spacing(2)
    .highlight_style(entry_highlight);

  StatefulWidget::render(entries_table, chunks[1], frame.buffer_mut(), entry_state);
}

// ─── Single-pane ──────────────────────────────────────────────────────────────

fn render_single_pane(
  frame: &mut Frame,
  area: Rect,
  raw_feeds: &[Feed],
  display_feeds: &[DisplayFeed],
  feed_state: &mut TableState,
  entry_state: &mut TableState,
  app_state: AppState,
  show_borders: bool,
  loading_state: &LoadingState,
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
        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black));

      StatefulWidget::render(feeds_table, area, frame.buffer_mut(), feed_state);
    }

    AppState::BrowsingEntries | AppState::ViewingEntry => {
      let selected_feed_idx = feed_state.selected().unwrap_or(0);
      let (entry_rows, is_query, source_width) =
        build_entry_rows(display_feeds, raw_feeds, selected_feed_idx);
      let entry_widths = entry_column_widths(is_query, source_width);
      let entry_count = entry_rows.len();

      let entries_table = Table::new(entry_rows, entry_widths)
        .block(create_entry_block(entry_count, show_borders))
        .column_spacing(2)
        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black).bold());

      StatefulWidget::render(entries_table, area, frame.buffer_mut(), entry_state);
    }
  }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Width of the date column in the entries table.
const DATE_COL_WIDTH: u16 = 8;

/// Build entry rows for the selected feed, returning (rows, is_query, max_source_col_width).
fn build_entry_rows(
  display_feeds: &[DisplayFeed],
  raw_feeds: &[Feed],
  selected_feed_idx: usize,
) -> (Vec<Row<'static>>, bool, u16) {
  if let Some(feed) = display_feeds.get(selected_feed_idx) {
    let entries = feed.entries(raw_feeds);

    if entries.is_empty() {
      return (
        vec![Row::new(vec![
          Cell::from(""),
          Cell::from(""),
          Cell::from(" No entries"),
        ])],
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
    vec![
      Constraint::Length(DATE_COL_WIDTH),
      Constraint::Length(0),
      Constraint::Fill(1),
    ]
  }
}

// ─── Blocks ───────────────────────────────────────────────────────────────────

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
      .padding(Padding::horizontal(1))
  } else {
    Block::default()
      .title(title)
      .title(count_title)
      .padding(Padding::horizontal(1))
  }
}

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
      .padding(Padding::horizontal(1))
  } else {
    Block::default()
      .title(title)
      .title(count_title)
      .padding(Padding::horizontal(1))
  }
}

// ─── Popups ───────────────────────────────────────────────────────────────────

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

fn render_loading_popup(
  frame: &mut Frame,
  area: Rect,
  loading_state: &LoadingState,
  current_feed: Option<&str>,
  display_feeds: &[DisplayFeed],
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
    format!(" ✓ {} feeds loaded ", display_feeds.len())
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

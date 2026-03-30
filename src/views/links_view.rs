use ratatui::{
  prelude::*,
  symbols::border,
  widgets::{
    Block, Borders, Cell, Clear, Row, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Table, TableState, Widget,
  },
};

/// Build table rows for the links popup.
fn build_links_rows(links: &[String]) -> Vec<Row<'_>> {
  links
    .iter()
    .enumerate()
    .map(|(i, link)| {
      Row::new(vec![
        Cell::from(format!(" {}:", i + 1)).yellow(),
        Cell::from(link.as_str()).blue(),
      ])
    })
    .collect()
}

/// Render a centered, scrollable links popup with a two-column table.
pub fn render_links_popup(
  frame: &mut Frame,
  area: Rect,
  links: &[String],
  selected: &mut usize,
  scroll: &mut usize,
  show_scrollbar: bool,
) {
  let popup_width = area.width.saturating_sub(8).min(80);
  let popup_height = area.height.saturating_sub(6).min(30);

  let popup_area = Rect {
    x: area.x + (area.width.saturating_sub(popup_width)) / 2,
    y: area.y + (area.height.saturating_sub(popup_height)) / 2,
    width: popup_width,
    height: popup_height,
  };

  Clear.render(popup_area, frame.buffer_mut());

  // Clamp selected index
  if !links.is_empty() {
    let max_selected = links.len().saturating_sub(1);
    *selected = (*selected).min(max_selected);
  } else {
    *selected = 0;
  }

  let block = Block::default()
    .title(" Links ".bold().yellow())
    .title_bottom(" <Esc/L> close  <o> open ".gray())
    .borders(Borders::ALL)
    .border_style(Style::new().blue())
    .border_set(border::PLAIN);

  if links.is_empty() {
    let empty_rows: Vec<Row> = vec![];
    let empty = Table::new(empty_rows, [Constraint::Min(1)]).block(block);
    StatefulWidget::render(
      empty,
      popup_area,
      frame.buffer_mut(),
      &mut TableState::default(),
    );

    // Render "No links available" centered in the inner area
    let inner_area = Block::default().borders(Borders::ALL).inner(popup_area);
    let msg = ratatui::widgets::Paragraph::new("No links available")
      .alignment(Alignment::Center)
      .style(Style::default().dim());
    msg.render(inner_area, frame.buffer_mut());
    return;
  }

  let rows = build_links_rows(links);
  let row_count = rows.len();

  // Determine index column width based on the largest index label " N:"
  let idx_width = format!("{}", links.len().saturating_sub(1)).len() as u16 + 2;

  let widths = [Constraint::Length(idx_width), Constraint::Min(10)];

  let table = Table::new(rows, widths)
    .block(block)
    .column_spacing(1)
    .row_highlight_style(Style::new().bold().yellow().on_dark_gray());

  let mut table_state = TableState::default().with_selected(Some(*selected));

  StatefulWidget::render(table, popup_area, frame.buffer_mut(), &mut table_state);

  // Scrollbar when rows overflow
  let inner_height = Block::default()
    .borders(Borders::ALL)
    .inner(popup_area)
    .height as usize;

  if row_count > inner_height && show_scrollbar {
    let max_scroll = row_count.saturating_sub(inner_height);
    *scroll = (*scroll).min(max_scroll);

    let scrollbar_area = Rect {
      x: popup_area.x + popup_area.width.saturating_sub(1),
      y: popup_area.y + 1,
      width: 1,
      height: popup_area.height.saturating_sub(2),
    };

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
      .begin_symbol(Some("▲"))
      .end_symbol(Some("▼"));

    let mut scrollbar_state = ScrollbarState::new(max_scroll + 1).position(*selected);
    scrollbar.render(scrollbar_area, frame.buffer_mut(), &mut scrollbar_state);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_build_links_rows_empty() {
    let links: Vec<String> = vec![];
    let rows = build_links_rows(&links);
    assert!(rows.is_empty());
  }

  #[test]
  fn test_build_links_rows_with_links() {
    let links = vec![
      "https://example.com/main".to_string(),
      "https://example.com/ref1".to_string(),
      "https://example.com/ref2".to_string(),
    ];
    let rows = build_links_rows(&links);
    assert_eq!(rows.len(), 3);
  }

  #[test]
  fn test_build_links_rows_single_link() {
    let links = vec!["https://example.com".to_string()];
    let rows = build_links_rows(&links);
    assert_eq!(rows.len(), 1);
  }
}

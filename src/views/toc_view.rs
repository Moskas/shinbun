use crate::content::markdown::HeadingInfo;
use crate::theme::Theme;
use ratatui::{
  prelude::*,
  symbols::border,
  widgets::{
    Block, Borders, Cell, Clear, Row, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Table, TableState, Widget,
  },
};

fn heading_color(theme: &Theme, level: u8) -> Color {
  match level {
    1 => theme.h1,
    2 => theme.h2,
    3 => theme.h3,
    4 => theme.h4,
    _ => theme.h5,
  }
}

/// Nesting depth of each heading relative to its *document* position, not
/// its raw Markdown level — a flat run of same-level headings all sit at
/// depth 0, while a heading nested under a shallower one gets indented one
/// level deeper than its nearest preceding shallower heading.
fn compute_depths(levels: &[u8]) -> Vec<usize> {
  let mut depths = Vec::with_capacity(levels.len());
  let mut stack: Vec<u8> = Vec::new();
  for &level in levels {
    while matches!(stack.last(), Some(&top) if top >= level) {
      stack.pop();
    }
    depths.push(stack.len());
    stack.push(level);
  }
  depths
}

/// Whether each node is the last child among its siblings at its depth, by
/// scanning forward to the next node at the same or a shallower depth.
fn compute_is_last(depths: &[usize]) -> Vec<bool> {
  let n = depths.len();
  let mut is_last = vec![true; n];
  for i in 0..n {
    for &d in &depths[i + 1..] {
      if d < depths[i] {
        break;
      }
      if d == depths[i] {
        is_last[i] = false;
        break;
      }
    }
  }
  is_last
}

/// Build ASCII tree-branch prefixes ("├─ ", "└─ ", "│  " guides) for each
/// heading, from its depth and last-sibling status.
fn build_tree_prefixes(depths: &[usize], is_last: &[bool]) -> Vec<String> {
  let mut ancestor_last: Vec<bool> = Vec::new();
  let mut prefixes = Vec::with_capacity(depths.len());
  for (&depth, &last) in depths.iter().zip(is_last.iter()) {
    ancestor_last.truncate(depth);
    let mut prefix = String::new();
    // Skip the top-level ancestor: root siblings draw no guide line between
    // them, so only ancestors at depth >= 1 contribute a guide column.
    for &al in ancestor_last.iter().skip(1) {
      prefix.push_str(if al { "   " } else { "│  " });
    }
    if depth > 0 {
      prefix.push_str(if last { "└─ " } else { "├─ " });
    }
    prefixes.push(prefix);
    ancestor_last.push(last);
  }
  prefixes
}

/// Build table rows for the table-of-contents popup: a tree view indented by
/// nesting depth (not raw heading level), with the currently-viewed section
/// marked.
fn build_toc_rows<'a>(
  headings: &'a [HeadingInfo],
  current: Option<usize>,
  theme: &Theme,
) -> Vec<Row<'a>> {
  let levels: Vec<u8> = headings.iter().map(|h| h.level).collect();
  let depths = compute_depths(&levels);
  let is_last = compute_is_last(&depths);
  let prefixes = build_tree_prefixes(&depths, &is_last);

  headings
    .iter()
    .zip(prefixes)
    .enumerate()
    .map(|(i, (h, prefix))| {
      let is_current = Some(i) == current;
      let marker = if is_current { "●" } else { " " };
      let mut style = Style::default().fg(heading_color(theme, h.level));
      if is_current {
        style = style.add_modifier(Modifier::BOLD);
      }
      Row::new(vec![
        Cell::from(marker).style(Style::default().fg(theme.link_index)),
        Cell::from(format!("{}{}", prefix, h.text)).style(style),
      ])
    })
    .collect()
}

/// Render a centered, scrollable table-of-contents popup listing an entry's
/// headings as a tree; `Enter` jumps the entry scroll to the selected one.
#[allow(clippy::too_many_arguments)]
pub fn render_toc_popup(
  frame: &mut Frame,
  area: Rect,
  headings: &[HeadingInfo],
  current: Option<usize>,
  selected: &mut usize,
  scroll: &mut usize,
  show_scrollbar: bool,
  theme: &Theme,
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
  if !headings.is_empty() {
    let max_selected = headings.len().saturating_sub(1);
    *selected = (*selected).min(max_selected);
  } else {
    *selected = 0;
  }

  let block = Block::default()
    .title(Span::styled(" Table of Contents ", theme.title_style()))
    .title_bottom(Span::styled(
      " <Esc/t> close  <Enter> jump ",
      Style::default().fg(Color::Gray),
    ))
    .borders(Borders::ALL)
    .border_style(Style::new().fg(theme.links_border))
    .border_set(border::PLAIN);

  if headings.is_empty() {
    let empty_rows: Vec<Row> = vec![];
    let empty = Table::new(empty_rows, [Constraint::Min(1)]).block(block);
    StatefulWidget::render(
      empty,
      popup_area,
      frame.buffer_mut(),
      &mut TableState::default(),
    );

    let inner_area = Block::default().borders(Borders::ALL).inner(popup_area);
    let msg = ratatui::widgets::Paragraph::new("No headings available")
      .alignment(Alignment::Center)
      .style(Style::default().dim());
    msg.render(inner_area, frame.buffer_mut());
    return;
  }

  let rows = build_toc_rows(headings, current, theme);
  let row_count = rows.len();

  let widths = [Constraint::Length(1), Constraint::Min(10)];

  let table = Table::new(rows, widths)
    .block(block)
    .column_spacing(1)
    .row_highlight_style(theme.row_highlight_bold_style());

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

  fn heading(level: u8, text: &str) -> HeadingInfo {
    HeadingInfo {
      level,
      text: text.to_string(),
      block_index: 0,
      line_offset: 0,
    }
  }

  #[test]
  fn test_build_toc_rows_empty() {
    let headings: Vec<HeadingInfo> = vec![];
    let theme = Theme::default();
    let rows = build_toc_rows(&headings, None, &theme);
    assert!(rows.is_empty());
  }

  #[test]
  fn test_build_toc_rows_with_headings() {
    let headings = vec![
      heading(1, "Title"),
      heading(2, "Section One"),
      heading(3, "Sub Section"),
    ];
    let theme = Theme::default();
    let rows = build_toc_rows(&headings, None, &theme);
    assert_eq!(rows.len(), 3);
  }

  #[test]
  fn test_heading_color_falls_back_for_h6() {
    let theme = Theme::default();
    assert_eq!(heading_color(&theme, 6), theme.h5);
  }

  #[test]
  fn flat_same_level_headings_get_no_indent() {
    let levels = vec![3, 3, 3, 3, 3];
    let depths = compute_depths(&levels);
    assert_eq!(depths, vec![0, 0, 0, 0, 0]);
  }

  #[test]
  fn nested_headings_get_relative_depth() {
    // h1, h2, h2, h3 (under 2nd h2), h2
    let levels = vec![1, 2, 2, 3, 2];
    let depths = compute_depths(&levels);
    assert_eq!(depths, vec![0, 1, 1, 2, 1]);
  }

  #[test]
  fn last_sibling_detection() {
    let levels = vec![1, 2, 2, 3, 2];
    let depths = compute_depths(&levels);
    let is_last = compute_is_last(&depths);
    // Root h1 is the only depth-0 node: last.
    // First h2 has a later depth-1 sibling (the final h2): not last.
    // h3 is the only depth-2 node: last.
    // Second h2 (with the h3 child) is followed by another depth-1 sibling: not last.
    // Final h2 has no later sibling: last.
    assert_eq!(is_last, vec![true, false, false, true, true]);
  }

  #[test]
  fn flat_headings_get_no_tree_prefix() {
    // Mirrors the real-world case of five consecutive H3 sections with no
    // shared parent heading — all should render flush-left, no branches.
    let levels = vec![3, 3, 3, 3, 3];
    let depths = compute_depths(&levels);
    let is_last = compute_is_last(&depths);
    let prefixes = build_tree_prefixes(&depths, &is_last);
    assert!(prefixes.iter().all(|p| p.is_empty()));
  }

  #[test]
  fn tree_prefixes_render_branch_characters() {
    let levels = vec![1, 2, 2];
    let depths = compute_depths(&levels);
    let is_last = compute_is_last(&depths);
    let prefixes = build_tree_prefixes(&depths, &is_last);
    assert_eq!(prefixes[0], "");
    assert_eq!(prefixes[1], "├─ ");
    assert_eq!(prefixes[2], "└─ ");
  }
}

use crate::feeds::{Feed, FeedEntry};
use ratatui::{
  prelude::*,
  symbols::border,
  widgets::{
    block::{Position, Title},
    Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
  },
};

/// Calculate the wrapped height of text lines given a content width
fn calculate_wrapped_height(lines: &[Line], content_width: u16) -> usize {
  let width = content_width.max(1) as usize;
  lines
    .iter()
    .map(|line| {
      let raw_width = line.width();
      if raw_width == 0 {
        1
      } else {
        (raw_width + width - 1) / width // ceil division
      }
    })
    .sum()
}

/// Build the content lines for an entry view
fn build_entry_content(feed: &Feed, entry: &FeedEntry) -> Vec<Line<'static>> {
  let mut lines = Vec::new();

  // Metadata
  lines.push(Line::from(format!("Title: {}", entry.title)).magenta());
  lines.push(Line::from(format!("Feed: {}", feed.title)).cyan());
  lines.push(
    Line::from(format!(
      "Published: {}",
      entry.published.as_deref().unwrap_or("Unknown")
    ))
    .yellow(),
  );

  if !entry.links.is_empty() {
    lines.push(Line::from(format!("Link: {}", entry.links.join(", "))).blue());
  }

  if !entry.media.is_empty() {
    lines.push(Line::from(format!("Media: {}", entry.media)).blue());
  }

  lines.push(Line::from("")); // separator

  // Body content
  for line in entry.text.lines() {
    lines.push(Line::from(line.to_owned()));
  }

  lines
}

/// Render the entry view with scrolling support
pub fn render(
  frame: &mut Frame,
  area: Rect,
  feed: &Feed,
  entry: &FeedEntry,
  scroll: &mut usize,
  show_borders: bool,
) {
  // Create the outer container
  let title = Title::from(" Shinbun ".bold().yellow());
  let instructions = Title::from(Line::from(vec![" Back ".into(), "<h> ".bold()]));

  let outer_block = if show_borders {
    Block::default()
      .title(title.alignment(Alignment::Left))
      .title(
        instructions
          .alignment(Alignment::Left)
          .position(Position::Bottom),
      )
      .title_bottom(Line::from(" Quit <q> ").blue().right_aligned())
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
      .title_bottom(Line::from(" Quit <q> ").blue().right_aligned())
  };

  let inner_area = outer_block.inner(area);

  // Build the entry content
  let content = build_entry_content(feed, entry);

  // Create the entry block with padding
  let entry_block = if show_borders {
    Block::default()
      .title(" Entry ".yellow())
      .borders(Borders::ALL)
      .border_style(Style::new().blue())
      .padding(Padding::symmetric(4, 1))
  } else {
    Block::default()
      .title(" Entry ".yellow())
      .padding(Padding::symmetric(4, 1))
  };

  // Calculate text area dimensions (now using full width)
  let text_area = entry_block.inner(inner_area);
  let paragraph_width = text_area.width; // Use full width - no space reserved
  let visible_height = text_area.height as usize;

  // Calculate scrolling metrics
  let content_length = calculate_wrapped_height(&content, paragraph_width);
  let max_scroll = content_length.saturating_sub(visible_height);

  // Clamp scroll position
  *scroll = (*scroll).min(max_scroll);

  // Calculate visible line range for display
  let (first_visible, last_visible) = if content_length == 0 || visible_height == 0 {
    (0, 0)
  } else {
    let first = *scroll;
    let last = (*scroll + visible_height.saturating_sub(1)).min(content_length.saturating_sub(1));
    (first + 1, last + 1) // 1-indexed for display
  };

  let line_info = format!(
    " Lines: {}–{} / {} ",
    first_visible, last_visible, content_length
  );

  // Create paragraph with scrolling
  let paragraph = Paragraph::new(content)
    .block(entry_block.clone().title_bottom(line_info.yellow()))
    .scroll((*scroll as u16, 0))
    .wrap(Wrap { trim: true });

  // Render the outer block
  outer_block.render(area, frame.buffer_mut());

  // Render the paragraph
  paragraph.render(inner_area, frame.buffer_mut());

  // Render scrollbar on the border if content overflows
  if content_length > visible_height {
    // Position scrollbar on the right border edge, inset to avoid corner collision
    let scrollbar_area = Rect {
      x: inner_area.x + inner_area.width.saturating_sub(1),
      y: inner_area.y + 1, // Start below top border
      width: 1,
      height: inner_area.height.saturating_sub(2), // End above bottom border
    };

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
      .begin_symbol(Some("▲"))
      .end_symbol(Some("▼"));

    let mut scrollbar_state = ScrollbarState::new(max_scroll + 1).position(*scroll);
    scrollbar.render(scrollbar_area, frame.buffer_mut(), &mut scrollbar_state);
  }
}

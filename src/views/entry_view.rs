use crate::feeds::FeedEntry;
use crate::theme::Theme;
use ratatui::{
  Frame,
  layout::Rect,
  prelude::{Alignment, Color, Line, Modifier, Span, Style, Stylize},
  symbols::border,
  widgets::{
    Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Widget, Wrap,
  },
};
use tui_markdown::{self, Options, StyleSheet};

/// Configuration passed to [`render`] to avoid too many individual parameters.
pub struct EntryViewConfig<'a> {
  pub show_borders: bool,
  pub show_scrollbar: bool,
  pub theme: &'a Theme,
}

/// A theme-aware stylesheet for tui-markdown rendering.
#[derive(Clone, Copy, Debug)]
pub struct ShinbunStyleSheet {
  h1: Color,
  h2: Color,
  h3: Color,
  h4: Color,
  h5: Color,
  code: Option<Color>,
  link: Color,
  metadata_block: Color,
}

impl ShinbunStyleSheet {
  pub fn from_theme(theme: &Theme) -> Self {
    Self {
      h1: theme.h1,
      h2: theme.h2,
      h3: theme.h3,
      h4: theme.h4,
      h5: theme.h5,
      code: theme.code,
      link: theme.link,
      metadata_block: theme.metadata_block,
    }
  }
}

impl Default for ShinbunStyleSheet {
  fn default() -> Self {
    Self::from_theme(&Theme::default())
  }
}

impl StyleSheet for ShinbunStyleSheet {
  fn heading(&self, level: u8) -> Style {
    match level {
      1 => Style::new()
        .fg(self.h1)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
      2 => Style::new().fg(self.h2).add_modifier(Modifier::BOLD),
      3 => Style::new().fg(self.h3).add_modifier(Modifier::BOLD),
      4 => Style::new().fg(self.h4),
      5 => Style::new().fg(self.h5),
      _ => Style::new(),
    }
  }

  fn code(&self) -> Style {
    let s = Style::new().add_modifier(Modifier::BOLD);
    match self.code {
      Some(c) => s.fg(c),
      None => s,
    }
  }

  fn link(&self) -> Style {
    Style::new()
      .fg(self.link)
      .add_modifier(Modifier::UNDERLINED)
  }

  fn blockquote(&self) -> Style {
    Style::new().dim()
  }

  fn heading_meta(&self) -> Style {
    Style::new().dim()
  }

  fn metadata_block(&self) -> Style {
    Style::new().fg(self.metadata_block)
  }
}

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
        raw_width.div_ceil(width)
      }
    })
    .sum::<usize>()
    + 2
}

/// Build the content lines for an entry view
fn build_entry_content<'a>(
  feed_title: &'a str,
  entry: &'a FeedEntry,
  theme: &Theme,
) -> Vec<Line<'a>> {
  let mut lines = Vec::new();

  // Metadata
  lines.push(Line::from(format!("Title: {}", entry.title)).fg(theme.meta_title));
  lines.push(Line::from(format!("Feed: {}", feed_title)).fg(theme.meta_feed));
  lines.push(
    Line::from(format!(
      "Published: {}",
      entry.published.as_deref().unwrap_or("Unknown")
    ))
    .fg(theme.meta_published),
  );

  if !entry.links.is_empty() {
    lines.push(Line::from(format!("Link: {}", entry.links[0])).fg(theme.meta_link));
  }

  if let Some(ref url) = entry.media {
    lines.push(Line::from(format!("Media: {}", url)).fg(theme.meta_link));
  }

  lines.push(Line::from("")); // separator

  let stylesheet = ShinbunStyleSheet::from_theme(theme);
  let md = tui_markdown::from_str_with_options(&entry.text, &Options::new(stylesheet));
  lines.extend(md.lines);

  if entry.links.len() > 1 {
    lines.push(Line::from("")); // separator
    lines.push(Line::from("Links:").bold()); // separator
    lines.extend(
      entry
        .links
        .iter()
        .skip(1)
        .enumerate()
        .map(|(i, link)| Line::from(format!("[{}]: {}", i + 1, link)).fg(theme.meta_link)),
    );
  }

  lines
}

/// Render the entry view with scrolling support
pub fn render(
  frame: &mut Frame,
  area: Rect,
  feed_title: &str,
  entry: &FeedEntry,
  scroll: &mut usize,
  cfg: &EntryViewConfig,
) {
  let theme = cfg.theme;
  let show_borders = cfg.show_borders;
  let show_scrollbar = cfg.show_scrollbar;
  // Create the outer container
  let title = Span::styled(
    format!(" Shinbun - Articles in feed '{}' ", feed_title),
    theme.title_style(),
  );
  let instructions = Line::from(vec![" Help ".into(), "<?> ".bold()]);

  let outer_block = if show_borders {
    Block::default()
      .title(title)
      .title_bottom(instructions.alignment(Alignment::Left))
      .borders(Borders::ALL)
      .border_style(theme.border_style())
      .border_set(border::PLAIN)
  } else {
    Block::default()
      .title(title)
      .title_bottom(instructions.alignment(Alignment::Left))
  };

  let inner_area = outer_block.inner(area);

  // Build the entry content
  let content = build_entry_content(feed_title, entry, theme);

  // Create the entry block with padding
  let entry_block = if show_borders {
    Block::default()
      .title(Span::styled(
        format!(" Entry  - {} ", entry.title),
        Style::default().fg(theme.entry_title_bordered),
      ))
      .borders(Borders::ALL)
      .border_style(theme.border_style())
      .padding(Padding::symmetric(4, 1))
  } else {
    Block::default()
      .title(Span::styled(
        format!(" Entry - {}", entry.title),
        Style::default().fg(theme.entry_title_plain),
      ))
      .padding(Padding::symmetric(4, 0))
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
    .block(entry_block.clone().title_bottom(Span::styled(
      line_info,
      Style::default().fg(theme.line_info),
    )))
    .scroll((*scroll as u16, 0))
    .wrap(Wrap { trim: false });

  // Render the outer block
  outer_block.render(area, frame.buffer_mut());

  // Render the paragraph
  paragraph.render(inner_area, frame.buffer_mut());

  // Render scrollbar on the border if content overflows
  if content_length > visible_height && show_scrollbar {
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

#[cfg(test)]
mod tests {
  use super::*;

  fn test_theme() -> Theme {
    Theme::default()
  }

  #[test]
  fn test_calculate_wrapped_height_single_line() {
    let lines = vec![Line::from("Hello world")];
    // "Hello world" is 11 chars, width 80 → 1 line + 2 padding = 3
    let height = calculate_wrapped_height(&lines, 80);
    assert_eq!(height, 3);
  }

  #[test]
  fn test_calculate_wrapped_height_wrapping() {
    // 20 char line in 10-wide viewport → ceil(20/10) = 2 lines
    let lines = vec![Line::from("12345678901234567890")];
    let height = calculate_wrapped_height(&lines, 10);
    assert_eq!(height, 2 + 2); // 2 wrapped lines + 2 padding
  }

  #[test]
  fn test_calculate_wrapped_height_empty_line() {
    let lines = vec![Line::from("")];
    let height = calculate_wrapped_height(&lines, 80);
    assert_eq!(height, 1 + 2); // empty line counts as 1 + 2 padding
  }

  #[test]
  fn test_calculate_wrapped_height_zero_width() {
    // Width 0 should be clamped to 1
    let lines = vec![Line::from("Hello")];
    let height = calculate_wrapped_height(&lines, 0);
    // "Hello" is 5 chars, width clamped to 1 → ceil(5/1) = 5
    assert_eq!(height, 5 + 2);
  }

  #[test]
  fn test_calculate_wrapped_height_multiple_lines() {
    let lines = vec![
      Line::from("Short"),           // 5 chars, width 10 → 1 line
      Line::from(""),                // empty → 1 line
      Line::from("1234567890abcde"), // 15 chars, width 10 → 2 lines
    ];
    let height = calculate_wrapped_height(&lines, 10);
    assert_eq!(height, 1 + 1 + 2 + 2); // 4 lines + 2 padding
  }

  #[test]
  fn test_calculate_wrapped_height_exact_fit() {
    // Exactly 10 chars in 10-wide viewport → 1 line
    let lines = vec![Line::from("1234567890")];
    let height = calculate_wrapped_height(&lines, 10);
    assert_eq!(height, 1 + 2);
  }

  #[test]
  fn test_build_entry_content_basic() {
    let entry = FeedEntry {
      title: "Test Entry".to_string(),
      published: Some("2024-01-15".to_string()),
      text: "Entry body text".to_string(),
      links: vec!["https://example.com/post".to_string()],
      media: None,
      feed_title: None,
      read: false,
    };

    let lines = build_entry_content("My Feed", &entry, &test_theme());
    // Should contain metadata lines
    let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    assert!(text.iter().any(|l| l.contains("Test Entry")));
    assert!(text.iter().any(|l| l.contains("My Feed")));
    assert!(text.iter().any(|l| l.contains("2024-01-15")));
    assert!(text.iter().any(|l| l.contains("https://example.com/post")));
  }

  #[test]
  fn test_build_entry_content_unknown_published() {
    let entry = FeedEntry {
      title: "No Date".to_string(),
      published: None,
      text: "Content".to_string(),
      links: vec![],
      media: None,
      feed_title: None,
      read: false,
    };

    let lines = build_entry_content("Feed", &entry, &test_theme());
    let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    assert!(text.iter().any(|l| l.contains("Unknown")));
  }

  #[test]
  fn test_build_entry_content_with_media() {
    let entry = FeedEntry {
      title: "Podcast".to_string(),
      published: None,
      text: "Episode notes".to_string(),
      links: vec![],
      media: Some("https://example.com/episode.mp3".to_string()),
      feed_title: None,
      read: false,
    };

    let lines = build_entry_content("Podcast Feed", &entry, &test_theme());
    let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    assert!(text.iter().any(|l| l.contains("episode.mp3")));
  }

  #[test]
  fn test_build_entry_content_multiple_links() {
    let entry = FeedEntry {
      title: "Multi Link".to_string(),
      published: None,
      text: "Content".to_string(),
      links: vec![
        "https://example.com/main".to_string(),
        "https://example.com/ref1".to_string(),
        "https://example.com/ref2".to_string(),
      ],
      media: None,
      feed_title: None,
      read: false,
    };

    let lines = build_entry_content("Feed", &entry, &test_theme());
    let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    // First link shown in header
    assert!(text.iter().any(|l| l.contains("https://example.com/main")));
    // Additional links in footer section
    assert!(text.iter().any(|l| l.contains("Links:")));
    assert!(text.iter().any(|l| l.contains("[1]")));
    assert!(text.iter().any(|l| l.contains("ref1")));
    assert!(text.iter().any(|l| l.contains("[2]")));
    assert!(text.iter().any(|l| l.contains("ref2")));
  }
}

use crate::feeds::FeedEntry;
use crate::theme::Theme;
use ratatui::{
  layout::Rect,
  prelude::{Alignment, Color, Line, Modifier, Span, Style, Stylize},
  symbols::border,
  widgets::{
    Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Widget, Wrap,
  },
  Frame,
};
use ratatui_image::{protocol::StatefulProtocol, StatefulImage};
use std::collections::HashMap;
use tui_markdown::{self, Options, StyleSheet};

/// Fixed cell-row height reserved for each image segment.
const IMAGE_RENDER_ROWS: usize = 15;

/// A piece of entry content: either markdown text or an inline image.
#[derive(Debug)]
enum ContentSegment {
  /// Raw Markdown string; rendered lazily by tui-markdown each frame.
  Text(String),
  /// Pre-built styled lines (metadata header, footer links, etc.).
  PreStyled(Vec<Line<'static>>),
  /// Inline image: fetched asynchronously and stored in the App's image_cache.
  Image { src: String, alt: String },
}

/// Configuration passed to [`render`] to avoid too many individual parameters.
pub struct EntryViewConfig<'a> {
  pub show_borders: bool,
  pub show_scrollbar: bool,
  pub show_images: bool,
  pub theme: &'a Theme,
  /// Cache of decoded images keyed by URL; mutated as images are rendered.
  pub image_cache: &'a mut HashMap<String, StatefulProtocol>,
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

/// Calculate the wrapped height of text lines given a content width.
/// Adds 2 to provide natural spacing between segments.
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
}

/// Split a Markdown string into alternating text and image segments.
/// Images are detected by the `![alt](url)` syntax that `htmd` emits.
fn split_into_segments(md: &str) -> Vec<ContentSegment> {
  let mut segs = Vec::new();
  let mut text_start = 0;
  let mut i = 0;
  let bytes = md.as_bytes();
  let md_len = md.len();

  while i < md_len {
    if i + 1 < md_len && bytes[i] == b'!' && bytes[i + 1] == b'[' {
      let after_excl = i + 2;
      if let Some(rel_bracket) = md[after_excl..].find(']') {
        let after_bracket = after_excl + rel_bracket;
        if after_bracket + 1 < md_len && &md[after_bracket..after_bracket + 2] == "](" {
          let url_start = after_bracket + 2;
          if let Some(rel_paren) = md[url_start..].find(')') {
            let url_end = url_start + rel_paren;
            let url = &md[url_start..url_end];
            let alt = &md[after_excl..after_bracket];
            let img_end = url_end + 1;

            let text_before = &md[text_start..i];
            if !text_before.trim().is_empty() {
              segs.push(ContentSegment::Text(text_before.to_string()));
            }
            // Only emit image segments for HTTP URLs (relative paths can't be fetched).
            if url.starts_with("http") {
              segs.push(ContentSegment::Image {
                src: url.to_string(),
                alt: alt.to_string(),
              });
            }
            text_start = img_end;
            i = img_end;
            continue;
          }
        }
      }
    }
    i += 1;
  }

  let remaining = &md[text_start..];
  if !remaining.trim().is_empty() {
    segs.push(ContentSegment::Text(remaining.to_string()));
  }
  segs
}

/// Build the full ordered list of content segments for the entry view.
fn build_all_segments(feed_title: &str, entry: &FeedEntry, theme: &Theme) -> Vec<ContentSegment> {
  let mut segs = Vec::new();

  // Metadata header (theme-colored pre-styled lines)
  let mut meta: Vec<Line<'static>> = Vec::new();
  meta.push(Line::from(format!("Title: {}", entry.title)).fg(theme.meta_title));
  meta.push(Line::from(format!("Feed: {}", feed_title)).fg(theme.meta_feed));
  meta.push(
    Line::from(format!(
      "Published: {}",
      entry.published.as_deref().unwrap_or("Unknown")
    ))
    .fg(theme.meta_published),
  );
  if !entry.links.is_empty() {
    meta.push(Line::from(format!("Link: {}", entry.links[0])).fg(theme.meta_link));
  }
  if let Some(ref url) = entry.media {
    meta.push(Line::from(format!("Media: {}", url)).fg(theme.meta_link));
  }
  meta.push(Line::from(""));
  segs.push(ContentSegment::PreStyled(meta));

  // Body: split markdown into text + image segments
  segs.extend(split_into_segments(&entry.text));

  // Footer: additional links beyond the first
  if entry.links.len() > 1 {
    let mut footer: Vec<Line<'static>> = Vec::new();
    footer.push(Line::from(""));
    footer.push(Line::from("Links:").bold());
    footer.extend(
      entry
        .links
        .iter()
        .skip(1)
        .enumerate()
        .map(|(i, link)| Line::from(format!("[{}]: {}", i + 1, link)).fg(theme.meta_link)),
    );
    segs.push(ContentSegment::PreStyled(footer));
  }

  segs
}

/// Return the virtual height of a single segment given the available width.
fn seg_height(
  seg: &ContentSegment,
  content_width: u16,
  stylesheet: ShinbunStyleSheet,
  show_images: bool,
) -> usize {
  match seg {
    ContentSegment::PreStyled(lines) => calculate_wrapped_height(lines, content_width),
    ContentSegment::Text(md) => {
      let rendered = tui_markdown::from_str_with_options(md, &Options::new(stylesheet));
      calculate_wrapped_height(&rendered.lines, content_width)
    }
    // 1 blank + alt text + 1 blank when images are off; full reserved rows otherwise.
    ContentSegment::Image { .. } => {
      if show_images {
        IMAGE_RENDER_ROWS
      } else {
        3
      }
    }
  }
}

/// Render visible segments into `area`, respecting the current scroll offset.
#[allow(clippy::too_many_arguments)]
fn render_segments(
  frame: &mut Frame,
  area: Rect,
  segments: &[ContentSegment],
  heights: &[usize],
  scroll: usize,
  stylesheet: ShinbunStyleSheet,
  theme: &Theme,
  image_cache: &mut HashMap<String, StatefulProtocol>,
  show_images: bool,
) {
  let visible_height = area.height as usize;
  let mut virtual_row = 0usize;

  for (seg, &height) in segments.iter().zip(heights.iter()) {
    let seg_end = virtual_row + height;

    // Entirely above viewport — skip.
    if seg_end <= scroll {
      virtual_row = seg_end;
      continue;
    }

    // Entirely below viewport — stop.
    let screen_y = virtual_row.saturating_sub(scroll);
    if screen_y >= visible_height {
      break;
    }

    let inner_skip = scroll.saturating_sub(virtual_row);
    let avail = visible_height.saturating_sub(screen_y);
    let render_h = height.saturating_sub(inner_skip).min(avail);

    if render_h == 0 {
      virtual_row = seg_end;
      continue;
    }

    let seg_area = Rect {
      x: area.x,
      y: area.y + screen_y as u16,
      width: area.width,
      height: render_h as u16,
    };

    match seg {
      ContentSegment::PreStyled(lines) => {
        Paragraph::new(lines.clone())
          .scroll((inner_skip as u16, 0))
          .wrap(Wrap { trim: false })
          .render(seg_area, frame.buffer_mut());
      }
      ContentSegment::Text(md) => {
        let rendered = tui_markdown::from_str_with_options(md, &Options::new(stylesheet));
        Paragraph::new(rendered)
          .scroll((inner_skip as u16, 0))
          .wrap(Wrap { trim: false })
          .render(seg_area, frame.buffer_mut());
      }
      ContentSegment::Image { src, alt } => {
        let ph = if alt.is_empty() {
          " [image] ".to_string()
        } else {
          format!(" [image: {}] ", alt)
        };
        if show_images {
          if inner_skip == 0 && avail >= IMAGE_RENDER_ROWS {
            // Only render when the image is fully visible. Passing a variable-height
            // area to StatefulImage causes it to re-encode on every scroll step, which
            // is extremely expensive. A fixed IMAGE_RENDER_ROWS area means it encodes
            // once and caches thereafter.
            let full_area = Rect {
              height: IMAGE_RENDER_ROWS as u16,
              ..seg_area
            };
            if let Some(protocol) = image_cache.get_mut(src.as_str()) {
              frame.render_stateful_widget(StatefulImage::default(), full_area, protocol);
            } else {
              Paragraph::new(Line::from(ph).fg(theme.meta_link))
                .render(full_area, frame.buffer_mut());
            }
          } else {
            Paragraph::new(Line::from(ph).fg(theme.meta_link)).render(seg_area, frame.buffer_mut());
          }
        } else {
          // Images disabled: compact 3-row layout (blank / alt text / blank).
          let lines = vec![
            Line::from(""),
            Line::from(ph).fg(theme.meta_link),
            Line::from(""),
          ];
          Paragraph::new(lines)
            .scroll((inner_skip as u16, 0))
            .render(seg_area, frame.buffer_mut());
        }
      }
    }

    virtual_row = seg_end;
  }
}

/// Render the entry view with scrolling support.
pub fn render(
  frame: &mut Frame,
  area: Rect,
  feed_title: &str,
  entry: &FeedEntry,
  scroll: &mut usize,
  cfg: &mut EntryViewConfig,
) {
  let theme = cfg.theme;
  let show_borders = cfg.show_borders;
  let show_scrollbar = cfg.show_scrollbar;

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
      .padding(Padding::new(4, 4, 0, 1))
  };

  let text_area = entry_block.inner(inner_area);
  let content_width = text_area.width;
  let visible_height = text_area.height as usize;

  // Build segments and compute layout heights.
  let stylesheet = ShinbunStyleSheet::from_theme(theme);
  let segments = build_all_segments(feed_title, entry, theme);
  let show_images = cfg.show_images;
  let heights: Vec<usize> = segments
    .iter()
    .map(|seg| seg_height(seg, content_width, stylesheet, show_images))
    .collect();
  let content_length: usize = heights.iter().sum();

  let max_scroll = content_length.saturating_sub(visible_height);
  *scroll = (*scroll).min(max_scroll);
  let cur_scroll = *scroll;

  let (first_visible, last_visible) = if content_length == 0 || visible_height == 0 {
    (0, 0)
  } else {
    let first = cur_scroll;
    let last =
      (cur_scroll + visible_height.saturating_sub(1)).min(content_length.saturating_sub(1));
    (first + 1, last + 1)
  };

  let line_info = format!(
    " Lines: {}–{} / {} ",
    first_visible, last_visible, content_length
  );

  // Render outer and entry blocks.
  outer_block.render(area, frame.buffer_mut());
  entry_block
    .title_bottom(Span::styled(
      line_info,
      Style::default().fg(theme.line_info),
    ))
    .render(inner_area, frame.buffer_mut());

  // Render content segments.
  render_segments(
    frame,
    text_area,
    &segments,
    &heights,
    cur_scroll,
    stylesheet,
    theme,
    cfg.image_cache,
    cfg.show_images,
  );

  // Scrollbar.
  if content_length > visible_height && show_scrollbar {
    let scrollbar_area = Rect {
      x: inner_area.x + inner_area.width.saturating_sub(1),
      y: inner_area.y + 1,
      width: 1,
      height: inner_area.height.saturating_sub(2),
    };

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
      .begin_symbol(Some("▲"))
      .end_symbol(Some("▼"));

    let mut scrollbar_state = ScrollbarState::new(max_scroll + 1).position(cur_scroll);
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
    // "Hello world" is 11 chars, width 80 → 1 line
    let height = calculate_wrapped_height(&lines, 80);
    assert_eq!(height, 1);
  }

  #[test]
  fn test_calculate_wrapped_height_wrapping() {
    // 20 char line in 10-wide viewport → ceil(20/10) = 2 lines
    let lines = vec![Line::from("12345678901234567890")];
    let height = calculate_wrapped_height(&lines, 10);
    assert_eq!(height, 2);
  }

  #[test]
  fn test_calculate_wrapped_height_empty_line() {
    let lines = vec![Line::from("")];
    let height = calculate_wrapped_height(&lines, 80);
    assert_eq!(height, 1); // empty line counts as 1
  }

  #[test]
  fn test_calculate_wrapped_height_zero_width() {
    // Width 0 should be clamped to 1
    let lines = vec![Line::from("Hello")];
    let height = calculate_wrapped_height(&lines, 0);
    // "Hello" is 5 chars, width clamped to 1 → ceil(5/1) = 5
    assert_eq!(height, 5);
  }

  #[test]
  fn test_calculate_wrapped_height_multiple_lines() {
    let lines = vec![
      Line::from("Short"),           // 5 chars, width 10 → 1 line
      Line::from(""),                // empty → 1 line
      Line::from("1234567890abcde"), // 15 chars, width 10 → 2 lines
    ];
    let height = calculate_wrapped_height(&lines, 10);
    assert_eq!(height, 1 + 1 + 2); // 4 lines
  }

  #[test]
  fn test_calculate_wrapped_height_exact_fit() {
    // Exactly 10 chars in 10-wide viewport → 1 line
    let lines = vec![Line::from("1234567890")];
    let height = calculate_wrapped_height(&lines, 10);
    assert_eq!(height, 1);
  }

  #[test]
  fn test_split_into_segments_no_images() {
    let segs = split_into_segments("Hello world");
    assert_eq!(segs.len(), 1);
    assert!(matches!(&segs[0], ContentSegment::Text(s) if s == "Hello world"));
  }

  #[test]
  fn test_split_into_segments_single_image() {
    let md = "Before\n\n![alt text](https://example.com/img.png)\n\nAfter";
    let segs = split_into_segments(md);
    assert_eq!(segs.len(), 3);
    assert!(matches!(&segs[0], ContentSegment::Text(_)));
    assert!(
      matches!(&segs[1], ContentSegment::Image { src, alt } if src == "https://example.com/img.png" && alt == "alt text")
    );
    assert!(matches!(&segs[2], ContentSegment::Text(_)));
  }

  #[test]
  fn test_split_into_segments_skips_non_http_images() {
    let md = "![local](./relative/path.png) text";
    let segs = split_into_segments(md);
    // Non-HTTP image is skipped; remaining text becomes one text segment
    assert!(segs
      .iter()
      .all(|s| !matches!(s, ContentSegment::Image { .. })));
  }

  #[test]
  fn test_build_all_segments_metadata() {
    use crate::feeds::FeedEntry;
    let entry = FeedEntry {
      title: "Test Entry".to_string(),
      published: Some("2024-01-15".to_string()),
      text: "Entry body text".to_string(),
      links: vec!["https://example.com/post".to_string()],
      media: None,
      feed_title: None,
      feed_url: None,
      read: false,
    };

    let segs = build_all_segments("My Feed", &entry, &test_theme());
    // First segment should be PreStyled metadata
    assert!(matches!(&segs[0], ContentSegment::PreStyled(_)));
    if let ContentSegment::PreStyled(lines) = &segs[0] {
      let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
      assert!(text.iter().any(|l| l.contains("Test Entry")));
      assert!(text.iter().any(|l| l.contains("My Feed")));
      assert!(text.iter().any(|l| l.contains("2024-01-15")));
    }
  }

  #[test]
  fn test_build_all_segments_footer_links() {
    use crate::feeds::FeedEntry;
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
      feed_url: None,
      read: false,
    };

    let segs = build_all_segments("Feed", &entry, &test_theme());
    let last = segs.last().unwrap();
    assert!(matches!(last, ContentSegment::PreStyled(_)));
    if let ContentSegment::PreStyled(lines) = last {
      let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
      assert!(text.iter().any(|l| l.contains("Links:")));
      assert!(text.iter().any(|l| l.contains("ref1")));
      assert!(text.iter().any(|l| l.contains("ref2")));
    }
  }
}

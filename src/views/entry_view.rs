use crate::content::markdown::{render_markdown, MarkdownStyles, MdBlock};
use crate::feeds::FeedEntry;
use crate::theme::Theme;
use ratatui::{
  layout::Rect,
  prelude::{Alignment, Line, Span, Style, Stylize},
  symbols::border,
  widgets::{
    Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Widget, Wrap,
  },
  Frame,
};
use ratatui_image::{protocol::StatefulProtocol, StatefulImage};
use std::collections::HashMap;

/// Fixed cell-row height reserved for each image segment.
const IMAGE_RENDER_ROWS: usize = 15;

/// A piece of entry content laid out for the viewport.
#[derive(Debug)]
enum ContentSegment {
  /// Styled lines soft-wrapped by the renderer (paragraphs, metadata, footer).
  Wrapped(Vec<Line<'static>>),
  /// Pre-formatted lines already fitted to the width (tables) — never wrapped.
  Fixed(Vec<Line<'static>>),
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

/// Calculate the wrapped height of text lines given a content width.
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

/// Build the full ordered list of content segments for the entry view.
fn build_all_segments(
  feed_title: &str,
  entry: &FeedEntry,
  theme: &Theme,
  content_width: u16,
) -> Vec<ContentSegment> {
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
  segs.push(ContentSegment::Wrapped(meta));

  // Body: render markdown into text/table/image blocks fitted to the width.
  let styles = MarkdownStyles::from_theme(theme);
  for block in render_markdown(&entry.text, content_width, &styles) {
    segs.push(match block {
      MdBlock::Text(lines) => ContentSegment::Wrapped(lines),
      MdBlock::Table(lines) => ContentSegment::Fixed(lines),
      MdBlock::Image { src, alt } => ContentSegment::Image { src, alt },
    });
  }

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
    segs.push(ContentSegment::Wrapped(footer));
  }

  segs
}

/// Return the virtual height of a single segment given the available width.
fn seg_height(seg: &ContentSegment, content_width: u16, show_images: bool) -> usize {
  match seg {
    ContentSegment::Wrapped(lines) => calculate_wrapped_height(lines, content_width),
    ContentSegment::Fixed(lines) => lines.len(),
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
      ContentSegment::Wrapped(lines) => {
        Paragraph::new(lines.clone())
          .scroll((inner_skip as u16, 0))
          .wrap(Wrap { trim: false })
          .render(seg_area, frame.buffer_mut());
      }
      ContentSegment::Fixed(lines) => {
        Paragraph::new(lines.clone())
          .scroll((inner_skip as u16, 0))
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
  let segments = build_all_segments(feed_title, entry, theme, content_width);
  let show_images = cfg.show_images;
  let heights: Vec<usize> = segments
    .iter()
    .map(|seg| seg_height(seg, content_width, show_images))
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

  fn make_entry(text: &str, links: Vec<String>) -> FeedEntry {
    FeedEntry {
      title: "Test Entry".to_string(),
      published: Some("2024-01-15".to_string()),
      text: text.to_string(),
      links,
      media: None,
      feed_title: None,
      feed_url: None,
      read: false,
    }
  }

  #[test]
  fn test_build_all_segments_metadata() {
    let entry = make_entry(
      "Entry body text",
      vec!["https://example.com/post".to_string()],
    );

    let segs = build_all_segments("My Feed", &entry, &test_theme(), 80);
    // First segment should be wrapped metadata lines
    assert!(matches!(&segs[0], ContentSegment::Wrapped(_)));
    if let ContentSegment::Wrapped(lines) = &segs[0] {
      let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
      assert!(text.iter().any(|l| l.contains("Test Entry")));
      assert!(text.iter().any(|l| l.contains("My Feed")));
      assert!(text.iter().any(|l| l.contains("2024-01-15")));
    }
  }

  #[test]
  fn test_build_all_segments_image_and_table() {
    let entry = make_entry(
      "Before\n\n![alt text](https://example.com/img.png)\n\n| a | b |\n| --- | --- |\n| 1 | 2 |",
      vec![],
    );

    let segs = build_all_segments("Feed", &entry, &test_theme(), 80);
    assert!(segs.iter().any(
      |s| matches!(s, ContentSegment::Image { src, alt } if src == "https://example.com/img.png" && alt == "alt text")
    ));
    assert!(segs.iter().any(|s| matches!(s, ContentSegment::Fixed(_))));
  }

  #[test]
  fn test_build_all_segments_footer_links() {
    let entry = make_entry(
      "Content",
      vec![
        "https://example.com/main".to_string(),
        "https://example.com/ref1".to_string(),
        "https://example.com/ref2".to_string(),
      ],
    );

    let segs = build_all_segments("Feed", &entry, &test_theme(), 80);
    let last = segs.last().unwrap();
    assert!(matches!(last, ContentSegment::Wrapped(_)));
    if let ContentSegment::Wrapped(lines) = last {
      let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
      assert!(text.iter().any(|l| l.contains("Links:")));
      assert!(text.iter().any(|l| l.contains("ref1")));
      assert!(text.iter().any(|l| l.contains("ref2")));
    }
  }
}

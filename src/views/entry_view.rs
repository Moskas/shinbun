use crate::content::markdown::{render_markdown, wrap_line, MarkdownStyles, MdBlock};
use crate::feeds::FeedEntry;
use crate::theme::Theme;
use image::DynamicImage;
use ratatui::{
  layout::{Rect, Size},
  prelude::{Alignment, Line, Span, Style, Stylize},
  symbols::border,
  widgets::{
    Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Widget,
  },
  Frame,
};
use ratatui_image::{
  picker::Picker,
  sliced::{SignedPosition, SlicedImage, SlicedProtocol},
  Resize,
};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Fixed cell-row height reserved for each image segment.
const IMAGE_RENDER_ROWS: usize = 15;

/// A piece of entry content laid out for the viewport.
#[derive(Debug)]
enum ContentSegment {
  /// Pre-wrapped styled lines; each `Line` is exactly one screen row.
  Lines(Vec<Line<'static>>),
  /// Inline image: fetched asynchronously and stored in the App's image_cache.
  Image { src: String, alt: String },
}

/// Cached segment layout for the entry view.
///
/// Building segments parses the entry's Markdown and lays out tables, so it
/// is done once per (entry, width) and reused across frames — scrolling then
/// costs only the visible-slice clone. The theme is fixed for the lifetime of
/// the process, so it is not part of the key.
#[derive(Default)]
pub struct EntryRenderCache {
  key: u64,
  segments: Vec<ContentSegment>,
  /// Per-URL sliced image protocols, built lazily at render time and
  /// cleared whenever `key` changes (a new width invalidates existing
  /// slices, since they're encoded at a fixed target size).
  sliced: HashMap<String, SlicedProtocol>,
}

/// Configuration passed to [`render`] to avoid too many individual parameters.
pub struct EntryViewConfig<'a> {
  pub show_borders: bool,
  pub show_scrollbar: bool,
  pub show_images: bool,
  /// Horizontal padding (columns) applied on each side of entry text.
  pub horizontal_padding: u16,
  /// Maximum content width; `None` means use the full available width.
  pub max_width: Option<u16>,
  pub theme: &'a Theme,
  /// Protocol picker, needed to encode images into sliced protocols.
  pub picker: &'a Picker,
  /// Cache of decoded images keyed by URL.
  pub image_cache: &'a HashMap<String, DynamicImage>,
  /// Cached segment layout, reused while the entry, width and links match.
  pub render_cache: &'a mut EntryRenderCache,
}

/// Build the full ordered list of content segments for the entry view.
fn build_all_segments(
  feed_title: &str,
  entry: &FeedEntry,
  theme: &Theme,
  content_width: u16,
) -> Vec<ContentSegment> {
  let mut segs = Vec::new();

  // Metadata header (theme-colored, wrapped to the viewport width).
  let mut meta_src: Vec<Line<'static>> = Vec::new();
  meta_src.push(Line::from(format!("Title: {}", entry.title)).fg(theme.meta_title));
  meta_src.push(Line::from(format!("Feed: {}", feed_title)).fg(theme.meta_feed));
  meta_src.push(
    Line::from(format!(
      "Published: {}",
      entry.published.as_deref().unwrap_or("Unknown")
    ))
    .fg(theme.meta_published),
  );
  if !entry.links.is_empty() {
    meta_src.push(Line::from(format!("Link: {}", entry.links[0])).fg(theme.meta_link));
  }
  if let Some(ref url) = entry.media {
    meta_src.push(Line::from(format!("Media: {}", url)).fg(theme.meta_link));
  }
  meta_src.push(Line::from(""));
  let mut meta = Vec::new();
  for line in meta_src {
    meta.extend(wrap_line(line, content_width));
  }
  segs.push(ContentSegment::Lines(meta));

  // Body: render markdown into text/image blocks fitted to the width.
  // Inline links are numbered by their 1-based position in the entry's link
  // list, matching the links popup ('L' keybind).
  let styles = MarkdownStyles::from_theme(theme);
  let link_numbers: HashMap<String, usize> = entry
    .links
    .iter()
    .enumerate()
    .map(|(i, url)| (url.clone(), i + 1))
    .collect();
  for block in render_markdown(&entry.text, content_width, &styles, &link_numbers) {
    segs.push(match block {
      MdBlock::Text(lines) => ContentSegment::Lines(lines),
      MdBlock::Image { src, alt } => ContentSegment::Image { src, alt },
    });
  }

  // Bottom padding, mirroring the blank line placed after the metadata header.
  segs.push(ContentSegment::Lines(vec![Line::from("")]));

  segs
}

/// Return the virtual height of a single segment. All text lines are
/// pre-wrapped to the content width, so heights are exact.
fn seg_height(seg: &ContentSegment, show_images: bool) -> usize {
  match seg {
    ContentSegment::Lines(lines) => lines.len(),
    // 1 blank + alt text + 1 blank when images are off; full reserved rows otherwise.
    ContentSegment::Image { .. } => {
      if show_images {
        IMAGE_RENDER_ROWS
      } else {
        1
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
  picker: &Picker,
  image_cache: &HashMap<String, DynamicImage>,
  sliced_cache: &mut HashMap<String, SlicedProtocol>,
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
      ContentSegment::Lines(lines) => {
        // Lines are pre-wrapped: clone only the visible slice.
        let start = inner_skip.min(lines.len());
        let end = (inner_skip + render_h).min(lines.len());
        Paragraph::new(lines[start..end].to_vec()).render(seg_area, frame.buffer_mut());
      }
      ContentSegment::Image { src, alt } => {
        let ph = if alt.is_empty() {
          " [image] ".to_string()
        } else {
          format!(" [image: {}] ", alt)
        };
        if show_images {
          // Build (and cache) the sliced protocol once the raw image has been
          // decoded. Slices are sized for this render's content width, so
          // they're cleared by the caller whenever that width changes.
          if !sliced_cache.contains_key(src.as_str()) {
            if let Some(img) = image_cache.get(src.as_str()) {
              let target = Size::new(area.width, IMAGE_RENDER_ROWS as u16);
              if let Ok(sp) =
                SlicedProtocol::new_with_resize(picker, img.clone(), target, Resize::Fit(None))
              {
                sliced_cache.insert(src.clone(), sp);
              }
            }
          }

          if let Some(sliced) = sliced_cache.get(src.as_str()) {
            // Position is the image's row offset relative to the viewport
            // top; negative when scrolled past its start. SlicedImage crops
            // to whatever's visible per-protocol instead of requiring the
            // whole image to be in view.
            let y = (virtual_row as i64 - scroll as i64)
              .clamp(i16::MIN as i64, i16::MAX as i64) as i16;
            SlicedImage::new(sliced, SignedPosition::from((0, y))).render(area, frame.buffer_mut());
          } else {
            // Not decoded yet — placeholder in the segment's own sub-area.
            Paragraph::new(Line::from(ph).fg(theme.meta_link)).render(seg_area, frame.buffer_mut());
          }
        } else {
          // Images disabled: compact single-line placeholder, no padding.
          Paragraph::new(Line::from(ph).fg(theme.meta_link)).render(seg_area, frame.buffer_mut());
        }
      }
    }

    virtual_row = seg_end;
  }
}

/// Fingerprint of everything the segment layout depends on.
fn cache_key(feed_title: &str, entry: &FeedEntry, content_width: u16) -> u64 {
  let mut h = DefaultHasher::new();
  feed_title.hash(&mut h);
  entry.title.hash(&mut h);
  entry.published.hash(&mut h);
  entry.text.hash(&mut h);
  entry.links.hash(&mut h);
  entry.media.hash(&mut h);
  content_width.hash(&mut h);
  h.finish()
}

/// Clamp `area` to `max_width` columns (if set and narrower than `area`),
/// centering the result horizontally within `area`.
fn clamp_content_area(area: Rect, max_width: Option<u16>) -> Rect {
  match max_width {
    Some(max) if area.width > max => {
      let extra = area.width - max;
      Rect {
        x: area.x + extra / 2,
        width: max,
        ..area
      }
    }
    _ => area,
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
      .padding(Padding::symmetric(cfg.horizontal_padding, 1))
  } else {
    Block::default()
      .title(Span::styled(
        format!(" Entry - {}", entry.title),
        Style::default().fg(theme.entry_title_plain),
      ))
      .padding(Padding::new(
        cfg.horizontal_padding,
        cfg.horizontal_padding,
        0,
        1,
      ))
  };

  let text_area = clamp_content_area(entry_block.inner(inner_area), cfg.max_width);
  let content_width = text_area.width;
  let visible_height = text_area.height as usize;

  // Build (or reuse) segments and compute layout heights.
  let key = cache_key(feed_title, entry, content_width);
  if cfg.render_cache.key != key || cfg.render_cache.segments.is_empty() {
    cfg.render_cache.segments = build_all_segments(feed_title, entry, theme, content_width);
    cfg.render_cache.key = key;
    cfg.render_cache.sliced.clear();
  }
  let segments: &[ContentSegment] = &cfg.render_cache.segments;

  let show_images = cfg.show_images;
  let heights: Vec<usize> = segments
    .iter()
    .map(|seg| seg_height(seg, show_images))
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
    segments,
    &heights,
    cur_scroll,
    theme,
    cfg.picker,
    cfg.image_cache,
    &mut cfg.render_cache.sliced,
    show_images,
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
    // First segment should be the metadata lines
    assert!(matches!(&segs[0], ContentSegment::Lines(_)));
    if let ContentSegment::Lines(lines) = &segs[0] {
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
    // Table lines are laid out with column separators.
    let all: String = segs
      .iter()
      .filter_map(|s| match s {
        ContentSegment::Lines(lines) => Some(
          lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        ),
        _ => None,
      })
      .collect::<Vec<_>>()
      .join("\n");
    assert!(all.contains("─┼─"), "missing table separator in:\n{all}");
  }

  #[test]
  fn test_build_all_segments_exact_heights() {
    // A long single-line paragraph must be pre-wrapped: every line fits.
    let long = "word ".repeat(60);
    let entry = make_entry(&long, vec![]);
    let segs = build_all_segments("Feed", &entry, &test_theme(), 40);
    for seg in &segs {
      if let ContentSegment::Lines(lines) = seg {
        for line in lines {
          assert!(line.width() <= 40, "line too wide: {:?}", line.to_string());
        }
      }
    }
  }

  #[test]
  fn test_no_footer_links_section() {
    // Links are not repeated below the body — the links popup ('L') covers
    // them; inline [n] markers reference its numbering.
    let entry = make_entry(
      "Content",
      vec![
        "https://example.com/main".to_string(),
        "https://example.com/ref1".to_string(),
      ],
    );

    let segs = build_all_segments("Feed", &entry, &test_theme(), 80);
    for seg in &segs {
      if let ContentSegment::Lines(lines) = seg {
        for line in lines {
          assert!(!line.to_string().contains("Links:"));
        }
      }
    }
  }

  #[test]
  fn test_metadata_lines_keep_line_style() {
    let entry = make_entry("body", vec!["https://example.com/post".to_string()]);
    let segs = build_all_segments("Feed", &entry, &test_theme(), 80);
    let ContentSegment::Lines(lines) = &segs[0] else {
      panic!("expected metadata lines")
    };
    let theme = test_theme();
    let title_line = lines
      .iter()
      .find(|l| l.to_string().starts_with("Title:"))
      .unwrap();
    assert_eq!(title_line.style.fg, Some(theme.meta_title));
    let link_line = lines
      .iter()
      .find(|l| l.to_string().starts_with("Link:"))
      .unwrap();
    assert_eq!(link_line.style.fg, Some(theme.meta_link));
  }

  #[test]
  fn test_inline_links_numbered_to_match_popup() {
    let entry = make_entry(
      "See [the reference](https://example.com/ref1) for details.",
      vec![
        "https://example.com/main".to_string(),
        "https://example.com/ref1".to_string(),
      ],
    );

    let segs = build_all_segments("Feed", &entry, &test_theme(), 80);
    let all: String = segs
      .iter()
      .filter_map(|s| match s {
        ContentSegment::Lines(lines) => Some(
          lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        ),
        _ => None,
      })
      .collect::<Vec<_>>()
      .join("\n");
    assert!(
      all.contains("the reference[2]"),
      "inline number missing in:\n{all}"
    );
  }

  #[test]
  fn test_cache_key_changes_with_content_and_width() {
    let entry_a = make_entry("body a", vec![]);
    let entry_b = make_entry("body b", vec![]);
    let k1 = cache_key("Feed", &entry_a, 80);
    assert_eq!(k1, cache_key("Feed", &entry_a, 80));
    assert_ne!(k1, cache_key("Feed", &entry_b, 80));
    assert_ne!(k1, cache_key("Feed", &entry_a, 60));
    assert_ne!(k1, cache_key("Other", &entry_a, 80));
  }

  #[test]
  fn test_clamp_content_area_no_cap_returns_unchanged() {
    let area = Rect::new(0, 0, 120, 40);
    assert_eq!(clamp_content_area(area, None), area);
  }

  #[test]
  fn test_clamp_content_area_narrower_than_max_returns_unchanged() {
    let area = Rect::new(0, 0, 60, 40);
    assert_eq!(clamp_content_area(area, Some(80)), area);
  }

  #[test]
  fn test_clamp_content_area_caps_and_centers() {
    let area = Rect::new(10, 0, 120, 40);
    let clamped = clamp_content_area(area, Some(80));
    assert_eq!(clamped.width, 80);
    assert_eq!(clamped.x, 10 + (120 - 80) / 2);
    assert_eq!(clamped.height, area.height);
    assert_eq!(clamped.y, area.y);
  }

  #[test]
  fn test_body_has_trailing_blank_line() {
    let entry = make_entry("body", vec![]);
    let segs = build_all_segments("Feed", &entry, &test_theme(), 80);
    let ContentSegment::Lines(lines) = segs.last().expect("expected a trailing segment") else {
      panic!("expected trailing blank-line segment")
    };
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].to_string(), "");
  }
}

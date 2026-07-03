//! Markdown → ratatui rendering for the entry view.
//!
//! Produces a sequence of [`MdBlock`]s: wrappable styled text, pre-formatted
//! table lines fitted to the viewport width, and image references that the
//! entry view turns into inline image segments.

use crate::theme::Theme;
use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};

/// A rendered piece of Markdown content.
#[derive(Debug)]
pub enum MdBlock {
  /// Styled lines that may be soft-wrapped by the renderer.
  Text(Vec<Line<'static>>),
  /// Pre-formatted table lines already fitted to the width — must not wrap.
  Table(Vec<Line<'static>>),
  /// Inline image to be fetched and drawn by the entry view.
  Image { src: String, alt: String },
}

/// Theme-derived styles used while rendering Markdown.
#[derive(Clone, Copy, Debug)]
pub struct MarkdownStyles {
  h1: Color,
  h2: Color,
  h3: Color,
  h4: Color,
  h5: Color,
  code: Option<Color>,
  link: Color,
}

impl MarkdownStyles {
  pub fn from_theme(theme: &Theme) -> Self {
    Self {
      h1: theme.h1,
      h2: theme.h2,
      h3: theme.h3,
      h4: theme.h4,
      h5: theme.h5,
      code: theme.code,
      link: theme.link,
    }
  }

  fn heading(&self, level: u8) -> Style {
    match level {
      1 => Style::new()
        .fg(self.h1)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
      2 => Style::new().fg(self.h2).add_modifier(Modifier::BOLD),
      3 => Style::new().fg(self.h3).add_modifier(Modifier::BOLD),
      4 => Style::new().fg(self.h4),
      _ => Style::new().fg(self.h5),
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

  fn quote(&self) -> Style {
    Style::new().add_modifier(Modifier::DIM)
  }
}

fn md_options() -> Options {
  Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH
}

/// Render a Markdown string into blocks fitted to `width` terminal columns.
pub fn render_markdown(md: &str, width: u16, styles: &MarkdownStyles) -> Vec<MdBlock> {
  let mut r = Renderer {
    styles: *styles,
    width: width.max(10) as usize,
    blocks: Vec::new(),
    lines: Vec::new(),
    spans: Vec::new(),
    bold: 0,
    italic: 0,
    strike: 0,
    heading: None,
    quote: 0,
    in_code_block: false,
    list_stack: Vec::new(),
    links: Vec::new(),
    table: None,
    image: None,
  };
  for ev in Parser::new_ext(md, md_options()) {
    r.event(ev);
  }
  r.end_text_block();
  r.blocks
}

/// All `https?://` image URLs referenced by `![alt](url)` in a Markdown string.
pub fn extract_image_urls(md: &str) -> Vec<String> {
  Parser::new_ext(md, md_options())
    .filter_map(|ev| match ev {
      Event::Start(Tag::Image { dest_url, .. }) if dest_url.starts_with("http") => {
        Some(dest_url.into_string())
      }
      _ => None,
    })
    .collect()
}

struct TableBuild {
  aligns: Vec<Alignment>,
  rows: Vec<Vec<String>>,
  cur_row: Vec<String>,
  in_cell: bool,
}

struct Renderer {
  styles: MarkdownStyles,
  width: usize,
  blocks: Vec<MdBlock>,
  lines: Vec<Line<'static>>,
  spans: Vec<Span<'static>>,
  bold: u32,
  italic: u32,
  strike: u32,
  heading: Option<u8>,
  quote: usize,
  in_code_block: bool,
  list_stack: Vec<Option<u64>>,
  /// Open links: (span count at open, destination URL).
  links: Vec<(usize, String)>,
  table: Option<TableBuild>,
  image: Option<(String, String)>,
}

impl Renderer {
  fn event(&mut self, ev: Event) {
    // Inline content is diverted into an open image alt or table cell first.
    if self.try_sink(&ev) {
      return;
    }
    match ev {
      Event::Start(tag) => self.start(tag),
      Event::End(tag) => self.end(tag),
      Event::Text(t) => self.text(&t),
      Event::Code(t) => {
        self.ensure_prefix();
        self
          .spans
          .push(Span::styled(t.into_string(), self.styles.code()));
      }
      Event::SoftBreak => self.spans.push(Span::raw(" ")),
      Event::HardBreak => self.flush_line(),
      Event::Rule => {
        self.start_block();
        self
          .lines
          .push(Line::styled("─".repeat(self.width), self.styles.quote()));
      }
      // Raw HTML from old cached content (or edge cases) — show as plain text.
      Event::Html(h) | Event::InlineHtml(h) => self.text(&h),
      _ => {}
    }
  }

  fn try_sink(&mut self, ev: &Event) -> bool {
    let text: &str = match ev {
      Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => t,
      Event::SoftBreak | Event::HardBreak => " ",
      _ => return false,
    };
    self.sink_inline(text)
  }

  fn start(&mut self, tag: Tag) {
    match tag {
      Tag::Paragraph => self.start_block(),
      Tag::Heading { level, .. } => {
        self.start_block();
        self.heading = Some(heading_level(level));
      }
      Tag::BlockQuote(_) => {
        self.start_block();
        self.quote += 1;
      }
      Tag::CodeBlock(_) => {
        self.start_block();
        self.in_code_block = true;
      }
      Tag::List(start) => {
        if self.list_stack.is_empty() {
          self.start_block();
        } else {
          self.flush_line();
        }
        self.list_stack.push(start);
      }
      Tag::Item => {
        self.flush_line();
        self.ensure_prefix();
        let depth = self.list_stack.len().saturating_sub(1);
        let marker = match self.list_stack.last_mut() {
          Some(Some(i)) => {
            let m = format!("{}. ", i);
            *i += 1;
            m
          }
          _ => "• ".to_string(),
        };
        self
          .spans
          .push(Span::raw(format!("{}{}", "  ".repeat(depth), marker)));
      }
      Tag::Emphasis => self.italic += 1,
      Tag::Strong => self.bold += 1,
      Tag::Strikethrough => self.strike += 1,
      Tag::Link { dest_url, .. } => {
        self.links.push((self.spans.len(), dest_url.into_string()));
      }
      Tag::Image { dest_url, .. } => {
        self.image = Some((dest_url.into_string(), String::new()));
      }
      Tag::Table(aligns) => {
        self.break_with_gap();
        self.table = Some(TableBuild {
          aligns,
          rows: Vec::new(),
          cur_row: Vec::new(),
          in_cell: false,
        });
      }
      Tag::TableCell => {
        if let Some(t) = &mut self.table {
          t.cur_row.push(String::new());
          t.in_cell = true;
        }
      }
      _ => {}
    }
  }

  fn end(&mut self, tag: TagEnd) {
    match tag {
      TagEnd::Paragraph => self.flush_line(),
      TagEnd::Heading(_) => {
        self.flush_line();
        self.heading = None;
      }
      TagEnd::BlockQuote(_) => {
        self.flush_line();
        self.quote = self.quote.saturating_sub(1);
      }
      TagEnd::CodeBlock => {
        self.flush_line();
        self.in_code_block = false;
      }
      TagEnd::List(_) => {
        self.flush_line();
        self.list_stack.pop();
      }
      TagEnd::Item => self.flush_line(),
      TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
      TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
      TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
      TagEnd::Link => {
        if let Some((span_count, dest)) = self.links.pop() {
          // Autolink-style: no visible text was produced, show the URL itself.
          if self.spans.len() == span_count {
            self.ensure_prefix();
            self.spans.push(Span::styled(dest, self.styles.link()));
          }
        }
      }
      TagEnd::Image => {
        if let Some((src, alt)) = self.image.take() {
          if src.starts_with("http") {
            self.break_with_gap();
            self.blocks.push(MdBlock::Image { src, alt });
          } else {
            // Unresolvable image: keep a textual placeholder inline.
            self.ensure_prefix();
            let ph = if alt.is_empty() {
              "[image]".to_string()
            } else {
              format!("[image: {}]", alt)
            };
            self.spans.push(Span::styled(ph, self.styles.link()));
          }
        }
      }
      TagEnd::TableHead | TagEnd::TableRow => {
        if let Some(t) = &mut self.table {
          t.rows.push(std::mem::take(&mut t.cur_row));
        }
      }
      TagEnd::TableCell => {
        if let Some(t) = &mut self.table {
          if let Some(cell) = t.cur_row.last_mut() {
            let trimmed = cell.trim().to_string();
            *cell = trimmed;
          }
          t.in_cell = false;
        }
      }
      TagEnd::Table => {
        if let Some(t) = self.table.take() {
          let mut lines = layout_table(&t.rows, &t.aligns, self.width);
          lines.push(Line::default());
          self.blocks.push(MdBlock::Table(lines));
        }
      }
      _ => {}
    }
  }

  fn text(&mut self, t: &str) {
    if self.in_code_block {
      self.code_text(t);
      return;
    }
    self.ensure_prefix();
    self.spans.push(Span::styled(t.to_string(), self.style()));
  }

  /// Route inline content into an open table cell or image alt text.
  /// Returns true if the text was consumed.
  fn sink_inline(&mut self, t: &str) -> bool {
    if let Some((_, alt)) = &mut self.image {
      alt.push_str(t);
      return true;
    }
    if let Some(tb) = &mut self.table {
      if tb.in_cell {
        if let Some(cell) = tb.cur_row.last_mut() {
          cell.push_str(t);
        }
        return true;
      }
    }
    false
  }

  fn code_text(&mut self, t: &str) {
    let style = self.styles.code();
    for (i, part) in t.split('\n').enumerate() {
      if i > 0 {
        // Preserve blank lines inside code blocks.
        self.lines.push(Line::from(std::mem::take(&mut self.spans)));
      }
      if !part.is_empty() {
        self.ensure_prefix();
        self.spans.push(Span::styled(part.to_string(), style));
      }
    }
    // `split` leaves a trailing empty part when the text ends with '\n';
    // drop the resulting empty line so code blocks don't grow a blank tail.
    if t.ends_with('\n') && self.spans.is_empty() {
      if let Some(last) = self.lines.last() {
        if last.spans.is_empty() {
          self.lines.pop();
        }
      }
    }
  }

  fn style(&self) -> Style {
    let mut s = match self.heading {
      Some(l) => self.styles.heading(l),
      None => Style::new(),
    };
    if self.bold > 0 {
      s = s.add_modifier(Modifier::BOLD);
    }
    if self.italic > 0 {
      s = s.add_modifier(Modifier::ITALIC);
    }
    if self.strike > 0 {
      s = s.add_modifier(Modifier::CROSSED_OUT);
    }
    if !self.links.is_empty() {
      s = s.patch(self.styles.link());
    }
    if self.quote > 0 && self.heading.is_none() {
      s = s.add_modifier(Modifier::DIM);
    }
    s
  }

  /// Prepend the blockquote gutter when starting a fresh line inside a quote.
  fn ensure_prefix(&mut self) {
    if self.spans.is_empty() && self.quote > 0 {
      self
        .spans
        .push(Span::styled("│ ".repeat(self.quote), self.styles.quote()));
    }
  }

  fn flush_line(&mut self) {
    if !self.spans.is_empty() {
      self.lines.push(Line::from(std::mem::take(&mut self.spans)));
    }
  }

  /// Begin a new block element: blank-line separation within the text block.
  fn start_block(&mut self) {
    self.flush_line();
    if !self.lines.is_empty() {
      self.lines.push(Line::default());
    }
  }

  /// Close the current text block before a table or image, leaving a gap.
  fn break_with_gap(&mut self) {
    self.flush_line();
    if !self.lines.is_empty() {
      self.lines.push(Line::default());
    }
    self.end_text_block();
  }

  fn end_text_block(&mut self) {
    self.flush_line();
    if !self.lines.is_empty() {
      self
        .blocks
        .push(MdBlock::Text(std::mem::take(&mut self.lines)));
    }
  }
}

fn heading_level(level: HeadingLevel) -> u8 {
  match level {
    HeadingLevel::H1 => 1,
    HeadingLevel::H2 => 2,
    HeadingLevel::H3 => 3,
    HeadingLevel::H4 => 4,
    HeadingLevel::H5 => 5,
    HeadingLevel::H6 => 6,
  }
}

// ─── Table layout ─────────────────────────────────────────────────────────────

/// Lay out table rows into fixed-width lines. The first row is the header.
/// Columns are sized to their content, shrunk to fit `width`, and cell text
/// is word-wrapped inside its column, so wide tables stay readable.
fn layout_table(rows: &[Vec<String>], aligns: &[Alignment], width: usize) -> Vec<Line<'static>> {
  let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
  if cols == 0 {
    return Vec::new();
  }

  const SEP: &str = " │ "; // 3 display columns
  const MIN_COL: usize = 3;
  let sep_width = 3 * cols.saturating_sub(1);

  // Natural width of each column.
  let mut widths = vec![MIN_COL; cols];
  for row in rows {
    for (c, cell) in row.iter().enumerate() {
      widths[c] = widths[c].max(display_width(cell));
    }
  }

  // Shrink the widest columns until the table fits.
  let avail = width.saturating_sub(sep_width).max(cols * MIN_COL);
  while widths.iter().sum::<usize>() > avail {
    let (idx, _) = widths.iter().enumerate().max_by_key(|(_, w)| **w).unwrap();
    if widths[idx] <= MIN_COL {
      break;
    }
    widths[idx] -= 1;
  }

  let border_style = Style::new().add_modifier(Modifier::DIM);
  let mut lines = Vec::new();

  for (r, row) in rows.iter().enumerate() {
    let cell_style = if r == 0 {
      Style::new().add_modifier(Modifier::BOLD)
    } else {
      Style::new()
    };

    // Wrap every cell to its column width; the row is as tall as its
    // tallest cell.
    let wrapped: Vec<Vec<String>> = (0..cols)
      .map(|c| wrap_cell(row.get(c).map(String::as_str).unwrap_or(""), widths[c]))
      .collect();
    let row_height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);

    for line_idx in 0..row_height {
      let mut spans = Vec::new();
      for c in 0..cols {
        if c > 0 {
          spans.push(Span::styled(SEP, border_style));
        }
        let text = wrapped[c].get(line_idx).map(String::as_str).unwrap_or("");
        spans.push(Span::styled(
          pad_cell(
            text,
            widths[c],
            aligns.get(c).copied().unwrap_or(Alignment::None),
          ),
          cell_style,
        ));
      }
      lines.push(Line::from(spans));
    }

    // Header separator.
    if r == 0 {
      let mut spans = Vec::new();
      for (c, w) in widths.iter().enumerate() {
        if c > 0 {
          spans.push(Span::styled("─┼─", border_style));
        }
        spans.push(Span::styled("─".repeat(*w), border_style));
      }
      lines.push(Line::from(spans));
    }
  }

  lines
}

fn display_width(s: &str) -> usize {
  Span::raw(s).width()
}

/// Greedy word-wrap of a cell into lines no wider than `width` columns.
/// Words longer than the column are hard-split.
fn wrap_cell(s: &str, width: usize) -> Vec<String> {
  let width = width.max(1);
  let mut out: Vec<String> = Vec::new();
  let mut cur = String::new();
  let mut cur_w = 0;

  for word in s.split_whitespace() {
    let mut word = word;
    let mut word_w = display_width(word);

    // Hard-split words that can't fit on a line of their own.
    while word_w > width {
      if cur_w > 0 {
        out.push(std::mem::take(&mut cur));
        cur_w = 0;
      }
      let mut take_bytes = 0;
      let mut take_w = 0;
      for ch in word.chars() {
        let cw = display_width(ch.encode_utf8(&mut [0u8; 4]));
        if take_w + cw > width {
          break;
        }
        take_bytes += ch.len_utf8();
        take_w += cw;
      }
      // Always consume at least one char to guarantee progress.
      if take_bytes == 0 {
        take_bytes = word
          .chars()
          .next()
          .map(char::len_utf8)
          .unwrap_or(word.len());
      }
      out.push(word[..take_bytes].to_string());
      word = &word[take_bytes..];
      word_w = display_width(word);
    }
    if word.is_empty() {
      continue;
    }

    let needed = if cur_w == 0 { word_w } else { word_w + 1 };
    if cur_w + needed > width && cur_w > 0 {
      out.push(std::mem::take(&mut cur));
      cur_w = 0;
    }
    if cur_w > 0 {
      cur.push(' ');
      cur_w += 1;
    }
    cur.push_str(word);
    cur_w += word_w;
  }
  if !cur.is_empty() {
    out.push(cur);
  }
  if out.is_empty() {
    out.push(String::new());
  }
  out
}

/// Pad (or align) `text` to exactly `width` display columns.
fn pad_cell(text: &str, width: usize, align: Alignment) -> String {
  let w = display_width(text);
  let pad = width.saturating_sub(w);
  match align {
    Alignment::Right => format!("{}{}", " ".repeat(pad), text),
    Alignment::Center => {
      let left = pad / 2;
      format!("{}{}{}", " ".repeat(left), text, " ".repeat(pad - left))
    }
    _ => format!("{}{}", text, " ".repeat(pad)),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn styles() -> MarkdownStyles {
    MarkdownStyles::from_theme(&Theme::default())
  }

  fn text_of(blocks: &[MdBlock]) -> String {
    blocks
      .iter()
      .map(|b| match b {
        MdBlock::Text(lines) | MdBlock::Table(lines) => lines
          .iter()
          .map(|l| l.to_string())
          .collect::<Vec<_>>()
          .join("\n"),
        MdBlock::Image { src, .. } => format!("<img {}>", src),
      })
      .collect::<Vec<_>>()
      .join("\n")
  }

  #[test]
  fn paragraphs_render_with_blank_separator() {
    let blocks = render_markdown("First para.\n\nSecond para.", 80, &styles());
    assert_eq!(blocks.len(), 1);
    let MdBlock::Text(lines) = &blocks[0] else {
      panic!("expected text block")
    };
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].to_string(), "First para.");
    assert_eq!(lines[1].to_string(), "");
    assert_eq!(lines[2].to_string(), "Second para.");
  }

  #[test]
  fn images_become_separate_blocks() {
    let blocks = render_markdown(
      "Before\n\n![cat pic](https://example.com/cat.png)\n\nAfter",
      80,
      &styles(),
    );
    assert_eq!(blocks.len(), 3);
    assert!(matches!(&blocks[0], MdBlock::Text(_)));
    assert!(
      matches!(&blocks[1], MdBlock::Image { src, alt } if src == "https://example.com/cat.png" && alt == "cat pic")
    );
    assert!(matches!(&blocks[2], MdBlock::Text(_)));
  }

  #[test]
  fn non_http_images_render_placeholder_text() {
    let blocks = render_markdown("![local](./pic.png)", 80, &styles());
    assert_eq!(blocks.len(), 1);
    assert!(text_of(&blocks).contains("[image: local]"));
  }

  #[test]
  fn tables_render_aligned_columns() {
    let md = "| Name | Value |\n| --- | --- |\n| alpha | 1 |\n| b | 22 |";
    let blocks = render_markdown(md, 80, &styles());
    assert_eq!(blocks.len(), 1);
    let MdBlock::Table(lines) = &blocks[0] else {
      panic!("expected table block")
    };
    let rendered: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    assert_eq!(rendered[0], "Name  │ Value");
    assert_eq!(rendered[1], "──────┼──────");
    assert_eq!(rendered[2], "alpha │ 1    ");
    assert_eq!(rendered[3], "b     │ 22   ");
  }

  #[test]
  fn wide_tables_wrap_cells_to_fit() {
    let md = "| A | B |\n| --- | --- |\n| this is a rather long cell | short |";
    let blocks = render_markdown(md, 24, &styles());
    let MdBlock::Table(lines) = &blocks[0] else {
      panic!("expected table block")
    };
    // Every rendered line must fit in the viewport.
    for line in lines {
      assert!(line.width() <= 24, "line too wide: {:?}", line.to_string());
    }
    // The long cell wraps over multiple lines but keeps all its words.
    let all = lines
      .iter()
      .map(|l| l.to_string())
      .collect::<Vec<_>>()
      .join("\n");
    for word in ["this", "rather", "long", "cell", "short"] {
      assert!(all.contains(word), "missing {word} in:\n{all}");
    }
  }

  #[test]
  fn lists_render_markers() {
    let blocks = render_markdown("- one\n- two\n\n1. first\n2. second", 80, &styles());
    let out = text_of(&blocks);
    assert!(out.contains("• one"));
    assert!(out.contains("• two"));
    assert!(out.contains("1. first"));
    assert!(out.contains("2. second"));
  }

  #[test]
  fn nested_lists_indent() {
    let blocks = render_markdown("- a\n  - a1\n- b", 80, &styles());
    let out = text_of(&blocks);
    assert!(out.contains("• a"));
    assert!(out.contains("  • a1"));
  }

  #[test]
  fn blockquotes_get_gutter() {
    let blocks = render_markdown("> quoted text", 80, &styles());
    let out = text_of(&blocks);
    assert!(out.starts_with("│ quoted text"), "got: {out}");
  }

  #[test]
  fn code_blocks_preserve_lines() {
    let blocks = render_markdown("```\nfn main() {\n\n  body();\n}\n```", 80, &styles());
    let MdBlock::Text(lines) = &blocks[0] else {
      panic!("expected text block")
    };
    let rendered: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    assert_eq!(rendered, vec!["fn main() {", "", "  body();", "}"]);
  }

  #[test]
  fn bare_link_shows_url() {
    let blocks = render_markdown("[](https://example.com/x)", 80, &styles());
    assert!(text_of(&blocks).contains("https://example.com/x"));
  }

  #[test]
  fn extract_image_urls_finds_http_only() {
    let md =
      "![a](https://example.com/a.png) text ![b](./local.png) ![c](http://example.com/c.gif)";
    let urls = extract_image_urls(md);
    assert_eq!(
      urls,
      vec!["https://example.com/a.png", "http://example.com/c.gif"]
    );
  }

  #[test]
  fn hard_break_starts_new_line() {
    let blocks = render_markdown("line one\\\nline two", 80, &styles());
    let MdBlock::Text(lines) = &blocks[0] else {
      panic!("expected text block")
    };
    assert_eq!(lines.len(), 2);
  }

  #[test]
  fn escaped_metacharacters_render_literally() {
    let blocks = render_markdown("5 \\* 3 and \\[brackets\\]", 80, &styles());
    let out = text_of(&blocks);
    assert!(out.contains("5 * 3 and [brackets]"));
  }
}

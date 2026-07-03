//! Markdown → ratatui rendering for the entry view.
//!
//! Produces a sequence of [`MdBlock`]s: styled lines already word-wrapped to
//! the viewport width (so every `Line` is exactly one screen row — heights
//! are exact), and image references that the entry view turns into inline
//! image segments. Tables are laid out with content-sized columns and cell
//! wrapping. Wrapped list items and blockquote lines keep their hanging
//! indent / gutter on continuation lines.

use crate::theme::Theme;
use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use std::collections::HashMap;

/// A rendered piece of Markdown content.
#[derive(Debug)]
pub enum MdBlock {
  /// Pre-wrapped styled lines; each `Line` occupies exactly one screen row.
  Text(Vec<Line<'static>>),
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

  /// Inline `[n]` footnote marker tying a link to the links popup numbering.
  fn link_number(&self) -> Style {
    Style::new().fg(self.link)
  }

  fn quote(&self) -> Style {
    Style::new().add_modifier(Modifier::DIM)
  }
}

fn md_options() -> Options {
  Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH
}

/// Render a Markdown string into blocks fitted to `width` terminal columns.
///
/// `link_numbers` maps a URL to its 1-based position in the entry's link
/// list; inline links whose URL is found there get an `[n]` marker matching
/// the links popup.
pub fn render_markdown(
  md: &str,
  width: u16,
  styles: &MarkdownStyles,
  link_numbers: &HashMap<String, usize>,
) -> Vec<MdBlock> {
  let mut r = Renderer {
    styles: *styles,
    width: width.max(10) as usize,
    link_numbers,
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
    item_hangs: Vec::new(),
    fresh_item: false,
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

/// Word-wrap a single styled line to `width` columns (no hanging indent).
/// Used by the entry view for metadata lines. The line-level style (e.g. a
/// color set via `Line::fg`) is preserved on every wrapped line.
pub fn wrap_line(line: Line<'static>, width: u16) -> Vec<Line<'static>> {
  let style = line.style;
  wrap_styled(line.spans, width.max(1) as usize, &[])
    .into_iter()
    .map(|l| l.style(style))
    .collect()
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

struct Renderer<'a> {
  styles: MarkdownStyles,
  width: usize,
  link_numbers: &'a HashMap<String, usize>,
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
  /// Hanging-indent widths of the currently open list items (marker widths).
  item_hangs: Vec<usize>,
  /// A list item marker was just emitted and no content followed yet; the
  /// item's first paragraph must continue on the marker's line.
  fresh_item: bool,
  /// Open links: (span count at open, destination URL).
  links: Vec<(usize, String)>,
  table: Option<TableBuild>,
  image: Option<(String, String)>,
}

impl Renderer<'_> {
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
      // Space between soft-wrapped source lines; dropped at line start.
      Event::SoftBreak if !self.spans.is_empty() => self.spans.push(Span::raw(" ")),
      Event::SoftBreak => {}
      Event::HardBreak => self.flush_line(),
      Event::Rule => {
        self.gap();
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
        // Outer hangs indent nested items; the explicit depth prefix is
        // covered by the accumulated hang widths.
        self.ensure_prefix();
        let marker = match self.list_stack.last_mut() {
          Some(Some(i)) => {
            let m = format!("{}. ", i);
            *i += 1;
            m
          }
          _ => "• ".to_string(),
        };
        self.item_hangs.push(display_width(&marker));
        self.spans.push(Span::raw(marker));
        self.fresh_item = true;
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
        self.gap();
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
      TagEnd::Item => {
        self.flush_line();
        self.item_hangs.pop();
      }
      TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
      TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
      TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
      TagEnd::Link => self.end_link(),
      TagEnd::Image => {
        if let Some((src, alt)) = self.image.take() {
          if src.starts_with("http") {
            self.gap();
            self.end_text_block();
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
          self
            .lines
            .extend(layout_table(&t.rows, &t.aligns, self.width));
        }
      }
      _ => {}
    }
  }

  fn end_link(&mut self) {
    let Some((span_count, dest)) = self.links.pop() else {
      return;
    };
    // Autolink-style: no visible text was produced, show the URL itself.
    if self.spans.len() <= span_count {
      self.ensure_prefix();
      self.spans.push(Span::styled(dest, self.styles.link()));
      return;
    }
    // Number the link to match the links popup, unless the visible
    // text is already the URL itself.
    let text: String = self.spans[span_count..]
      .iter()
      .map(|s| s.content.as_ref())
      .collect();
    if text != dest {
      if let Some(&n) = self.link_numbers.get(&dest) {
        self
          .spans
          .push(Span::styled(format!("[{}]", n), self.styles.link_number()));
      }
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
    let prefix = self.cont_prefix();
    let prefix_w: usize = prefix.iter().map(Span::width).sum();
    let avail = self.width.saturating_sub(prefix_w).max(1);

    for (i, part) in t.split('\n').enumerate() {
      if i > 0 {
        // Preserve blank lines inside code blocks.
        self.push_raw_line();
      }
      if part.is_empty() {
        continue;
      }
      // Code is hard-split by columns to keep indentation intact.
      for (j, chunk) in char_chunks(part, avail).into_iter().enumerate() {
        if j > 0 {
          self.push_raw_line();
        }
        self.ensure_prefix();
        self.spans.push(Span::styled(chunk, style));
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

  /// Spans that begin every fresh line at the current nesting: blockquote
  /// gutter plus hanging indent of open list items.
  fn cont_prefix(&self) -> Vec<Span<'static>> {
    let mut p = Vec::new();
    if self.quote > 0 {
      p.push(Span::styled("│ ".repeat(self.quote), self.styles.quote()));
    }
    let hang: usize = self.item_hangs.iter().sum();
    if hang > 0 {
      p.push(Span::raw(" ".repeat(hang)));
    }
    p
  }

  /// Prepend gutter/indent when starting a fresh line.
  fn ensure_prefix(&mut self) {
    if self.spans.is_empty() {
      self.spans = self.cont_prefix();
    }
  }

  /// Word-wrap and emit the pending spans as one or more visual lines.
  fn flush_line(&mut self) {
    self.fresh_item = false;
    if self.spans.is_empty() {
      return;
    }
    let spans = std::mem::take(&mut self.spans);
    let cont = self.cont_prefix();
    self.lines.extend(wrap_styled(spans, self.width, &cont));
  }

  /// Emit the pending spans as a single pre-fitted line (code chunks).
  fn push_raw_line(&mut self) {
    self.fresh_item = false;
    self.lines.push(Line::from(std::mem::take(&mut self.spans)));
  }

  /// Begin a new block element: blank-line separation within the text block.
  fn start_block(&mut self) {
    // The first paragraph of a list item continues on the marker's line.
    if self.fresh_item {
      self.fresh_item = false;
      return;
    }
    self.gap();
  }

  /// Flush pending spans and ensure the block ends with one blank line.
  fn gap(&mut self) {
    self.flush_line();
    if let Some(last) = self.lines.last() {
      if !last.spans.is_empty() {
        self.lines.push(Line::default());
      }
    }
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

// ─── Word wrapping ────────────────────────────────────────────────────────────

enum Tok {
  Word(Vec<Span<'static>>),
  Space,
}

/// Split spans into whitespace-separated word tokens, preserving styles.
/// Words spanning style boundaries stay together as one token.
fn tokenize(spans: Vec<Span<'static>>) -> Vec<Tok> {
  let mut toks: Vec<Tok> = Vec::new();
  let mut word: Vec<Span<'static>> = Vec::new();
  for span in spans {
    let style = span.style;
    let mut chunk = String::new();
    for ch in span.content.chars() {
      if ch.is_whitespace() {
        if !chunk.is_empty() {
          word.push(Span::styled(std::mem::take(&mut chunk), style));
        }
        if !word.is_empty() {
          toks.push(Tok::Word(std::mem::take(&mut word)));
        }
        if matches!(toks.last(), Some(Tok::Word(_))) {
          toks.push(Tok::Space);
        }
      } else {
        chunk.push(ch);
      }
    }
    if !chunk.is_empty() {
      word.push(Span::styled(chunk, style));
    }
  }
  if !word.is_empty() {
    toks.push(Tok::Word(word));
  }
  toks
}

/// Greedy word-wrap of styled spans into lines no wider than `width` columns.
/// `cont_prefix` is prepended to every line after the first. Words wider than
/// a full line are hard-split.
fn wrap_styled(
  spans: Vec<Span<'static>>,
  width: usize,
  cont_prefix: &[Span<'static>],
) -> Vec<Line<'static>> {
  let width = width.max(2);
  let total: usize = spans.iter().map(Span::width).sum();
  if total <= width {
    return vec![Line::from(spans)];
  }

  // Cap the prefix width so every line keeps at least one content column.
  let prefix_w = cont_prefix
    .iter()
    .map(Span::width)
    .sum::<usize>()
    .min(width - 1);

  let mut lines: Vec<Line<'static>> = Vec::new();
  let mut cur: Vec<Span<'static>> = Vec::new();
  let mut cur_w = 0usize;
  let mut line_has_word = false;
  let mut pending_space = false;

  macro_rules! newline {
    () => {{
      lines.push(Line::from(std::mem::take(&mut cur)));
      cur.extend(cont_prefix.iter().cloned());
      cur_w = prefix_w;
    }};
  }

  for tok in tokenize(spans) {
    match tok {
      Tok::Space => {
        if line_has_word {
          pending_space = true;
        }
      }
      Tok::Word(parts) => {
        let word_w: usize = parts.iter().map(Span::width).sum();
        let space = usize::from(pending_space);
        if cur_w + space + word_w <= width {
          if pending_space {
            cur.push(Span::raw(" "));
            cur_w += 1;
          }
          cur.extend(parts);
          cur_w += word_w;
        } else if prefix_w + word_w <= width {
          newline!();
          cur.extend(parts);
          cur_w += word_w;
        } else {
          // Word wider than a full line: hard-split by columns.
          if pending_space && cur_w < width {
            cur.push(Span::raw(" "));
            cur_w += 1;
          }
          for part in parts {
            let style = part.style;
            let mut chunk = String::new();
            for ch in part.content.chars() {
              let cw = display_width(ch.encode_utf8(&mut [0u8; 4]));
              if cur_w + cw > width {
                if !chunk.is_empty() {
                  cur.push(Span::styled(std::mem::take(&mut chunk), style));
                }
                newline!();
              }
              chunk.push(ch);
              cur_w += cw;
            }
            if !chunk.is_empty() {
              cur.push(Span::styled(chunk, style));
            }
          }
        }
        pending_space = false;
        line_has_word = true;
      }
    }
  }
  if line_has_word {
    lines.push(Line::from(cur));
  }
  lines
}

/// Hard-split a string into chunks of at most `width` display columns.
fn char_chunks(s: &str, width: usize) -> Vec<String> {
  let width = width.max(1);
  let mut out: Vec<String> = Vec::new();
  let mut cur = String::new();
  let mut cur_w = 0usize;
  for ch in s.chars() {
    let cw = display_width(ch.encode_utf8(&mut [0u8; 4]));
    if cur_w + cw > width && !cur.is_empty() {
      out.push(std::mem::take(&mut cur));
      cur_w = 0;
    }
    cur.push(ch);
    cur_w += cw;
  }
  if !cur.is_empty() {
    out.push(cur);
  }
  if out.is_empty() {
    out.push(String::new());
  }
  out
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
    let word_w = display_width(word);

    // Hard-split words that can't fit on a line of their own.
    if word_w > width {
      if cur_w > 0 {
        out.push(std::mem::take(&mut cur));
      }
      let mut chunks = char_chunks(word, width);
      let last = chunks.pop().unwrap_or_default();
      out.extend(chunks);
      cur_w = display_width(&last);
      cur = last;
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

  fn render(md: &str, width: u16) -> Vec<MdBlock> {
    render_markdown(md, width, &styles(), &HashMap::new())
  }

  fn text_of(blocks: &[MdBlock]) -> String {
    blocks
      .iter()
      .map(|b| match b {
        MdBlock::Text(lines) => lines
          .iter()
          .map(|l| l.to_string())
          .collect::<Vec<_>>()
          .join("\n"),
        MdBlock::Image { src, .. } => format!("<img {}>", src),
      })
      .collect::<Vec<_>>()
      .join("\n")
  }

  fn lines_of(block: &MdBlock) -> Vec<String> {
    match block {
      MdBlock::Text(lines) => lines.iter().map(|l| l.to_string()).collect(),
      MdBlock::Image { .. } => panic!("expected text block"),
    }
  }

  #[test]
  fn paragraphs_render_with_blank_separator() {
    let blocks = render("First para.\n\nSecond para.", 80);
    assert_eq!(blocks.len(), 1);
    assert_eq!(
      lines_of(&blocks[0]),
      vec!["First para.", "", "Second para."]
    );
  }

  #[test]
  fn long_paragraphs_wrap_at_word_boundaries() {
    let blocks = render("alpha beta gamma delta epsilon", 12);
    let lines = lines_of(&blocks[0]);
    assert_eq!(lines, vec!["alpha beta", "gamma delta", "epsilon"]);
    // Exact-height invariant: every line fits the width.
    for l in &lines {
      assert!(l.len() <= 12);
    }
  }

  #[test]
  fn long_words_hard_split() {
    let blocks = render("see https://example.com/a/very/long/url/segment/path", 20);
    let lines = lines_of(&blocks[0]);
    for l in &lines {
      assert!(l.chars().count() <= 20, "too wide: {l:?}");
    }
    assert!(lines.len() > 1);
    // No content lost.
    assert_eq!(
      lines.join(""),
      "see https://example.com/a/very/long/url/segment/path"
    );
  }

  #[test]
  fn images_become_separate_blocks() {
    let blocks = render(
      "Before\n\n![cat pic](https://example.com/cat.png)\n\nAfter",
      80,
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
    let blocks = render("![local](./pic.png)", 80);
    assert_eq!(blocks.len(), 1);
    assert!(text_of(&blocks).contains("[image: local]"));
  }

  #[test]
  fn tables_render_aligned_columns() {
    let md = "| Name | Value |\n| --- | --- |\n| alpha | 1 |\n| b | 22 |";
    let blocks = render(md, 80);
    assert_eq!(blocks.len(), 1);
    let rendered = lines_of(&blocks[0]);
    assert_eq!(rendered[0], "Name  │ Value");
    assert_eq!(rendered[1], "──────┼──────");
    assert_eq!(rendered[2], "alpha │ 1    ");
    assert_eq!(rendered[3], "b     │ 22   ");
  }

  #[test]
  fn tables_are_separated_from_text() {
    let md = "Intro text.\n\n| A |\n| --- |\n| 1 |\n\nOutro.";
    let blocks = render(md, 80);
    assert_eq!(blocks.len(), 1);
    let lines = lines_of(&blocks[0]);
    assert_eq!(lines[0], "Intro text.");
    assert_eq!(lines[1], "");
    assert!(lines[2].starts_with("A"));
    // Blank before outro paragraph too.
    assert_eq!(lines[lines.len() - 2], "");
    assert_eq!(lines[lines.len() - 1], "Outro.");
  }

  #[test]
  fn wide_tables_wrap_cells_to_fit() {
    let md = "| A | B |\n| --- | --- |\n| this is a rather long cell | short |";
    let blocks = render(md, 24);
    let MdBlock::Text(lines) = &blocks[0] else {
      panic!("expected text block")
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
    let blocks = render("- one\n- two\n\n1. first\n2. second", 80);
    let out = text_of(&blocks);
    assert!(out.contains("• one"));
    assert!(out.contains("• two"));
    assert!(out.contains("1. first"));
    assert!(out.contains("2. second"));
  }

  #[test]
  fn nested_lists_indent() {
    let blocks = render("- a\n  - a1\n- b", 80);
    let out = text_of(&blocks);
    assert!(out.contains("• a"));
    assert!(out.contains("  • a1"), "got:\n{out}");
  }

  #[test]
  fn wrapped_list_items_keep_hanging_indent() {
    let blocks = render("- alpha beta gamma delta epsilon zeta", 14);
    let lines = lines_of(&blocks[0]);
    assert_eq!(lines[0], "• alpha beta");
    // Continuation lines are indented under the item text, not the marker.
    for cont in &lines[1..] {
      assert!(cont.starts_with("  "), "missing hang: {cont:?}");
      assert!(!cont.starts_with("• "));
    }
  }

  #[test]
  fn loose_list_items_keep_marker_on_first_line() {
    // Blank line between items makes pulldown-cmark wrap contents in
    // paragraphs; the marker must still share the line with the text.
    let blocks = render("- first item\n\n- second item", 80);
    let out = text_of(&blocks);
    assert!(out.contains("• first item"), "got:\n{out}");
    assert!(out.contains("• second item"), "got:\n{out}");
  }

  #[test]
  fn blockquotes_get_gutter() {
    let blocks = render("> quoted text", 80);
    let out = text_of(&blocks);
    assert!(out.starts_with("│ quoted text"), "got: {out}");
  }

  #[test]
  fn wrapped_blockquotes_keep_gutter() {
    let blocks = render("> alpha beta gamma delta epsilon", 14);
    let lines = lines_of(&blocks[0]);
    assert!(lines.len() > 1);
    for l in &lines {
      assert!(l.starts_with("│ "), "missing gutter: {l:?}");
    }
  }

  #[test]
  fn code_blocks_preserve_lines() {
    let blocks = render("```\nfn main() {\n\n  body();\n}\n```", 80);
    let rendered = lines_of(&blocks[0]);
    assert_eq!(rendered, vec!["fn main() {", "", "  body();", "}"]);
  }

  #[test]
  fn long_code_lines_split_by_columns() {
    let blocks = render(
      "```\n    let indented = very_long_identifier_name;\n```",
      20,
    );
    let lines = lines_of(&blocks[0]);
    assert!(lines.len() > 1);
    // Leading indentation preserved on the first chunk.
    assert!(lines[0].starts_with("    let"));
    for l in &lines {
      assert!(l.chars().count() <= 20);
    }
  }

  #[test]
  fn bare_link_shows_url() {
    let blocks = render("[](https://example.com/x)", 80);
    assert!(text_of(&blocks).contains("https://example.com/x"));
  }

  #[test]
  fn links_get_footnote_numbers() {
    let numbers: HashMap<String, usize> = [("https://example.com/a".to_string(), 2)]
      .into_iter()
      .collect();
    let blocks = render_markdown(
      "See [the docs](https://example.com/a) and [unknown](https://other.com/b).",
      80,
      &styles(),
      &numbers,
    );
    let out = text_of(&blocks);
    assert!(out.contains("the docs[2]"), "got: {out}");
    // Links not present in the entry's link list get no number.
    assert!(
      out.contains("unknown and") || out.contains("unknown."),
      "got: {out}"
    );
    assert!(!out.contains("unknown["), "got: {out}");
  }

  #[test]
  fn url_text_links_get_no_number() {
    let numbers: HashMap<String, usize> = [("https://example.com/a".to_string(), 2)]
      .into_iter()
      .collect();
    let blocks = render_markdown(
      "[https://example.com/a](https://example.com/a)",
      80,
      &styles(),
      &numbers,
    );
    let out = text_of(&blocks);
    assert_eq!(out, "https://example.com/a");
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
    let blocks = render("line one\\\nline two", 80);
    let lines = lines_of(&blocks[0]);
    assert_eq!(lines.len(), 2);
  }

  #[test]
  fn escaped_metacharacters_render_literally() {
    let blocks = render("5 \\* 3 and \\[brackets\\]", 80);
    let out = text_of(&blocks);
    assert!(out.contains("5 * 3 and [brackets]"));
  }

  #[test]
  fn wrap_line_wraps_plain_lines() {
    let wrapped = wrap_line(
      Line::from("Link: https://example.com/extremely/long/path"),
      20,
    );
    assert!(wrapped.len() > 1);
    for l in &wrapped {
      assert!(l.width() <= 20);
    }
  }
}

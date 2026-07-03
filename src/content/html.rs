//! HTML → Markdown conversion for feed entry content.
//!
//! A single walk over the parsed DOM produces GitHub-flavored Markdown
//! (pipe tables, images, links) and collects every http(s) link in document
//! order. Relative URLs are resolved against the entry's own link so images
//! and references from feeds that emit relative paths still work.

use ego_tree::NodeRef;
use reqwest::Url;
use scraper::node::Element;
use scraper::{Html, Node};

/// Result of converting an HTML fragment.
pub struct Converted {
  /// GitHub-flavored Markdown.
  pub markdown: String,
  /// Absolute http(s) URLs of anchors/embeds, in document order.
  pub links: Vec<String>,
}

/// Convert an HTML fragment to Markdown. `base_url` (typically the entry's
/// article link) is used to resolve relative hrefs and image sources.
pub fn html_to_markdown(html: &str, base_url: Option<&str>) -> Converted {
  let dom = Html::parse_fragment(html);
  let mut w = Walker {
    out: String::new(),
    links: Vec::new(),
    base: base_url.and_then(|b| Url::parse(b).ok()),
  };
  w.walk_children(dom.tree.root(), Ctx::default());
  Converted {
    markdown: tidy(&w.out),
    links: w.links,
  }
}

#[derive(Clone, Copy, Default)]
struct Ctx {
  /// Inside a table cell: no block breaks, escape pipes.
  in_cell: bool,
  /// Current list nesting depth.
  list_depth: usize,
}

struct Walker {
  out: String,
  links: Vec<String>,
  base: Option<Url>,
}

impl Walker {
  fn walk_children(&mut self, node: NodeRef<Node>, ctx: Ctx) {
    for child in node.children() {
      self.walk(child, ctx);
    }
  }

  fn walk(&mut self, node: NodeRef<Node>, ctx: Ctx) {
    match node.value() {
      Node::Text(t) => self.text(&t.text, ctx),
      Node::Element(ref el) => self.element(node, el, ctx),
      _ => {}
    }
  }

  fn element(&mut self, node: NodeRef<Node>, el: &Element, ctx: Ctx) {
    match el.name() {
      "script" | "style" | "head" | "title" | "noscript" | "template" | "svg" | "math" => {}

      "p" | "div" | "section" | "article" | "main" | "aside" | "header" | "footer" | "nav"
      | "figure" | "figcaption" | "details" | "summary" | "dl" | "dt" | "dd" | "address" => {
        if ctx.in_cell {
          // Blocks flatten to inline inside table cells; separate with a space.
          self.cell_space();
          self.walk_children(node, ctx);
          self.cell_space();
        } else {
          self.blank_line();
          self.walk_children(node, ctx);
          self.blank_line();
        }
      }

      h @ ("h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
        if ctx.in_cell {
          self.cell_space();
          self.walk_children(node, ctx);
          self.cell_space();
          return;
        }
        self.blank_line();
        let level = (h.as_bytes()[1] - b'0') as usize;
        self.out.push_str(&"#".repeat(level));
        self.out.push(' ');
        self.walk_children(node, ctx);
        self.blank_line();
      }

      "br" => {
        if ctx.in_cell {
          self.out.push(' ');
        } else if !self.out.is_empty() && !self.out.ends_with('\n') {
          // Backslash-newline is a Markdown hard break.
          self.out.push_str("\\\n");
        }
      }

      "hr" => {
        if !ctx.in_cell {
          self.blank_line();
          self.out.push_str("---");
          self.blank_line();
        }
      }

      "strong" | "b" => self.inline_wrap(node, ctx, "**"),
      "em" | "i" | "cite" | "var" => self.inline_wrap(node, ctx, "*"),
      "del" | "s" | "strike" => self.inline_wrap(node, ctx, "~~"),

      "code" | "kbd" | "samp" | "tt" => {
        let content = collapse_ws(&collect_text(node));
        if content.is_empty() {
          return;
        }
        if content.contains('`') {
          self.out.push_str("`` ");
          self.out.push_str(&content);
          self.out.push_str(" ``");
        } else {
          self.out.push('`');
          self.out.push_str(&content);
          self.out.push('`');
        }
      }

      "pre" => {
        if ctx.in_cell {
          let content = collapse_ws(&collect_text(node));
          self.out.push('`');
          self.out.push_str(&content);
          self.out.push('`');
          return;
        }
        let text = collect_text(node);
        let text = text.trim_matches('\n');
        // Pick a fence longer than any backtick run in the content.
        let fence = if text.contains("```") { "````" } else { "```" };
        self.blank_line();
        self.out.push_str(fence);
        self.out.push('\n');
        self.out.push_str(text);
        if !text.ends_with('\n') {
          self.out.push('\n');
        }
        self.out.push_str(fence);
        self.blank_line();
      }

      "a" => {
        let text = self.capture(|w| w.walk_children(node, ctx));
        let text = collapse_ws(&text);
        // Anchors with no visible content (heading permalinks, icon links)
        // are dropped entirely — nothing on screen would reference them.
        if text.is_empty() {
          return;
        }
        match self.resolve(el.attr("href")) {
          Some(url) => {
            self.links.push(url.clone());
            self.out.push_str(&format!("[{}]({})", text, url));
          }
          None => self.out.push_str(&text),
        }
      }

      "img" => {
        let src = ["src", "data-src", "data-lazy-src"]
          .iter()
          .find_map(|a| el.attr(a).filter(|v| !v.trim().is_empty()))
          .or_else(|| el.attr("srcset").and_then(first_srcset_url));
        if let Some(url) = self.resolve(src) {
          let alt = collapse_ws(el.attr("alt").unwrap_or("")).replace(['[', ']'], "");
          self.out.push_str(&format!("![{}]({})", alt, url));
        }
      }

      "ul" | "ol" => {
        if ctx.in_cell {
          self.walk_children(node, ctx);
          return;
        }
        if ctx.list_depth == 0 {
          self.blank_line();
        } else {
          self.newline();
        }
        let mut index: Option<u64> = (el.name() == "ol").then(|| {
          el.attr("start")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1)
        });
        for child in node.children() {
          let is_li = matches!(child.value(), Node::Element(c) if c.name() == "li");
          if !is_li {
            continue;
          }
          let marker = match &mut index {
            Some(i) => {
              let m = format!("{}. ", i);
              *i += 1;
              m
            }
            None => "- ".to_string(),
          };
          let content = self.capture(|w| {
            w.walk_children(
              child,
              Ctx {
                list_depth: ctx.list_depth + 1,
                ..ctx
              },
            )
          });
          let content = content.trim_matches('\n').trim_end();
          let cont_indent = " ".repeat(marker.len());
          self.out.push_str(&marker);
          for (i, line) in content.lines().enumerate() {
            if i > 0 {
              self.out.push('\n');
              self.out.push_str(&cont_indent);
            }
            self.out.push_str(line);
          }
          self.newline();
        }
        if ctx.list_depth == 0 {
          self.blank_line();
        }
      }

      "blockquote" => {
        if ctx.in_cell {
          self.walk_children(node, ctx);
          return;
        }
        let content = self.capture(|w| w.walk_children(node, ctx));
        let content = content.trim_matches('\n');
        if content.is_empty() {
          return;
        }
        self.blank_line();
        for line in content.lines() {
          self.out.push_str("> ");
          self.out.push_str(line);
          self.out.push('\n');
        }
        self.blank_line();
      }

      "table" => {
        if ctx.in_cell {
          self.walk_children(node, ctx);
          return;
        }
        // Caption first, as an emphasized paragraph.
        for child in node.children() {
          if matches!(child.value(), Node::Element(c) if c.name() == "caption") {
            self.inline_wrap(child, ctx, "*");
            self.blank_line();
          }
        }
        let mut rows: Vec<Vec<String>> = Vec::new();
        self.collect_table_rows(node, ctx, &mut rows);
        rows.retain(|r| !r.is_empty());
        if rows.is_empty() {
          return;
        }
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1);
        self.blank_line();
        for (i, row) in rows.iter().enumerate() {
          self.out.push('|');
          for c in 0..cols {
            self.out.push(' ');
            self
              .out
              .push_str(row.get(c).map(String::as_str).unwrap_or(""));
            self.out.push_str(" |");
          }
          self.out.push('\n');
          // GFM requires a delimiter row after the header (first row).
          if i == 0 {
            self.out.push('|');
            for _ in 0..cols {
              self.out.push_str(" --- |");
            }
            self.out.push('\n');
          }
        }
        self.blank_line();
      }

      "iframe" | "embed" => {
        if let Some(url) = self.resolve(el.attr("src")) {
          self.links.push(url.clone());
          if ctx.in_cell {
            self.out.push_str(&format!("[embedded content]({})", url));
          } else {
            self.blank_line();
            self.out.push_str(&format!("[embedded content]({})", url));
            self.blank_line();
          }
        }
      }

      "video" | "audio" => {
        let src = el.attr("src").or_else(|| {
          node.children().find_map(|c| match c.value() {
            Node::Element(s) if s.name() == "source" => s.attr("src"),
            _ => None,
          })
        });
        if let Some(url) = self.resolve(src) {
          self.links.push(url.clone());
          self
            .out
            .push_str(&format!("[{} attachment]({})", el.name(), url));
        }
      }

      // Skipped structurally, handled by their parent element.
      "caption" | "li" | "source" | "track" => {}

      // Everything else (span, small, u, mark, font, picture, …) is transparent.
      _ => self.walk_children(node, ctx),
    }
  }

  /// Emit a text node: collapse whitespace runs and escape Markdown
  /// metacharacters so feed text can't accidentally trigger formatting.
  fn text(&mut self, s: &str, ctx: Ctx) {
    for c in s.chars() {
      if c.is_whitespace() {
        if !self.out.is_empty() && !self.out.ends_with([' ', '\n']) {
          self.out.push(' ');
        }
      } else {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']') || (ctx.in_cell && c == '|') {
          self.out.push('\\');
        }
        self.out.push(c);
      }
    }
  }

  /// Wrap the element's inline content in a Markdown delimiter pair,
  /// keeping the delimiters flush against the content as Markdown requires.
  fn inline_wrap(&mut self, node: NodeRef<Node>, ctx: Ctx, delim: &str) {
    let content = self.capture(|w| w.walk_children(node, ctx));
    let trimmed = content.trim().replace('\n', " ");
    if trimmed.is_empty() {
      return;
    }
    self.out.push_str(delim);
    self.out.push_str(&trimmed);
    self.out.push_str(delim);
    if content.ends_with(' ') {
      self.out.push(' ');
    }
  }

  /// Run `f` with a fresh output buffer and return what it produced.
  fn capture(&mut self, f: impl FnOnce(&mut Self)) -> String {
    let saved = std::mem::take(&mut self.out);
    f(self);
    std::mem::replace(&mut self.out, saved)
  }

  /// Recursively gather table rows from `table`/`thead`/`tbody`/`tfoot`.
  fn collect_table_rows(&mut self, node: NodeRef<Node>, ctx: Ctx, rows: &mut Vec<Vec<String>>) {
    for child in node.children() {
      let Node::Element(el) = child.value() else {
        continue;
      };
      match el.name() {
        "thead" | "tbody" | "tfoot" => self.collect_table_rows(child, ctx, rows),
        "tr" => {
          let mut row = Vec::new();
          for cell in child.children() {
            if matches!(cell.value(), Node::Element(c) if matches!(c.name(), "td" | "th")) {
              let content = self.capture(|w| {
                w.walk_children(
                  cell,
                  Ctx {
                    in_cell: true,
                    ..ctx
                  },
                )
              });
              row.push(collapse_ws(&content));
            }
          }
          rows.push(row);
        }
        _ => {}
      }
    }
  }

  /// Resolve an href/src to an absolute http(s) URL, or `None` if it can't
  /// be resolved or uses another scheme (mailto:, javascript:, …).
  fn resolve(&self, href: Option<&str>) -> Option<String> {
    let href = href?.trim();
    if href.is_empty() {
      return None;
    }
    let url = match Url::parse(href) {
      Ok(u) => u,
      Err(_) => self.base.as_ref()?.join(href).ok()?,
    };
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
  }

  /// Ensure the output ends with a blank line (block separator).
  fn blank_line(&mut self) {
    while self.out.ends_with(' ') {
      self.out.pop();
    }
    if self.out.is_empty() {
      return;
    }
    let trailing = self.out.len() - self.out.trim_end_matches('\n').len();
    for _ in trailing..2 {
      self.out.push('\n');
    }
  }

  /// Separate flattened blocks inside a table cell with a single space.
  fn cell_space(&mut self) {
    if !self.out.is_empty() && !self.out.ends_with([' ', '\n']) {
      self.out.push(' ');
    }
  }

  /// Ensure the output ends with a newline.
  fn newline(&mut self) {
    while self.out.ends_with(' ') {
      self.out.pop();
    }
    if !self.out.is_empty() && !self.out.ends_with('\n') {
      self.out.push('\n');
    }
  }
}

/// Concatenate all descendant text nodes verbatim.
fn collect_text(node: NodeRef<Node>) -> String {
  let mut s = String::new();
  for d in node.descendants() {
    if let Node::Text(t) = d.value() {
      s.push_str(&t.text);
    }
  }
  s
}

/// Collapse whitespace runs to single spaces and trim.
fn collapse_ws(s: &str) -> String {
  s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// First URL of a `srcset` attribute (`url [descriptor], url [descriptor]`).
fn first_srcset_url(srcset: &str) -> Option<&str> {
  srcset
    .split(',')
    .next()?
    .split_whitespace()
    .next()
    .filter(|s| !s.is_empty())
}

/// Final cleanup: strip trailing spaces, collapse 3+ newlines to 2, trim ends.
fn tidy(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  let mut blank_run = 0;
  for line in s.lines() {
    let line = line.trim_end();
    if line.is_empty() {
      blank_run += 1;
      if blank_run > 1 {
        continue;
      }
    } else {
      blank_run = 0;
    }
    out.push_str(line);
    out.push('\n');
  }
  out.trim_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn md(html: &str) -> String {
    html_to_markdown(html, None).markdown
  }

  #[test]
  fn paragraphs_and_inline() {
    let out = md("<p>Hello <strong>bold</strong> and <em>italic</em> text.</p><p>Second.</p>");
    assert_eq!(out, "Hello **bold** and *italic* text.\n\nSecond.");
  }

  #[test]
  fn headings() {
    let out = md("<h1>Top</h1><p>Body</p><h3>Sub</h3>");
    assert_eq!(out, "# Top\n\nBody\n\n### Sub");
  }

  #[test]
  fn links_are_collected_and_rendered() {
    let c = html_to_markdown(
      r#"<p>See <a href="https://example.com/a">first</a> and <a href="https://example.com/b">second</a>.</p>"#,
      None,
    );
    assert_eq!(
      c.markdown,
      "See [first](https://example.com/a) and [second](https://example.com/b)."
    );
    assert_eq!(
      c.links,
      vec!["https://example.com/a", "https://example.com/b"]
    );
  }

  #[test]
  fn empty_anchors_are_dropped() {
    let c = html_to_markdown(
      r##"<h2><a href="#what-s-new"></a>Section title</h2>"##,
      Some("https://example.com/post"),
    );
    assert_eq!(c.markdown, "## Section title");
    assert!(c.links.is_empty());
  }

  #[test]
  fn non_http_links_keep_text_only() {
    let c = html_to_markdown(
      r#"<a href="mailto:x@y.z">mail me</a> <a href="javascript:void(0)">js</a>"#,
      None,
    );
    assert_eq!(c.markdown, "mail me js");
    assert!(c.links.is_empty());
  }

  #[test]
  fn relative_urls_resolve_against_base() {
    let c = html_to_markdown(
      r#"<a href="/page">rel</a><img src="images/pic.png" alt="pic">"#,
      Some("https://blog.example.com/posts/entry.html"),
    );
    assert!(c.markdown.contains("[rel](https://blog.example.com/page)"));
    assert!(c
      .markdown
      .contains("![pic](https://blog.example.com/posts/images/pic.png)"));
    assert_eq!(c.links, vec!["https://blog.example.com/page"]);
  }

  #[test]
  fn images_with_lazy_src() {
    let out = md(r#"<img data-src="https://cdn.example.com/i.jpg" alt="lazy">"#);
    assert_eq!(out, "![lazy](https://cdn.example.com/i.jpg)");
    let out = md(
      r#"<img srcset="https://cdn.example.com/i-480.jpg 480w, https://cdn.example.com/i-800.jpg 800w">"#,
    );
    assert_eq!(out, "![](https://cdn.example.com/i-480.jpg)");
  }

  #[test]
  fn unordered_and_ordered_lists() {
    let out =
      md("<ul><li>one</li><li>two</li></ul><ol start=\"3\"><li>three</li><li>four</li></ol>");
    assert_eq!(out, "- one\n- two\n\n3. three\n4. four");
  }

  #[test]
  fn nested_lists() {
    let out = md("<ul><li>a<ul><li>a1</li><li>a2</li></ul></li><li>b</li></ul>");
    assert_eq!(out, "- a\n  - a1\n  - a2\n- b");
  }

  #[test]
  fn blockquote() {
    let out = md("<blockquote><p>Quoted line.</p></blockquote>");
    assert_eq!(out, "> Quoted line.");
  }

  #[test]
  fn code_blocks_and_inline_code() {
    let out = md("<p>Use <code>foo()</code>:</p><pre><code>fn main() {\n  foo();\n}</code></pre>");
    assert_eq!(out, "Use `foo()`:\n\n```\nfn main() {\n  foo();\n}\n```");
  }

  #[test]
  fn table_with_thead() {
    let out = md(
      "<table><thead><tr><th>Name</th><th>Value</th></tr></thead>\
       <tbody><tr><td>a</td><td>1</td></tr><tr><td>b</td><td>2</td></tr></tbody></table>",
    );
    assert_eq!(out, "| Name | Value |\n| --- | --- |\n| a | 1 |\n| b | 2 |");
  }

  #[test]
  fn table_without_thead_uses_first_row_as_header() {
    let out = md("<table><tr><td>x</td><td>y</td></tr><tr><td>1</td><td>2</td></tr></table>");
    assert_eq!(out, "| x | y |\n| --- | --- |\n| 1 | 2 |");
  }

  #[test]
  fn table_cells_escape_pipes_and_flatten_blocks() {
    let out = md("<table><tr><td>a|b</td><td><p>multi</p><p>block</p></td></tr></table>");
    assert!(out.starts_with("| a\\|b | multi block |"), "got: {out:?}");
  }

  #[test]
  fn markdown_metacharacters_are_escaped() {
    let out = md("<p>5 * 3 = 15 and [brackets] and `ticks`</p>");
    assert_eq!(out, "5 \\* 3 = 15 and \\[brackets\\] and \\`ticks\\`");
  }

  #[test]
  fn script_and_style_are_dropped() {
    let out = md("<p>Visible</p><script>alert(1)</script><style>p{}</style>");
    assert_eq!(out, "Visible");
  }

  #[test]
  fn br_is_hard_break() {
    let out = md("<p>line one<br>line two</p>");
    assert_eq!(out, "line one\\\nline two");
  }

  #[test]
  fn iframe_becomes_embed_link() {
    let c = html_to_markdown(
      r#"<iframe src="https://www.youtube.com/embed/abc123"></iframe>"#,
      None,
    );
    assert_eq!(
      c.markdown,
      "[embedded content](https://www.youtube.com/embed/abc123)"
    );
    assert_eq!(c.links, vec!["https://www.youtube.com/embed/abc123"]);
  }

  #[test]
  fn malformed_html_does_not_panic() {
    let out = md("<p>unclosed <b>bold <table><tr><td>cell");
    assert!(out.contains("unclosed"));
    assert!(out.contains("cell"));
  }

  #[test]
  fn plain_divs_separate_blocks() {
    let out = md("<div>first</div><div>second</div>");
    assert_eq!(out, "first\n\nsecond");
  }
}

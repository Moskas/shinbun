use ratatui::prelude::{Color, Modifier, Style};
use serde::Deserialize;

/// Parses a color string into a ratatui Color.
///
/// Supports:
/// - Named colors: "black", "red", "green", "yellow", "blue", "magenta", "cyan",
///   "gray", "darkgray", "lightred", "lightgreen", "lightyellow", "lightblue",
///   "lightmagenta", "lightcyan", "white"
/// - Hex RGB: "#rrggbb" (e.g. "#ff0000")
/// - RGB tuple: "rgb(r,g,b)" (e.g. "rgb(255,0,0)")
/// - ANSI 256 index: "0"–"255"
fn parse_color(s: &str) -> Color {
  let s = s.trim().to_lowercase();
  match s.as_str() {
    "black" => Color::Black,
    "red" => Color::Red,
    "green" => Color::Green,
    "yellow" => Color::Yellow,
    "blue" => Color::Blue,
    "magenta" => Color::Magenta,
    "cyan" => Color::Cyan,
    "gray" | "grey" => Color::Gray,
    "darkgray" | "darkgrey" | "dark_gray" | "dark_grey" => Color::DarkGray,
    "lightred" | "light_red" => Color::LightRed,
    "lightgreen" | "light_green" => Color::LightGreen,
    "lightyellow" | "light_yellow" => Color::LightYellow,
    "lightblue" | "light_blue" => Color::LightBlue,
    "lightmagenta" | "light_magenta" => Color::LightMagenta,
    "lightcyan" | "light_cyan" => Color::LightCyan,
    "white" => Color::White,
    "reset" | "default" => Color::Reset,
    _ => {
      // Try hex: #rrggbb
      if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
          if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
          ) {
            return Color::Rgb(r, g, b);
          }
        }
      }
      // Try rgb(r,g,b)
      if let Some(inner) = s.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() == 3 {
          if let (Ok(r), Ok(g), Ok(b)) = (
            parts[0].trim().parse::<u8>(),
            parts[1].trim().parse::<u8>(),
            parts[2].trim().parse::<u8>(),
          ) {
            return Color::Rgb(r, g, b);
          }
        }
      }
      // Try ANSI 256 index
      if let Ok(idx) = s.parse::<u8>() {
        return Color::Indexed(idx);
      }
      // Fallback
      Color::Reset
    }
  }
}

// ─── Serde-friendly config struct ────────────────────────────────────────────

/// Deserializable theme configuration from `[ui.theme]` in config.toml.
/// All fields are optional strings; missing values fall back to built-in defaults.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct ThemeConfig {
  // ── Global / chrome ───────────────────────────────────────────────────────
  /// Border color for main outer blocks
  #[serde(default)]
  pub border: Option<String>,
  /// Title text color (bold yellow by default)
  #[serde(default)]
  pub title: Option<String>,

  // ── Feed / tag list ───────────────────────────────────────────────────────
  /// Feed/tag count column color
  #[serde(default)]
  pub count: Option<String>,
  /// Read (dimmed) item foreground
  #[serde(default)]
  pub read: Option<String>,
  /// Table header style (bold by default, this sets foreground if desired)
  #[serde(default)]
  pub header: Option<String>,
  /// Row highlight background
  #[serde(default)]
  pub highlight_bg: Option<String>,
  /// Row highlight foreground
  #[serde(default)]
  pub highlight_fg: Option<String>,

  // ── Entry list ────────────────────────────────────────────────────────────
  /// Unread entry date color
  #[serde(default)]
  pub date: Option<String>,
  /// Unread entry source/feed-title color
  #[serde(default)]
  pub source: Option<String>,

  // ── Search ────────────────────────────────────────────────────────────────
  /// Search prompt color
  #[serde(default)]
  pub search_prompt: Option<String>,
  /// Search cursor color
  #[serde(default)]
  pub search_cursor: Option<String>,
  /// Search match info color
  #[serde(default)]
  pub search_info: Option<String>,
  /// Search no-matches indicator color
  #[serde(default)]
  pub search_no_match: Option<String>,
  /// Foreground for search-matching read entries
  #[serde(default)]
  pub search_match_read: Option<String>,
  /// Foreground for non-matching entries during search
  #[serde(default)]
  pub search_dim: Option<String>,

  // ── Entry view (article reading) ──────────────────────────────────────────
  /// Metadata "Title:" line color
  #[serde(default)]
  pub meta_title: Option<String>,
  /// Metadata "Feed:" line color
  #[serde(default)]
  pub meta_feed: Option<String>,
  /// Metadata "Published:" line color
  #[serde(default)]
  pub meta_published: Option<String>,
  /// Metadata "Link:"/"Media:" line color
  #[serde(default)]
  pub meta_link: Option<String>,
  /// Line info color in entry view
  #[serde(default)]
  pub line_info: Option<String>,
  /// Entry block title color (with borders)
  #[serde(default)]
  pub entry_title_bordered: Option<String>,
  /// Entry block title color (without borders)
  #[serde(default)]
  pub entry_title_plain: Option<String>,

  // ── Markdown heading styles ───────────────────────────────────────────────
  #[serde(default)]
  pub h1: Option<String>,
  #[serde(default)]
  pub h2: Option<String>,
  #[serde(default)]
  pub h3: Option<String>,
  #[serde(default)]
  pub h4: Option<String>,
  #[serde(default)]
  pub h5: Option<String>,
  /// Inline code color
  #[serde(default)]
  pub code: Option<String>,
  /// Link color
  #[serde(default)]
  pub link: Option<String>,
  /// Metadata block color (tui-markdown)
  #[serde(default)]
  pub metadata_block: Option<String>,

  // ── Popups ────────────────────────────────────────────────────────────────
  /// Error popup border color
  #[serde(default)]
  pub error_border: Option<String>,
  /// Error popup title color
  #[serde(default)]
  pub error_title: Option<String>,
  /// Loading popup border (while loading)
  #[serde(default)]
  pub loading_border: Option<String>,
  /// Loading popup border (done)
  #[serde(default)]
  pub loading_done_border: Option<String>,
  /// Confirm popup border/title color
  #[serde(default)]
  pub confirm: Option<String>,
  /// Confirm popup message foreground
  #[serde(default)]
  pub confirm_message: Option<String>,

  // ── Help popup ────────────────────────────────────────────────────────────
  /// Help section header color
  #[serde(default)]
  pub help_section: Option<String>,
  /// Help keybind key column color
  #[serde(default)]
  pub help_key: Option<String>,
  /// Help popup border color (falls back to `border` then blue)
  #[serde(default)]
  pub help_border: Option<String>,

  // ── Links popup ───────────────────────────────────────────────────────────
  /// Link index number color
  #[serde(default)]
  pub link_index: Option<String>,
  /// Link URL color
  #[serde(default)]
  pub link_url: Option<String>,
  /// Links popup border color (falls back to `border` then blue)
  #[serde(default)]
  pub links_border: Option<String>,
}

// ─── Resolved theme (ratatui Colors) ─────────────────────────────────────────

/// Fully resolved theme with concrete `Color` values, ready for use in rendering.
#[derive(Debug, Clone)]
pub struct Theme {
  // ── Chrome ──
  pub border: Color,
  pub title: Color,

  // ── Feed/tag list ──
  pub count: Color,
  pub read: Color,
  pub header: Option<Color>,
  pub highlight_bg: Color,
  pub highlight_fg: Color,

  // ── Entry list ──
  pub date: Color,
  pub source: Color,

  // ── Search ──
  pub search_prompt: Color,
  pub search_cursor: Color,
  pub search_info: Color,
  pub search_no_match: Color,
  pub search_match_read: Color,
  pub search_dim: Color,

  // ── Entry view ──
  pub meta_title: Color,
  pub meta_feed: Color,
  pub meta_published: Color,
  pub meta_link: Color,
  pub line_info: Color,
  pub entry_title_bordered: Color,
  pub entry_title_plain: Color,

  // ── Markdown ──
  pub h1: Color,
  pub h2: Color,
  pub h3: Color,
  pub h4: Color,
  pub h5: Color,
  pub code: Option<Color>,
  pub link: Color,
  pub metadata_block: Color,

  // ── Popups ──
  pub error_border: Color,
  pub error_title: Color,
  pub loading_border: Color,
  pub loading_done_border: Color,
  pub confirm: Color,
  pub confirm_message: Color,

  // ── Help ──
  pub help_section: Color,
  pub help_key: Color,
  pub help_border: Color,

  // ── Links ──
  pub link_index: Color,
  pub link_url: Color,
  pub links_border: Color,
}

impl Theme {
  pub fn from_config(cfg: &ThemeConfig) -> Self {
    let resolve = |opt: &Option<String>, default: Color| -> Color {
      opt.as_ref().map(|s| parse_color(s)).unwrap_or(default)
    };

    let border = resolve(&cfg.border, Color::Blue);

    Self {
      border,
      title: resolve(&cfg.title, Color::Yellow),

      count: resolve(&cfg.count, Color::Blue),
      read: resolve(&cfg.read, Color::DarkGray),
      header: cfg.header.as_ref().map(|s| parse_color(s)),
      highlight_bg: resolve(&cfg.highlight_bg, Color::DarkGray),
      highlight_fg: resolve(&cfg.highlight_fg, Color::Yellow),

      date: resolve(&cfg.date, Color::Cyan),
      source: resolve(&cfg.source, Color::Yellow),

      search_prompt: resolve(&cfg.search_prompt, Color::Yellow),
      search_cursor: resolve(&cfg.search_cursor, Color::Gray),
      search_info: resolve(&cfg.search_info, Color::DarkGray),
      search_no_match: resolve(&cfg.search_no_match, Color::Red),
      search_match_read: resolve(&cfg.search_match_read, Color::Gray),
      search_dim: resolve(&cfg.search_dim, Color::DarkGray),

      meta_title: resolve(&cfg.meta_title, Color::Magenta),
      meta_feed: resolve(&cfg.meta_feed, Color::Cyan),
      meta_published: resolve(&cfg.meta_published, Color::Yellow),
      meta_link: resolve(&cfg.meta_link, Color::Blue),
      line_info: resolve(&cfg.line_info, Color::Yellow),
      entry_title_bordered: resolve(&cfg.entry_title_bordered, Color::Yellow),
      entry_title_plain: resolve(&cfg.entry_title_plain, Color::Green),

      h1: resolve(&cfg.h1, Color::Cyan),
      h2: resolve(&cfg.h2, Color::Magenta),
      h3: resolve(&cfg.h3, Color::Blue),
      h4: resolve(&cfg.h4, Color::Red),
      h5: resolve(&cfg.h5, Color::LightCyan),
      code: cfg.code.as_ref().map(|s| parse_color(s)),
      link: resolve(&cfg.link, Color::Blue),
      metadata_block: resolve(&cfg.metadata_block, Color::LightYellow),

      error_border: resolve(&cfg.error_border, Color::Red),
      error_title: resolve(&cfg.error_title, Color::Yellow),
      loading_border: resolve(&cfg.loading_border, Color::Cyan),
      loading_done_border: resolve(&cfg.loading_done_border, Color::Green),
      confirm: resolve(&cfg.confirm, Color::Yellow),
      confirm_message: resolve(&cfg.confirm_message, Color::White),

      help_section: resolve(&cfg.help_section, Color::Cyan),
      help_key: resolve(&cfg.help_key, Color::Yellow),
      help_border: resolve(&cfg.help_border, border),

      link_index: resolve(&cfg.link_index, Color::Yellow),
      link_url: resolve(&cfg.link_url, Color::Blue),
      links_border: resolve(&cfg.links_border, border),
    }
  }

  // ─── Style helpers ──────────────────────────────────────────────────────

  /// Style for the main title (bold + title color).
  pub fn title_style(&self) -> Style {
    Style::default().fg(self.title).add_modifier(Modifier::BOLD)
  }

  /// Style for outer block borders.
  pub fn border_style(&self) -> Style {
    Style::new().fg(self.border)
  }

  /// Style for table row highlight.
  pub fn row_highlight_style(&self) -> Style {
    Style::default().bg(self.highlight_bg).fg(self.highlight_fg)
  }

  /// Style for table row highlight (bold variant, used for entry rows).
  pub fn row_highlight_bold_style(&self) -> Style {
    self.row_highlight_style().add_modifier(Modifier::BOLD)
  }

  /// Style for table header row.
  pub fn header_style(&self) -> Style {
    let s = Style::new().add_modifier(Modifier::BOLD);
    match self.header {
      Some(c) => s.fg(c),
      None => s,
    }
  }

  /// Style for read/dimmed items.
  pub fn read_style(&self) -> Style {
    Style::default().fg(self.read)
  }
}

impl Default for Theme {
  fn default() -> Self {
    Self::from_config(&ThemeConfig::default())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_named_colors() {
    assert_eq!(parse_color("red"), Color::Red);
    assert_eq!(parse_color("Blue"), Color::Blue);
    assert_eq!(parse_color("DARKGRAY"), Color::DarkGray);
    assert_eq!(parse_color("dark_gray"), Color::DarkGray);
    assert_eq!(parse_color("light_cyan"), Color::LightCyan);
    assert_eq!(parse_color("reset"), Color::Reset);
    assert_eq!(parse_color("default"), Color::Reset);
  }

  #[test]
  fn test_parse_hex_color() {
    assert_eq!(parse_color("#ff0000"), Color::Rgb(255, 0, 0));
    assert_eq!(parse_color("#00ff00"), Color::Rgb(0, 255, 0));
    assert_eq!(parse_color("#0000FF"), Color::Rgb(0, 0, 255));
    assert_eq!(parse_color("#1a2b3c"), Color::Rgb(26, 43, 60));
  }

  #[test]
  fn test_parse_rgb_color() {
    assert_eq!(parse_color("rgb(255,0,0)"), Color::Rgb(255, 0, 0));
    assert_eq!(parse_color("rgb(0, 128, 255)"), Color::Rgb(0, 128, 255));
  }

  #[test]
  fn test_parse_ansi_index() {
    assert_eq!(parse_color("42"), Color::Indexed(42));
    assert_eq!(parse_color("0"), Color::Indexed(0));
    assert_eq!(parse_color("255"), Color::Indexed(255));
  }

  #[test]
  fn test_parse_invalid_falls_back() {
    assert_eq!(parse_color("notacolor"), Color::Reset);
    assert_eq!(parse_color("#xyz"), Color::Reset);
  }

  #[test]
  fn test_default_theme_matches_hardcoded_values() {
    let theme = Theme::default();
    assert_eq!(theme.border, Color::Blue);
    assert_eq!(theme.title, Color::Yellow);
    assert_eq!(theme.count, Color::Blue);
    assert_eq!(theme.read, Color::DarkGray);
    assert_eq!(theme.highlight_bg, Color::DarkGray);
    assert_eq!(theme.highlight_fg, Color::Yellow);
    assert_eq!(theme.date, Color::Cyan);
    assert_eq!(theme.source, Color::Yellow);
    assert_eq!(theme.error_border, Color::Red);
    assert_eq!(theme.h1, Color::Cyan);
    assert_eq!(theme.h2, Color::Magenta);
  }

  #[test]
  fn test_theme_from_partial_config() {
    let cfg = ThemeConfig {
      border: Some("red".to_string()),
      title: Some("#00ff00".to_string()),
      ..Default::default()
    };
    let theme = Theme::from_config(&cfg);
    assert_eq!(theme.border, Color::Red);
    assert_eq!(theme.title, Color::Rgb(0, 255, 0));
    // Unset fields keep defaults
    assert_eq!(theme.count, Color::Blue);
    assert_eq!(theme.read, Color::DarkGray);
  }

  #[test]
  fn test_theme_style_helpers() {
    let theme = Theme::default();
    let title_style = theme.title_style();
    assert_eq!(title_style.fg, Some(Color::Yellow));

    let border_style = theme.border_style();
    assert_eq!(border_style.fg, Some(Color::Blue));

    let highlight = theme.row_highlight_style();
    assert_eq!(highlight.bg, Some(Color::DarkGray));
    assert_eq!(highlight.fg, Some(Color::Yellow));
  }
}

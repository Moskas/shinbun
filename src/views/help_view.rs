use ratatui::{
  prelude::*,
  symbols::border,
  widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Widget, Wrap,
  },
};

/// A single keybind entry for the help popup.
struct Keybind {
  key: &'static str,
  description: &'static str,
}

/// Build the full list of keybind lines grouped by section.
fn build_help_content() -> Vec<Line<'static>> {
  let mut lines: Vec<Line<'static>> = Vec::new();

  // ── Navigation ──
  lines.push(Line::from(" Navigation").bold().cyan());
  lines.push(Line::from(""));

  let nav_binds = [
    Keybind {
      key: "j / Down",
      description: "Move down",
    },
    Keybind {
      key: "k / Up",
      description: "Move up",
    },
    Keybind {
      key: "l / Enter / Right",
      description: "Open / Enter",
    },
    Keybind {
      key: "h / Backspace / Left",
      description: "Go back",
    },
    Keybind {
      key: "g g / Home",
      description: "Go to top",
    },
    Keybind {
      key: "G / End",
      description: "Go to bottom",
    },
  ];
  for bind in &nav_binds {
    lines.push(keybind_line(bind));
  }

  // ── Actions ──
  lines.push(Line::from(""));
  lines.push(Line::from(" Actions").bold().cyan());
  lines.push(Line::from(""));

  let action_binds = [
    Keybind {
      key: "r",
      description: "Refresh selected feed",
    },
    Keybind {
      key: "R",
      description: "Refresh all feeds",
    },
    Keybind {
      key: "m",
      description: "Toggle read / unread",
    },
    Keybind {
      key: "A",
      description: "Mark feed as read",
    },
    Keybind {
      key: "u",
      description: "Toggle hide read entries",
    },
    Keybind {
      key: "o",
      description: "Open entry in browser",
    },
    Keybind {
      key: "p",
      description: "Play media attachment",
    },
    Keybind {
      key: "e",
      description: "Show feed errors",
    },
  ];
  for bind in &action_binds {
    lines.push(keybind_line(bind));
  }

  // ── General ──
  lines.push(Line::from(""));
  lines.push(Line::from(" General").bold().cyan());
  lines.push(Line::from(""));

  let general_binds = [
    Keybind {
      key: "?",
      description: "Toggle this help",
    },
    Keybind {
      key: "q",
      description: "Quit",
    },
  ];
  for bind in &general_binds {
    lines.push(keybind_line(bind));
  }

  lines
}

/// Format a single keybind as a styled Line.
fn keybind_line(bind: &Keybind) -> Line<'static> {
  Line::from(vec![
    Span::styled(
      format!("  {:24}", bind.key),
      Style::default().bold().yellow(),
    ),
    Span::raw(bind.description.to_string()),
  ])
}

/// Render a centered, scrollable help popup.
pub fn render_help_popup(frame: &mut Frame, area: Rect, scroll: &mut usize) {
  let popup_width = area.width.saturating_sub(8).min(60);
  let popup_height = area.height.saturating_sub(6).min(30);

  let popup_area = Rect {
    x: area.x + (area.width.saturating_sub(popup_width)) / 2,
    y: area.y + (area.height.saturating_sub(popup_height)) / 2,
    width: popup_width,
    height: popup_height,
  };

  Clear.render(popup_area, frame.buffer_mut());

  let content = build_help_content();
  let content_len = content.len();

  let block = Block::default()
    .title(" Keyboard Shortcuts ".bold().yellow())
    .title_bottom(" <?> or <Esc> to close ".gray())
    .borders(Borders::ALL)
    .border_style(Style::new().blue())
    .border_set(border::PLAIN);

  let inner_height = block.inner(popup_area).height as usize;
  let max_scroll = content_len.saturating_sub(inner_height);
  *scroll = (*scroll).min(max_scroll);

  let paragraph = Paragraph::new(content)
    .block(block)
    .scroll((*scroll as u16, 0))
    .wrap(Wrap { trim: false });

  paragraph.render(popup_area, frame.buffer_mut());

  // Scrollbar when content overflows
  if content_len > inner_height {
    let scrollbar_area = Rect {
      x: popup_area.x + popup_area.width.saturating_sub(1),
      y: popup_area.y + 1,
      width: 1,
      height: popup_area.height.saturating_sub(2),
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

  #[test]
  fn test_build_help_content_not_empty() {
    let content = build_help_content();
    assert!(!content.is_empty());
  }

  #[test]
  fn test_build_help_content_has_sections() {
    let content = build_help_content();
    let text: Vec<String> = content.iter().map(|l| l.to_string()).collect();
    assert!(text.iter().any(|l| l.contains("Navigation")));
    assert!(text.iter().any(|l| l.contains("Actions")));
    assert!(text.iter().any(|l| l.contains("General")));
  }

  #[test]
  fn test_build_help_content_has_keybinds() {
    let content = build_help_content();
    let text: Vec<String> = content.iter().map(|l| l.to_string()).collect();
    // Check some essential keybinds are present
    assert!(text.iter().any(|l| l.contains("Move down")));
    assert!(text.iter().any(|l| l.contains("Move up")));
    assert!(text.iter().any(|l| l.contains("Quit")));
    assert!(text.iter().any(|l| l.contains("Toggle this help")));
    assert!(text.iter().any(|l| l.contains("Refresh")));
    assert!(text.iter().any(|l| l.contains("browser")));
    assert!(text.iter().any(|l| l.contains("Mark feed as read")));
  }

  #[test]
  fn test_keybind_line_format() {
    let bind = Keybind {
      key: "j / Down",
      description: "Move down",
    };
    let line = keybind_line(&bind);
    let text = line.to_string();
    assert!(text.contains("j / Down"));
    assert!(text.contains("Move down"));
  }

  #[test]
  fn test_keybind_line_alignment() {
    let bind = Keybind {
      key: "q",
      description: "Quit",
    };
    let line = keybind_line(&bind);
    // The key should be padded to 24 chars
    let spans: Vec<String> = line.spans.iter().map(|s| s.content.to_string()).collect();
    assert_eq!(spans[0].len(), 26); // "  " prefix + 24 padded key
  }
}

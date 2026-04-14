use super::App;
use std::process::Command;

impl App {
  // Browser / player helpers
  pub(super) fn spawn_cmd(cmd: &str, url: &str) -> Result<(), String> {
    let mut parts = cmd.split_whitespace();
    if let Some(bin) = parts.next() {
      let args: Vec<&str> = parts.collect();
      Command::new(bin)
        .args(args)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to launch '{}': {}", cmd, e))?;
    }
    Ok(())
  }

  /// Open the first link of the currently selected entry in a browser.
  pub(super) fn open_current_entry_in_browser(&mut self) {
    let Some(real_idx) = self.resolve_current_entry_idx() else {
      return;
    };
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let Some(entry) = df.entries(&self.feeds).get(real_idx) else {
      return;
    };
    let Some(url) = entry.links.first() else {
      return;
    };

    let result = if let Some(ref cmd) = self.general_config.browser {
      Self::spawn_cmd(cmd, url)
    } else {
      open::that(url).map_err(|e| format!("Failed to open URL in default browser: {}", e))
    };
    if let Err(e) = result {
      self.push_error("Browser", e);
    }
  }

  /// Open the currently selected link from the links popup in a browser.
  pub(super) fn open_selected_link(&mut self) {
    let Some(real_idx) = self.resolve_current_entry_idx() else {
      return;
    };
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let Some(entry) = df.entries(&self.feeds).get(real_idx) else {
      return;
    };
    let selected = self.links_selected.min(entry.links.len().saturating_sub(1));
    let Some(url) = entry.links.get(selected) else {
      return;
    };

    let result = if let Some(ref cmd) = self.general_config.browser {
      Self::spawn_cmd(cmd, url)
    } else {
      open::that(url).map_err(|e| format!("Failed to open URL in default browser: {}", e))
    };
    if let Err(e) = result {
      self.push_error("Browser", e);
    }
  }

  /// Write text to the system clipboard, suppressing arboard's Drop warning.
  pub(super) fn copy_to_clipboard(text: &str) -> Result<(), arboard::Error> {
    let mut cb = arboard::Clipboard::new()?;
    cb.set_text(text)?;
    // Forget the clipboard to prevent its Drop impl from printing to stderr
    // on X11 when dropped within 100ms of writing (arboard warns about
    // clipboard managers potentially missing the content).
    std::mem::forget(cb);
    Ok(())
  }

  /// Yank the current entry's first link to the clipboard.
  pub(super) fn yank_entry_link(&mut self) {
    let Some(real_idx) = self.resolve_current_entry_idx() else {
      return;
    };
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let Some(entry) = df.entries(&self.feeds).get(real_idx) else {
      return;
    };
    let Some(url) = entry.links.first() else {
      return;
    };
    if let Err(e) = Self::copy_to_clipboard(url) {
      self.push_error("Clipboard", format!("Failed to copy link: {}", e));
    }
  }

  /// Yank the selected link from the links popup to the clipboard.
  pub(super) fn yank_selected_link(&mut self) {
    let Some(real_idx) = self.resolve_current_entry_idx() else {
      return;
    };
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let Some(entry) = df.entries(&self.feeds).get(real_idx) else {
      return;
    };
    let selected = self.links_selected.min(entry.links.len().saturating_sub(1));
    let Some(url) = entry.links.get(selected) else {
      return;
    };
    if let Err(e) = Self::copy_to_clipboard(url) {
      self.push_error("Clipboard", format!("Failed to copy link: {}", e));
    }
  }

  /// Open the media attachment of the currently selected entry.
  pub(super) fn open_media_in_player(&mut self) {
    let Some(real_idx) = self.resolve_current_entry_idx() else {
      return;
    };
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let Some(entry) = df.entries(&self.feeds).get(real_idx) else {
      return;
    };
    let Some(url) = &entry.media else { return };

    let result = if let Some(ref cmd) = self.general_config.media_player {
      Self::spawn_cmd(cmd, url)
    } else {
      open::that(url).map_err(|e| format!("Failed to open media URL with OS default: {}", e))
    };
    if let Err(e) = result {
      self.push_error("Media player", e);
    }
  }
}

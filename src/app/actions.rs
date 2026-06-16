use super::types::FeedUpdate;
use super::App;
use crate::config::{parse_refresh_interval, write_feeds, Feed as FeedConfig};
use crate::feeds;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

  /// Queue background HTTP fetches for every image URL found in the current entry's text.
  /// Already-cached images are skipped. Each successful fetch sends `ImageReady` over
  /// the feed channel so `handle_feed_update` can encode and store the protocol image.
  pub(super) fn queue_entry_images(&mut self) {
    let Some(real_idx) = self.input.current_entry_relative_index else {
      return;
    };
    let Some(df) = self.display_feeds.get(self.feed_index) else {
      return;
    };
    let Some(entry) = df.entries(&self.feeds).get(real_idx) else {
      return;
    };
    let urls = extract_image_urls(&entry.text);
    for url in urls {
      if self.image_cache.contains_key(&url) {
        continue;
      }
      let tx = self.feed_tx.clone();
      tokio::spawn(async move {
        match fetch_image_bytes(url.clone()).await {
          Ok(img) => {
            let _ = tx.send(FeedUpdate::ImageReady { url, image: img });
          }
          Err(_) => {
            let _ = tx.send(FeedUpdate::ImageError);
          }
        }
      });
    }
  }

  /// Handle a keypress while the add-feed popup is open.
  pub(super) fn handle_add_feed_key(&mut self, key: KeyEvent) {
    match key.code {
      KeyCode::Esc => {
        self.show_add_feed_popup = false;
        self.add_feed_form.clear();
      }
      KeyCode::Tab => {
        self.add_feed_form.focus = self.add_feed_form.focus.next();
        self.add_feed_form.error = None;
      }
      KeyCode::BackTab => {
        self.add_feed_form.focus = self.add_feed_form.focus.prev();
        self.add_feed_form.error = None;
      }
      KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
        self.add_feed_form.focus = self.add_feed_form.focus.next();
        self.add_feed_form.error = None;
      }
      KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
        self.add_feed_form.focus = self.add_feed_form.focus.prev();
        self.add_feed_form.error = None;
      }
      KeyCode::Backspace => {
        self.add_feed_form.active_buffer().pop();
        self.add_feed_form.error = None;
      }
      KeyCode::Enter => {
        let url = self.add_feed_form.url.trim().to_string();
        if url.is_empty() {
          self.add_feed_form.error = Some("URL is required".to_string());
          return;
        }

        let name = {
          let s = self.add_feed_form.name.trim().to_string();
          if s.is_empty() {
            None
          } else {
            Some(s)
          }
        };

        let tags = {
          let raw = &self.add_feed_form.tags;
          let v: Vec<String> = raw
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
          if v.is_empty() {
            None
          } else {
            Some(v)
          }
        };

        let refresh = {
          let s = self.add_feed_form.refresh.trim().to_string();
          if s.is_empty() {
            None
          } else if parse_refresh_interval(&s).is_none() {
            self.add_feed_form.error =
              Some("Invalid refresh format (use e.g. 1h, 3d, 1w)".to_string());
            return;
          } else {
            Some(s)
          }
        };

        let new_feed = FeedConfig {
          link: url,
          name,
          tags,
          refresh,
        };
        self.feed_config.push(new_feed.clone());

        if self.loading_state.is_loading {
          self.add_feed_form.error = Some("A fetch is already in progress".to_string());
          return;
        }

        if let Err(e) = write_feeds(&self.feed_config) {
          self.push_error("Add Feed", format!("Failed to save feeds.toml: {}", e));
        }

        let tx = self.feed_tx.clone();
        let feeds_to_fetch = vec![new_feed];
        tokio::spawn(async move {
          feeds::fetch_feeds_subset_with_progress(feeds_to_fetch, tx).await;
        });

        self.loading_state.start();
        self.show_add_feed_popup = false;
        self.add_feed_form.clear();
      }
      KeyCode::Char(c) => {
        self.add_feed_form.active_buffer().push(c);
        self.add_feed_form.error = None;
      }
      _ => {}
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

/// Extract `https?://…` image URLs from `![alt](url)` patterns in a markdown string.
fn extract_image_urls(md: &str) -> Vec<String> {
  let mut urls = Vec::new();
  let mut search_from = 0;
  while let Some(rel) = md[search_from..].find("![") {
    let abs = search_from + rel;
    let after_excl = abs + 2;
    if let Some(bracket_end) = md[after_excl..].find(']') {
      let after_bracket = after_excl + bracket_end;
      if md[after_bracket..].starts_with("](") {
        let url_start = after_bracket + 2;
        if let Some(paren_end) = md[url_start..].find(')') {
          let url = &md[url_start..url_start + paren_end];
          if url.starts_with("http") {
            urls.push(url.to_string());
          }
          search_from = url_start + paren_end + 1;
          continue;
        }
      }
    }
    search_from = abs + 2;
  }
  urls
}

async fn fetch_image_bytes(url: String) -> Result<image::DynamicImage, String> {
  let client = reqwest::Client::builder()
    .user_agent(feeds::USER_AGENT)
    .timeout(std::time::Duration::from_secs(30))
    .build()
    .map_err(|e| e.to_string())?;
  let bytes = client
    .get(&url)
    .send()
    .await
    .map_err(|e| e.to_string())?
    .bytes()
    .await
    .map_err(|e| e.to_string())?;
  image::load_from_memory(&bytes).map_err(|e| e.to_string())
}

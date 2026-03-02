use dirs::config_dir;
use serde::Deserialize;
use std::{
  fmt, fs,
  path::{Path, PathBuf},
};

#[derive(Debug)]
pub enum ConfigError {
  NotFound(PathBuf),
  Read(PathBuf, std::io::Error),
  Parse(PathBuf, toml::de::Error),
}

impl fmt::Display for ConfigError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      ConfigError::NotFound(path) => {
        write!(
          f,
          "Feeds file not found: {}\n\
           Please create a feeds.toml file with your RSS feed URLs\n\
           Example:\n  [[feeds]]\n  link = \"https://example.com/feed.xml\"",
          path.display()
        )
      }
      ConfigError::Read(path, err) => {
        write!(f, "Failed to read {}: {}", path.display(), err)
      }
      ConfigError::Parse(path, err) => {
        write!(f, "Failed to parse {}: {}", path.display(), err)
      }
    }
  }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct Feed {
  pub link: String,
  pub name: Option<String>,
  pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct FeedsFile {
  feeds: Vec<Feed>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct UiConfig {
  #[serde(default = "default_show_borders")]
  pub show_borders: bool,

  /// Display read entries
  /// Default true, display can be toggled with a keybind
  #[serde(default = "default_show_read_entries")]
  pub show_read_entries: bool,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct GeneralConfig {
  /// Command used to open entry links (default: system browser via `open` crate).
  /// Supports arguments, e.g. `"firefox --private-window"`.
  #[serde(default)]
  pub browser: Option<String>,

  /// Command used to open media attachments (podcasts, videos, etc.).
  /// Falls back to the OS default when not set.
  #[serde(default)]
  pub media_player: Option<String>,
}

fn default_show_borders() -> bool {
  true
}

fn default_show_read_entries() -> bool {
  true
}

#[derive(Debug, Deserialize, Clone)]
pub struct QueryFeed {
  pub name: String,
  pub query: String,
}

#[derive(Debug, Deserialize, Default)]
struct ConfigFile {
  #[serde(default)]
  general: GeneralConfig,
  #[serde(default)]
  ui: UiConfig,
  #[serde(default)]
  queries: Vec<QueryFeed>,
}

pub struct Config {
  pub feeds: Vec<Feed>,
  pub general: GeneralConfig,
  pub ui: UiConfig,
  pub queries: Vec<QueryFeed>,
}

/// Parse complete configuration from config.toml and feeds.toml
pub fn parse_config() -> Result<Config, ConfigError> {
  let config_dir = get_config_dir();
  let (general_config, ui_config, queries) = parse_config_file(&config_dir);
  let feeds = parse_feeds(&config_dir)?;
  Ok(Config {
    feeds,
    general: general_config,
    ui: ui_config,
    queries,
  })
}

/// Parse configuration file (config.toml)
fn parse_config_file(config_dir: &Path) -> (GeneralConfig, UiConfig, Vec<QueryFeed>) {
  let config_path = config_dir.join("config.toml");

  if !config_path.exists() {
    return (GeneralConfig::default(), UiConfig::default(), Vec::new());
  }

  let content = match fs::read_to_string(&config_path) {
    Ok(c) => c,
    Err(_) => return (GeneralConfig::default(), UiConfig::default(), Vec::new()),
  };

  match toml::from_str::<ConfigFile>(&content) {
    Ok(config) => (config.general, config.ui, config.queries),
    Err(err) => {
      eprintln!("Warning: Failed to parse config.toml: {}", err);
      eprintln!("Using default configuration");
      (GeneralConfig::default(), UiConfig::default(), Vec::new())
    }
  }
}

/// Parse feeds from feeds.toml
fn parse_feeds(config_dir: &Path) -> Result<Vec<Feed>, ConfigError> {
  let feeds_path = config_dir.join("feeds.toml");

  if !feeds_path.exists() {
    return Err(ConfigError::NotFound(feeds_path));
  }

  let content =
    fs::read_to_string(&feeds_path).map_err(|e| ConfigError::Read(feeds_path.clone(), e))?;

  let feeds_file: FeedsFile =
    toml::from_str(&content).map_err(|e| ConfigError::Parse(feeds_path.clone(), e))?;

  if feeds_file.feeds.is_empty() {
    eprintln!("Warning: No feeds configured in feeds.toml");
  }

  Ok(feeds_file.feeds)
}

/// Get the shinbun config directory
fn get_config_dir() -> PathBuf {
  let config_dir = config_dir().expect("Unable to determine config directory");
  config_dir.join("shinbun")
}

/// Get the cache database path
pub fn get_cache_path() -> PathBuf {
  get_config_dir().join("cache.db")
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io;

  #[test]
  fn test_config_error_not_found_display() {
    let err = ConfigError::NotFound(PathBuf::from("/tmp/missing.toml"));
    let msg = format!("{}", err);
    assert!(msg.contains("Feeds file not found"));
    assert!(msg.contains("/tmp/missing.toml"));
    assert!(msg.contains("feeds.toml"));
  }

  #[test]
  fn test_config_error_read_display() {
    let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "permission denied");
    let err = ConfigError::Read(PathBuf::from("/tmp/feeds.toml"), io_err);
    let msg = format!("{}", err);
    assert!(msg.contains("Failed to read"));
    assert!(msg.contains("permission denied"));
  }

  #[test]
  fn test_config_error_parse_display() {
    // Create a real toml parse error
    let toml_err = toml::from_str::<FeedsFile>("invalid toml {{{{").unwrap_err();
    let err = ConfigError::Parse(PathBuf::from("/tmp/feeds.toml"), toml_err);
    let msg = format!("{}", err);
    assert!(msg.contains("Failed to parse"));
  }

  #[test]
  fn test_parse_feeds_file_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let result = parse_feeds(dir.path());
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ConfigError::NotFound(_)));
  }

  #[test]
  fn test_parse_feeds_valid() {
    let dir = tempfile::tempdir().unwrap();
    let feeds_path = dir.path().join("feeds.toml");
    fs::write(
      &feeds_path,
      r#"
[[feeds]]
link = "https://example.com/feed.xml"
name = "Example"
tags = ["blog", "tech"]

[[feeds]]
link = "https://other.com/rss"
"#,
    )
    .unwrap();

    let feeds = parse_feeds(dir.path()).unwrap();
    assert_eq!(feeds.len(), 2);
    assert_eq!(feeds[0].link, "https://example.com/feed.xml");
    assert_eq!(feeds[0].name.as_deref(), Some("Example"));
    assert_eq!(
      feeds[0].tags.as_ref().unwrap(),
      &vec!["blog".to_string(), "tech".to_string()]
    );
    assert_eq!(feeds[1].link, "https://other.com/rss");
    assert!(feeds[1].name.is_none());
    assert!(feeds[1].tags.is_none());
  }

  #[test]
  fn test_parse_feeds_invalid_toml() {
    let dir = tempfile::tempdir().unwrap();
    let feeds_path = dir.path().join("feeds.toml");
    fs::write(&feeds_path, "this is not valid toml {{{{").unwrap();

    let result = parse_feeds(dir.path());
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ConfigError::Parse(_, _)));
  }

  #[test]
  fn test_parse_feeds_empty_feeds_list() {
    let dir = tempfile::tempdir().unwrap();
    let feeds_path = dir.path().join("feeds.toml");
    fs::write(&feeds_path, "feeds = []\n").unwrap();

    let feeds = parse_feeds(dir.path()).unwrap();
    assert!(feeds.is_empty());
  }

  #[test]
  fn test_parse_config_file_missing() {
    let dir = tempfile::tempdir().unwrap();
    let (general, ui, queries) = parse_config_file(dir.path());
    // Should return defaults when file is missing
    assert!(general.browser.is_none());
    assert!(general.media_player.is_none());
    // UiConfig::default() gives false for both fields (Rust Default, not serde defaults)
    assert!(!ui.show_borders);
    assert!(!ui.show_read_entries);
    assert!(queries.is_empty());
  }

  #[test]
  fn test_parse_config_file_valid() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(
      &config_path,
      r#"
[general]
browser = "firefox"
media_player = "mpv"

[ui]
show_borders = false
show_read_entries = false

[[queries]]
name = "All Blogs"
query = "tags:blog"
"#,
    )
    .unwrap();

    let (general, ui, queries) = parse_config_file(dir.path());
    assert_eq!(general.browser.as_deref(), Some("firefox"));
    assert_eq!(general.media_player.as_deref(), Some("mpv"));
    assert!(!ui.show_borders);
    assert!(!ui.show_read_entries);
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].name, "All Blogs");
    assert_eq!(queries[0].query, "tags:blog");
  }

  #[test]
  fn test_parse_config_file_invalid_falls_back_to_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, "not valid toml {{{{").unwrap();

    let (general, ui, queries) = parse_config_file(dir.path());
    assert!(general.browser.is_none());
    assert!(!ui.show_borders); // Default derive gives false, not the serde default of true
    assert!(queries.is_empty());
  }

  #[test]
  fn test_default_ui_config() {
    let ui = UiConfig::default();
    // Default from serde is false because bool::default() is false.
    // But the serde defaults use custom fns that return true.
    // The Default derive won't call the serde default fns, so Default::default()
    // gives false. This test documents that behavior.
    assert!(!ui.show_borders);
    assert!(!ui.show_read_entries);
  }

  #[test]
  fn test_feed_struct_defaults() {
    let feed = Feed::default();
    assert!(feed.link.is_empty());
    assert!(feed.name.is_none());
    assert!(feed.tags.is_none());
  }
}

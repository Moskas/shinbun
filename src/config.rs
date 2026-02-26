use dirs::config_dir;
use serde::Deserialize;
use std::{fs, path::PathBuf, process};

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
pub fn parse_config() -> Config {
  let config_dir = get_config_dir();
  let (general_config, ui_config, queries) = parse_config_file(&config_dir);
  let feeds = parse_feeds(&config_dir);
  Config {
    feeds,
    general: general_config,
    ui: ui_config,
    queries,
  }
}

/// Parse configuration file (config.toml)
fn parse_config_file(config_dir: &PathBuf) -> (GeneralConfig, UiConfig, Vec<QueryFeed>) {
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
fn parse_feeds(config_dir: &PathBuf) -> Vec<Feed> {
  let feeds_path = config_dir.join("feeds.toml");

  if !feeds_path.exists() {
    eprintln!("Feeds file not found: {}", feeds_path.display());
    eprintln!("Please create a feeds.toml file with your RSS feed URLs");
    eprintln!("Example:");
    eprintln!("  [[feeds]]");
    eprintln!("  link = \"https://example.com/feed.xml\"");
    process::exit(1);
  }

  let content = fs::read_to_string(&feeds_path).unwrap_or_else(|err| {
    eprintln!("Failed to read feeds.toml: {}", err);
    process::exit(1);
  });

  let feeds_file: FeedsFile = toml::from_str(&content).unwrap_or_else(|err| {
    eprintln!("Failed to parse feeds.toml: {}", err);
    process::exit(1);
  });

  if feeds_file.feeds.is_empty() {
    eprintln!("Warning: No feeds configured in feeds.toml");
  }

  feeds_file.feeds
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

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
  #[serde(default)]
  pub split_view: bool,
  #[serde(default = "default_show_borders")]
  pub show_borders: bool,
}

fn default_show_borders() -> bool {
  true
}

#[derive(Debug, Deserialize, Default)]
struct ConfigFile {
  #[serde(default)]
  ui: UiConfig,
}

pub struct Config {
  pub feeds: Vec<Feed>,
  pub ui: UiConfig,
}

/// Parse complete configuration from config.toml and feeds.toml
pub fn parse_config() -> Config {
  let config_dir = get_config_dir();

  // Parse UI config from config.toml (optional)
  let ui_config = parse_ui_config(&config_dir);

  // Parse feeds from feeds.toml (required)
  let feeds = parse_feeds(&config_dir);

  Config {
    feeds,
    ui: ui_config,
  }
}

/// Parse UI configuration from config.toml
fn parse_ui_config(config_dir: &PathBuf) -> UiConfig {
  let config_path = config_dir.join("config.toml");

  // If config.toml doesn't exist, use defaults
  if !config_path.exists() {
    return UiConfig::default();
  }

  // Read and parse config.toml
  let content = match fs::read_to_string(&config_path) {
    Ok(c) => c,
    Err(_) => return UiConfig::default(),
  };

  match toml::from_str::<ConfigFile>(&content) {
    Ok(config) => config.ui,
    Err(err) => {
      eprintln!("Warning: Failed to parse config.toml: {}", err);
      eprintln!("Using default configuration");
      UiConfig::default()
    }
  }
}

/// Parse feeds from feeds.toml
fn parse_feeds(config_dir: &PathBuf) -> Vec<Feed> {
  let feeds_path = config_dir.join("feeds.toml");

  // Check if feeds.toml exists
  if !feeds_path.exists() {
    eprintln!("Feeds file not found: {}", feeds_path.display());
    eprintln!("Please create a feeds.toml file with your RSS feed URLs");
    eprintln!("Example:");
    eprintln!("  [[feeds]]");
    eprintln!("  link = \"https://example.com/feed.xml\"");
    process::exit(1);
  }

  // Read feeds.toml
  let content = fs::read_to_string(&feeds_path).unwrap_or_else(|err| {
    eprintln!("Failed to read feeds.toml: {}", err);
    process::exit(1);
  });

  // Parse feeds
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

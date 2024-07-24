use dirs::config_dir;
use serde::Deserialize;
use std::{fs, process::exit};

#[derive(Debug, Default, Deserialize)]
pub struct Feeds {
    pub link: String,
    pub name: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct Config {
    feeds: Vec<Feeds>,
}

pub fn parse_feed_urls() -> Vec<Feeds> {
    // Read the configuration file
    let url_file = format!(
        "{}/shinbun/urls.toml",
        config_dir()
            .expect("Config directory doesn't exist")
            .display(),
    );

    if fs::File::open(&url_file).is_err() {
        println!("File urls.toml not found in path: {}", &url_file);
        exit(-1)
    }

    // Read the TOML file
    let toml_content = fs::read_to_string(&url_file).expect("Error reading configuration file");

    // Parse the TOML content into Config struct
    let config: Config = toml::from_str(&toml_content).expect("Error parsing TOML configuration");
    // Return the list of feeds
    config.feeds
}

pub fn _parse_config() {
    todo!()
}

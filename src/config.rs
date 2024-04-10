use dirs::config_dir;

pub fn parse_feed_urls() -> Vec<String> {
  // Read the configuration file
  let config_file = format!(
    "{}/shinbun/urls",
    config_dir()
      .expect("Config directory doesn't exist")
      .display(),
  );

  // Parse the file as a list of URLs
  let mut feed_urls = Vec::new();
  for line in std::fs::read_to_string(&config_file)
    .expect("Error reading configuration file")
    .lines()
    .filter(|line| !line.trim().starts_with('#'))
  // Ignore lines starting with '#'
  {
    let feed_url = line.split_whitespace().next().unwrap_or("");
    feed_urls.push(String::from(feed_url));
  }

  feed_urls
}

pub fn _parse_config() {
  todo!()
}

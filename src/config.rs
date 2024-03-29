use dirs::config_dir;

pub fn parse_feed_urls() -> Vec<String> {
  std::fs::read_to_string(
    format!(
      "{}/shinbun/urls",
      config_dir()
        .expect("Config directory doesn't exist")
        .display()
    )
    .to_string(),
  )
  .expect("No urls file provided in configuration directory")
  .lines()
  .filter(|line| !line.trim().starts_with('#')) // Ignore lines starting with '#'
  .map(|line| {
    let feed_url = line.split_whitespace().next().unwrap_or("");
    String::from(feed_url)
  })
  .collect()
}

fn _parse_config() {
  todo!()
}

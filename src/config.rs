use dirs::config_dir;

#[derive(Debug, Default)]
pub struct Feeds {
  pub link: String,
  pub name: Option<String>,
  pub tags: Option<Vec<String>>,
}

pub fn parse_feed_urls() -> Vec<Feeds> {
  // Read the configuration file
  let url_file = format!(
    "{}/shinbun/urls",
    config_dir()
      .expect("Config directory doesn't exist")
      .display(),
  );

  // Parse the file as a list of URLs
  let mut feed_urls = Vec::new();
  for line in std::fs::read_to_string(url_file)
    .expect("Error reading configuration file")
    .lines()
    // Ignore lines starting with '#'
    .filter(|line| !line.trim().starts_with('#'))
  {
    let feed = line
      .split_whitespace()
      .map(|word| word.to_string())
      .collect::<Vec<String>>();
    // Get the tags that are between url and name override
    // FIX That might skip a tag if no name override is set
    let tags = if feed.len() > 2 {
      Some(feed[1..feed.len() - 1].to_owned())
    } else {
      None
    };
    // Name override for the feed
    let name = if feed.last().unwrap().to_string().starts_with('~') {
      Some(feed.last().unwrap().to_string())
    } else {
      None
    };

    let feed_struct = Feeds {
      link: feed.first().unwrap().to_string(),
      tags,
      name,
    };
    feed_urls.push(feed_struct);
  }

  feed_urls
}

pub fn _parse_config() {
  todo!()
}

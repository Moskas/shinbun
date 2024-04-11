use dirs::config_dir;

#[derive(Debug, Default)]
pub struct Urls {
  link: String,
  name: Option<String>,
  tags: Option<Vec<String>>,
}

pub struct FeedsFromUrls {
  feeds: Vec<Urls>,
}

pub fn parse_feed_urls() -> Vec<String> {
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
    let tags = if feed.len() > 2 {
      Some(feed[1..feed.len() - 1].to_owned())
    } else {
      None
    };
    let _feed_struct = Urls {
      link: feed.first().unwrap().to_string(),
      tags,
      name: if feed.last().unwrap().to_string().starts_with('~') {
        Some(feed.last().unwrap().to_string())
      } else {
        None
      },
    };
    //println!("{:#?}", feed_struct);
    feed_urls.push(feed.first().unwrap().to_string());
  }

  feed_urls
}

pub fn _parse_config() {
  todo!()
}

use crate::config::Feed;
use quick_xml::{
  events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
  Reader, Writer,
};
use std::{
  error::Error,
  io::{BufRead, Write},
};

pub fn export_opml(feeds: &[Feed], sink: impl Write) -> Result<(), Box<dyn Error>> {
  let mut w = Writer::new_with_indent(sink, b' ', 2);

  w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

  let mut opml = BytesStart::new("opml");
  opml.push_attribute(("version", "2.0"));
  w.write_event(Event::Start(opml))?;

  w.write_event(Event::Start(BytesStart::new("head")))?;
  w.write_event(Event::Start(BytesStart::new("title")))?;
  w.write_event(Event::Text(BytesText::new("Shinbun Feeds")))?;
  w.write_event(Event::End(BytesEnd::new("title")))?;
  w.write_event(Event::End(BytesEnd::new("head")))?;

  w.write_event(Event::Start(BytesStart::new("body")))?;

  for feed in feeds {
    let mut outline = BytesStart::new("outline");
    outline.push_attribute(("type", "rss"));
    let text = feed.name.as_deref().unwrap_or(&feed.link);
    outline.push_attribute(("text", text));
    outline.push_attribute(("xmlUrl", feed.link.as_str()));
    if let Some(tags) = &feed.tags {
      if !tags.is_empty() {
        outline.push_attribute(("category", tags.join(",").as_str()));
      }
    }
    w.write_event(Event::Empty(outline))?;
  }

  w.write_event(Event::End(BytesEnd::new("body")))?;
  w.write_event(Event::End(BytesEnd::new("opml")))?;

  Ok(())
}

pub fn import_opml(reader: impl BufRead) -> Result<Vec<Feed>, Box<dyn Error>> {
  let mut xml = Reader::from_reader(reader);
  xml.config_mut().trim_text(true);

  let mut feeds: Vec<Feed> = Vec::new();
  let mut folder_tag: Option<String> = None;
  let mut buf = Vec::new();

  loop {
    match xml.read_event_into(&mut buf)? {
      Event::Empty(ref e) if e.name().as_ref() == b"outline" => {
        if let Some(feed) = parse_outline(e, folder_tag.as_deref()) {
          feeds.push(feed);
        }
      }
      Event::Start(ref e) if e.name().as_ref() == b"outline" => {
        if get_attr(e, b"xmlUrl").is_some() {
          if let Some(feed) = parse_outline(e, folder_tag.as_deref()) {
            feeds.push(feed);
          }
        } else {
          folder_tag = get_attr(e, b"text");
        }
      }
      Event::End(ref e) if e.name().as_ref() == b"outline" => {
        folder_tag = None;
      }
      Event::Eof => break,
      _ => {}
    }
    buf.clear();
  }

  Ok(feeds)
}

fn get_attr(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
  e.attributes()
    .flatten()
    .find(|a| a.key.as_ref() == key)
    .map(|a| String::from_utf8_lossy(a.value.as_ref()).into_owned())
}

fn parse_outline(e: &BytesStart<'_>, folder_tag: Option<&str>) -> Option<Feed> {
  let link = get_attr(e, b"xmlUrl")?;
  let name = get_attr(e, b"title").or_else(|| get_attr(e, b"text"));
  let tags = get_attr(e, b"category")
    .map(|cat| {
      cat
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
    })
    .filter(|t| !t.is_empty())
    .or_else(|| folder_tag.map(|t| vec![t.to_string()]));
  Some(Feed { link, name, tags })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn round_trip() {
    let feeds = vec![
      Feed {
        link: "https://blog.rust-lang.org/feed.xml".into(),
        name: Some("Rust Blog".into()),
        tags: Some(vec!["tech".into(), "rust".into()]),
      },
      Feed {
        link: "https://news.ycombinator.com/rss".into(),
        name: Some("Hacker News".into()),
        tags: None,
      },
    ];

    let mut buf: Vec<u8> = Vec::new();
    export_opml(&feeds, &mut buf).unwrap();
    let xml = String::from_utf8(buf.clone()).unwrap();
    assert!(xml.contains("xmlUrl=\"https://blog.rust-lang.org/feed.xml\""));
    assert!(xml.contains("category=\"tech,rust\""));
    assert!(xml.contains("xmlUrl=\"https://news.ycombinator.com/rss\""));

    let imported = import_opml(buf.as_slice()).unwrap();
    assert_eq!(imported.len(), 2);
    assert_eq!(imported[0].link, feeds[0].link);
    assert_eq!(imported[0].name, feeds[0].name);
    assert_eq!(imported[0].tags, feeds[0].tags);
    assert_eq!(imported[1].link, feeds[1].link);
    assert!(imported[1].tags.is_none());
  }

  #[test]
  fn import_folder_style() {
    let opml = r#"<?xml version="1.0"?>
<opml version="2.0">
  <head><title>Test</title></head>
  <body>
    <outline text="Tech">
      <outline type="rss" text="Feed A" xmlUrl="https://a.example/feed"/>
    </outline>
    <outline type="rss" text="Feed B" xmlUrl="https://b.example/feed"/>
  </body>
</opml>"#;

    let feeds = import_opml(opml.as_bytes()).unwrap();
    assert_eq!(feeds.len(), 2);
    assert_eq!(feeds[0].tags, Some(vec!["Tech".into()]));
    assert!(feeds[1].tags.is_none());
  }
}

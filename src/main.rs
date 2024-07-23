use config::Feeds;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use feeds::Feed;
use ratatui::{
  prelude::*,
  symbols::border,
  widgets::{block::*, *},
};

use std::io;

mod config;
mod feeds;
mod ui;

#[tokio::main]
async fn main() -> std::io::Result<()> {
  let feeds_urls = config::parse_feed_urls;
  let xml = feeds::fetch_feed(feeds_urls()).await;
  let list: Vec<Feed> = feeds::parse_feed(xml.expect("Failed to fetch feed"), feeds_urls());
  let mut terminal = ui::init()?;
  let app = App::new(list).run(&mut terminal);
  ui::restore()?;
  app
}

#[derive(Debug)]
pub struct App {
  list: Vec<Feed>,
  index: usize,
  state: ListState,
  exit: bool,
}

//pub struct FeedsList {
//  list: Vec<String>,
//  index: usize,
//  state: ListState,
//}
//
//pub struct PostList {
//  index: usize,
//  state: ListState,
//}

impl App {
  pub fn new(list: Vec<Feed>) -> Self {
    App {
      list,
      state: ListState::default().with_selected(Some(0)),
      index: 0,
      exit: false,
    }
  }

  pub fn run(&mut self, terminal: &mut ui::Tui) -> io::Result<()> {
    while !self.exit {
      terminal.draw(|frame| self.render_frame(frame))?;
      self.handle_events()?;
    }
    Ok(())
  }

  fn render_frame(&self, frame: &mut Frame) {
    frame.render_widget(self, frame.size());
  }

  fn handle_events(&mut self) -> std::io::Result<()> {
    match event::read()? {
      // it's important to check that the event is a key press event as
      // crossterm also emits key release and repeat events on Windows.
      Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
        self.handle_key_event(key_event)
      }
      _ => {}
    };
    Ok(())
  }
  fn handle_key_event(&mut self, key_event: KeyEvent) {
    match key_event.code {
      KeyCode::Char('q') | KeyCode::Char('Q') => self.exit(),
      KeyCode::Up | KeyCode::Char('k') => self.previous(),
      KeyCode::Down | KeyCode::Char('j') => self.next(),
      KeyCode::Right | KeyCode::Char('l') => self.enter(),
      KeyCode::Left | KeyCode::Char('h') => self.back(),
      KeyCode::Char('?') => self.help(),
      _ => {}
    }
  }
  fn exit(&mut self) {
    self.exit = true;
  }
  fn previous(&mut self) {
    if self.index > 0 {
      self.index -= 1;
      self.state.select(Some(self.index))
    } else {
      self.state.select(Some(0))
    }
  }
  fn next(&mut self) {
    if self.index + 1 < self.list.len() {
      self.index += 1;
      self.state.select(Some(self.index))
    }
  }
  fn enter(&mut self) {
    todo!()
  }
  fn back(&mut self) {
    todo!()
  }
  fn help(&mut self) {
    todo!()
  }
}

impl Widget for &App {
  fn render(self, area: Rect, buf: &mut Buffer) {
    let title = Title::from(" Shinbun ".bold().yellow());
    let instructions = Title::from(Line::from(vec![" Quit ".into(), "<Q> ".bold()]));
    let block = Block::default()
      .title(title.alignment(Alignment::Left))
      .title(
        instructions
          .alignment(Alignment::Left)
          .position(Position::Bottom),
      )
      .borders(Borders::ALL)
      .border_style(Style::new().blue())
      .border_set(border::PLAIN);

    let inner_area = block.inner(area);
    block.render(area, buf);

    let horizontal_split = Layout::default()
      .direction(Direction::Horizontal)
      .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
      .split(inner_area);

    let feeds = self
      .list
      .iter()
      .map(|l| {
        format!(
          " {} [{}] | {}",
          &l.title,
          l.entries.len(),
          if l.tags.is_some() {
            l.tags.as_ref().unwrap().join(",")
          } else {
            "".to_string()
          }
        )
      })
      .collect::<List>();

    let left_block = Block::default()
      .title(" Feeds ".green())
      .borders(Borders::ALL)
      .border_style(Style::new().blue())
      .border_set(border::PLAIN);

    StatefulWidget::render(
      feeds
        .block(left_block)
        .highlight_style(Style::default().yellow().bold()),
      horizontal_split[0],
      buf,
      &mut self.state.to_owned(),
    );

    let right_block = Block::default()
      .title(" Entries ".green())
      .borders(Borders::ALL)
      .border_style(Style::new().blue())
      .border_set(border::PLAIN);

    let selected_index = self.state.selected().unwrap_or(0);
    let entries = if let Some(feed) = self.list.get(selected_index) {
      feed
        .entries
        .iter()
        .map(|e| ListItem::new(format!("{}", e.title.clone().unwrap().content)))
        .collect::<Vec<_>>()
    } else {
      vec![]
    };

    let secondary_list = List::new(entries)
      .block(right_block.clone())
      .highlight_style(Style::default().yellow().bold());
    StatefulWidget::render(
      secondary_list.block(right_block),
      horizontal_split[1],
      buf,
      &mut ListState::default(),
    );
  }
}

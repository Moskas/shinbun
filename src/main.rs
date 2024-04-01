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
  let xml = feeds::fetch_feed(feeds_urls());
  let list: Vec<Feed> = feeds::parse_feed(xml.await, feeds_urls());
  let mut terminal = ui::init()?;
  let app = App::new(list).run(&mut terminal);
  ui::restore()?;
  app
}

#[derive(Debug, Default)]
pub struct App {
  //mode: String, // TODO Change into something like Mode type
  list: Vec<Feed>,
  exit: bool,
}

impl App {
  pub fn new(list: Vec<Feed>) -> Self {
    App {
      //mode: String::new(),
      list,
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
      _ => {}
    }
  }
  fn exit(&mut self) {
    self.exit = true;
  }
}

impl Widget for &App {
  fn render(self, area: Rect, buf: &mut Buffer) {
    let title = Title::from(" Feed list ".bold());
    let instructions = Title::from(Line::from(vec![" Quit ".into(), "<Q> ".blue().bold()]));
    let block = Block::default()
      .title(title.alignment(Alignment::Left))
      .title(
        instructions
          .alignment(Alignment::Left)
          .position(Position::Bottom),
      )
      .borders(Borders::ALL)
      .border_style(Style::new().white())
      .border_set(border::PLAIN);

    //let text = Text::from(self.list.entries.join("\n"));
    let text = Text::from(format!(
      "{}",
      self
        .list
        .iter()
        .map(|l| format!("- {}", &l.title))
        .collect::<Vec<String>>()
        .join("\n")
    ));

    Paragraph::new(text)
      .style(Style::new().green())
      .alignment(Alignment::Left)
      .block(block)
      .render(area, buf);
  }
}

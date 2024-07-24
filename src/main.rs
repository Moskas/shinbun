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
  entries_state: ListState,
  active_list: ActiveList,
  entry_open: bool,
  scroll: usize,
  exit: bool,
}

#[derive(Debug)]
enum ActiveList {
  Feeds,
  Entries,
  Entry,
}

impl App {
  pub fn new(list: Vec<Feed>) -> Self {
    App {
      list,
      state: ListState::default().with_selected(Some(0)),
      entries_state: ListState::default(),
      index: 0,
      active_list: ActiveList::Feeds,
      entry_open: false,
      scroll: 0,
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
      KeyCode::Enter => self.toggle_pane(),
      KeyCode::Char('?') => self.help(),
      _ => {}
    }
  }

  fn exit(&mut self) {
    self.exit = true;
  }

  fn previous(&mut self) {
    if !self.entry_open {
      match self.active_list {
        ActiveList::Feeds => {
          if self.index > 0 {
            self.index -= 1;
            self.state.select(Some(self.index));
          }
        }
        ActiveList::Entries => {
          if let Some(selected) = self.entries_state.selected() {
            if selected > 0 {
              self.entries_state.select(Some(selected - 1));
            }
          }
        }
        _ => {}
      }
    } else {
      self.scroll = self.scroll.saturating_sub(1)
    }
  }

  fn next(&mut self) {
    if !self.entry_open {
      match self.active_list {
        ActiveList::Feeds => {
          if self.index + 1 < self.list.len() {
            self.index += 1;
            self.state.select(Some(self.index));
          }
        }
        ActiveList::Entries => {
          if let Some(selected) = self.entries_state.selected() {
            let entries_len = self.list[self.index].entries.len();
            if selected + 1 < entries_len {
              self.entries_state.select(Some(selected + 1));
            }
          }
        }
        _ => {}
      }
    } else {
      self.scroll += 1
    }
  }

  fn enter(&mut self) {
    if !self.entry_open {
      if let ActiveList::Feeds = self.active_list {
        self.active_list = ActiveList::Entries;
        self.entries_state.select(Some(0));
      }
    }
  }

  fn back(&mut self) {
    if !self.entry_open {
      if let ActiveList::Entries = self.active_list {
        self.active_list = ActiveList::Feeds;
      }
    }
  }

  fn toggle_pane(&mut self) {
    self.scroll = 0;
    self.entry_open = !self.entry_open;
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

    if self.entry_open {
      // Render the pane
      if let Some(feed) = self.list.get(self.index) {
        if let Some(selected_entry) = self.entries_state.selected() {
          if let Some(entry) = feed.entries.get(selected_entry) {
            let content = entry.clone();
            let entry_content = content.summary.clone().unwrap().content;
            let text = Text::from(entry_content);

            let content_height = text.height() as usize;
            //let max_scroll = content_height.saturating_sub(inner_area.height as usize);
            //self.scroll = self.scroll.clone().min(max_scroll);

            let mut scrollbar_state = ScrollbarState::default()
              .content_length(content_height)
              .position(self.scroll);

            let paragraph = Paragraph::new(text)
              .block(
                Block::default()
                  .title(format!(" {} ", entry.title.clone().unwrap().content).green())
                  .padding(Padding::new(area.width / 10, area.width / 10, 1, 1))
                  .borders(Borders::NONE),
              )
              .scroll((self.scroll as u16, 0))
              .wrap(Wrap { trim: false });

            let scrollbar = Scrollbar::default()
              .orientation(ScrollbarOrientation::VerticalRight)
              .begin_symbol(None)
              .end_symbol(None);

            paragraph.render(inner_area, buf);
            scrollbar.render(inner_area, buf, &mut scrollbar_state);
          }
        }
      }
    } else {
      // Render the lists
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

      let feeds_highlight_style = match self.active_list {
        ActiveList::Feeds => Style::default().yellow().bold(),
        ActiveList::Entries => Style::default(),
        _ => Style::default(),
      };

      StatefulWidget::render(
        feeds
          .block(left_block)
          .highlight_style(feeds_highlight_style),
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
          .map(|e| ListItem::new(format!("- {}", e.title.clone().unwrap().content)))
          .collect::<Vec<_>>()
      } else {
        vec![]
      };

      let secondary_list = List::new(entries)
        .block(right_block.clone())
        .highlight_style(Style::default().yellow().bold());

      let entries_highlight_style = match self.active_list {
        ActiveList::Entries => Style::default().yellow().bold(),
        ActiveList::Feeds => Style::default(),
        _ => Style::default(),
      };

      StatefulWidget::render(
        secondary_list
          .block(right_block)
          .highlight_style(entries_highlight_style),
        horizontal_split[1],
        buf,
        &mut self.entries_state.to_owned(),
      );
    }
  }
}

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
  scroll_state: ScrollbarState,
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
      scroll_state: ScrollbarState::new(0),
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
      self.scroll = self.scroll.saturating_sub(1);
      //self.scroll_state = self.scroll_state.position(self.scroll)
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
      //self.scroll = self.scroll.clamp(0, 150).into();
      self.scroll = self.scroll.saturating_add(1);
      //self.scroll_state = self.scroll_state.position(self.scroll)
    }
  }

  fn enter(&mut self) {
    match self.active_list {
      ActiveList::Feeds => {
        self.active_list = ActiveList::Entries;
        self.entries_state.select(Some(0));
      }
      ActiveList::Entries => {
        self.active_list = ActiveList::Entry;
        self.scroll = 0;
        self.entry_open = true;
      }
      _ => {}
    }
  }

  fn back(&mut self) {
    match self.active_list {
      ActiveList::Entry => {
        self.active_list = ActiveList::Entries;
        self.entry_open = false;
      },
      ActiveList::Entries => self.active_list = ActiveList::Feeds,
      _ => {}
    }
  }

  fn help(&mut self) {
    todo!()
  }
}

impl Widget for &App {
  fn render(self, area: Rect, buf: &mut Buffer) {
    let title = Title::from(" Shinbun ".bold().yellow());
    let instructions = Title::from(Line::from(vec![" Quit ".into(), "<q> ".bold()]));
    let block = Block::default()
      .title(title.alignment(Alignment::Left))
      .title(
        instructions
          .alignment(Alignment::Left)
          .position(Position::Bottom),
      )
      .title_bottom(Line::from(" Help <?> ".blue()).right_aligned())
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

            let main_text = content
              .content
              .as_ref()
              .and_then(|c| c.clone().body)
              .or_else(|| content.clone().summary.map(|s| s.content))
              .unwrap_or_default();

            let plain_text = html2text::config::plain()
              .lines_from_read(main_text.as_bytes(), area.width as usize - 15 as usize)
              .expect("Failed to parse the page")
              .iter()
              .map(|l| l.chars().collect())
              .collect::<Vec<String>>()
              .join("\n");

            let text_height = main_text.split("\n").count() + 5;

            let entry_content = vec![
              Line::from(format!("Title: {}", content.title.clone().unwrap().content).magenta()),
              Line::from(format!("Feed: {}", feed.title).cyan()),
              Line::from(
                format!(
                  "Published: {}",
                  if content.clone().published.is_some() {
                    content.clone().published.unwrap().to_string()
                  } else {
                    content.clone().updated.unwrap().to_string()
                  }
                )
                .yellow(),
              ),
              Line::from(
                format!(
                  "Link: {}",
                  content.clone().links.first().unwrap().to_owned().href
                )
                .blue(),
              ),
              Line::from(""),
            ];
            let plain_text_lines: Vec<Line> = plain_text.lines().map(Line::from).collect();

            // Combine entry_content and plain_text_lines
            let mut combined_lines = entry_content;
            combined_lines.extend(plain_text_lines);
            //let content_height = text_height as usize;
            let paragraph = Paragraph::new(combined_lines)
              .block(
                Block::default()
                  //.title_bottom(format!("{} {} ", self.scroll as i32, content_height as i32))
                  .padding(Padding::new(area.width / 20, area.width / 20, 1, 1))
                  .borders(Borders::NONE),
              )
              .scroll((self.scroll as u16, 0))
              .wrap(Wrap { trim: false });

            //let mut scrollbar_state = ScrollbarState::default()
            //  .content_length(content_height)
            //  .position(self.scroll);
            //let mut scroll_state = self.scroll_state.content_length(content_height);

            //let scrollbar = Scrollbar::default()
            //  .orientation(ScrollbarOrientation::VerticalRight)
            //  .begin_symbol(None)
            //  .end_symbol(None);

            paragraph.render(inner_area, buf);
            //scrollbar.render(inner_area, buf, &mut scroll_state);
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
        .map(|l| format!(" {}", &l.title,))
        .collect::<List>();

      let left_block = Block::default()
        .title(" Feeds ".green())
        .borders(Borders::ALL)
        .border_style(Style::new().blue())
        .border_set(border::PLAIN);

      let feeds_highlight_style = match self.active_list {
        ActiveList::Feeds => Style::default().bg(Color::Yellow).fg(Color::Black),
        ActiveList::Entries => Style::default().yellow(),
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

      let selected_index = self.state.selected().unwrap_or(0);
      let entries = if let Some(feed) = self.list.get(selected_index) {
        feed
          .entries
          .iter()
          .map(|e| ListItem::new(format!(" {}", e.title.clone().unwrap().content)))
          .collect::<Vec<_>>()
      } else {
        vec![]
      };

      let right_block = Block::default()
        .title(" Entries ".green())
        .title(format!(" {} ", entries.iter().count()).yellow())
        .borders(Borders::ALL)
        .border_style(Style::new().blue())
        .border_set(border::PLAIN);

      let secondary_list = List::new(entries)
        .block(right_block.clone())
        .highlight_style(Style::default().yellow().bold());

      let entries_highlight_style = match self.active_list {
        ActiveList::Entries => Style::default().bg(Color::Yellow).fg(Color::Black).bold(),
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

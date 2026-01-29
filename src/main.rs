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
    let mut terminal = ui::init()?;
    let area_width = terminal.size()?.width as usize;

    let feeds_urls = config::parse_feed_urls;
    let xml = feeds::fetch_feed(feeds_urls()).await;

    let list: Vec<Feed> =
        feeds::parse_feed(xml.expect("Failed to fetch feed"), feeds_urls(), area_width);

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
    // NEW: cache viewport metrics for the entry view
    view_content_length: usize,
    view_visible_height: usize,
    view_max_scroll: usize,
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
            view_content_length: 0,
            view_visible_height: 0,
            view_max_scroll: 0,
            exit: false,
        }
    }

    pub fn run(&mut self, terminal: &mut ui::Tui) -> io::Result<()> {
        while !self.exit {
            terminal.draw(|frame| {
                self.render_frame(frame) // now takes &mut self
            })?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn render_frame(&mut self, frame: &mut Frame) {
        // Compute the outer app block to get the same inner area you use in Widget::render
        let title = Title::from(" Shinbun ".bold().yellow());
        let instructions = Title::from(Line::from(vec![" Quit ".into(), "<q> ".bold()]));
        let app_block = Block::default()
            .title(title.alignment(Alignment::Left))
            .title(
                instructions
                    .alignment(Alignment::Left)
                    .position(block::Position::Bottom),
            )
            .title_bottom(Line::from(" Help <?> ").blue().right_aligned())
            .borders(Borders::ALL)
            .border_style(Style::new().blue())
            .border_set(border::PLAIN);

        let area = frame.area();
        let inner_area = app_block.inner(area);

        // If entry is open, compute viewport metrics for the entry pane
        if self.entry_open {
            if let Some(feed) = self.list.get(self.index) {
                if let Some(selected) = self.entries_state.selected() {
                    if let Some(entry) = feed.entries.get(selected) {
                        // Count lines that will be in the Paragraph:
                        // 3 metadata lines + optional link + optional media + 1 blank + body lines
                        let meta_base = 3usize;
                        let link_lines = if entry.links.is_empty() { 0 } else { 1 };
                        let media_lines = if entry.media.is_empty() { 0 } else { 1 };
                        let sep_line = 1usize;
                        let body_lines = entry.plain_text.lines().count();
                        let content_length =
                            meta_base + link_lines + media_lines + sep_line + body_lines;

                        // Use the same entry block you use in Widget::render to get accurate height
                        let entry_block = Block::default()
                            .title(" Entry ".yellow())
                            .borders(Borders::ALL)
                            .border_style(Style::new().blue());
                        let inner = entry_block.inner(inner_area);
                        let visible_height = inner.height as usize;
                        let max_scroll = content_length.saturating_sub(visible_height);

                        // Cache for key handlers
                        self.view_content_length = content_length;
                        self.view_visible_height = visible_height;
                        self.view_max_scroll = max_scroll;

                        // Clamp the state *now* so UI and state stay in sync
                        if self.scroll > self.view_max_scroll {
                            self.scroll = self.view_max_scroll;
                        }
                    }
                }
            }
        } else {
            // Not in entry view; nothing scrollable here
            self.view_content_length = 0;
            self.view_visible_height = 0;
            self.view_max_scroll = 0;
            self.scroll = 0; // optional: reset when leaving entry view
        }

        // Draw the outer block and the rest via your Widget impl
        app_block.render(area, frame.buffer_mut());
        let app_ref: &App = self;
        frame.render_widget(app_ref, frame.area());
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
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => self.enter(),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => self.back(),
            KeyCode::Char('s') => self.save_entry(),
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
            // self.scroll_state = self.scroll_state.position(self.scroll)
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
            self.scroll = self.scroll.saturating_add(1).min(self.view_max_scroll);
            // self.scroll_state = self.scroll_state.position(self.scroll)
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
            }
            ActiveList::Entries => self.active_list = ActiveList::Feeds,
            _ => {}
        }
    }

    fn help(&mut self) {
        todo!()
    }

    fn save_entry(&mut self) {
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
                    .position(block::Position::Bottom),
            )
            .title_bottom(Line::from(" Help <?> ".blue()).right_aligned())
            .borders(Borders::ALL)
            .border_style(Style::new().blue())
            .border_set(border::PLAIN);

        let inner_area = block.inner(area);
        block.render(area, buf);

        // NEW: responsive flag based on drawable content width
        let is_wide = inner_area.width >= 80u16;

        if self.entry_open {
            if let Some(feed) = self.list.get(self.index) {
                if let Some(selected_entry) = self.entries_state.selected() {
                    if let Some(entry) = feed.entries.get(selected_entry) {
                        // 1) Build visible text buffer
                        let mut entry_content = vec![
                            Line::from(format!("Title: {}", entry.title)).magenta(),
                            Line::from(format!("Feed: {}", feed.title)).cyan(),
                            Line::from(format!(
                                "Published: {}",
                                entry.published.as_deref().unwrap_or("Unknown")
                            ))
                            .yellow(),
                        ];
                        if !entry.links.is_empty() {
                            entry_content.push(
                                Line::from(format!("Link: {}", entry.links.join(", "))).blue(),
                            );
                        }
                        if !entry.media.is_empty() {
                            entry_content
                                .push(Line::from(format!("Media: {}", entry.media)).blue());
                        }
                        entry_content.push(Line::from("")); // separator

                        // Add wrapped plain text lines
                        let plain_lines: Vec<Line> =
                            entry.plain_text.lines().map(Line::from).collect();
                        entry_content.extend(plain_lines);

                        // 2) Compute scroll + visible height
                        let block = Block::default()
                            .title(" Entry ".yellow())
                            .borders(Borders::ALL)
                            .border_style(Style::new().blue());
                        let inner = block.inner(inner_area);
                        let visible_height = inner.height as usize;
                        let content_length = entry_content.len();
                        let max_scroll = content_length.saturating_sub(visible_height).max(0);

                        // Clamp scroll for safety (render-only clamp; main loop clamps too)
                        let scroll = self.scroll.min(max_scroll);

                        // 3) Visible line info (for bottom title)
                        let first_visible = scroll;
                        let last_visible = (scroll + visible_height - 1).min(content_length - 1);
                        let line_info = format!(
                            " Lines: {}–{} / {} ",
                            first_visible + 1,
                            last_visible + 1,
                            content_length
                        );

                        // 4) Whether to show scrollbar
                        let show_scrollbar = content_length > visible_height;

                        // 5) Render paragraph
                        let paragraph = Paragraph::new(entry_content.clone())
                            .block(block.clone().title_bottom(line_info.yellow()))
                            .scroll((scroll as u16, 0))
                            .wrap(Wrap { trim: false });
                        paragraph.render(inner_area, buf);

                        // 6) Render scrollbar (conditionally)
                        if show_scrollbar {
                            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                                .begin_symbol(Some("↑"))
                                .end_symbol(Some("↓"))
                                .track_symbol(Some("░"))
                                .thumb_symbol("█");
                            let mut scrollbar_state =
                                ScrollbarState::new(max_scroll + 1).position(scroll);
                            scrollbar.render(inner, buf, &mut scrollbar_state);
                        }
                    }
                }
            }
        } else if is_wide {
            // ==== WIDE MODE: dual pane ====
            let horizontal_split = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(inner_area);

            // Left: Feeds
            let feeds_items: Vec<ListItem> = self
                .list
                .iter()
                .map(|l| ListItem::new(format!(" {}", &l.title)))
                .collect();

            let left_block = Block::default()
                .title(" Feeds ".green())
                .title(format!(" {} ", self.list.len()).yellow())
                .borders(Borders::ALL)
                .border_style(Style::new().blue())
                .border_set(border::PLAIN);

            let feeds_highlight_style = match self.active_list {
                ActiveList::Feeds => Style::default().bg(Color::Yellow).fg(Color::Black),
                ActiveList::Entries => Style::default().yellow(),
                _ => Style::default(),
            };

            let feeds_list = List::new(feeds_items)
                .block(left_block)
                .highlight_style(feeds_highlight_style);

            StatefulWidget::render(
                feeds_list,
                horizontal_split[0],
                buf,
                &mut self.state.to_owned(),
            );

            // Right: Entries of selected feed
            let selected_index = self.state.selected().unwrap_or(0);
            let entries_items: Vec<ListItem> = if let Some(feed) = self.list.get(selected_index) {
                feed.entries
                    .iter()
                    .map(|e| ListItem::new(format!(" {}", e.title)))
                    .collect()
            } else {
                vec![]
            };

            let right_block = Block::default()
                .title(" Entries ".green())
                .title(format!(" {} ", entries_items.len()).yellow())
                .borders(Borders::ALL)
                .border_style(Style::new().blue())
                .border_set(border::PLAIN);

            let entries_highlight_style = match self.active_list {
                ActiveList::Entries => Style::default().bg(Color::Yellow).fg(Color::Black).bold(),
                ActiveList::Feeds => Style::default(),
                _ => Style::default(),
            };

            let entries_list = List::new(entries_items)
                .block(right_block)
                .highlight_style(entries_highlight_style);

            StatefulWidget::render(
                entries_list,
                horizontal_split[1],
                buf,
                &mut self.entries_state.to_owned(),
            );
        } else {
            // ==== NARROW MODE: single pane ====
            match self.active_list {
                ActiveList::Feeds => {
                    // Show only feeds list
                    let feeds_items: Vec<ListItem> = self
                        .list
                        .iter()
                        .map(|l| ListItem::new(format!(" {}", &l.title)))
                        .collect();

                    let feeds_block = Block::default()
                        .title(" Feeds ".green())
                        .title(format!(" {} ", self.list.len()).yellow())
                        .borders(Borders::ALL)
                        .border_style(Style::new().blue())
                        .border_set(border::PLAIN);

                    let feeds_highlight_style = Style::default().bg(Color::Yellow).fg(Color::Black);

                    let feeds_list = List::new(feeds_items)
                        .block(feeds_block)
                        .highlight_style(feeds_highlight_style);

                    StatefulWidget::render(feeds_list, inner_area, buf, &mut self.state.to_owned());
                }
                ActiveList::Entries | ActiveList::Entry => {
                    // Show only entries list (the actual reader is handled by the self.entry_open branch above)
                    let selected_index = self.state.selected().unwrap_or(0);
                    let entries_items: Vec<ListItem> =
                        if let Some(feed) = self.list.get(selected_index) {
                            feed.entries
                                .iter()
                                .map(|e| ListItem::new(format!(" {}", e.title)))
                                .collect()
                        } else {
                            vec![]
                        };

                    let entries_block = Block::default()
                        .title(" Entries ".green())
                        .title(format!(" {} ", entries_items.len()).yellow())
                        .borders(Borders::ALL)
                        .border_style(Style::new().blue())
                        .border_set(border::PLAIN);

                    let entries_list = List::new(entries_items)
                        .block(entries_block.clone())
                        .highlight_style(
                            Style::default().bg(Color::Yellow).fg(Color::Black).bold(),
                        );

                    StatefulWidget::render(
                        entries_list.block(entries_block),
                        inner_area,
                        buf,
                        &mut self.entries_state.to_owned(),
                    );
                }
            }
        }
    }
}

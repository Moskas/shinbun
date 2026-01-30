use config::Feeds;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use feeds::{Feed, FeedEntry};
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

    let feeds_urls = config::parse_feed_urls;
    let results = feeds::fetch_feed(feeds_urls()).await;

    // NOTE: parse_feed now takes the vector of (Feeds, Result<String, _>)
    let list: Vec<Feed> = feeds::parse_feed(results);

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

    // Cached viewport metrics for the entry view
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
                self.render_frame(frame);
            })?;
            self.handle_events()?;
        }
        Ok(())
    }

    /// Build the exact text content that will go into the Paragraph (unstyled here).
    /// Keeping this centralized ensures measurement == render content.
    fn build_entry_lines<'a>(&self, feed: &Feed, entry: &FeedEntry) -> Vec<Line<'a>> {
        let mut lines: Vec<Line> = Vec::new();

        // Metadata
        lines.push(Line::from(format!("Title: {}", entry.title)));
        lines.push(Line::from(format!("Feed: {}", feed.title)));
        lines.push(Line::from(format!(
            "Published: {}",
            entry.published.as_deref().unwrap_or("Unknown")
        )));

        if !entry.links.is_empty() {
            lines.push(Line::from(format!("Link: {}", entry.links.join(", "))));
        }
        if !entry.media.is_empty() {
            lines.push(Line::from(format!("Media: {}", entry.media)));
        }

        // Separator
        lines.push(Line::from(""));

        // Body (already unwrapped; Paragraph will wrap dynamically)
        lines.extend(entry.text.lines().map(|s| Line::from(s.to_owned())));

        lines
    }

    /// Estimate the wrapped height (number of terminal rows) that `lines` will take
    /// when rendered into a text area of width `content_width` (after borders & padding).
    ///
    /// We count 1 row for empty lines, and for non-empty lines we ceil-divide their
    /// display width by the content width.
    pub fn wrapped_height(&self, lines: &[Line], content_width: u16) -> usize {
        let w = content_width.max(1) as usize;
        lines
            .iter()
            .map(|line| {
                let raw = line.width(); // display width (Unicode-aware)
                if raw == 0 {
                    1
                } else {
                    // ceil(raw / w)
                    (raw + w - 1) / w
                }
            })
            .sum::<usize>()
    }

    fn render_frame(&mut self, frame: &mut Frame) {
        // ---- Outer application block ----
        let title = Title::from(" Shinbun ".bold().yellow());
        let instructions = Title::from(Line::from(vec![" Quit ".into(), "<q> ".bold()]));
        let app_block = Block::default()
            .title(title.alignment(Alignment::Left))
            .title(instructions.alignment(Alignment::Left).position(block::Position::Bottom))
            .title_bottom(Line::from(" Help <?> ").blue().right_aligned())
            .borders(Borders::ALL)
            .border_style(Style::new().blue())
            .border_set(border::PLAIN);

        let area = frame.area();
        let inner_area = app_block.inner(area);

        // ---- Cache viewport metrics for Entry view ----
        if self.entry_open {
            if let Some(feed) = self.list.get(self.index) {
                if let Some(selected) = self.entries_state.selected() {
                    if let Some(entry) = feed.entries.get(selected) {
                        // Build the SAME content we will render
                        let entry_lines = self.build_entry_lines(feed, entry);

                        // Use the SAME entry block and padding as in render() below
                        let entry_block = Block::default()
                            .title(" Entry ".yellow())
                            .borders(Borders::ALL)
                            .border_style(Style::new().blue())
                            // IMPORTANT: padding must be considered for measurement
                            .padding(Padding::symmetric(4, 1));

                        // Compute the text area (borders + padding removed)
                        let text_area = entry_block.inner(inner_area);
                        let visible_height = text_area.height as usize;

                        // Accurate wrapped height over the *whole* content
                        let content_length = self.wrapped_height(&entry_lines, text_area.width);

                        // Max scroll rows
                        let max_scroll = content_length.saturating_sub(visible_height);

                        // Cache for key handlers and bottom info
                        self.view_content_length = content_length;
                        self.view_visible_height = visible_height;
                        self.view_max_scroll = max_scroll;

                        // Clamp current scroll to avoid overshoot
                        if self.scroll > self.view_max_scroll {
                            self.scroll = self.view_max_scroll;
                        }
                    }
                }
            }
        } else {
            // Not in entry view; no scrollable metrics needed
            self.view_content_length = 0;
            self.view_visible_height = 0;
            self.view_max_scroll = 0;
            self.scroll = 0; // optional reset
        }

        // Render outer block and the rest via Widget impl
        app_block.render(area, frame.buffer_mut());
        let app_ref: &App = self;
        frame.render_widget(app_ref, frame.area());
    }

    fn handle_events(&mut self) -> std::io::Result<()> {
        match event::read()? {
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
            // scroll up in entry view
            if self.scroll > 0 {
                self.scroll -= 1;
            }
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
            // scroll down in entry view; clamp to max
            self.scroll = self.scroll.saturating_add(1).min(self.view_max_scroll);
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
        // TODO
    }

    fn save_entry(&mut self) {
        // TODO
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = Title::from(" Shinbun ".bold().yellow());
        let instructions = Title::from(Line::from(vec![" Quit ".into(), "<q> ".bold()]));
        let block = Block::default()
            .title(title.alignment(Alignment::Left))
            .title(instructions.alignment(Alignment::Left).position(block::Position::Bottom))
            .title_bottom(Line::from(" Help <?> ".blue()).right_aligned())
            .borders(Borders::ALL)
            .border_style(Style::new().blue())
            .border_set(border::PLAIN);

        let inner_area = block.inner(area);
        block.render(area, buf);

        // Responsive split
        let is_wide = inner_area.width >= 80u16;

        if self.entry_open {
            if let Some(feed) = self.list.get(self.index) {
                if let Some(selected_entry) = self.entries_state.selected() {
                    if let Some(entry) = feed.entries.get(selected_entry) {
                        // --- Build content (same text as measurement) ---
                        let mut entry_content: Vec<Line> = Vec::new();

                        // Styled metadata
                        entry_content.push(Line::from(format!("Title: {}", entry.title)).magenta());
                        entry_content.push(Line::from(format!("Feed: {}", feed.title)).cyan());
                        entry_content.push(
                            Line::from(format!(
                                "Published: {}",
                                entry.published.as_deref().unwrap_or("Unknown")
                            ))
                            .yellow(),
                        );
                        if !entry.links.is_empty() {
                            entry_content.push(
                                Line::from(format!("Link: {}", entry.links.join(", "))).blue(),
                            );
                        }
                        if !entry.media.is_empty() {
                            entry_content.push(Line::from(format!("Media: {}", entry.media)).blue());
                        }
                        entry_content.push(Line::from("")); // separator

                        // Body lines (unstyled)
                        entry_content.extend(entry.text.lines().map(Line::from));

                        // --- Use the SAME block (with padding) as used in measurement ---
                        let entry_block = Block::default()
                            .title(" Entry ".yellow())
                            .borders(Borders::ALL)
                            .border_style(Style::new().blue())
                            .padding(Padding::symmetric(4, 1));

                        // This is the true text area (borders & padding removed)
                        let inner_text_area = entry_block.inner(inner_area);

                        // Pull cached scrolling metrics (set in render_frame)
                        let content_length = self.view_content_length;
                        let visible_height = self.view_visible_height;
                        let max_scroll = self.view_max_scroll;
                        let scroll = self.scroll.min(max_scroll);

                        // Bottom info: "Lines: a–b / total"
                        let (first_visible, last_visible) = if content_length == 0 || visible_height == 0 {
                            (0usize, 0usize)
                        } else {
                            let first = scroll;
                            let last = (scroll + visible_height.saturating_sub(1))
                                .min(content_length.saturating_sub(1));
                            (first, last)
                        };
                        let line_info = format!(
                            " Lines: {}–{} / {} ",
                            first_visible.saturating_add(1),
                            last_visible.saturating_add(1),
                            content_length
                        );

                        // Prepare the Paragraph
                        let paragraph = Paragraph::new(entry_content)
                            .block(entry_block.clone().title_bottom(line_info.yellow()))
                            .scroll((scroll as u16, 0))
                            .wrap(Wrap { trim: true });

                        // === Reserve 1 column on the right for the scrollbar ===
                        // Paragraph will render into everything except the last column
                        let paragraph_area = Rect {
                            x: inner_text_area.x,
                            y: inner_text_area.y,
                            width: inner_text_area.width.saturating_sub(1),
                            height: inner_text_area.height,
                        };

                        // The scrollbar uses a dedicated 1-column strip on the right
                        let scrollbar_area = Rect {
                            x: inner_text_area.x + inner_text_area.width.saturating_sub(0),
                            y: inner_text_area.y,
                            width: 1,
                            height: inner_text_area.height,
                        };

                        // Render the paragraph inside the inner (bordered + padded) entry area
                        // NOTE: we still pass `inner_area` to render the block decoration,
                        // and the paragraph text is constrained by `paragraph_area`.
                        paragraph.render(inner_area, buf);

                        // Render the scrollbar only if content overflows
                        if content_length > visible_height && paragraph_area.width > 0 {
                            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                                .begin_symbol(Some("↑"))
                                .end_symbol(Some("↓"))
                                .track_symbol(Some("░"))
                                .thumb_symbol("█");

                            let mut scrollbar_state =
                                ScrollbarState::new(max_scroll + 1).position(scroll);

                            // Draw the scrollbar in its own 1‑column strip
                            scrollbar.render(scrollbar_area, buf, &mut scrollbar_state);
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

            StatefulWidget::render(feeds_list, horizontal_split[0], buf, &mut self.state.to_owned());

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
                        .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black).bold());

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
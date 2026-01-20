//! Chat view widget for displaying Matrix room messages
//!
//! Renders a message list with sender, timestamp, and content,
//! plus an input area for composing messages.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Widget},
};

/// A chat message
#[derive(Clone, Debug)]
pub struct ChatMessage {
    /// Sender display name or user ID
    pub sender: String,
    /// Message content
    pub content: String,
    /// Timestamp (formatted string)
    pub timestamp: String,
    /// Whether this message is from the local user
    pub is_own: bool,
}

impl ChatMessage {
    /// Create a new chat message
    pub fn new(sender: String, content: String, timestamp: String, is_own: bool) -> Self {
        Self {
            sender,
            content,
            timestamp,
            is_own,
        }
    }
}

/// Chat view state for a room
#[derive(Clone, Debug, Default)]
pub struct ChatState {
    /// Messages in this chat
    pub messages: Vec<ChatMessage>,
    /// Current input text
    pub input: String,
    /// Scroll offset from bottom (0 = at bottom)
    pub scroll_offset: usize,
    /// Whether there are unread messages
    pub has_unread: bool,
}

// Some methods reserved for future scroll/unread UI
#[allow(dead_code)]
impl ChatState {
    /// Create a new empty chat state
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a message to the chat
    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(message);
        // If not scrolled, new messages are visible
        // If scrolled up, mark as unread
        if self.scroll_offset > 0 {
            self.has_unread = true;
        }
    }

    /// Clear unread flag
    pub fn mark_read(&mut self) {
        self.has_unread = false;
    }

    /// Scroll up by n lines
    pub fn scroll_up(&mut self, n: usize) {
        let max = self.messages.len().saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }

    /// Scroll down by n lines
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        if self.scroll_offset == 0 {
            self.mark_read();
        }
    }

    /// Scroll to bottom
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.mark_read();
    }

    /// Check if scrolled up
    pub fn is_scrolled(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Push a character to the input
    pub fn push_input(&mut self, c: char) {
        self.input.push(c);
    }

    /// Pop a character from the input
    pub fn pop_input(&mut self) {
        self.input.pop();
    }

    /// Clear input and return the content
    pub fn take_input(&mut self) -> String {
        std::mem::take(&mut self.input)
    }

    /// Check if input is empty
    pub fn input_is_empty(&self) -> bool {
        self.input.is_empty()
    }
}

/// Widget for rendering a chat view
pub struct ChatViewWidget<'a> {
    state: &'a ChatState,
    room_name: &'a str,
    is_focused: bool,
}

impl<'a> ChatViewWidget<'a> {
    /// Create a new chat view widget
    pub fn new(state: &'a ChatState, room_name: &'a str, is_focused: bool) -> Self {
        Self {
            state,
            room_name,
            is_focused,
        }
    }
}

impl Widget for ChatViewWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Split into messages area and input area
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(area);

        // Render messages
        self.render_messages(chunks[0], buf);

        // Render input
        self.render_input(chunks[1], buf);
    }
}

impl ChatViewWidget<'_> {
    fn render_messages(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(format!(" #{} ", self.room_name))
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(if self.is_focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let inner = block.inner(area);
        block.render(area, buf);

        if self.state.messages.is_empty() {
            // Empty state
            let empty_msg = Paragraph::new("No messages yet")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(ratatui::layout::Alignment::Center);
            empty_msg.render(inner, buf);
            return;
        }

        // Calculate visible messages
        let visible_height = inner.height as usize;
        let total_messages = self.state.messages.len();
        let start_idx = total_messages
            .saturating_sub(visible_height)
            .saturating_sub(self.state.scroll_offset);
        let end_idx = total_messages.saturating_sub(self.state.scroll_offset);

        let items: Vec<ListItem> = self.state.messages[start_idx..end_idx]
            .iter()
            .map(|msg| {
                let sender_style = if msg.is_own {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                };

                let time_style = Style::default().fg(Color::DarkGray);
                let content_style = Style::default().fg(Color::White);

                let line = Line::from(vec![
                    Span::styled(&msg.timestamp, time_style),
                    Span::raw(" "),
                    Span::styled(&msg.sender, sender_style),
                    Span::raw(": "),
                    Span::styled(&msg.content, content_style),
                ]);

                ListItem::new(line)
            })
            .collect();

        let list = List::new(items);
        list.render(inner, buf);

        // Scroll indicator if scrolled up
        if self.state.is_scrolled() {
            let indicator = format!("↑ {} more", self.state.scroll_offset);
            let indicator_area = Rect::new(
                inner.x + inner.width.saturating_sub(indicator.len() as u16 + 1),
                inner.y,
                indicator.len() as u16,
                1,
            );
            let indicator_widget = Paragraph::new(indicator)
                .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
            indicator_widget.render(indicator_area, buf);
        }
    }

    fn render_input(&self, area: Rect, buf: &mut Buffer) {
        let input_style = if self.is_focused {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let cursor = if self.is_focused { "▌" } else { "" };
        let input_text = format!("{}{}", self.state.input, cursor);

        let input = Paragraph::new(input_text)
            .style(input_style)
            .block(
                Block::default()
                    .title(" Message ")
                    .title_style(Style::default().fg(Color::DarkGray))
                    .borders(Borders::ALL)
                    .border_style(if self.is_focused {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    }),
            );

        input.render(area, buf);
    }
}

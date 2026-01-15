//! Sidebar widget for bz
//!
//! Displays channel list with activity indicators.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Widget},
};

use crate::channel::Channel;
use crate::pty::ActivityState;

/// Width of the sidebar in columns
pub const SIDEBAR_WIDTH: u16 = 20;

/// Sidebar widget showing channel list
pub struct Sidebar<'a> {
    channels: &'a [Channel],
    focused: usize,
}

impl<'a> Sidebar<'a> {
    pub fn new(channels: &'a [Channel], focused: usize) -> Self {
        Self { channels, focused }
    }
}

impl Widget for Sidebar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let items: Vec<ListItem> = self
            .channels
            .iter()
            .enumerate()
            .map(|(i, ch)| {
                let is_focused = i == self.focused;
                let activity = ch.activity();

                // Build channel line: " #channel_name"
                let mut spans = vec![Span::raw(" #"), Span::raw(&ch.name)];

                // Activity indicator
                match activity {
                    ActivityState::Idle => {}
                    ActivityState::Active(0) => {
                        spans.push(Span::styled(" *", Style::default().fg(Color::Yellow)));
                    }
                    ActivityState::Active(n) => {
                        spans.push(Span::styled(
                            format!(" ({})", n),
                            Style::default()
                                .fg(Color::Red)
                                .add_modifier(Modifier::BOLD),
                        ));
                    }
                }

                // Style based on focus/activity
                let style = if is_focused {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else if matches!(activity, ActivityState::Active(_)) {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                ListItem::new(Line::from(spans)).style(style)
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().title(" CHANNELS ").borders(Borders::RIGHT));

        Widget::render(list, area, buf);
    }
}

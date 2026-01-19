//! Sidebar widget for bz
//!
//! Displays channel list with activity indicators.
//! Uses Nerd Font icons for visual polish.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Widget},
};

use crate::channel::Channel;
use crate::picker::{HasNameActivity, HasPtyStatus};
use crate::pty::{ActivityState, PtyStatus};

/// Width of the sidebar in columns
pub const SIDEBAR_WIDTH: u16 = 22;

// UI icons (pure Unicode for maximum compatibility)
const ICON_CHANNEL: &str = "#";
const ICON_FOCUSED: &str = "▸";
const ICON_ACTIVITY: &str = "●";
const ICON_BELL: &str = "◆";  // bell count indicator
const ICON_EXITED: &str = "✕";  // process exited indicator

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

                // Build channel line with focus indicator
                let prefix = if is_focused {
                    format!(" {} ", ICON_FOCUSED)
                } else {
                    "   ".to_string()
                };

                let mut spans = vec![
                    Span::styled(
                        prefix,
                        if is_focused {
                            Style::default().fg(Color::Cyan)
                        } else {
                            Style::default()
                        },
                    ),
                    Span::styled(
                        ICON_CHANNEL,
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw(&ch.name),
                ];

                // Exit indicator (takes priority over activity)
                let is_exited = ch.pty.status == PtyStatus::Exited;
                if is_exited {
                    spans.push(Span::styled(
                        format!(" {}", ICON_EXITED),
                        Style::default().fg(Color::Red),
                    ));
                } else {
                    // Activity indicator (only for confirmed activity)
                    match activity {
                        ActivityState::Idle | ActivityState::Pending { .. } => {
                            // No indicator for idle or pending (not yet confirmed)
                        }
                        ActivityState::Active(0) => {
                            // Unread activity (no bells) - yellow dot
                            spans.push(Span::styled(
                                format!(" {}", ICON_ACTIVITY),
                                Style::default().fg(Color::Yellow),
                            ));
                        }
                        ActivityState::Active(n) => {
                            // Bells - red bell icon with count
                            spans.push(Span::styled(
                                format!(" {} {}", ICON_BELL, n),
                                Style::default()
                                    .fg(Color::Red)
                                    .add_modifier(Modifier::BOLD),
                            ));
                        }
                    }
                }

                // Style based on focus/activity/exit status
                let style = if is_focused {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else if is_exited {
                    Style::default().fg(Color::Red)
                } else if matches!(activity, ActivityState::Active(_)) {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                ListItem::new(Line::from(spans)).style(style)
            })
            .collect();

        let version = concat!(" bz v", env!("CARGO_PKG_VERSION"), " ");
        let block = Block::default()
            .title(version)
            .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(Color::DarkGray));

        let list = List::new(items).block(block);

        Widget::render(list, area, buf);
    }
}

impl Sidebar<'_> {
    /// Create sidebar from session channels
    pub fn from_session_channels<'a, T: HasNameActivity + HasPtyStatus>(
        channels: &'a [T],
        focused: usize,
    ) -> SessionSidebar<'a, T> {
        SessionSidebar {
            channels,
            focused,
            rooms: Vec::new(),
        }
    }
}

/// Generic sidebar for session channels
pub struct SessionSidebar<'a, T: HasNameActivity + HasPtyStatus> {
    channels: &'a [T],
    focused: usize,
    /// Matrix rooms to display (id, name)
    rooms: Vec<(String, String)>,
}

impl<'a, T: HasNameActivity + HasPtyStatus> SessionSidebar<'a, T> {
    /// Set Matrix rooms to display
    pub fn with_rooms(mut self, rooms: Vec<(String, String)>) -> Self {
        self.rooms = rooms;
        self
    }
}

impl<T: HasNameActivity + HasPtyStatus> Widget for SessionSidebar<'_, T> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut items: Vec<ListItem> = self
            .channels
            .iter()
            .enumerate()
            .map(|(i, ch)| {
                let is_focused = i == self.focused;
                let activity = ch.activity();

                let prefix = if is_focused {
                    format!(" {} ", ICON_FOCUSED)
                } else {
                    "   ".to_string()
                };

                let mut spans = vec![
                    Span::styled(
                        prefix,
                        if is_focused {
                            Style::default().fg(Color::Cyan)
                        } else {
                            Style::default()
                        },
                    ),
                    Span::styled(
                        ICON_CHANNEL,
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw(ch.name()),
                ];

                let is_exited = *ch.pty_status() == PtyStatus::Exited;
                if is_exited {
                    spans.push(Span::styled(
                        format!(" {}", ICON_EXITED),
                        Style::default().fg(Color::Red),
                    ));
                } else {
                    match activity {
                        ActivityState::Idle | ActivityState::Pending { .. } => {}
                        ActivityState::Active(0) => {
                            spans.push(Span::styled(
                                format!(" {}", ICON_ACTIVITY),
                                Style::default().fg(Color::Yellow),
                            ));
                        }
                        ActivityState::Active(n) => {
                            spans.push(Span::styled(
                                format!(" {} {}", ICON_BELL, n),
                                Style::default()
                                    .fg(Color::Red)
                                    .add_modifier(Modifier::BOLD),
                            ));
                        }
                    }
                }

                let style = if is_focused {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else if is_exited {
                    Style::default().fg(Color::Red)
                } else if matches!(activity, ActivityState::Active(_)) {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                ListItem::new(Line::from(spans)).style(style)
            })
            .collect();

        // Add Matrix rooms section if any rooms provided
        if !self.rooms.is_empty() {
            // Separator
            items.push(ListItem::new(Line::from(vec![
                Span::styled("─── Rooms ───", Style::default().fg(Color::DarkGray)),
            ])));

            for (_id, name) in &self.rooms {
                let spans = vec![
                    Span::raw("   "),
                    Span::styled("◉ ", Style::default().fg(Color::Green)),
                    Span::styled(name.as_str(), Style::default().fg(Color::DarkGray)),
                ];
                items.push(ListItem::new(Line::from(spans)));
            }
        }

        let version = concat!(" bz v", env!("CARGO_PKG_VERSION"), " ");
        let block = Block::default()
            .title(version)
            .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(Color::DarkGray));

        let list = List::new(items).block(block);

        Widget::render(list, area, buf);
    }
}

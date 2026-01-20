//! Room picker overlay
//!
//! A floating picker for quick room switching, modeled after Slack's Cmd+K.
//!
//! Default display (empty query):
//! - Rooms with bells (sorted by count, highest first)
//! - Rooms with activity
//! - Other rooms hidden until you type
//!
//! When typing: fuzzy filter across ALL rooms

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border,
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget},
};

use crate::pty::{ActivityState, PtyStatus};

// UI icons (pure Unicode for maximum compatibility)
const ICON_SEARCH: &str = ">";
const ICON_CHANNEL: &str = "#";
const ICON_SELECTED: &str = "▸";
const ICON_ACTIVITY: &str = "●";
const ICON_BELL: &str = "◆";

/// Room picker state
pub struct Picker {
    /// Current search query
    pub query: String,
    /// Currently selected index in filtered list
    pub selected: usize,
    /// Filtered room indices (into the room list)
    pub filtered: Vec<usize>,
}

impl Picker {
    /// Create a new picker
    pub fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            filtered: Vec::new(),
        }
    }

    /// Get the currently selected index
    pub fn selected_index(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
    }

    /// Update filter for rooms
    pub fn update_filter_from_rooms(&mut self, rooms: &[crate::room_view::RoomView]) {
        if self.query.is_empty() {
            self.filtered = rooms
                .iter()
                .enumerate()
                .filter(|(_, room)| matches!(room.activity(), ActivityState::Active(_)))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(i, _)| i)
                .collect();

            self.filtered.sort_by(|&a, &b| {
                let a_room = &rooms[a];
                let b_room = &rooms[b];

                let a_bells = match a_room.activity() {
                    ActivityState::Active(n) if n > 0 => n,
                    _ => 0,
                };
                let b_bells = match b_room.activity() {
                    ActivityState::Active(n) if n > 0 => n,
                    _ => 0,
                };

                match b_bells.cmp(&a_bells) {
                    std::cmp::Ordering::Equal => a_room.name.cmp(&b_room.name),
                    other => other,
                }
            });
        } else {
            let query_lower = self.query.to_lowercase();
            self.filtered = rooms
                .iter()
                .enumerate()
                .filter(|(_, room)| room.name.to_lowercase().contains(&query_lower))
                .map(|(i, _)| i)
                .collect();
        }

        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    /// Push char for rooms
    pub fn push_char_from_rooms(&mut self, c: char, rooms: &[crate::room_view::RoomView]) {
        self.query.push(c);
        self.selected = 0;
        self.update_filter_from_rooms(rooms);
    }

    /// Pop char for rooms
    pub fn pop_char_from_rooms(&mut self, rooms: &[crate::room_view::RoomView]) {
        self.query.pop();
        self.selected = 0;
        self.update_filter_from_rooms(rooms);
    }
}

/// Trait for types that have a name and activity state
pub trait HasNameActivity {
    #[allow(dead_code)]
    fn name(&self) -> &str;
    fn activity(&self) -> &ActivityState;
}

/// Trait for types that have PTY status
pub trait HasPtyStatus {
    fn pty_status(&self) -> &PtyStatus;
}

/// Picker widget namespace for factory methods
pub struct PickerWidget;

impl PickerWidget {
    /// Create picker widget for rooms
    pub fn from_rooms<'a>(picker: &'a Picker, rooms: &'a [crate::room_view::RoomView]) -> RoomPickerWidget<'a> {
        RoomPickerWidget { picker, rooms }
    }
}

/// Picker widget for rooms
pub struct RoomPickerWidget<'a> {
    picker: &'a Picker,
    rooms: &'a [crate::room_view::RoomView],
}

impl<'a> Widget for RoomPickerWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let picker_width = 50.min(area.width.saturating_sub(4));
        let picker_height = 14.min(area.height.saturating_sub(4));
        let x = area.x + (area.width.saturating_sub(picker_width)) / 2;
        let y = area.y + (area.height.saturating_sub(picker_height)) / 4;
        let picker_area = Rect::new(x, y, picker_width, picker_height);

        Clear.render(picker_area, buf);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(picker_area);

        let input_text = if self.picker.query.is_empty() {
            format!(" {} Type to search...", ICON_SEARCH)
        } else {
            format!(" {} {}", ICON_SEARCH, self.picker.query)
        };
        let input_style = if self.picker.query.is_empty() {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };
        let input = Paragraph::new(input_text)
            .style(input_style)
            .block(
                Block::default()
                    .title(" Switch Room ")
                    .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(Style::default().fg(Color::Cyan))
                    .style(Style::default().bg(Color::Black)),
            );
        input.render(chunks[0], buf);

        let items: Vec<ListItem> = if self.picker.filtered.is_empty() {
            if self.picker.query.is_empty() {
                vec![ListItem::new(format!("   {} No rooms with activity", ICON_ACTIVITY)).style(
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )]
            } else {
                vec![ListItem::new("   No matching rooms").style(
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )]
            }
        } else {
            self.picker
                .filtered
                .iter()
                .enumerate()
                .map(|(i, &room_idx)| {
                    let room = &self.rooms[room_idx];
                    let is_selected = i == self.picker.selected;

                    let prefix = if is_selected {
                        format!(" {} ", ICON_SELECTED)
                    } else {
                        "   ".to_string()
                    };

                    let activity_indicator = match room.activity() {
                        ActivityState::Active(0) => format!(" {}", ICON_ACTIVITY),
                        ActivityState::Active(n) => format!(" {} {}", ICON_BELL, n),
                        _ => String::new(),
                    };

                    let text = format!("{}{}{}{}", prefix, ICON_CHANNEL, room.name, activity_indicator);

                    let style = if is_selected {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    ListItem::new(text).style(style)
                })
                .collect()
        };

        let mut bottom_border = border::ROUNDED;
        bottom_border.top_left = border::ROUNDED.vertical_left;
        bottom_border.top_right = border::ROUNDED.vertical_right;

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                .border_set(bottom_border)
                .border_style(Style::default().fg(Color::Cyan))
                .style(Style::default().bg(Color::Black)),
        );
        list.render(chunks[1], buf);
    }
}

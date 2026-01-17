//! Channel picker overlay
//!
//! A floating picker for quick channel switching, modeled after Slack's Cmd+K.
//!
//! Default display (empty query):
//! - Channels with bells (sorted by count, highest first)
//! - Channels with activity
//! - Other channels hidden until you type
//!
//! When typing: fuzzy filter across ALL channels

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border,
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget},
};

use crate::channel::Channel;
use crate::pty::ActivityState;

// UI icons (pure Unicode for maximum compatibility)
const ICON_SEARCH: &str = ">";
const ICON_CHANNEL: &str = "#";
const ICON_SELECTED: &str = "▸";
const ICON_ACTIVITY: &str = "●";
const ICON_BELL: &str = "◆";

/// Channel picker state
pub struct Picker {
    /// Current search query
    pub query: String,
    /// Currently selected index in filtered list
    pub selected: usize,
    /// Filtered channel indices (into the channel list)
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

    /// Update the filtered list based on current query and channel states
    ///
    /// When query is empty: show channels with bells/activity only, sorted by priority
    /// When query has text: fuzzy filter all channels
    pub fn update_filter(&mut self, channels: &[Channel]) {
        if self.query.is_empty() {
            // Slack-style: only show channels with notifications/activity
            self.filtered = channels
                .iter()
                .enumerate()
                .filter(|(_, ch)| matches!(ch.activity(), ActivityState::Active(_)))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(i, _)| i)
                .collect();

            // Sort by priority: bells (desc) > activity > alphabetical
            self.filtered.sort_by(|&a, &b| {
                let a_ch = &channels[a];
                let b_ch = &channels[b];

                // Get bell counts (Active with n > 0 means bells)
                let a_bells = match a_ch.activity() {
                    ActivityState::Active(n) if *n > 0 => *n,
                    _ => 0,
                };
                let b_bells = match b_ch.activity() {
                    ActivityState::Active(n) if *n > 0 => *n,
                    _ => 0,
                };

                // Sort by bells descending, then by name
                match b_bells.cmp(&a_bells) {
                    std::cmp::Ordering::Equal => a_ch.name.cmp(&b_ch.name),
                    other => other,
                }
            });
        } else {
            // Filter all channels by query (case-insensitive contains)
            let query_lower = self.query.to_lowercase();
            self.filtered = channels
                .iter()
                .enumerate()
                .filter(|(_, ch)| ch.name.to_lowercase().contains(&query_lower))
                .map(|(i, _)| i)
                .collect();
        }

        // Keep selection in bounds
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    /// Get the currently selected channel index
    pub fn selected_channel(&self) -> Option<usize> {
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

    /// Add a character to the query
    pub fn push_char(&mut self, c: char, channels: &[Channel]) {
        self.query.push(c);
        self.selected = 0; // Reset selection on query change
        self.update_filter(channels);
    }

    /// Remove a character from the query
    pub fn pop_char(&mut self, channels: &[Channel]) {
        self.query.pop();
        self.selected = 0; // Reset selection on query change
        self.update_filter(channels);
    }
}

/// Picker widget for rendering
pub struct PickerWidget<'a> {
    picker: &'a Picker,
    channels: &'a [Channel],
}

impl<'a> PickerWidget<'a> {
    pub fn new(picker: &'a Picker, channels: &'a [Channel]) -> Self {
        Self { picker, channels }
    }
}

impl Widget for PickerWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Calculate picker dimensions (centered, upper third)
        let picker_width = 50.min(area.width.saturating_sub(4));
        let picker_height = 14.min(area.height.saturating_sub(4));
        let x = area.x + (area.width.saturating_sub(picker_width)) / 2;
        let y = area.y + (area.height.saturating_sub(picker_height)) / 4;
        let picker_area = Rect::new(x, y, picker_width, picker_height);

        // Clear background
        Clear.render(picker_area, buf);

        // Split into input and list areas
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(picker_area);

        // Input box with query and search icon
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
                    .title(" Switch Channel ")
                    .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(Style::default().fg(Color::Cyan))
                    .style(Style::default().bg(Color::Black)),
            );
        input.render(chunks[0], buf);

        // Channel list
        let items: Vec<ListItem> = if self.picker.filtered.is_empty() {
            if self.picker.query.is_empty() {
                vec![ListItem::new(format!("   {} No channels with activity", ICON_ACTIVITY)).style(
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )]
            } else {
                vec![ListItem::new("   No matching channels").style(
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )]
            }
        } else {
            self.picker
                .filtered
                .iter()
                .enumerate()
                .map(|(i, &ch_idx)| {
                    let ch = &self.channels[ch_idx];
                    let is_selected = i == self.picker.selected;

                    // Selection indicator
                    let prefix = if is_selected {
                        format!(" {} ", ICON_SELECTED)
                    } else {
                        "   ".to_string()
                    };

                    // Build channel name with activity indicator
                    let mut text = format!("{}{}{}", prefix, ICON_CHANNEL, ch.name);
                    match ch.activity() {
                        ActivityState::Active(n) if *n > 0 => {
                            text.push_str(&format!("  {} {}", ICON_BELL, n));
                        }
                        ActivityState::Active(_) => {
                            text.push_str(&format!("  {}", ICON_ACTIVITY));
                        }
                        _ => {}
                    }

                    let style = if is_selected {
                        Style::default()
                            .bg(Color::Rgb(40, 80, 120))
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    ListItem::new(text).style(style)
                })
                .collect()
        };

        // Use rounded border set for bottom part too
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

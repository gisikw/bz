//! Room and agent picker overlay
//!
//! A floating picker for quick room/agent switching, modeled after Slack's Cmd+K.
//!
//! Default display (empty query):
//! - Rooms with activity (sorted by name)
//! - All agents (sorted by name)
//!
//! When typing: fuzzy filter across both rooms and agents

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
const ICON_AGENT: &str = "@";
const ICON_SELECTED: &str = "▸";
const ICON_ACTIVITY: &str = "●";

/// Item in the picker - either a channel or an agent
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickerItem {
    /// Channel (room) with index into rooms array
    Channel(usize),
    /// Agent with index into agents array
    Agent(usize),
}

/// Room picker state
pub struct Picker {
    /// Current search query
    pub query: String,
    /// Currently selected index in filtered list
    pub selected: usize,
    /// Filtered items (channels and agents)
    pub filtered: Vec<PickerItem>,
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

    /// Get the currently selected item
    pub fn selected_item(&self) -> Option<PickerItem> {
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

    /// Update filter for both rooms and agents
    pub fn update_filter<A: HasAgentName>(&mut self, rooms: &[crate::room_view::RoomView], agents: &[A]) {
        self.filtered.clear();

        // Build set of room indices that are agent DMs (to exclude from channels)
        let agent_dm_indices: std::collections::HashSet<usize> = agents
            .iter()
            .filter_map(|a| a.dm_room_idx())
            .collect();

        if self.query.is_empty() {
            // Show rooms with activity (excluding agent DMs) + all agents
            let mut channel_items: Vec<_> = rooms
                .iter()
                .enumerate()
                .filter(|(i, room)| room.activity() == ActivityState::Active && !agent_dm_indices.contains(i))
                .map(|(i, _)| PickerItem::Channel(i))
                .collect();

            // Sort channels by name
            channel_items.sort_by(|a, b| {
                let name_a = match a {
                    PickerItem::Channel(i) => &rooms[*i].name,
                    _ => "",
                };
                let name_b = match b {
                    PickerItem::Channel(i) => &rooms[*i].name,
                    _ => "",
                };
                name_a.cmp(name_b)
            });

            // All agents sorted by name
            let mut agent_items: Vec<_> = agents
                .iter()
                .enumerate()
                .map(|(i, _)| PickerItem::Agent(i))
                .collect();
            agent_items.sort_by(|a, b| {
                let name_a = match a {
                    PickerItem::Agent(i) => agents[*i].agent_name(),
                    _ => "",
                };
                let name_b = match b {
                    PickerItem::Agent(i) => agents[*i].agent_name(),
                    _ => "",
                };
                name_a.cmp(name_b)
            });

            self.filtered.extend(channel_items);
            self.filtered.extend(agent_items);
        } else {
            let query_lower = self.query.to_lowercase();

            // Filter channels by name (excluding agent DMs)
            let mut matching: Vec<_> = rooms
                .iter()
                .enumerate()
                .filter(|(i, room)| room.name.to_lowercase().contains(&query_lower) && !agent_dm_indices.contains(i))
                .map(|(i, _)| PickerItem::Channel(i))
                .collect();

            // Filter agents by name
            let agent_matches: Vec<_> = agents
                .iter()
                .enumerate()
                .filter(|(_, agent)| agent.agent_name().to_lowercase().contains(&query_lower))
                .map(|(i, _)| PickerItem::Agent(i))
                .collect();

            matching.extend(agent_matches);

            // Sort all by name
            matching.sort_by(|a, b| {
                let name_a = match a {
                    PickerItem::Channel(i) => rooms[*i].name.as_str(),
                    PickerItem::Agent(i) => agents[*i].agent_name(),
                };
                let name_b = match b {
                    PickerItem::Channel(i) => rooms[*i].name.as_str(),
                    PickerItem::Agent(i) => agents[*i].agent_name(),
                };
                name_a.cmp(name_b)
            });

            self.filtered = matching;
        }

        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
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

/// Trait for agent entries that have a name
pub trait HasAgentName {
    fn agent_name(&self) -> &str;
    /// Returns the DM room index if the agent has an open DM
    fn dm_room_idx(&self) -> Option<usize>;
}

/// Picker widget namespace for factory methods
pub struct PickerWidget;

impl PickerWidget {
    /// Create picker widget for rooms and agents
    pub fn from_rooms_and_agents<'a, A: HasAgentName + HasAgentActivity>(
        picker: &'a Picker,
        rooms: &'a [crate::room_view::RoomView],
        agents: &'a [A],
    ) -> RoomPickerWidget<'a, A> {
        RoomPickerWidget { picker, rooms, agents }
    }
}

/// Trait for checking agent activity (via DM room)
pub trait HasAgentActivity {
    fn has_activity(&self, rooms: &[crate::room_view::RoomView]) -> bool;
}

/// Picker widget for rooms and agents
pub struct RoomPickerWidget<'a, A: HasAgentName + HasAgentActivity = NoAgent> {
    picker: &'a Picker,
    rooms: &'a [crate::room_view::RoomView],
    agents: &'a [A],
}

/// Placeholder type for no agents
pub struct NoAgent;

impl HasAgentName for NoAgent {
    fn agent_name(&self) -> &str {
        ""
    }

    fn dm_room_idx(&self) -> Option<usize> {
        None
    }
}

impl HasAgentActivity for NoAgent {
    fn has_activity(&self, _rooms: &[crate::room_view::RoomView]) -> bool {
        false
    }
}

impl<'a, A: HasAgentName + HasAgentActivity> Widget for RoomPickerWidget<'a, A> {
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
                vec![ListItem::new("   No matching items").style(
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )]
            }
        } else {
            self.picker
                .filtered
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    let is_selected = i == self.picker.selected;

                    let prefix = if is_selected {
                        format!(" {} ", ICON_SELECTED)
                    } else {
                        "   ".to_string()
                    };

                    let (icon, name, has_activity) = match item {
                        PickerItem::Channel(idx) => {
                            let room = &self.rooms[*idx];
                            (ICON_CHANNEL, room.name.as_str(), room.activity() == ActivityState::Active)
                        }
                        PickerItem::Agent(idx) => {
                            let agent = &self.agents[*idx];
                            (ICON_AGENT, agent.agent_name(), agent.has_activity(self.rooms))
                        }
                    };

                    let activity_indicator = if has_activity {
                        format!(" {}", ICON_ACTIVITY)
                    } else {
                        String::new()
                    };

                    let text = format!("{}{}{}{}", prefix, icon, name, activity_indicator);

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

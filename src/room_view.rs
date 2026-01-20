//! Room view model
//!
//! Each room has multiple screens: chat (screen 0) + PTYs (screens 1..n).

use color_eyre::eyre::Result;

use crate::chaperone_channel::ChaperoneChannel;
use crate::chat_view::ChatState;
use crate::picker::HasNameActivity;
use crate::pty::ActivityState;

/// A screen within a room
pub enum Screen {
    /// Chat screen (always at index 0)
    Chat(ChatState),
    /// PTY screen
    Pty(ChaperoneChannel),
}

// Some methods reserved for future use
#[allow(dead_code)]
impl Screen {
    /// Create a new chat screen
    pub fn chat() -> Self {
        Screen::Chat(ChatState::new())
    }

    /// Create a new PTY screen from a ChaperoneChannel
    pub fn pty(channel: ChaperoneChannel) -> Self {
        Screen::Pty(channel)
    }

    /// Get activity state for this screen
    pub fn activity(&self) -> ActivityState {
        match self {
            Screen::Chat(state) => {
                if state.has_unread {
                    ActivityState::Active
                } else {
                    ActivityState::Idle
                }
            }
            Screen::Pty(channel) => channel.activity().clone(),
        }
    }

    /// Check if this is a chat screen
    pub fn is_chat(&self) -> bool {
        matches!(self, Screen::Chat(_))
    }

    /// Get the PTY channel if this is a PTY screen
    pub fn as_pty(&self) -> Option<&ChaperoneChannel> {
        match self {
            Screen::Pty(channel) => Some(channel),
            _ => None,
        }
    }

    /// Get mutable PTY channel if this is a PTY screen
    pub fn as_pty_mut(&mut self) -> Option<&mut ChaperoneChannel> {
        match self {
            Screen::Pty(channel) => Some(channel),
            _ => None,
        }
    }

    /// Get the chat state if this is a chat screen
    pub fn as_chat(&self) -> Option<&ChatState> {
        match self {
            Screen::Chat(state) => Some(state),
            _ => None,
        }
    }

    /// Get mutable chat state if this is a chat screen
    pub fn as_chat_mut(&mut self) -> Option<&mut ChatState> {
        match self {
            Screen::Chat(state) => Some(state),
            _ => None,
        }
    }
}

/// A room with multiple screens
pub struct RoomView {
    /// Room ID (Matrix room ID or "default" for local-only)
    pub room_id: String,
    /// Display name for the room
    pub name: String,
    /// Current screen index (0 = chat, 1+ = PTYs)
    current_screen: usize,
    /// All screens in this room
    screens: Vec<Screen>,
}

// Some methods reserved for future use
#[allow(dead_code)]
impl RoomView {
    /// Create a new room view with just a chat screen
    pub fn new(room_id: String, name: String) -> Self {
        Self {
            room_id,
            name,
            current_screen: 0,
            screens: vec![Screen::chat()],
        }
    }

    /// Create a room view with an initial PTY (starts on PTY screen)
    pub fn with_pty(room_id: String, name: String, channel: ChaperoneChannel) -> Self {
        Self {
            room_id,
            name,
            current_screen: 1, // Start on the PTY
            screens: vec![Screen::chat(), Screen::pty(channel)],
        }
    }

    /// Get current screen index
    pub fn current_screen_index(&self) -> usize {
        self.current_screen
    }

    /// Get total number of screens
    pub fn screen_count(&self) -> usize {
        self.screens.len()
    }

    /// Get current screen
    pub fn current_screen(&self) -> &Screen {
        &self.screens[self.current_screen]
    }

    /// Get current screen mutably
    pub fn current_screen_mut(&mut self) -> &mut Screen {
        &mut self.screens[self.current_screen]
    }

    /// Navigate to previous screen (h key)
    pub fn prev_screen(&mut self) {
        if self.current_screen > 0 {
            self.current_screen -= 1;
        }
    }

    /// Navigate to next screen (l key)
    pub fn next_screen(&mut self) {
        if self.current_screen + 1 < self.screens.len() {
            self.current_screen += 1;
        }
    }

    /// Add a PTY screen and optionally switch to it
    pub fn add_pty(&mut self, channel: ChaperoneChannel, switch_to: bool) {
        self.screens.push(Screen::pty(channel));
        if switch_to {
            self.current_screen = self.screens.len() - 1;
        }
    }

    /// Get all PTY screens
    pub fn ptys(&self) -> impl Iterator<Item = &ChaperoneChannel> {
        self.screens.iter().filter_map(|s| s.as_pty())
    }

    /// Get all PTY screens mutably
    pub fn ptys_mut(&mut self) -> impl Iterator<Item = &mut ChaperoneChannel> {
        self.screens.iter_mut().filter_map(|s| s.as_pty_mut())
    }

    /// Check if current screen is chat
    pub fn on_chat(&self) -> bool {
        self.current_screen == 0
    }

    /// Check if current screen is a PTY
    pub fn on_pty(&self) -> bool {
        self.current_screen > 0
    }

    /// Get the current PTY if on a PTY screen
    pub fn current_pty(&self) -> Option<&ChaperoneChannel> {
        self.current_screen().as_pty()
    }

    /// Get the current PTY mutably if on a PTY screen
    pub fn current_pty_mut(&mut self) -> Option<&mut ChaperoneChannel> {
        self.current_screen_mut().as_pty_mut()
    }

    /// Get the chat state (screen 0)
    pub fn chat_state(&self) -> Option<&ChatState> {
        self.screens.first().and_then(|s| s.as_chat())
    }

    /// Get the chat state mutably
    pub fn chat_state_mut(&mut self) -> Option<&mut ChatState> {
        self.screens.first_mut().and_then(|s| s.as_chat_mut())
    }

    /// Get the current chat if on chat screen
    pub fn current_chat(&self) -> Option<&ChatState> {
        self.current_screen().as_chat()
    }

    /// Get the current chat mutably if on chat screen
    pub fn current_chat_mut(&mut self) -> Option<&mut ChatState> {
        self.current_screen_mut().as_chat_mut()
    }

    /// Get aggregate activity for this room (for sidebar)
    pub fn activity(&self) -> ActivityState {
        // Return Active if any screen has activity
        for screen in &self.screens {
            if screen.activity() == ActivityState::Active {
                return ActivityState::Active;
            }
        }
        ActivityState::Idle
    }

    /// Process pending output for all PTYs
    pub fn process_pending(&mut self, is_focused_room: bool) {
        for (i, screen) in self.screens.iter_mut().enumerate() {
            if let Screen::Pty(channel) = screen {
                let is_focused_screen = is_focused_room && i == self.current_screen;
                channel.process_pending(is_focused_screen);
            }
        }
    }

    /// Clear activity on current screen
    pub fn clear_current_activity(&mut self) {
        match self.current_screen_mut() {
            Screen::Pty(channel) => {
                channel.clear_activity();
            }
            Screen::Chat(state) => {
                state.has_unread = false;
            }
        }
    }

    /// Handle screen focus change within this room
    ///
    /// Disconnects the old screen's PTY (if any) and connects the new screen's PTY (if any).
    pub async fn handle_screen_focus_change(
        &mut self,
        old_screen: usize,
        new_screen: usize,
    ) -> Result<()> {
        // Disconnect old screen if it's a PTY
        if let Some(Screen::Pty(channel)) = self.screens.get_mut(old_screen) {
            channel.disconnect();
        }

        // Connect new screen if it's a PTY
        if let Some(Screen::Pty(channel)) = self.screens.get_mut(new_screen) {
            channel.connect().await?;
        }

        Ok(())
    }

    /// Called when this room gains focus
    ///
    /// Connects the current screen's PTY if applicable.
    pub async fn on_focus_gained(&mut self) -> Result<()> {
        if let Some(channel) = self.current_pty_mut() {
            channel.connect().await?;
        }
        Ok(())
    }

    /// Called when this room loses focus
    ///
    /// Disconnects all PTYs in this room.
    pub fn on_focus_lost(&mut self) {
        for screen in &mut self.screens {
            if let Screen::Pty(channel) = screen {
                channel.disconnect();
            }
        }
    }

    /// Disconnect all PTYs in this room
    pub fn disconnect_all(&mut self) {
        for screen in &mut self.screens {
            if let Screen::Pty(channel) = screen {
                channel.disconnect();
            }
        }
    }

    /// Connect the current screen's PTY if applicable
    pub async fn connect_current(&mut self) -> Result<()> {
        if let Some(channel) = self.current_pty_mut() {
            channel.connect().await?;
        }
        Ok(())
    }
}

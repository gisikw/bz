//! Matrix client wrapper for bz
//!
//! Handles user registration, login, and sync with the local Conduit server.

use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::eyre::{eyre, Result, WrapErr};
use matrix_sdk::{
    config::SyncSettings,
    ruma::{
        api::client::account::register::v3::Request as RegistrationRequest,
        events::{
            room::message::{MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent},
            StateEventType,
        },
        OwnedDeviceId, OwnedRoomId, OwnedUserId,
    },
    Client, Room,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// A chat message received from Matrix
#[derive(Clone, Debug)]
pub struct MatrixMessage {
    /// Room ID the message was sent to
    pub room_id: String,
    /// Sender user ID
    pub sender: String,
    /// Sender display name (if available)
    pub sender_display_name: Option<String>,
    /// Message content (text)
    pub content: String,
    /// Timestamp (Unix milliseconds)
    pub timestamp: u64,
}

/// Custom state event type for attached PTYs
const ATTACHED_PTYS_EVENT_TYPE: &str = "bz.attached_ptys";

/// Content for the bz.attached_ptys state event
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AttachedPtysContent {
    /// List of attached PTYs with their info
    pub ptys: Vec<AttachedPty>,
}

/// Information about an attached PTY
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AttachedPty {
    /// PTY ID (UUID)
    pub pty_id: String,
    /// User who owns this PTY
    pub user_id: String,
    /// Socket path for connecting
    pub socket: String,
    /// Command being run
    pub command: String,
}

/// Stored credentials for Matrix login
#[derive(Debug, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub user_id: String,
    pub device_id: String,
    pub access_token: String,
    pub homeserver: String,
}

impl StoredCredentials {
    /// Path to credentials file for the main user
    pub fn path() -> PathBuf {
        Self::path_for_user("user")
    }

    /// Path to credentials file for a specific user/agent
    pub fn path_for_user(name: &str) -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(format!("bz/matrix/{}.json", name))
    }

    /// Load credentials from disk
    pub fn load() -> Result<Option<Self>> {
        Self::load_from(&Self::path())
    }

    /// Load credentials from a specific path
    pub fn load_from(path: &PathBuf) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(path)
            .wrap_err("Failed to read credentials file")?;
        let creds: Self = serde_json::from_str(&content)
            .wrap_err("Failed to parse credentials")?;
        Ok(Some(creds))
    }

    /// Save credentials to disk (default path)
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    /// Save credentials to a specific path
    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .wrap_err("Failed to create credentials directory")?;
        }
        let content = serde_json::to_string_pretty(self)
            .wrap_err("Failed to serialize credentials")?;
        std::fs::write(path, content)
            .wrap_err("Failed to write credentials file")?;
        Ok(())
    }
}

/// Matrix client for bz
pub struct BzMatrixClient {
    client: Client,
    user_id: OwnedUserId,
}

impl BzMatrixClient {
    /// Get the underlying matrix-sdk Client
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Get the logged-in user ID
    pub fn user_id(&self) -> &OwnedUserId {
        &self.user_id
    }

    /// Register a new user or login with existing credentials
    ///
    /// Flow:
    /// 1. Check for stored credentials, try to restore session
    /// 2. If no credentials, try to register new account
    /// 3. Save credentials on success
    pub async fn register_or_login(
        homeserver: &str,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        // Check for existing credentials
        if let Some(creds) = StoredCredentials::load()? {
            if creds.homeserver == homeserver {
                match Self::restore_session(&creds).await {
                    Ok(client) => {
                        eprintln!("bz: restored Matrix session for {}", creds.user_id);
                        return Ok(client);
                    }
                    Err(e) => {
                        eprintln!("bz: failed to restore session: {}, will re-login", e);
                    }
                }
            }
        }

        // Build client
        let client = Client::builder()
            .homeserver_url(homeserver)
            .build()
            .await
            .wrap_err("Failed to build Matrix client")?;

        // Try login first (account may already exist)
        match client
            .matrix_auth()
            .login_username(username, password)
            .send()
            .await
        {
            Ok(response) => {
                eprintln!("bz: logged in as {}", response.user_id);
                let access_token = client
                    .session()
                    .map(|s| s.access_token().to_string())
                    .unwrap_or_default();
                let creds = StoredCredentials {
                    user_id: response.user_id.to_string(),
                    device_id: response.device_id.to_string(),
                    access_token,
                    homeserver: homeserver.to_string(),
                };
                creds.save()?;
                return Ok(Self {
                    client,
                    user_id: response.user_id,
                });
            }
            Err(e) => {
                eprintln!("bz: login failed ({}), attempting registration", e);
            }
        }

        // Try registration
        let mut request = RegistrationRequest::new();
        request.username = Some(username.to_string());
        request.password = Some(password.to_string());

        let response = client
            .matrix_auth()
            .register(request)
            .await
            .wrap_err("Failed to register Matrix account")?;

        let user_id = response.user_id;
        eprintln!("bz: registered new user {}", user_id);

        // Get access token from session (registration should have set it)
        let access_token = client
            .session()
            .map(|s| s.access_token().to_string())
            .unwrap_or_default();

        // Save credentials
        let creds = StoredCredentials {
            user_id: user_id.to_string(),
            device_id: response
                .device_id
                .map(|d| d.to_string())
                .unwrap_or_default(),
            access_token,
            homeserver: homeserver.to_string(),
        };
        creds.save()?;

        Ok(Self { client, user_id })
    }

    /// Register or login for an agent (uses per-agent credential storage)
    pub async fn agent_register_or_login(
        homeserver: &str,
        agent_name: &str,
        password: &str,
    ) -> Result<Self> {
        let creds_path = StoredCredentials::path_for_user(&format!("agent-{}", agent_name));

        // Check for existing credentials
        if let Some(creds) = StoredCredentials::load_from(&creds_path)? {
            if creds.homeserver == homeserver {
                match Self::restore_session(&creds).await {
                    Ok(client) => {
                        eprintln!("bzc: restored Matrix session for agent {}", creds.user_id);
                        return Ok(client);
                    }
                    Err(e) => {
                        eprintln!("bzc: failed to restore agent session: {}, will re-login", e);
                    }
                }
            }
        }

        // Build client
        let client = Client::builder()
            .homeserver_url(homeserver)
            .build()
            .await
            .wrap_err("Failed to build Matrix client")?;

        // Try login first (account may already exist)
        match client
            .matrix_auth()
            .login_username(agent_name, password)
            .send()
            .await
        {
            Ok(response) => {
                eprintln!("bzc: agent logged in as {}", response.user_id);
                let access_token = client
                    .session()
                    .map(|s| s.access_token().to_string())
                    .unwrap_or_default();
                let creds = StoredCredentials {
                    user_id: response.user_id.to_string(),
                    device_id: response.device_id.to_string(),
                    access_token,
                    homeserver: homeserver.to_string(),
                };
                creds.save_to(&creds_path)?;
                return Ok(Self {
                    client,
                    user_id: response.user_id,
                });
            }
            Err(e) => {
                eprintln!("bzc: agent login failed ({}), attempting registration", e);
            }
        }

        // Try registration
        let mut request = RegistrationRequest::new();
        request.username = Some(agent_name.to_string());
        request.password = Some(password.to_string());

        let response = client
            .matrix_auth()
            .register(request)
            .await
            .wrap_err("Failed to register agent Matrix account")?;

        let user_id = response.user_id;
        eprintln!("bzc: registered new agent {}", user_id);

        // Get access token from session
        let access_token = client
            .session()
            .map(|s| s.access_token().to_string())
            .unwrap_or_default();

        // Save credentials
        let creds = StoredCredentials {
            user_id: user_id.to_string(),
            device_id: response
                .device_id
                .map(|d| d.to_string())
                .unwrap_or_default(),
            access_token,
            homeserver: homeserver.to_string(),
        };
        creds.save_to(&creds_path)?;

        Ok(Self { client, user_id })
    }

    /// Restore a session from stored credentials
    async fn restore_session(creds: &StoredCredentials) -> Result<Self> {
        let client = Client::builder()
            .homeserver_url(&creds.homeserver)
            .build()
            .await
            .wrap_err("Failed to build Matrix client")?;

        // Parse user_id
        let user_id: OwnedUserId = creds
            .user_id
            .parse()
            .wrap_err("Invalid user_id in credentials")?;

        // Restore the session
        let device_id: OwnedDeviceId = creds.device_id.as_str().into();
        let session = matrix_sdk::matrix_auth::MatrixSession {
            meta: matrix_sdk::SessionMeta {
                user_id: user_id.clone(),
                device_id,
            },
            tokens: matrix_sdk::matrix_auth::MatrixSessionTokens {
                access_token: creds.access_token.clone(),
                refresh_token: None,
            },
        };

        client
            .matrix_auth()
            .restore_session(session)
            .await
            .wrap_err("Failed to restore Matrix session")?;

        Ok(Self { client, user_id })
    }

    /// Start the sync loop with message handling
    ///
    /// Returns a receiver for incoming chat messages.
    /// The sync runs in the background and updates room state.
    pub fn start_sync(&self) -> mpsc::Receiver<MatrixMessage> {
        let (tx, rx) = mpsc::channel(256);
        let client = self.client.clone();
        let own_user_id = self.user_id.clone();

        // Add event handler for room messages
        client.add_event_handler({
            let tx = tx.clone();
            move |event: OriginalSyncRoomMessageEvent, room: Room| {
                let tx = tx.clone();
                let own_user_id = own_user_id.clone();
                async move {
                    // Skip messages from ourselves
                    if event.sender == own_user_id {
                        return;
                    }

                    // Extract text content
                    let content = match &event.content.msgtype {
                        MessageType::Text(text) => text.body.clone(),
                        MessageType::Notice(notice) => notice.body.clone(),
                        MessageType::Emote(emote) => format!("* {}", emote.body),
                        _ => return, // Skip non-text messages
                    };

                    // Get sender display name
                    let sender_display_name = room
                        .get_member(&event.sender)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|m| m.display_name().map(|s| s.to_string()));

                    let msg = MatrixMessage {
                        room_id: room.room_id().to_string(),
                        sender: event.sender.to_string(),
                        sender_display_name,
                        content,
                        timestamp: event.origin_server_ts.as_secs().into(),
                    };

                    let _ = tx.send(msg).await;
                }
            }
        });

        // Start sync in background
        tokio::spawn(async move {
            let settings = SyncSettings::default();
            if let Err(e) = client.sync(settings).await {
                eprintln!("bz: Matrix sync error: {}", e);
            }
        });

        rx
    }

    /// Get list of joined rooms
    pub fn rooms(&self) -> Vec<matrix_sdk::Room> {
        self.client.rooms()
    }

    /// Send a text message to a room
    pub async fn send_message(&self, room_id: &str, message: &str) -> Result<()> {
        let room_id: OwnedRoomId = room_id.parse().wrap_err("Invalid room ID")?;
        let room = self
            .client
            .get_room(&room_id)
            .ok_or_else(|| eyre!("Room not found: {}", room_id))?;

        let content = RoomMessageEventContent::text_plain(message);
        room.send(content)
            .await
            .wrap_err("Failed to send message")?;

        Ok(())
    }

    /// Get list of joined room names (for sidebar)
    ///
    /// Returns (room_id, display_name) pairs. Display name falls back to room_id
    /// if not available synchronously.
    pub async fn room_names(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for room in self.client.rooms() {
            let id = room.room_id().to_string();
            let name = room
                .display_name()
                .await
                .map(|n| n.to_string())
                .unwrap_or_else(|_| id.clone());
            result.push((id, name));
        }
        result
    }

    /// Create a new room with the given name
    ///
    /// Returns the room ID of the created room.
    pub async fn create_room(&self, name: &str) -> Result<String> {
        use matrix_sdk::ruma::api::client::room::create_room::v3::Request as CreateRoomRequest;

        let mut request = CreateRoomRequest::new();
        request.name = Some(name.to_string());
        request.is_direct = false;

        let response = self
            .client
            .create_room(request)
            .await
            .wrap_err_with(|| format!("Failed to create room '{}'", name))?;

        Ok(response.room_id().to_string())
    }

    /// Ensure rooms exist for the given channel names
    ///
    /// Creates rooms that don't exist, returns mapping of name -> room_id.
    pub async fn ensure_rooms_for_channels(&self, channel_names: &[String]) -> Result<Vec<(String, String)>> {
        // Get existing rooms
        let existing = self.room_names().await;
        let existing_names: std::collections::HashSet<_> = existing.iter().map(|(_, name)| name.clone()).collect();

        let mut mappings = Vec::new();

        for name in channel_names {
            if let Some((id, _)) = existing.iter().find(|(_, n)| n == name) {
                // Room already exists
                mappings.push((name.clone(), id.clone()));
            } else if !existing_names.contains(name) {
                // Create new room
                match self.create_room(name).await {
                    Ok(room_id) => {
                        eprintln!("bz: created Matrix room '{}' -> {}", name, room_id);
                        mappings.push((name.clone(), room_id));
                    }
                    Err(e) => {
                        eprintln!("bz: failed to create room '{}': {}", name, e);
                    }
                }
            }
        }

        Ok(mappings)
    }

    /// Update attached PTYs state in a room
    pub async fn set_attached_ptys(&self, room_id: &str, ptys: Vec<AttachedPty>) -> Result<()> {
        let room_id: OwnedRoomId = room_id.parse().wrap_err("Invalid room ID")?;
        let room = self
            .client
            .get_room(&room_id)
            .ok_or_else(|| eyre!("Room not found: {}", room_id))?;

        let content = AttachedPtysContent { ptys };

        // Convert to JSON value for raw state event
        let content_value: serde_json::Value = serde_json::to_value(&content)
            .wrap_err("Failed to serialize attached_ptys content")?;

        // Send raw state event
        room.send_state_event_raw(ATTACHED_PTYS_EVENT_TYPE, "", content_value)
            .await
            .wrap_err("Failed to set attached_ptys state")?;

        Ok(())
    }

    /// Add a PTY to the attached list in a room
    pub async fn attach_pty(&self, room_id: &str, pty: AttachedPty) -> Result<()> {
        let mut ptys = self.get_attached_ptys(room_id).await.unwrap_or_default();

        // Remove any existing entry for this PTY ID
        ptys.retain(|p| p.pty_id != pty.pty_id);
        ptys.push(pty);

        self.set_attached_ptys(room_id, ptys).await
    }

    /// Remove a PTY from the attached list in a room
    pub async fn detach_pty(&self, room_id: &str, pty_id: &str) -> Result<()> {
        let mut ptys = self.get_attached_ptys(room_id).await.unwrap_or_default();
        ptys.retain(|p| p.pty_id != pty_id);
        self.set_attached_ptys(room_id, ptys).await
    }

    /// Get attached PTYs for a room
    pub async fn get_attached_ptys(&self, room_id: &str) -> Result<Vec<AttachedPty>> {
        let room_id: OwnedRoomId = room_id.parse().wrap_err("Invalid room ID")?;
        let room = self
            .client
            .get_room(&room_id)
            .ok_or_else(|| eyre!("Room not found: {}", room_id))?;

        // Try to get the raw state event
        let event_type = StateEventType::from(ATTACHED_PTYS_EVENT_TYPE);
        let state_event = room
            .get_state_event(event_type, "")
            .await
            .wrap_err("Failed to get room state")?;

        match state_event {
            Some(raw) => {
                use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState;
                // The raw event contains the full Matrix event JSON
                // We need to extract the "content" field
                let json_str = match &raw {
                    RawAnySyncOrStrippedState::Sync(r) => r.json().get(),
                    RawAnySyncOrStrippedState::Stripped(r) => r.json().get(),
                };
                let value: serde_json::Value = serde_json::from_str(json_str)
                    .wrap_err("Failed to parse state event JSON")?;
                if let Some(content) = value.get("content") {
                    let attached: AttachedPtysContent = serde_json::from_value(content.clone())
                        .wrap_err("Failed to deserialize attached_ptys content")?;
                    Ok(attached.ptys)
                } else {
                    Ok(Vec::new())
                }
            }
            None => Ok(Vec::new()),
        }
    }
}

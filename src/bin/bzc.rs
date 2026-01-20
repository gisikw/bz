//! bzc - bz chaperone
//!
//! The chaperone process manages PTYs for a single principal (user or agent).
//! In PTY-only mode, it just manages PTY sockets.
//! In Matrix mode, it also connects to Matrix as an agent.

use std::path::PathBuf;
use std::process::Command;

use color_eyre::eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use tokio::signal::unix::{signal, SignalKind};

use bz::chaperone::{Chaperone, ChaperoneConfig, ChaperoneMode};
use bz::env;

/// Path to agent credentials file
fn agent_credentials_path(name: &str) -> PathBuf {
    env::data_dir().join(format!("matrix/agent-{}.json", name))
}

/// Persisted agent state (survives restarts)
#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentState {
    /// Timestamp (Unix ms) of the last message that triggered a dispatch.
    /// Messages with timestamps <= this are skipped on reconnect.
    last_dispatch_ts: u64,
}

impl AgentState {
    fn path(agent_name: &str) -> PathBuf {
        env::data_dir().join(format!("matrix/agent-{}-state.json", agent_name))
    }

    fn load(agent_name: &str) -> Self {
        let path = Self::path(agent_name);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, agent_name: &str) {
        let path = Self::path(agent_name);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, serde_json::to_string(self).unwrap_or_default());
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;

    // Parse args: bzc --config <path>
    let args: Vec<String> = std::env::args().collect();

    let config_path = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .ok_or_else(|| eyre!("Usage: bzc --config <path>"))?;

    let config = ChaperoneConfig::load(&config_path)?;

    let rt = tokio::runtime::Runtime::new()?;

    match config.mode {
        ChaperoneMode::PtyOnly => {
            let chaperone = Chaperone::new(config.name);

            rt.block_on(async {
                let mut sigterm = signal(SignalKind::terminate())?;
                let mut sigint = signal(SignalKind::interrupt())?;

                tokio::select! {
                    result = chaperone.run() => {
                        result?;
                    }
                    _ = sigterm.recv() => {
                        eprintln!("bzc: received SIGTERM, shutting down");
                    }
                    _ = sigint.recv() => {
                        eprintln!("bzc: received SIGINT, shutting down");
                    }
                }

                Ok::<_, color_eyre::eyre::Report>(())
            })?;
        }
        ChaperoneMode::Matrix => {
            // Agent mode - connect to Matrix
            rt.block_on(async {
                let mut sigterm = signal(SignalKind::terminate())?;
                let mut sigint = signal(SignalKind::interrupt())?;

                // Register or login to Matrix
                let agent_name = &config.name;
                let password = format!("agent-{}-password", agent_name); // Simple password for local agents

                eprintln!("bzc: connecting to Matrix as agent '{}'", agent_name);

                match bz::matrix_client::BzMatrixClient::agent_register_or_login(
                    &env::conduit_url(),
                    agent_name,
                    &password,
                ).await {
                    Ok(client) => {
                        eprintln!("bzc: agent '{}' connected to Matrix as {}", agent_name, client.user_id());

                        // Set up auto-accept for room invites
                        let agent_name_clone = agent_name.clone();
                        client.client().add_event_handler(
                            move |event: matrix_sdk::ruma::events::room::member::StrippedRoomMemberEvent,
                                  room: matrix_sdk::Room| {
                                let agent_name = agent_name_clone.clone();
                                async move {
                                    // Only handle invites to ourselves
                                    if event.state_key != room.own_user_id().as_str() {
                                        return;
                                    }
                                    if event.content.membership != matrix_sdk::ruma::events::room::member::MembershipState::Invite {
                                        return;
                                    }

                                    eprintln!("bzc [{}]: received invite to room {}", agent_name, room.room_id());

                                    // Auto-accept invite
                                    match room.join().await {
                                        Ok(_) => {
                                            eprintln!("bzc [{}]: joined room {}", agent_name, room.room_id());
                                        }
                                        Err(e) => {
                                            eprintln!("bzc [{}]: failed to join room: {}", agent_name, e);
                                        }
                                    }
                                }
                            }
                        );

                        // Start sync loop
                        let mut message_rx = client.start_sync();

                        // Load persisted agent state (tracks last dispatched message)
                        let mut agent_state = AgentState::load(agent_name);
                        eprintln!("bzc [{}]: loaded state, last_dispatch_ts={}", agent_name, agent_state.last_dispatch_ts);

                        // Track if we should quit
                        let mut should_quit = false;

                        // Main loop - handle messages and signals
                        loop {
                            tokio::select! {
                                Some(msg) = message_rx.recv() => {
                                    let sender_name = msg.sender_display_name.clone()
                                        .unwrap_or_else(|| msg.sender.clone());

                                    eprintln!("bzc [{}]: message in {}: <{}> {}",
                                        agent_name,
                                        msg.room_id,
                                        sender_name,
                                        msg.content
                                    );

                                    // Parse commands (messages starting with /)
                                    let content = msg.content.trim();
                                    if content.starts_with('/') {
                                        let parts: Vec<&str> = content.splitn(2, ' ').collect();
                                        let command = parts[0];
                                        let _args = parts.get(1).unwrap_or(&"");

                                        match command {
                                            "/quit" => {
                                                eprintln!("bzc [{}]: received /quit command", agent_name);
                                                // Send confirmation
                                                if let Err(e) = client.send_message(&msg.room_id, "Shutting down...").await {
                                                    eprintln!("bzc [{}]: failed to send quit message: {}", agent_name, e);
                                                }
                                                should_quit = true;
                                                break;
                                            }
                                            "/restart" => {
                                                eprintln!("bzc [{}]: received /restart command", agent_name);
                                                // Send confirmation - actual restart handled by bzd
                                                if let Err(e) = client.send_message(&msg.room_id, "Restart requested (not yet implemented)").await {
                                                    eprintln!("bzc [{}]: failed to send restart message: {}", agent_name, e);
                                                }
                                            }
                                            "/help" => {
                                                let help_msg = "Available commands:\n- /quit - Shut down this agent\n- /restart - Restart this agent\n- /help - Show this message";
                                                if let Err(e) = client.send_message(&msg.room_id, help_msg).await {
                                                    eprintln!("bzc [{}]: failed to send help message: {}", agent_name, e);
                                                }
                                            }
                                            _ => {
                                                // Unknown command - ignore or echo help
                                                eprintln!("bzc [{}]: unknown command: {}", agent_name, command);
                                            }
                                        }
                                    } else {
                                        // Check for @mentions
                                        // Match @agentname or @agentname:server patterns
                                        let mention_simple = format!("@{}", agent_name);

                                        let is_mentioned = content.contains(&mention_simple)
                                            || content.to_lowercase().contains(&mention_simple.to_lowercase());

                                        if is_mentioned {
                                            eprintln!("bzc [{}]: @mentioned in {}: {}",
                                                agent_name, msg.room_id, content);

                                            // Extract the prompt (message without the @mention)
                                            let prompt = content
                                                .replace(&mention_simple, "")
                                                .replace(&mention_simple.to_lowercase(), "")
                                                .trim()
                                                .to_string();

                                            if prompt.is_empty() {
                                                let ack = "👋 You mentioned me! What can I help with?";
                                                if let Err(e) = client.send_message(&msg.room_id, ack).await {
                                                    eprintln!("bzc [{}]: failed to send ack: {}", agent_name, e);
                                                }
                                            } else if let Some(ref cwd) = config.cwd {
                                                // Skip messages we've already dispatched (prevents duplicates on reconnect)
                                                if msg.timestamp <= agent_state.last_dispatch_ts {
                                                    eprintln!("bzc [{}]: skipping already-dispatched message (ts={} <= {})",
                                                        agent_name, msg.timestamp, agent_state.last_dispatch_ts);
                                                    continue;
                                                }

                                                // Invoke wicket with the prompt
                                                eprintln!("bzc [{}]: invoking wicket in {} with prompt: {}",
                                                    agent_name, cwd, prompt);

                                                // Send "thinking" indicator
                                                if let Err(e) = client.send_message(&msg.room_id, "🤔 Thinking...").await {
                                                    eprintln!("bzc [{}]: failed to send thinking msg: {}", agent_name, e);
                                                }

                                                // Spawn wicket synchronously (blocking)
                                                // -c = continue (maintain conversation context)
                                                // -p = prompt (non-interactive)
                                                let output = Command::new("wicket")
                                                    .arg("-c")
                                                    .arg("-p")
                                                    .arg(&prompt)
                                                    .current_dir(cwd)
                                                    .output();

                                                match output {
                                                    Ok(out) => {
                                                        let response = if out.status.success() {
                                                            String::from_utf8_lossy(&out.stdout).to_string()
                                                        } else {
                                                            format!("Error: {}", String::from_utf8_lossy(&out.stderr))
                                                        };

                                                        // Truncate if too long
                                                        let response = if response.len() > 4000 {
                                                            format!("{}... (truncated)", &response[..4000])
                                                        } else {
                                                            response
                                                        };

                                                        if let Err(e) = client.send_message(&msg.room_id, &response).await {
                                                            eprintln!("bzc [{}]: failed to send response: {}", agent_name, e);
                                                        }

                                                        // Update dispatch timestamp after successful response
                                                        agent_state.last_dispatch_ts = msg.timestamp;
                                                        agent_state.save(agent_name);
                                                    }
                                                    Err(e) => {
                                                        let err_msg = format!("Failed to invoke wicket: {}", e);
                                                        eprintln!("bzc [{}]: {}", agent_name, err_msg);
                                                        if let Err(e) = client.send_message(&msg.room_id, &err_msg).await {
                                                            eprintln!("bzc [{}]: failed to send error: {}", agent_name, e);
                                                        }
                                                        // Still update timestamp to avoid retrying failed dispatches
                                                        agent_state.last_dispatch_ts = msg.timestamp;
                                                        agent_state.save(agent_name);
                                                    }
                                                }
                                            } else {
                                                let err_msg = "No working directory configured - can't invoke wicket";
                                                eprintln!("bzc [{}]: {}", agent_name, err_msg);
                                                if let Err(e) = client.send_message(&msg.room_id, err_msg).await {
                                                    eprintln!("bzc [{}]: failed to send error: {}", agent_name, e);
                                                }
                                            }
                                        }
                                    }
                                }
                                _ = sigterm.recv() => {
                                    eprintln!("bzc: received SIGTERM, shutting down");
                                    break;
                                }
                                _ = sigint.recv() => {
                                    eprintln!("bzc: received SIGINT, shutting down");
                                    break;
                                }
                            }
                        }

                        if should_quit {
                            eprintln!("bzc [{}]: exiting due to /quit command", agent_name);
                        }
                    }
                    Err(e) => {
                        eprintln!("bzc: failed to connect to Matrix: {}", e);
                        return Err(e);
                    }
                }

                Ok::<_, color_eyre::eyre::Report>(())
            })?;
        }
    }

    Ok(())
}

//! bzc - bz chaperone
//!
//! The chaperone process manages PTYs for a single principal (user or agent).
//! In PTY-only mode, it just manages PTY sockets.
//! In Matrix mode, it also connects to Matrix as an agent.

use std::path::PathBuf;

use color_eyre::eyre::{eyre, Result};
use tokio::signal::unix::{signal, SignalKind};

use bz::chaperone::{Chaperone, ChaperoneConfig, ChaperoneMode};

/// Path to agent credentials file
fn agent_credentials_path(name: &str) -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(format!("bz/matrix/agent-{}.json", name))
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

    eprintln!(
        "bzc: starting chaperone '{}' in {:?} mode",
        config.name, config.mode
    );

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
                    "http://localhost:6167",
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

                        // Main loop - handle messages and signals
                        loop {
                            tokio::select! {
                                Some(msg) = message_rx.recv() => {
                                    // Log received messages for now
                                    eprintln!("bzc [{}]: message in {}: <{}> {}",
                                        agent_name,
                                        msg.room_id,
                                        msg.sender_display_name.unwrap_or(msg.sender),
                                        msg.content
                                    );
                                    // TODO: Route to agent logic
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

# Comms Backbone Implementation Plan

Implementation plan for [comms-backbone](../definitions/comms-backbone.md).

**Status**: Draft

## Current State

bz is a terminal multiplexer with session persistence. It manages PTY sessions across restarts via a daemon architecture.

### What's Working

- **TUI Framework**: ratatui + crossterm for rendering, with sidebar, terminal widget, and channel picker
- **Session Daemon (bzd)**: Unix socket-based IPC, PTY lifecycle management, client attach/detach
- **PTY Management**: Spawn, resize, output buffering, history replay on reconnect
- **Channel Model**: Named channels from `bz.toml`, each backed by a PTY
- **Protocol**: Length-prefixed bincode messages between client and daemon

### What Must Change

- **bzd needs to host Conduit**: Daemon becomes the homeserver runtime
- **Config needs agent definitions**: Channels need associated agent identities
- **TUI needs chat pane**: Space for rendering messages alongside terminal
- **New subsystem: Agent Registry**: Manage Matrix identities for agents
- **New subsystem: Chaperones**: Event listeners for agent interrupts

## Gap Analysis

| Requirement | Current State | Gap | Approach |
|-------------|---------------|-----|----------|
| Self-contained homeserver | No Matrix | Missing entirely | Embed Conduit in bzd |
| Agent Matrix identities | No agents | No identity system | Agent registry with auto-provisioning |
| Chaperone event listeners | No event system | No Matrix client | matrix-sdk-rust per agent |
| Chat in workspace view | Terminal only | No chat UI | Add chat pane to TUI |
| Mobile access | N/A | No network exposure | Conduit exposes client API |
| Presence tracking | No presence | No state system | Matrix presence + custom busy/idle |
| Room ↔ Channel mapping | Channels exist | No rooms | Auto-create rooms from config |

## Architectural Decisions

### AD-1: Conduit as Library vs Subprocess

**Context**: Conduit can be embedded as a Rust library or run as a subprocess that bzd manages.

**Decision**: Start with subprocess (sidecar), migrate to library if warranted.

**Rationale**:
- Conduit's embedding story is still maturing
- Subprocess isolation simplifies debugging
- Can upgrade to embedded later without changing contracts
- Matches Option B from definition as "reasonable fallback"

### AD-2: One Matrix Client per Agent

**Context**: Could share one client with multiple "users" or run separate clients.

**Decision**: One `matrix-sdk` client instance per agent identity.

**Rationale**:
- Clean separation of concerns
- Each chaperone owns its client
- Avoids multiplexing complexity
- Memory overhead acceptable for dev-machine scale

### AD-3: Chat Pane Position

**Context**: Chat could be a sidebar, bottom pane, or overlay.

**Decision**: Right-side pane, collapsible, similar to sidebar.

**Rationale**:
- Symmetric with existing sidebar (left: channels, right: chat)
- Doesn't steal vertical space from terminal
- Can be toggled off when not needed

### AD-4: Storage Location

**Context**: Where does Conduit store its data?

**Decision**: `~/.local/share/bz/matrix/` as resolved in definition.

**Rationale**: Follows XDG conventions, keeps all bz state together.

## Implementation Phases

### Phase 1: Conduit Sidecar

Get a Matrix homeserver running alongside bzd.

- [ ] Add Conduit binary management (download/verify/launch)
- [ ] bzd spawns Conduit on startup, manages lifecycle
- [ ] Conduit config generation (ports, storage path, server name)
- [ ] Health check endpoint monitoring
- [ ] Graceful shutdown coordination

**Depends on**: Nothing
**Enables**: Phase 2 (can't have clients without a server)

### Phase 2: User Identity & Basic Client

Human user can send/receive messages.

- [ ] Add `matrix-sdk` dependency
- [ ] User identity provisioning on first run (register `@user:localhost`)
- [ ] Store user credentials in `~/.local/share/bz/matrix/user.json`
- [ ] Basic sync loop in bzd
- [ ] Protocol messages for chat events (send/receive)

**Depends on**: Phase 1
**Enables**: Phase 3 (agents follow same pattern as user)

### Phase 3: Agent Registry

Agents get Matrix identities.

- [ ] Agent definition in config (`bz.toml` or separate manifest)
- [ ] Auto-provisioning of agent accounts (`@exo:localhost`, etc.)
- [ ] Credential storage per agent
- [ ] Agent ↔ channel assignment (which agents are in which rooms)

**Depends on**: Phase 2
**Enables**: Phase 4 (chaperones need agent identities to act as)

### Phase 4: Room Management

Channels become rooms.

- [ ] Auto-create room for each channel on startup
- [ ] Room membership management (user + assigned agents)
- [ ] Room ↔ channel ID mapping persistence
- [ ] Support for chat-only rooms (no directory, like `#alerts`)

**Depends on**: Phase 3
**Enables**: Phase 5 (chat pane needs rooms to display)

### Phase 5: Chat Pane

TUI shows messages.

- [ ] New `ChatPane` widget (message list, input line)
- [ ] Layout changes (terminal + chat side-by-side)
- [ ] Toggle chat visibility (Ctrl+B, C?)
- [ ] Message rendering (sender, timestamp, content)
- [ ] Input handling for compose mode

**Depends on**: Phase 4
**Enables**: Phase 6 (presence is shown in chat)

### Phase 6: Presence & Status

Agents show availability.

- [ ] Set user presence on TUI focus
- [ ] Agent presence tied to workspace activity
- [ ] Busy/idle state tracking (custom or Matrix presence)
- [ ] Presence display in sidebar and chat

**Depends on**: Phase 5
**Enables**: Phase 7 (chaperones use presence for interrupt decisions)

### Phase 7: Chaperones

Agents can be interrupted.

- [ ] Chaperone actor per agent (subscribes to mentions/DMs)
- [ ] Event filtering (which messages trigger interrupts)
- [ ] Workspace state capture (what was agent doing?)
- [ ] Context injection protocol (how to interrupt an agent)
- [ ] Response routing (agent reply → Matrix room)

**Depends on**: Phase 6
**Enables**: Full comms-backbone functionality

## Migration Strategy

No data migration needed - this is greenfield. Existing bz installations will simply gain new capabilities when they update.

First-run behavior:
1. bzd detects no Conduit data
2. Downloads/extracts Conduit binary
3. Generates config with random server name suffix
4. Provisions user account
5. Creates rooms for existing channels
6. Normal operation begins

## Risks

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Conduit API instability | Medium | Medium | Pin to specific version, abstract behind trait |
| matrix-sdk complexity | Medium | High | Start with minimal sync, expand incrementally |
| Resource usage on laptops | Low | Medium | Profile early, Conduit is designed lightweight |
| Subprocess coordination bugs | Medium | Medium | Robust health checks, restart logic |
| Chat pane UX iteration | High | Low | Ship minimal, iterate based on usage |

## Open Decisions

1. **Conduit version pinning** — Which version to target? Need to evaluate stability of recent releases.

2. **Agent manifest format** — Extend `bz.toml` or separate file? Needs design for agent definitions (name, persona, channel assignments).

3. **Interrupt protocol** — How does a chaperone actually interrupt an agent mid-task? This touches agent runtime (Claude Code, etc.) integration which is outside bz's direct control.

4. **Mobile auth flow** — How does user authenticate from Element? Password? SSO? Device verification?

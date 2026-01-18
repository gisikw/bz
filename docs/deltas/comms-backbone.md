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
- **Config needs agent definitions**: Agents defined per working directory
- **Channel view model**: Channels multiplex views (chat, workspaces) — not side-by-side
- **New subsystem: Agent Registry**: Manage Matrix identities for agents
- **New subsystem: Chaperones**: Event listeners for agent interrupts

## Gap Analysis

| Requirement | Current State | Gap | Approach |
|-------------|---------------|-----|----------|
| Self-contained homeserver | No Matrix | Missing entirely | Embed Conduit in bzd |
| Agent Matrix identities | No agents | No identity system | Agent registry with auto-provisioning |
| Chaperone event listeners | No event system | No Matrix client | Single matrix-sdk, route to chaperone actors |
| Chat in workspace view | Terminal only | No chat UI | Chat as channel view (tab within channel) |
| Mobile access | N/A | No network exposure | Conduit exposes client API |
| Presence tracking | No presence | No state system | Matrix presence + custom busy/idle |
| Room ↔ Channel mapping | Channels exist | No rooms | Auto-create rooms from config |

## Architectural Decisions

### AD-1: Embedded Conduit

**Context**: Conduit can be embedded as a Rust library or run as a subprocess.

**Decision**: Embed Conduit directly in bzd.

**Rationale**:
- Single daemon, single lifecycle — core constraint from definition
- No subprocess coordination, no orphan processes, no race conditions on startup
- Conduit is designed for lightweight/embedded use (this is its intended mode)
- User gets `bzd` and everything works — no "also make sure conduit is running"
- Shared async runtime, shared memory space, direct function calls
- If embedding proves truly problematic, sidecar is a fallback — but we lead with the cleaner architecture

### AD-2: Single Matrix Client with Event Routing

**Context**: Could run one matrix-sdk client per agent, or share a single client.

**Decision**: Single matrix-sdk client in bzd, events routed to chaperone actors.

**Rationale**:
- One sync loop, one connection to homeserver — not N connections
- Chaperones are lightweight actors that receive filtered events
- Event routing is cheaper than duplicated sync state
- Agents don't need full client capabilities — just send/receive messages
- bzd acts as the Matrix "hub" with chaperones as spokes

### AD-3: Channel View Multiplexing

**Context**: How does chat relate to workspaces in the UI?

**Decision**: Chat is a view within the channel, not a separate pane. Channels multiplex views.

**Rationale**:
- Think Slack: a channel has chat, but also canvases, files, etc. — tabs at the top
- The main window shows one view at a time: chat OR workspace OR other
- User cycles through views within a channel (tabs/keybind)
- No split-screen complexity — full takeover of main area
- Sidebar shows channels; main area shows the active view of the focused channel
- This matches the "terminal multiplexer" mental model — we're multiplexing views, not just PTYs

### AD-4: Storage Location

**Context**: Where does Conduit store its data?

**Decision**: `~/.local/share/bz/matrix/` as resolved in definition.

**Rationale**: Follows XDG conventions, keeps all bz state together.

### AD-5: Dynamic Agent Membership

**Context**: How are agents associated with channels?

**Decision**: Agents can be invited to rooms dynamically, not just statically assigned.

**Rationale**:
- Directory-backed channels have agents defined in their working directory config
- But agents are Matrix users — they can be `/invite`d to any room
- `@security-ops` might be invited ad-hoc to help with something
- Free-floating agents (not tied to a directory) can exist and be invited anywhere
- Static assignment is just "auto-invite on room creation" — the underlying model is dynamic

## Implementation Phases

### Phase 1: Embedded Conduit

Get a Matrix homeserver running inside bzd.

- [ ] Add Conduit as Cargo dependency (or git submodule if needed)
- [ ] Initialize Conduit database on first bzd start
- [ ] Conduit config: ports, storage path (`~/.local/share/bz/matrix/`), server name
- [ ] Expose client API on localhost (port 8448 or configured)
- [ ] Graceful startup/shutdown integrated with bzd lifecycle

**Depends on**: Nothing
**Enables**: Phase 2

**Success Criteria**:
- `curl http://localhost:8448/_matrix/client/versions` returns valid JSON
- Element can connect to `http://localhost:8448` and see the server
- Stopping bzd cleanly shuts down Conduit (no orphan processes, no corrupt DB)

### Phase 2: User Identity & Basic Client

Human user can send/receive messages.

- [ ] Add `matrix-sdk` dependency
- [ ] User registration on first run (`@<username>:localhost`)
- [ ] Credential storage in `~/.local/share/bz/matrix/user.json`
- [ ] Single sync loop in bzd
- [ ] Protocol extension: chat events between bzd and bz client

**Depends on**: Phase 1
**Enables**: Phase 3

**Success Criteria**:
- User can register and login automatically on first run
- Sending a message via Element appears in bzd logs
- bzd can send a message that appears in Element
- Sync survives bzd restart (messages don't disappear)

### Phase 3: Agent Registry

Agents get Matrix identities.

- [ ] Agent definition format (in working directory, e.g., `.bz/agent.toml`)
- [ ] Auto-provisioning of agent accounts (`@exo:localhost`, etc.)
- [ ] Credential storage per agent (`~/.local/share/bz/matrix/agents/<name>/`)
- [ ] Agent roster API in bzd (list known agents, their status)

**Depends on**: Phase 2
**Enables**: Phase 4

**Success Criteria**:
- Agents defined in config are auto-registered on bzd start
- Each agent has a distinct Matrix identity visible in Element
- `/invite @agentname:localhost` works from Element
- Agent credentials persist across restarts

### Phase 4: Room Management

Channels become rooms.

- [ ] Auto-create room for each channel on startup
- [ ] Room ↔ channel ID mapping persistence
- [ ] User auto-joins channel rooms
- [ ] Support for chat-only rooms (no directory, like `#alerts`)
- [ ] `/invite` handling — agents join rooms when invited

**Depends on**: Phase 3
**Enables**: Phase 5

**Success Criteria**:
- Starting bz with a `bz.toml` creates corresponding Matrix rooms
- Rooms appear in Element with correct names
- Inviting an agent to a room from Element succeeds
- Chat-only room can be created without a backing directory

### Phase 5: Chat View

TUI shows messages as a channel view.

- [ ] New `ChatView` widget (message list, input line)
- [ ] Channel view multiplexing: chat / workspace / (future: other)
- [ ] View switching keybind (e.g., `Ctrl+B, v` to cycle views)
- [ ] Message rendering (sender, timestamp, content)
- [ ] Compose mode for typing messages

**Depends on**: Phase 4
**Enables**: Phase 6

**Success Criteria**:
- User can switch to chat view for a channel
- Messages sent from Element appear in TUI
- Messages typed in TUI appear in Element
- View state persists when switching channels (each channel remembers its active view)

### Phase 6: Presence & Status

Agents show availability.

- [ ] User presence: online when TUI focused, idle on blur
- [ ] Agent presence: online/busy/idle based on workspace activity
- [ ] Presence display in sidebar (indicator next to agent names)
- [ ] Presence visible in Element for mobile users

**Depends on**: Phase 5
**Enables**: Phase 7

**Success Criteria**:
- Agent shows as "busy" in Element when actively working
- Agent shows as "idle" when workspace is quiet
- Closing bz sets presence to offline
- Presence updates within ~5 seconds of state change

### Phase 7: Chaperones

Agents can be interrupted.

- [ ] Chaperone actor per agent (receives routed events)
- [ ] Event filtering: @mentions, DMs, configurable room messages
- [ ] Workspace state capture (what was agent doing?)
- [ ] Interrupt protocol (how to signal the agent runtime)
- [ ] Response routing (agent reply → Matrix room)

**Depends on**: Phase 6
**Enables**: Full comms-backbone functionality

**Success Criteria**:
- @mentioning an agent in Element triggers chaperone
- Chaperone can capture current workspace context
- Agent can reply to the mention (appears in Element)
- DMs to agent are received by chaperone

## Migration Strategy

No data migration needed — this is greenfield. Existing bz installations gain new capabilities on update.

First-run behavior:
1. bzd initializes Conduit database
2. Provisions user account
3. Creates rooms for channels in config
4. Normal operation begins

## Risks

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Conduit embedding complexity | Medium | High | Evaluate Conduit's library interface early; sidecar is fallback |
| matrix-sdk async integration | Medium | Medium | Both use tokio; should compose well |
| Resource usage on laptops | Low | Medium | Profile early; Conduit is designed lightweight |
| View multiplexing UX | Medium | Low | Start simple (2 views), iterate |
| Interrupt protocol design | High | Medium | Define minimal protocol first, extend as needed |

## Open Decisions

1. **Conduit version/embedding approach** — Need to evaluate Conduit's library interface. Is it a clean Cargo dependency or does it need special handling?

2. **Agent definition format** — Working directory config (`.bz/agent.toml`)? What fields? Name, persona reference, default rooms?

3. **Interrupt protocol** — How does chaperone signal the agent runtime (Claude Code, etc.)? This is an integration boundary we need to define.

4. **Mobile auth flow** — Password registration? Local-only initially acceptable?

5. **View switching UX** — Keybind for cycling views? Visual indicator of which view is active?

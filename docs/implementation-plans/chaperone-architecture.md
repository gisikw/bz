# Chaperone Architecture Implementation Plan

Implementation plan for [chaperone-architecture](../prds/chaperone-architecture.md).

**Status**: Draft

## Current State

bz is a terminal multiplexer with session persistence (~3,400 lines of Rust):

### What Works Today

- **Daemon (bzd)**: Unix socket IPC, PTY lifecycle, output buffering (1MB ring buffers), client attach/detach/takeover
- **Client (bz)**: ratatui TUI with sidebar, terminal widget, channel picker, Ctrl+B leader mode
- **PTY Management**: `portable-pty` spawning, `vt100` parsing, activity detection, scrollback (10K lines)
- **Protocol**: Length-prefixed bincode between client and daemon
- **Config**: TOML-based channel definitions with optional cwd/command

### What Gets Replaced

The current architecture conflates concerns: bzd manages PTYs directly and bz connects to bzd for PTY I/O. In the new architecture:

| Current | New |
|---------|-----|
| bzd owns PTYs | bzc (chaperone) owns PTYs |
| bz connects to bzd for PTY I/O | bz connects directly to chaperone PTY sockets |
| Channels are local PTY containers | Channels are Matrix rooms with attached PTYs |
| No communication layer | Matrix (Conduit) as backbone |
| No agent identities | Agent chaperones have Matrix identities |

### What Gets Reused

- **PTY spawning/buffering**: `ManagedPty`, output ring buffers → moves into chaperone
- **Terminal emulation**: `vt100` parser, scrollback → stays in bz
- **TUI framework**: sidebar, terminal widget, input handling → adapts for Matrix
- **IPC pattern**: Length-prefixed bincode → used for chaperone coordination
- **Config loading**: TOML parsing → extended for agents/rooms

## What Needs to Change

1. **New binary: bzc** — Chaperone process for PTY management (user) or Matrix identity + PTY (agents)
2. **bzd transformation** — Manage Conduit sidecar, coordinate chaperones, track room metadata
3. **bz transformation** — Become Matrix client, spawn user chaperone, connect to PTY sockets
4. **New protocol** — Chaperone ↔ bzd coordination (attach/detach notifications)
5. **Config extension** — Agent definitions, room mappings, Conduit settings

## Architectural Decisions

### AD-1: Conduit as Managed Sidecar

**Context**: Conduit could be embedded as a library or run as a subprocess.

**Decision**: Run Conduit as a managed sidecar process, spawned and supervised by bzd.

**Rationale**:
- `matrix-conduit` on crates.io appears designed as a standalone binary, not a library
- Sidecar still achieves "single command starts everything" UX
- bzd spawns Conduit on start, monitors health, shuts down on exit
- Clean process boundaries; Conduit crash doesn't take down bzd
- If library embedding becomes viable later, can revisit

### AD-2: Chaperone Process Model

**Context**: Chaperones could be threads, actors, or processes.

**Decision**: Chaperones are separate OS processes (`bzc`) with two modes: PTY-only (user) or Matrix+PTY (agents).

**Rationale**:
- Crash isolation — one chaperone dying doesn't affect others
- User chaperone needs no Matrix identity — bz is the human's Matrix client
- Agent chaperones are Matrix bots with their own identities
- Testable in isolation — can run `bzc` standalone
- Process overhead is minimal for long-lived services

### AD-3: PTY Socket Protocol

**Context**: How does bz connect to chaperone PTYs for terminal I/O?

**Decision**: Reuse current protocol (length-prefixed bincode) on Unix sockets at `~/.local/share/bz/chaperones/<name>/<pty_id>.sock`.

**Rationale**:
- Protocol already proven for PTY I/O (Input, Output, Resize messages)
- Same framing, different socket location
- Chaperone acts as the "daemon" from current architecture's perspective
- bz terminal code needs minimal changes — just different socket path

### AD-4: bzd ↔ Chaperone Coordination

**Context**: How do chaperones notify bzd of PTY state changes?

**Decision**: Dedicated control socket per chaperone at `~/.local/share/bz/chaperones/<name>/control.sock`. Protocol supports `attach` and `detach` events only — no `move` (use detach+attach).

**Rationale**:
- Not Matrix (too much latency for coordination as noted in PRD)
- Separate from PTY sockets (different concerns)
- Same bincode framing for consistency
- No `move` command — detach+attach is sufficient and reduces test surface
- bzd listens; chaperones connect and send events

### AD-5: bz as Human's Matrix Client

**Context**: How does the human interact with Matrix?

**Decision**: bz itself is the Matrix client for the human. The user chaperone is PTY-only (no Matrix identity).

**Rationale**:
- Avoids duplicate Matrix connections for the same human
- User chaperone becomes simpler — just PTY management via bincode
- bz already needs matrix-sdk for room list, chat — let it own the identity
- Cleaner separation: chaperones manage PTYs, bz manages Matrix UI
- bzd doesn't need matrix-sdk at all — just Conduit sidecar management

### AD-6: bz Spawns User Chaperone

**Context**: Who spawns the user's chaperone?

**Decision**: bz spawns its own user chaperone with `bzc --config=<path> --homeserver=<url>`.

**Rationale**:
- Decouples bzd from single-user assumptions
- bz knows its user identity and homeserver URL
- Better positioned for future multi-user support
- bzd becomes purely infrastructure (Conduit + agent chaperones)
- User chaperone lifecycle tied to bz session

### AD-7: Screen Navigation Model

**Context**: How does the user navigate between chat and PTYs within a room?

**Decision**: Rooms have "screens" — screen 1 is always Matrix chat, screens 2..n are attached PTYs. Navigate with h/l (horizontal), rooms with j/k (vertical). bz attaches to one PTY at a time.

**Rationale**:
- Consistent with existing j/k channel navigation
- h/l is intuitive for horizontal screen switching
- One PTY attachment at a time reduces complexity and resource usage
- Chat is always screen 1 — predictable, always accessible
- PTY screens ordered by attachment time

## Implementation Phases

### Phase 1: Conduit Sidecar

Get Conduit running as a managed subprocess of bzd.

- [ ] Download/install Conduit binary (or expect it on PATH)
- [ ] Create `src/daemon/conduit.rs` — process spawning, health monitoring
- [ ] Generate Conduit config at `~/.local/share/bz/conduit.toml`
- [ ] Initialize Conduit database at `~/.local/share/bz/matrix/`
- [ ] Spawn Conduit on bzd start, bind to localhost:8448
- [ ] Health check loop (process alive, API responding)
- [ ] Graceful shutdown — SIGTERM to Conduit when bzd exits
- [ ] Existing PTY functionality unchanged (regression test)

**Acceptance criteria**:
- `curl http://localhost:8448/_matrix/client/versions` returns valid JSON after bzd starts
- Element can connect to `http://localhost:8448` and see the server
- Existing `bz` attach/detach/PTY functionality still works
- Stopping bzd cleanly stops Conduit (no orphan processes)
- Conduit crash triggers bzd health check failure (logged, potentially restart)

**Depends on**: Nothing
**Enables**: Phase 2

### Phase 2: Chaperone Skeleton (PTY-only mode)

Create the chaperone binary with PTY management, no Matrix.

- [ ] New binary: `src/bin/bzc.rs`
- [ ] CLI: `bzc --config=<path>` with TOML containing name, mode (pty-only vs matrix)
- [ ] Control socket listener at `~/.local/share/bz/chaperones/<name>/control.sock`
- [ ] Ready signal over control socket
- [ ] Graceful shutdown on SIGTERM
- [ ] Port `ManagedPty` and output buffering from `daemon/pty_manager.rs`
- [ ] PTY socket creation at `~/.local/share/bz/chaperones/<name>/<pty_id>.sock`
- [ ] PTY protocol handler (reuse `protocol.rs` message types)
- [ ] Attach/detach notifications over control socket

**Acceptance criteria**:
- `bzc --config=./user-chaperone.toml` starts in PTY-only mode
- Spawning a PTY creates socket, direct connection shows terminal output
- Input/resize commands work over PTY socket
- Attach/detach events sent over control socket
- SIGTERM causes clean exit (PTYs terminated)

**Depends on**: Nothing (parallel with Phase 1)
**Enables**: Phase 3

### Phase 3: bz Spawns User Chaperone

bz spawns and manages its own user chaperone.

- [ ] User chaperone config generation (tmpfile or inline)
- [ ] Spawn `bzc` on bz start with appropriate config
- [ ] Connect to user chaperone control socket
- [ ] Wait for ready signal before proceeding
- [ ] PTY socket connection based on control socket events
- [ ] Propagate SIGTERM to user chaperone on bz exit
- [ ] Remove legacy bzd PTY connection code

**Acceptance criteria**:
- `bz` starts and spawns `bzc` automatically
- `ps aux | grep bzc` shows user chaperone process
- Terminal I/O works through chaperone PTY socket
- Exiting bz terminates user chaperone
- Crash of bz (kill -9) leaves chaperone orphaned (acceptable for now)

**Depends on**: Phase 2
**Enables**: Phase 4

### Phase 4: bz Matrix Client

bz becomes a Matrix client for the human.

- [ ] Add `matrix-sdk` to bz
- [ ] User registration on first run (`@<username>:localhost`)
- [ ] Credential storage at `~/.local/share/bz/matrix/user.json`
- [ ] Password set during registration (for mobile access)
- [ ] Matrix sync loop in bz
- [ ] Fetch room list, display in sidebar
- [ ] Room state includes `attached_ptys` metadata

**Acceptance criteria**:
- First run creates `@<username>:localhost` account with password
- bz shows room list in sidebar
- Creating a room in Element appears in bz sidebar
- Element can login as same user with password
- Credentials persist across restarts

**Depends on**: Phase 1, Phase 3
**Enables**: Phase 5

### Phase 5: Room ↔ PTY Integration

Connect rooms to PTY management.

- [ ] Auto-create rooms from `bz.toml` channel definitions on first run
- [ ] Room state event for `attached_ptys` list
- [ ] `Ctrl+B t` spawns shell PTY in user chaperone for current room (if none exists)
- [ ] PTY attach updates room state via Matrix
- [ ] Chat-only rooms get tmpdir on PTY spawn
- [ ] Tmpdir cleanup on room deletion

**Acceptance criteria**:
- Starting bz creates Matrix rooms matching config channels
- `Ctrl+B t` in a room spawns a PTY (visible in room state)
- `Ctrl+B t` when PTY exists is noop (stretch: switches to that screen)
- Room state `attached_ptys` updates on attach/detach
- Chat-only room can spawn PTY (tmpdir created)

**Depends on**: Phase 4
**Enables**: Phase 6

### Phase 6: Screen Navigation

Implement h/l navigation between chat and PTYs.

- [ ] Screen model: screen 1 = chat, screens 2..n = PTYs
- [ ] `h` moves to previous screen, `l` moves to next screen
- [ ] `j`/`k` unchanged — room navigation
- [ ] Current screen indicator in UI
- [ ] bz attaches to PTY only when viewing that screen
- [ ] Detach from PTY when switching away

**Acceptance criteria**:
- Default view is chat (screen 1)
- `l` switches to first PTY (screen 2) if attached
- `h` switches back to chat
- Switching screens attaches/detaches PTY connection
- Screen indicator shows current position (e.g., "1/3")

**Depends on**: Phase 5
**Enables**: Phase 7

### Phase 7: Chat View

Full chat functionality in bz.

- [ ] `ChatView` widget — message list, input line
- [ ] Message rendering (sender, timestamp, content)
- [ ] Message composition and sending
- [ ] Sync incoming messages
- [ ] Unread indicator when on PTY screen
- [ ] Scroll through message history

**Acceptance criteria**:
- Messages sent from Element appear in bz chat view
- Messages typed in bz appear in Element
- Unread indicator shows when new messages arrive while on PTY screen
- Can scroll through chat history

**Depends on**: Phase 6
**Enables**: Phase 8

### Phase 8: bzd Agent Orchestration

bzd spawns and manages agent chaperones.

- [ ] Agent definition format in `bz.toml` (`[[agent]]` sections)
- [ ] Spawn agent chaperones on bzd start (Matrix+PTY mode)
- [ ] Track agent chaperone PIDs and control sockets
- [ ] Auto-provision agent Matrix accounts (`@<name>:localhost`)
- [ ] Agent credential storage at `~/.local/share/bz/matrix/agents/<name>/`
- [ ] SIGTERM propagation on bzd shutdown
- [ ] Aggregate PTY attachment state from agent chaperones

**Acceptance criteria**:
- `bz.toml` with `[[agent]]` sections spawns agent chaperones
- Agents appear as Matrix users (visible in Element)
- `ps aux | grep bzc` shows agent chaperone processes
- Stopping bzd terminates all agent chaperones
- Agent credentials persist across restarts

**Depends on**: Phase 1, Phase 2
**Enables**: Phase 9

### Phase 9: Agent Room Participation

Agents can be invited to rooms and participate.

- [ ] Agent chaperone Matrix client setup (login as agent identity)
- [ ] `/invite @agent:localhost` handling — agent joins room
- [ ] Agent presence in bz sidebar (when in focused room)
- [ ] DM support for agents
- [ ] Agent can send messages to rooms

**Acceptance criteria**:
- `/invite @exo:localhost` in Element makes agent join room
- Agent shows in bz sidebar when in focused room
- Can DM agent from Element or bz
- Agent chaperone can send messages (for future interrupt responses)

**Depends on**: Phase 8
**Enables**: Phase 10

### Phase 10: Chaperone Lifecycle Commands

Implement `/restart`, `/quit` DM commands for agent control.

- [ ] DM command parsing in agent chaperone
- [ ] `/quit` — chaperone exits, bzd removes from roster
- [ ] `/restart` — chaperone restarts (bzd respawns)
- [ ] Confirmation message before exit

**Acceptance criteria**:
- DMing `/quit` to agent chaperone causes it to exit
- DMing `/restart` to agent chaperone causes restart (new PID)
- Chaperone sends confirmation before exit

**Depends on**: Phase 9
**Enables**: Phase 11

### Phase 11: Presence & Interrupts

Agents show availability and can be interrupted.

- [ ] Presence tracking: online/busy/idle based on PTY activity
- [ ] Presence updates to Matrix
- [ ] Presence display in bz sidebar
- [ ] @mention detection in agent chaperone
- [ ] Interrupt protocol: suspend on @mention, resume after 1 min idle
- [ ] Workspace state capture on interrupt

**Acceptance criteria**:
- Agent shows "busy" when PTY has recent output
- Agent shows "idle" when PTY quiet for configured period
- @mentioning agent in room triggers interrupt
- Agent can capture and report current workspace context
- Conversation idle for 1 minute resumes agent work

**Depends on**: Phase 10
**Enables**: Complete system

## Risks

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Conduit sidecar management | Medium | Medium | Health checks, restart on crash, clear error messages |
| Conduit binary distribution | Medium | Medium | Document installation; consider bundling in release |
| matrix-sdk tokio integration | Low | Medium | Both use tokio; test early in Phase 4 |
| Multi-process coordination bugs | Medium | Medium | Comprehensive integration tests; clear protocol contracts |
| User chaperone orphaning on bz crash | Medium | Low | Acceptable for v1; can add cleanup daemon later |
| Screen navigation UX confusion | Medium | Low | Clear indicator; consistent h/l/j/k model |
| Interrupt protocol edge cases | High | Medium | Simple default first (1 min idle); defer sophistication |

## Resolved Questions

1. **Conduit embedding** — Not viable as library; use managed sidecar instead.

2. **User identity** — bz is the human's Matrix client. User chaperone is PTY-only (no Matrix identity). No duplicate connections.

3. **Mobile auth** — Password set during first run. User handles VPN/OIDC gating for network access.

4. **Config hot-reload** — No. Restart required.

5. **PTY attachment UX** — `Ctrl+B t` spawns shell PTY if none exists for user in current room, sends attach event. Noop if one exists. Stretch goal: swap to that screen.

6. **PTY move** — No dedicated move command. Use detach+attach sequence.

7. **Who spawns user chaperone** — bz spawns its own user chaperone, not bzd. Decouples bzd from single-user assumptions.

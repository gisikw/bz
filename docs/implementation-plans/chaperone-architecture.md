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
| No agent identities | Chaperones have Matrix identities |

### What Gets Reused

- **PTY spawning/buffering**: `ManagedPty`, output ring buffers → moves into chaperone
- **Terminal emulation**: `vt100` parser, scrollback → stays in bz
- **TUI framework**: sidebar, terminal widget, input handling → adapts for Matrix
- **IPC pattern**: Length-prefixed bincode → used for bzd ↔ chaperone coordination
- **Config loading**: TOML parsing → extended for agents/rooms

## What Needs to Change

1. **New binary: bzc** — Chaperone process with Matrix identity + PTY management
2. **bzd transformation** — Embed Conduit, spawn/coordinate chaperones, expose room metadata
3. **bz transformation** — Become Matrix client, connect to chaperone PTYs for terminal I/O
4. **New protocol** — Chaperone ↔ bzd coordination (attach/detach/move notifications)
5. **Config extension** — Agent definitions, room mappings, Conduit settings

## Architectural Decisions

### AD-1: Conduit Embedding Strategy

**Context**: Conduit can be embedded as a Rust library or run as a subprocess.

**Decision**: Embed Conduit directly in bzd using `conduit` as a library dependency.

**Rationale**:
- Single daemon, single lifecycle — matches PRD constraint
- No subprocess coordination or orphan processes
- Conduit is designed for lightweight/embedded use
- Shared async runtime (tokio), direct function calls
- If embedding proves problematic, sidecar is fallback

### AD-2: Chaperone Process Model

**Context**: Chaperones could be threads, actors, or processes.

**Decision**: Chaperones are separate OS processes (`bzc`) spawned by bzd.

**Rationale**:
- Crash isolation — one chaperone dying doesn't affect others
- Independent Matrix clients — each has its own identity and sync state
- Testable in isolation — can run `bzc` standalone
- Matches the PRD's "atomic building block" principle
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

**Decision**: Dedicated control socket per chaperone at `~/.local/share/bz/chaperones/<name>/control.sock`.

**Rationale**:
- Not Matrix (too much latency for coordination as noted in PRD)
- Separate from PTY sockets (different concerns)
- Same bincode framing for consistency
- bzd listens; chaperones connect and send attach/detach/move events

### AD-5: Matrix Client Distribution

**Context**: How many matrix-sdk clients run in the system?

**Decision**: One client per process — bzd has one, each chaperone has one, bz has one.

**Rationale**:
- Each process needs its own Matrix identity (user, agent, user-again)
- matrix-sdk is designed for one-client-per-process
- Local Conduit means low latency between clients
- Simpler than trying to share client state across processes

### AD-6: User Chaperone Bootstrap

**Context**: User needs a chaperone to have a Matrix identity, but bzd needs to start first.

**Decision**: bzd auto-spawns user chaperone on first start before accepting bz connections.

**Rationale**:
- User chaperone is mandatory per PRD
- bzd knows user identity from config
- User chaperone registers with Conduit, then signals ready
- bz can then connect (to Conduit as Matrix client, to user chaperone for PTYs)

## Implementation Phases

### Phase 1: Conduit Embedding

Get Conduit running inside bzd without changing existing functionality.

- [ ] Add `conduit` as Cargo dependency (evaluate git vs crates.io)
- [ ] Create `src/daemon/conduit.rs` — Conduit lifecycle wrapper
- [ ] Initialize Conduit database at `~/.local/share/bz/matrix/`
- [ ] Configure Conduit: localhost binding, server name, storage path
- [ ] Integrate Conduit startup into bzd boot sequence
- [ ] Graceful shutdown — Conduit stops cleanly when bzd exits
- [ ] Existing PTY functionality unchanged (regression test)

**Acceptance criteria**:
- `curl http://localhost:8448/_matrix/client/versions` returns valid JSON after bzd starts
- Element can connect to `http://localhost:8448` and see the server
- Existing `bz` attach/detach/PTY functionality still works
- Stopping bzd cleanly shuts down Conduit (no orphan processes, no corrupt DB)

**Depends on**: Nothing
**Enables**: Phase 2

### Phase 2: User Matrix Identity

Human user gets a Matrix account and can send/receive messages.

- [ ] Add `matrix-sdk` as Cargo dependency
- [ ] Auto-register user on first run (`@<username>:localhost`)
- [ ] Store credentials at `~/.local/share/bz/matrix/user.json`
- [ ] Create Matrix client in bzd with sync loop
- [ ] Log received messages to bzd stdout (verification)
- [ ] Send test message on startup (verification)

**Acceptance criteria**:
- First run creates `@<username>:localhost` account automatically
- Sending a message in Element to that user appears in bzd logs
- bzd can send a message that appears in Element
- Credentials persist — restart doesn't re-register
- Sync state survives restart (messages don't disappear)

**Depends on**: Phase 1
**Enables**: Phase 3

### Phase 3: Chaperone Skeleton

Create the chaperone binary with basic Matrix identity (no PTY yet).

- [ ] New binary: `src/bin/bzc.rs`
- [ ] Config format: `--config=<path>` pointing to TOML with name, credentials path
- [ ] Matrix client setup — login as `@<name>:localhost`
- [ ] Control socket listener at `~/.local/share/bz/chaperones/<name>/control.sock`
- [ ] Heartbeat/ready signal to bzd over control socket
- [ ] Graceful shutdown on SIGTERM

**Acceptance criteria**:
- `bzc --config=./test-chaperone.toml` starts and logs in as configured identity
- Chaperone appears in Element user list
- Can send DM to chaperone, chaperone logs receipt
- bzd can connect to control socket and receive ready signal
- SIGTERM causes clean exit

**Depends on**: Phase 2
**Enables**: Phase 4

### Phase 4: Chaperone PTY Management

Move PTY ownership from bzd into chaperone.

- [ ] Port `ManagedPty` and output buffering from `daemon/pty_manager.rs` to chaperone
- [ ] PTY socket creation at `~/.local/share/bz/chaperones/<name>/<pty_id>.sock`
- [ ] PTY protocol handler (reuse `protocol.rs` message types)
- [ ] Attach/detach notifications to bzd over control socket
- [ ] Multi-PTY support (user chaperone can have N PTYs)
- [ ] Policy enforcement: agent chaperones limited to 1 PTY

**Acceptance criteria**:
- Chaperone spawns PTY on command
- Direct socket connection to PTY socket shows terminal output
- Input/resize commands work over PTY socket
- Attach/detach events arrive at bzd control socket
- User chaperone can manage multiple PTYs
- Agent chaperone rejects second PTY spawn

**Depends on**: Phase 3
**Enables**: Phase 5, Phase 6

### Phase 5: bzd Chaperone Orchestration

bzd spawns and manages chaperone lifecycle.

- [ ] Read chaperone definitions from `bz.toml`
- [ ] Spawn user chaperone on bzd start (mandatory)
- [ ] Spawn agent chaperones from config
- [ ] Track chaperone PIDs and control socket connections
- [ ] Aggregate PTY attachment state from chaperone notifications
- [ ] SIGTERM propagation to all chaperones on bzd shutdown
- [ ] Remove direct PTY management from bzd (now in chaperones)

**Acceptance criteria**:
- `bz.toml` with `[[chaperone]]` sections spawns corresponding `bzc` processes
- User chaperone starts before bzd accepts bz connections
- `ps aux | grep bzc` shows expected chaperone processes
- bzd tracks which PTYs are attached to which rooms
- Stopping bzd sends SIGTERM to all chaperones (they exit cleanly)

**Depends on**: Phase 4
**Enables**: Phase 7

### Phase 6: Room Management

Channels become Matrix rooms with PTY attachment metadata.

- [ ] Auto-create room for each channel in config on first run
- [ ] Room ↔ channel ID mapping in bzd state
- [ ] Room state event for `attached_ptys` list
- [ ] User auto-joins channel rooms
- [ ] Chat-only room support (no backing directory)
- [ ] tmpdir creation for chat-only rooms needing a PTY
- [ ] tmpdir cleanup on room deletion

**Acceptance criteria**:
- Starting bz creates Matrix rooms matching `bz.toml` channels
- Rooms visible in Element with correct names
- Room state includes `attached_ptys` (empty initially)
- When PTY attaches, room state updates
- Chat-only room can exist without directory
- Spawning PTY in chat-only room creates tmpdir

**Depends on**: Phase 4
**Enables**: Phase 7

### Phase 7: bz Matrix Integration

Transform bz into a Matrix client that reads rooms and connects to chaperone PTYs.

- [ ] Add matrix-sdk to bz
- [ ] Login as user identity (same credentials as user chaperone? Or separate?)
- [ ] Fetch room list from Matrix
- [ ] Read `attached_ptys` from room state
- [ ] Connect to chaperone PTY sockets based on room metadata
- [ ] Sidebar shows rooms instead of hardcoded channels
- [ ] Remove legacy bzd connection code

**Acceptance criteria**:
- bz starts and shows rooms from Matrix server in sidebar
- Selecting a room shows its attached PTY (if any)
- Terminal input/output works through chaperone PTY socket
- Creating a room in Element appears in bz sidebar
- No connection to bzd for PTY I/O (only Matrix for metadata)

**Depends on**: Phase 5, Phase 6
**Enables**: Phase 8

### Phase 8: Chat View

Add chat as a view within channels (view multiplexing).

- [ ] `ChatView` widget — message list, input line
- [ ] View state per room: Chat | Workspace
- [ ] View switching keybind (e.g., `Ctrl+B v`)
- [ ] Message rendering (sender, timestamp, content)
- [ ] Message composition and sending
- [ ] Sync incoming messages to chat view

**Acceptance criteria**:
- User can switch between chat and workspace views with keybind
- Messages sent from Element appear in chat view
- Messages typed in bz appear in Element
- View state persists per room (switching rooms remembers view)
- Unread indicator when chat has new messages while in workspace view

**Depends on**: Phase 7
**Enables**: Phase 9

### Phase 9: Agent Chaperones & Invites

Agents get Matrix identities and can be invited to rooms.

- [ ] Agent definition format in config (name, persona, defaults)
- [ ] Auto-provision agent accounts on bzd start
- [ ] Agent credentials storage at `~/.local/share/bz/matrix/agents/<name>/`
- [ ] `/invite @agent:localhost` handling — agent joins room
- [ ] Agent presence in sidebar (when in room)
- [ ] DM support for agents

**Acceptance criteria**:
- Agents defined in config appear as Matrix users
- `/invite @exo:localhost` in Element makes agent join room
- Agent shows in bz sidebar when in focused room
- Can DM agent from Element or bz
- Agent credentials persist across restarts

**Depends on**: Phase 8
**Enables**: Phase 10

### Phase 10: Chaperone Lifecycle Commands

Implement `/restart`, `/quit` DM commands for chaperone control.

- [ ] DM command parsing in chaperone
- [ ] `/quit` — chaperone exits gracefully, bzd removes from roster
- [ ] `/restart` — chaperone restarts (bzd respawns)
- [ ] Confirmation message before destructive actions
- [ ] User chaperone special handling (can't quit while bz connected?)

**Acceptance criteria**:
- DMing `/quit` to agent chaperone causes it to exit
- DMing `/restart` to agent chaperone causes restart (new PID)
- Chaperone sends confirmation "Shutting down..." before exit
- User chaperone handles these commands appropriately

**Depends on**: Phase 9
**Enables**: Phase 11

### Phase 11: Presence & Interrupts

Agents show availability and can be interrupted.

- [ ] Presence tracking: online/busy/idle based on PTY activity
- [ ] Presence updates to Matrix
- [ ] Presence display in sidebar
- [ ] @mention detection in chaperone
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
| Conduit embedding complexity | Medium | High | Evaluate library interface in Phase 1; sidecar fallback ready |
| matrix-sdk tokio integration | Low | Medium | Both use tokio; test early in Phase 2 |
| Multi-process coordination bugs | Medium | Medium | Comprehensive integration tests; clear protocol contracts |
| PTY socket permission issues | Low | Low | Same user, standard Unix permissions; test on target platforms |
| Resource usage (many processes) | Low | Medium | Profile in Phase 5; chaperones are lightweight |
| View multiplexing UX confusion | Medium | Low | Start with 2 views only; iterate based on feedback |
| Interrupt protocol edge cases | High | Medium | Simple default first (1 min idle); defer sophistication |

## Open Questions

1. **Conduit crates.io vs git** — Is conduit published? What version? Need to evaluate in Phase 1.

2. **User identity sharing** — Does bz login as the same user as user-chaperone, or separate? Separate seems cleaner but means two Matrix clients for "the human."

3. **Mobile auth** — How does a user authenticate from Element mobile? Password set during first run? Acceptable to be local-only initially?

4. **Config hot-reload** — If user edits `bz.toml`, does bzd pick up changes? Probably not for v1 — restart required.

5. **PTY attachment UX** — PRD says `Ctrl+B t` but doesn't specify the interaction. Modal? Picker? Direct attach to current room?

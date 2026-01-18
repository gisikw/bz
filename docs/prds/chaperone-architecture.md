# Chaperone Architecture

Decompose bz into three components: bzd (Matrix server), bz (Matrix TUI), and chaperone (persistent PTY + Matrix bot) — where chaperone is the atomic building block for both user workspaces and agent sessions.

**Status**: Draft

## Problem

bz started as a terminal multiplexer with session persistence. We need to add a communication layer (Matrix) for multi-agent coordination: agents with identities, @mentions, interrupts, mobile access.

The original approach was monolithic: bzd manages PTYs, embeds Conduit, runs matrix-sdk, coordinates agents — all in one daemon. This conflates concerns and makes the system harder to reason about, test, and extend.

**Insight**: The "persistent PTY with a Matrix identity" is the fundamental unit. Both user workspaces and agent sessions are instances of this pattern. Factor it out as **chaperone** — then bzd and bz become simpler, single-purpose components.

## Constraints

1. **Self-contained by default** — `bzd` must work out of the box. No external Matrix server required.

2. **User workspaces need multiple PTYs** — A user in `#fort-nix` might have 3 terminals. Chaperones must support multiple PTYs (though agents typically use 1).

3. **Agents are first-class Matrix participants** — Agents have identities (`@exo:localhost`), can be @mentioned, DM'd, invited to rooms.

4. **Mobile access to chat** — Standard Matrix clients (Element) can connect for chat. PTYs are bz-only.

5. **Crash isolation** — One chaperone dying shouldn't take down the system.

6. **Persistence survives disconnection** — PTYs persist when bz disconnects. This is the chaperone's job, not bzd's.

## Contracts

### bzd ↔ Conduit

bzd embeds Conduit and exposes the Matrix client API on localhost.

- Conduit storage: `~/.local/share/bz/matrix/`
- Client API: `http://localhost:8448` (or configured)
- bzd starts Conduit on boot, shuts down cleanly on exit

### bzd ↔ Chaperone

bzd spawns and coordinates chaperones.

**Spawn**: bzd reads a manifest of chaperones to start on boot (user workspaces, known agents). Each chaperone is a separate process.

**PTY Attachment**: Chaperones notify bzd of PTY attachment changes:
- `attach(pty_socket, room_id)` — "My PTY is now attached to this room"
- `detach(pty_socket, room_id)` — "Remove my PTY from this room"
- `move(pty_socket, from_room, to_room)` — "Move my PTY between rooms"

**Protocol**: TBD — could be Matrix messages to a control room, or a separate Unix socket, or room state events.

### bzd ↔ bz

bz connects to bzd as a Matrix client.

- Standard Matrix client API for chat, rooms, presence
- Room state includes `attached_ptys` metadata (list of socket paths)
- bz reads `attached_ptys` to know which PTYs it can connect to for the focused room

### bz ↔ Chaperone PTY

bz connects directly to chaperone PTY sockets for terminal rendering.

- Socket path convention: `~/.local/share/bz/chaperones/<name>/pty-<n>.sock`
- Protocol: Same as current bzd ↔ bz PTY protocol (output streaming, input, resize)
- bz is just a viewer — chaperone owns the PTY lifecycle

### Chaperone ↔ Matrix

Each chaperone is a Matrix client with its own identity.

- Logs in as `@<name>:localhost` (e.g., `@exo:localhost`, `@user:localhost`)
- Joins rooms, receives messages, sends messages
- Manages own presence (online/busy/idle)
- Can be @mentioned, DM'd, invited

### Room ↔ Channel Mapping

A "channel" in bz is a Matrix room with optional metadata:

- `directory`: Working directory path (for directory-backed channels)
- `attached_ptys`: List of PTY sockets currently attached
- Chat-only rooms (like `#alerts`) have no directory, no PTYs

## Alternatives

### Option A: Monolithic bzd

bzd does everything: embeds Conduit, manages all PTYs, runs all Matrix clients, coordinates agents.

**Pros:**
- Single process, simpler deployment
- Shared memory, no IPC overhead
- One sync loop for all Matrix operations

**Cons:**
- Conflates concerns (PTY management + Matrix + agent coordination)
- One crash affects everything
- Harder to test components in isolation
- bzd becomes complex and hard to reason about

### Option B: Chaperone as Atomic Unit

Factor out "persistent PTY + Matrix identity" as the chaperone process. bzd becomes Matrix infrastructure + coordination. bz becomes Matrix TUI + PTY viewer.

**Pros:**
- Clean separation: each component has one job
- Crash isolation: chaperone death is contained
- Testable: chaperone works standalone
- Unified model: users and agents are both chaperones
- Simpler bzd: just Matrix server + light coordination
- Extensible: new chaperone types without changing bzd

**Cons:**
- More processes to manage
- IPC overhead (PTY sockets, coordination protocol)
- Multiple Matrix clients (one per chaperone)

### Option C: Chaperone Library (not process)

Chaperone as a Rust library linked into bzd, but with isolated "virtual" chaperone instances.

**Pros:**
- Single process
- No IPC overhead
- Could still have logical isolation

**Cons:**
- Crash isolation requires complex sandboxing
- Testing is harder (can't run chaperone standalone)
- Doesn't actually simplify bzd much

## Recommendation

**Option B: Chaperone as Atomic Unit**

The insight that "persistent PTY + Matrix identity" is the fundamental unit is too valuable to ignore. Factoring it out:

1. **Simplifies bzd** — It's a Matrix server with room metadata, not a god-object
2. **Simplifies bz** — It's a Matrix TUI that can view PTYs, not a session manager
3. **Makes chaperone reusable** — Could run headless agents without bz/bzd
4. **Unifies the model** — No special-casing user vs agent sessions

The costs (more processes, IPC) are manageable and are the standard tradeoffs for process isolation.

The multiple-Matrix-clients concern is mitigated because:
- Each client is lightweight (just its own rooms)
- Conduit is local (low latency)
- This is the natural model for Matrix bots
- Process isolation is worth more than shared sync state

## Out of Scope

1. **Federation** — Conduit supports it, but we're not prioritizing external Matrix servers.

2. **Voice/video** — Don't preclude architecturally, but not implementing.

3. **Web UI** — Mobile access is via existing Matrix clients. No bz-specific web interface.

4. **Multi-user bz** — Single human user, multiple agents. Not a team tool (yet).

5. **Distributed chaperones** — Chaperones on remote machines. Architecture allows it, not implementing now.

## Open Questions

1. **Chaperone ↔ bzd coordination protocol** — Matrix messages? Room state events? Separate control socket? Need to design this interface.

2. **Chaperone manifest format** — Where does bzd learn which chaperones to spawn? `bz.toml`? Separate file? Room membership?

3. **PTY socket permissions** — How does bz get permission to connect to chaperone-owned sockets? Same user? Socket in shared directory?

4. **User workspace chaperone** — Is it one chaperone with N PTYs, or N chaperones with 1 PTY each? Leaning toward one chaperone (matches "user as a Matrix identity").

5. **Agent interrupt protocol** — How does a chaperone interrupt its subprocess (e.g., Claude Code) when @mentioned? This is the chaperone's internal concern but needs design.

6. **Chaperone lifecycle** — When does a chaperone exit? When its PTYs all exit? When explicitly killed? When the room is deleted?

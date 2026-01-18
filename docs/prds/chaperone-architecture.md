# Chaperone Architecture

Decompose bz into three components: bzd (Matrix server), bz (Matrix TUI + PTY client), and chaperone (persistent PTY + Matrix bot) — where chaperone is the atomic building block for both user workspaces and agent sessions.

**Status**: Draft

## Problem

bz started as a terminal multiplexer with session persistence. We need to add a communication layer (Matrix) for multi-agent coordination: agents with identities, @mentions, interrupts, mobile access.

The original approach was monolithic: bzd manages PTYs, embeds Conduit, runs matrix-sdk, coordinates agents — all in one daemon. This conflates concerns and makes the system harder to reason about, test, and extend.

**Insight**: The "persistent PTY with a Matrix identity" is the fundamental unit. Both user workspaces and agent sessions are instances of this pattern. Factor it out as **chaperone** — then bzd and bz become simpler, single-purpose components.

## Constraints

1. **Self-contained by default** — `bzd` must work out of the box. No external Matrix server required.

2. **Chaperones support multiple PTYs** — User can have a PTY in `#fort-nix`, switch to `#wicket` and create a PTY there, without reaping the `#fort-nix` one. Persistence across channel switches, not simultaneous multiplexing. Agent chaperones are policy-limited to 1 PTY (chaperone-enforced, not architectural).

3. **Agents are first-class Matrix participants** — Agents have identities (`@exo:localhost`), can be @mentioned, DM'd, invited to rooms.

4. **Mobile access to chat** — Standard Matrix clients (Element) can connect for chat. PTYs are bz-only.

5. **Crash isolation** — One chaperone dying shouldn't take down the system.

6. **Persistence survives disconnection** — Both the Matrix server (bzd/Conduit) and PTYs (chaperones) must persist when bz disconnects. Autonomous agents need their nervous system even with flaky wifi. This is critical for eventual autonomous operation.

7. **User chaperone is mandatory** — bz requires a user chaperone to function. Without it, there's no Matrix identity for the human.

## Contracts

### bzd ↔ Conduit

bzd embeds Conduit and exposes the Matrix client API on localhost.

- Conduit storage: `~/.local/share/bz/matrix/`
- Client API: `http://localhost:8448` (or configured)
- bzd starts Conduit on boot, shuts down cleanly on exit
- **bzd must stay running** even when bz disconnects — it's the nervous system

### bzd ↔ Chaperone

bzd spawns and coordinates chaperones.

**Spawn**: bzd reads initial chaperones from `bz.toml` on first boot. Over time, chaperone membership becomes persistent state (via `/invite`, `/add`). User chaperone is always required.

**PTY Attachment**: Chaperones notify bzd of PTY attachment changes via control socket (not Matrix — too much latency/complexity for coordination):

```
attach {
  chaperone_id: string,    // e.g., "user" or "exo"
  pty_id: string,          // internal to chaperone, e.g., "pty-0"
  socket: path,            // e.g., ~/.local/share/bz/chaperones/user/pty-0.sock
  room_id: string          // Matrix room ID
}

detach {
  chaperone_id: string,
  pty_id: string,
  room_id: string
}

move {
  chaperone_id: string,
  pty_id: string,
  from_room: string,
  to_room: string
}
```

**Lifecycle**: bzd tracks chaperone PIDs for potential reaping. On bzd shutdown, TRAP signals all chaperones to stand down gracefully.

### bzd ↔ bz

bz connects to bzd as a Matrix client.

- Standard Matrix client API for chat, rooms, presence
- Room state includes `attached_ptys` metadata (list of PTY descriptors)
- bz reads `attached_ptys` to know which PTYs it can connect to for the focused room

### bz ↔ Chaperone PTY

bz connects directly to chaperone PTY sockets for terminal I/O (read AND write).

- Socket path convention: `~/.local/share/bz/chaperones/<name>/<pty_id>.sock`
- Protocol: Same as current bzd ↔ bz PTY protocol (output streaming, input, resize)
- Chaperone owns PTY lifecycle; bz is a read-write client

### Chaperone ↔ Matrix

Each chaperone is a Matrix client with its own identity.

- Logs in as `@<name>:localhost` (e.g., `@exo:localhost`, `@user:localhost`)
- Joins rooms, receives messages, sends messages
- Manages own presence (online/busy/idle)
- Can be @mentioned, DM'd, invited
- Responds to DM commands: `/restart`, `/quit` (user-initiated lifecycle control)

### Room ↔ Channel Mapping

A "channel" in bz is a Matrix room with optional metadata:

- `directory`: Working directory path (for directory-backed channels). Chat-only rooms get a tmpdir if a PTY is needed.
- `attached_ptys`: List of PTY descriptors currently attached

No special-casing of "chat-only" rooms — any room can have PTYs attached. Directory-backed rooms just have a persistent working directory; others get ephemeral tmpdirs on demand.

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

Factor out "persistent PTY + Matrix identity" as the chaperone process. bzd becomes Matrix infrastructure + coordination. bz becomes Matrix TUI + PTY client.

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
2. **Simplifies bz** — It's a Matrix TUI that can connect to PTYs, not a session manager
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

6. **Sophisticated interrupt policies** — Agents could have interruptability scores ("John drops everything", "Pat finishes current task"). Rabbit hole. Deferring.

## Resolved Questions

1. **Chaperone ↔ bzd coordination protocol** — Separate Unix control socket, not Matrix messages. Matrix is elegant but too much latency/complexity for tight coordination.

2. **Chaperone manifest format** — `bz.toml` for initial chaperones on first boot. Over time, chaperone set becomes persistent bzd state (via `/invite`, `/add`). User chaperone is mandatory and auto-created.

3. **PTY socket permissions** — Same user. Sockets in `~/.local/share/bz/chaperones/<name>/`.

4. **User workspace chaperone** — One chaperone with N PTYs. Agent chaperones are policy-limited to 1 PTY (enforced by chaperone, not architecture).

5. **Agent interrupt protocol** — Simple default: instant suspend on @mention/DM, resume work after 1 minute of conversation idle. Sophisticated policies deferred.

6. **Chaperone lifecycle** — Chaperones are permanent. They exist and are DM-able even when idle, even without a "home" room. They exit when:
   - Removed from the Matrix server (deregistered)
   - User DMs `/quit` to the chaperone
   - bzd shuts down (TRAP signals all chaperones to stand down)
   - User DMs `/restart` causes graceful restart

7. **Room tmpdir cleanup** — On room delete. bzd spawns tmpdirs for chat-only rooms, deletes them when room is deleted, cleans up in TRAP on shutdown. Keep it simple.

8. **PTY-to-room attachment UX** — Keybind in bz: `Ctrl+B t` (modal editing scheme). Future UX enhancements can add other triggers if needed.

9. **Chaperone config format** — Filepath only: `bzc --config=./path/to/chaperone.toml`. Decouples chaperone definition from bzd — they can evolve independently. Flow: user invokes `bz` → `bz` attaches to or spawns `bzd` → `bzd` spawns `bzc --config=<path>` for each chaperone.

## Open Questions

None currently — ready for implementation planning.

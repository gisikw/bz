# Comms Backbone

Matrix as the nervous system for multi-agent coordination in bz.

**Status**: Draft

## Problem

bz is a multi-agent workspace orchestration layer where communication is fundamental, not bolted on. Every channel has a chat. Agents have persistent identities that can be @mentioned, interrupted, and redirected. Users need mobile access to comms without terminal access.

Currently, bz has:
- Terminal multiplexing (PTY management)
- Session persistence (bzd daemon)
- No communication layer

Without comms:
- Agents can't be interrupted or redirected
- No @mentions, no DMs
- No mobile access
- No chat history or presence
- Supervisory processes have nothing to listen to

## Constraints

1. **Self-contained by default** — `bz` must work out of the box without requiring external infrastructure. No "bring your own Matrix server."

2. **Agents are first-class participants** — Agents need Matrix identities (`@exo:localhost`), not just the human user. They send and receive messages.

3. **Observable from workspaces** — The chat isn't separate from the workspace view. bz needs to render chat inline, show presence, display @mentions.

4. **Mobile access viable** — A standard Matrix client (Element, FluffyChat) should be able to connect and participate in conversations when the user is away from terminal.

5. **Lightweight enough for dev machines** — This runs on laptops alongside actual work. Can't be a resource hog.

6. **Rust ecosystem preferred** — Consistency with bz codebase, easier integration.

## Contracts

### Agent ↔ Matrix Identity

Each agent registered with bz has a corresponding Matrix user:
- `@exo:localhost` — Exo, chief of staff
- `@delegate:localhost` — Delegate agent
- User is `@kevin:localhost` (or configured name)

Agent identities are created/managed by bz, not manually provisioned.

### Chaperone ↔ Matrix Events

Each agent's chaperone (supervisor actor) subscribes to:
- @mentions of their agent across all rooms
- DMs to their agent
- Configurable: all messages in rooms where agent is present

Chaperone receives events, decides on interrupt behavior, synthesizes context.

### Workspace ↔ Chat Context

When an agent is interrupted:
- Chaperone captures current workspace state (what were they doing?)
- Constructs context message: "You were working on X in #channel"
- Injects into agent's next prompt

When agent responds:
- Response goes to Matrix room/DM
- Optionally updates workspace (if action required)

### Channel ↔ Room Mapping

Each bz channel corresponds to a Matrix room:
- `#exocortex` → `!exocortex:localhost`
- Room membership = channel participants (user + assigned agents)
- Room history = channel chat history

### Mobile ↔ Homeserver

Standard Matrix federation/client API:
- Expose port 8448 (or configured)
- Standard Element/FluffyChat can connect
- User authenticates as `@kevin:localhost`
- Full chat access, no workspace visibility

## Alternatives

### Option A: Embedded Conduit

Run Conduit (Rust Matrix homeserver) as part of bzd.

```
bzd
├── PTY Manager
├── Conduit (homeserver)
├── Agent Registry
└── Chaperones
```

**Pros:**
- Self-contained, single daemon
- Full Matrix protocol compliance
- Federation-ready if desired
- Conduit is lightweight (~10MB, Rust)
- matrix-sdk-rust for client operations

**Cons:**
- Added complexity in bzd
- Homeserver lifecycle management
- Storage requirements (SQLite for Conduit)
- More surface area for bugs

### Option B: Sidecar Conduit

Run Conduit as a separate process, bz connects as a client.

```
bzd ←→ conduit (separate process)
```

**Pros:**
- Cleaner separation of concerns
- Can restart homeserver independently
- Easier to debug/inspect

**Cons:**
- Two processes to manage
- IPC overhead
- User must ensure both are running

### Option C: Custom Protocol + Matrix Bridge

Build a simple internal message bus, bridge to Matrix for mobile.

```
bzd (custom protocol) ←→ bridge ←→ Matrix
```

**Pros:**
- Lighter weight for local-only use
- Full control over message format
- No Matrix complexity if you don't need mobile

**Cons:**
- Building two systems
- Bridge is another component to maintain
- Loses Matrix ecosystem benefits (E2EE, clients, etc.)

### Option D: Matrix Client Only (External Server)

bz connects to an existing Matrix server as a client.

**Pros:**
- No homeserver to run
- Leverage existing infrastructure

**Cons:**
- Not self-contained (violates constraint #1)
- Requires external setup
- Agent identity provisioning becomes complex

## Recommendation

**Option A: Embedded Conduit**

Matrix is the right protocol — it handles presence, history, E2EE, sync, and has a mature client ecosystem. The question is just how to run it.

Embedding Conduit in bzd gives us:
- Single daemon, single lifecycle
- True self-contained operation
- No user-facing complexity
- Federation available if wanted later

The added complexity is real but manageable. Conduit is designed for lightweight/embedded use and is actively maintained Rust code. The alternative (custom protocol) means building everything Matrix already solves.

Sidecar (Option B) is a reasonable fallback if embedding proves problematic, but adds user-facing complexity we'd like to avoid.

## Out of Scope

1. **Federation with public Matrix network** — We'll support it architecturally but won't prioritize or test it initially. This is a local-first tool.

2. **Web client** — Mobile access is via existing Matrix clients. We're not building a web UI.

3. **Voice/video** — Matrix supports this; we don't need it for agent coordination.

4. **Bridges to other platforms** — Slack, Discord, IRC bridges are possible via Matrix ecosystem but not our problem to solve.

5. **Multi-user bz** — Single user, multiple agents. Not a team collaboration tool (yet).

## Open Questions

1. **E2EE handling** — Do we enable encryption by default? Adds complexity for agent message handling (Vodozemac). Probably yes for DMs, maybe not for rooms?

2. **Agent credential management** — How do agents authenticate to the homeserver? Auto-provisioned tokens? Need secure storage.

3. **Conduit storage location** — `~/.local/share/bz/matrix/`? Size management for long-running instances?

4. **Room creation policy** — Are rooms auto-created from bz.toml channels? What about ad-hoc rooms?

5. **Presence semantics** — What does "agent is online" mean? Workspace active? Chaperone listening? Both?

6. **History limits** — How much chat history to retain? Configurable per-room?

7. **Startup sequencing** — Does Conduit need to be fully ready before bz TUI starts? Loading states?

# Rust TUI Rewrite

Replace bash+zellij+tmux stack with a custom ratatui-based TUI to enable activity detection and full UI control.

**Status**: Draft

## Problem

bz is a multi-agent coordination TUI — an "office building for agents" with Slack-like channel navigation. The core UX requires:

- **Unread indicators**: Know when a channel has new activity (bold in sidebar, badges)
- **Bell detection**: Catch terminal bells from embedded apps (Claude Code notifications, etc.)
- **Polished feel**: No chrome we can't control, smooth keybinds, responsive layout

The current POC (bash scripts + zellij + tmux) validated the concept but hit a hard ceiling: **zellij has no API for activity detection**. Neither does its plugin system — there's no `PaneActivity` or `TerminalBell` event. We're building workarounds for a fundamental gap.

This isn't polish work on top of a solid foundation. It's friction that will compound with every feature.

## Constraints

### Must Support

1. **PTY embedding** — Run real terminal applications (Claude Code, iamb, user shells) with full escape sequence support
2. **Activity tracking** — Know when any channel has output, even when not focused
3. **Bell detection** — Parse `\x07` from embedded terminals
4. **Single binary** — No runtime dependencies on zellij, tmux, or other multiplexers
5. **Cross-platform** — Linux required, macOS nice-to-have, Windows deferred

### Target Applications

These must render correctly:
- Claude Code (complex TUI with syntax highlighting, streaming output)
- iamb (ratatui-based Matrix client)
- OpenCode (bubbletea-based)
- vim/neovim (alternate screen, mouse support)
- tmux (nested multiplexer for session persistence within channels)
- Standard shells (bash, zsh, fish)

Mouse/cursor support is required — smooth channel-surfing on mobile depends on it.

### Non-Negotiables

- Activity detection must be per-channel, not global
- Keybinds must not conflict with embedded applications (bz uses its own leader key or mode)
- Must work over SSH (no GPU requirement)

## Contracts

### Channel → PTY

Each channel owns one or more PTY sessions. A channel is the unit of "attention" — switching channels switches which PTY receives input.

```
Channel {
    id: ChannelId,
    name: String,
    ptys: HashMap<PtyId, Pty>,
    focused_pty: PtyId,
}
```

### PTY → Terminal Parser

Each PTY wraps a child process and feeds output through vt100 for parsing.

```
Pty {
    id: PtyId,
    child: Child,
    parser: vt100::Parser,
    activity: ActivityState,
}

enum ActivityState {
    Idle,                    // No unread activity
    Active(u32),             // Unread output, with notification count
    AwaitingInput,           // Agent waiting on user (e.g., Claude Code prompt)
}
```

The `Active(u32)` count aggregates notification-worthy events:
- Each bell (`\x07`) increments the count
- Future: @-mentions parsed from terminal output could increment
- Future: Claude Code "waiting for input" detection sets `AwaitingInput`

This model supports richer sidebar UI later (badge counts, different icons for waiting-on-user vs background activity) without requiring schema changes.

The bz event loop reads from all PTYs (via polling/async), updates parser state, and updates activity.

### Input Routing

```
Input Flow:
  Raw input → bz keybind check → if not consumed → focused PTY stdin
```

bz reserves a leader key (e.g., `Ctrl+B` or `Ctrl+Space`) for its own commands. Everything else passes through.

### Sidebar ↔ Channels

Sidebar is a ratatui widget that renders the channel list. It reads from channel state but doesn't own it.

```
Sidebar::render(channels: &[Channel], focused: ChannelId) → Widget
```

### Activity Detection Contract

When bytes arrive from a PTY that is not currently focused:
1. If `activity` is `Idle`, transition to `Active(0)`
2. If bytes contain `\x07` (bell), increment the count in `Active(n)` → `Active(n+1)`

When channel becomes focused:
1. Reset all PTY activity states in that channel to `Idle`

Sidebar aggregates activity across all PTYs in a channel for display (sum of counts, highest-priority state wins for icon).

## Alternatives

### Option A: Continue with bash+zellij

Keep iterating on the current POC.

**Pros**: No rewrite cost, familiar stack
**Cons**: Activity detection impossible, keybind conflicts, floating pane chrome issues, two-multiplexer cognitive overhead

**Verdict**: We've hit the ceiling. Further investment has diminishing returns.

### Option B: Zellij WASM plugin

Write a zellij plugin to replace bash scripts.

**Pros**: Better control over sidebar UI, compiled (faster), remove file-based state
**Cons**: Same activity detection gap — plugin API lacks `PaneActivity` event. Still need tmux inside for session persistence.

**Verdict**: Pays migration cost without solving the core problem.

### Option C: Custom ratatui TUI (Recommended)

Build our own terminal UI with embedded PTY sessions.

**Pros**:
- Activity detection is trivial (we own the read loop)
- Bell detection via vt100 parser
- Full UI control, no chrome we can't remove
- Single binary, no dependencies
- Unlimited polish ceiling

**Cons**:
- Higher initial development cost
- Terminal emulation edge cases
- Must implement scrollback ourselves (or defer)

**Verdict**: Right answer for a production-quality product.

### Option D: tmux control mode

Use tmux's `-CC` mode as the backend, build ratatui UI as the frontend.

**Pros**: tmux handles session persistence, multiplexing
**Cons**: Still no activity detection without polling. Adds IPC complexity. Two processes instead of one.

**Verdict**: Doesn't solve the core problem, adds complexity.

## Recommendation

**Go with Option C: Custom ratatui TUI.**

The deciding factor is activity detection. Neither zellij nor tmux expose this capability. If we build on either, we're permanently working around a fundamental gap.

The development cost is bounded — we're building a tabbed terminal emulator, not tmux. The hard parts (PTY abstraction, terminal parsing) are solved by battle-tested libraries:
- **portable-pty**: Cross-platform PTY from WezTerm author
- **vt100**: Terminal parser designed for embedding
- **ratatui**: Mature TUI framework

Existing proofs of concept (mprocs, ratterm) demonstrate this is achievable.

## Out of Scope

### Explicitly Deferred

1. **Session persistence** — Channels can run tmux internally for now. Native persistence comes later.
2. **Windows support** — Linux first, Windows if there's demand.
3. **Splits within channels** — Each channel is one workspace. No tmux-style splits.
4. **Copy mode** — Embedded apps handle their own scrolling. Native scrollback can come later.
5. **Plugin system** — No extensibility in v1. We control the feature set.

### Not Doing

1. **GPU rendering** — Must work over SSH. Software rendering only.
2. **Embedding iamb as library** — Run it as subprocess via PTY like everything else.
3. **Custom chat protocol** — Use Matrix (iamb) for chat, not our own thing.

## Decisions

1. **Leader key**: `Ctrl+B` (configurable later, out of scope for v1)
2. **Async runtime**: tokio (standard-of-care for Rust async)
3. **Config format**: TOML
4. **Scrollback**: Out of scope — users can run tmux inside channels for scrollback
5. **Persona supervisor**: Out of scope — not spawning agents yet

## Open Questions

1. **tui-term maturity** — Is it production-ready or do we use portable-pty + vt100 directly? Needs investigation during M1.

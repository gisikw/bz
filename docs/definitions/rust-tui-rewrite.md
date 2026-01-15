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
- Standard shells (bash, zsh, fish)

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
    ptys: Vec<Pty>,
    focused_pty: usize,
    has_activity: bool,
    has_bell: bool,
}
```

### PTY → Terminal Parser

Each PTY wraps a child process and feeds output through vt100 for parsing.

```
Pty {
    child: Child,
    parser: vt100::Parser,
    has_unread_output: bool,
}
```

The bz event loop reads from all PTYs (via polling/async), updates parser state, and sets activity flags.

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
1. Set `channel.has_activity = true`
2. If bytes contain `\x07`, also set `channel.has_bell = true`

When channel becomes focused:
1. Clear `has_activity` and `has_bell`

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

## Open Questions

1. **Leader key choice** — What's the least conflicting option? `Ctrl+Space`? `Ctrl+B`? Configurable?

2. **tui-term maturity** — Is it production-ready or do we vendor/fork vt100 directly?

3. **Async runtime** — tokio? async-std? smol? Need to read from multiple PTYs concurrently.

4. **Scrollback strategy** — vt100 provides it, but how do we expose scroll mode UX?

5. **Config format** — TOML? KDL? Where do channel definitions live?

6. **Persona supervisor integration** — How does bz communicate with the persona supervisor? Unix socket? Embedded?

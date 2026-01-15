# Rust TUI Rewrite Implementation Plan

Implementation plan for [rust-tui-rewrite](../definitions/rust-tui-rewrite.md).

**Status**: Draft

## Current State

The POC uses bash scripts + zellij + tmux to create a Slack-like TUI for multi-agent orchestration.

### Architecture

```
zellij (multiplexer)
├── tab: fort-nix
│   ├── pane: sidebar.sh (bash, polls file state)
│   └── pane: workspace (tmux session inside)
├── tab: exocortex
│   └── ...
└── tab: wicket
    └── ...
```

### Components

| File | Purpose |
|------|---------|
| `layout.kdl` | Defines 4 tabs, each with sidebar + workspace pane |
| `config.kdl` | Clears zellij keybinds, sets Ctrl+K picker, Ctrl+Q quit |
| `scripts/sidebar.sh` | Renders channel list, polls `~/.bz/{current,activity,idle}` every 300ms |
| `scripts/picker.sh` | fzf-based channel switcher in floating pane, MRU ordering |
| `scripts/activity.sh` | File-based activity state manipulation |
| `scripts/cc-notify-hook.sh` | Claude Code hook → maps notifications to channel activity |
| `scripts/cc-stop-hook.sh` | Claude Code hook → marks channel idle when agent stops |

### What's Working

1. **Channel navigation** — Tabs work, picker switches tabs via `zellij action go-to-tab-name`
2. **Sidebar rendering** — Shows channels, highlights current, shows activity/idle indicators
3. **MRU ordering** — Picker remembers recently used channels
4. **Claude Code integration** — Notification hooks map CC events to bz activity (app-specific workaround)

### What Must Change

1. **Everything** — This is a full rewrite, not a migration. The bash+zellij stack is replaced entirely.

The POC validated the UX concept. The rewrite implements it properly.

## Gap Analysis

| Requirement | Current State | Gap | Approach |
|-------------|---------------|-----|----------|
| PTY embedding | zellij panes run tmux sessions | No direct PTY control | Own PTY via portable-pty |
| Activity detection | File-based, Claude Code hooks only | No generic detection, 300ms polling latency | Read loop on all PTYs, instant detection |
| Bell detection | Not implemented | Complete gap | Parse `\x07` from vt100 output |
| Mouse support | Depends on zellij/tmux passthrough | Unreliable, conflicts | Own mouse handling via crossterm |
| Single binary | Requires zellij + tmux + fzf + bash | Multiple dependencies | Rust binary with no runtime deps |
| Leader key (`Ctrl+B`) | zellij uses session mode hack | Conflicts with tmux default | Own input handling, clean passthrough |
| Activity state model | Booleans in files | No per-PTY tracking, no counts | `ActivityState` enum per PTY |
| Floating panes | zellij floating panes | Can't hide chrome completely | Own overlay rendering |
| Config | Hardcoded in bash scripts | Inflexible | TOML config file |

## Architectural Decisions

### AD1: Use tui-term vs raw portable-pty + vt100

**Context**: tui-term wraps portable-pty and vt100 into a ratatui widget. But it's "active development, work in progress."

**Decision**: Start with tui-term. Fall back to raw portable-pty + vt100 if we hit blockers.

**Rationale**: tui-term solves the integration problem. If it works, we save significant effort. If it doesn't, the fallback is well-understood (same underlying libraries). M1 will validate this.

### AD2: Async architecture

**Context**: Need to read from multiple PTYs concurrently while handling user input.

**Decision**: tokio with `tokio::select!` for multiplexing PTY reads and crossterm events.

**Rationale**:
- tokio is standard-of-care for Rust async
- portable-pty provides async-compatible readers
- crossterm has `EventStream` for async input

### AD3: State management

**Context**: Need to track channels, PTYs, activity state, focus.

**Decision**: Single `App` struct owns all state. No ECS, no message passing, no complexity.

**Rationale**: This is a small application. A channel list, a few PTYs, some flags. Over-engineering state management would slow us down. Refactor if/when complexity demands it.

### AD4: No abstraction layers yet

**Context**: Tempting to build "PTY manager", "channel manager", "activity tracker" abstractions.

**Decision**: Inline everything in M1-M3. Extract abstractions only when patterns emerge.

**Rationale**: Premature abstraction is the root of complexity. Build the thing, then see what wants to be extracted.

## Implementation Phases

These map directly to the milestones in epic `bz-5w9`.

### Phase 1: Hello PTY (`bz-5w9.1`)

Prove the stack works: spawn one PTY, render it, handle input.

- [ ] Initialize Rust project with ratatui, crossterm, tokio, tui-term (or portable-pty + vt100)
- [ ] Create main event loop: poll crossterm events + PTY output
- [ ] Spawn a shell PTY (bash)
- [ ] Render PTY output to full terminal
- [ ] Forward keyboard input to PTY
- [ ] Handle terminal resize (SIGWINCH → PTY resize)

**Entry criteria**: None
**Exit criteria**: Can run `vim`, `htop`, and basic shell commands in embedded PTY
**Enables**: Phase 2

### Phase 2: Multi-PTY with tabs (`bz-5w9.2`)

Multiple PTYs, tab-like switching between them.

- [ ] Data structure for multiple PTYs (simple `Vec<Pty>`)
- [ ] Read from all PTYs concurrently (tokio tasks or select)
- [ ] Track focused PTY index
- [ ] Keybind to cycle PTYs (temporary, will be replaced by sidebar)
- [ ] Only render focused PTY (others run in background)

**Entry criteria**: Phase 1 complete
**Exit criteria**: Can switch between 3+ shells, each maintains independent state
**Enables**: Phase 3

### Phase 3: Activity detection (`bz-5w9.3`)

The proof point. Detect output on unfocused PTYs.

- [ ] Add `ActivityState` to PTY struct
- [ ] On PTY read: if not focused, set `Active(0)`
- [ ] On PTY read: if contains `\x07`, increment bell count
- [ ] On focus change: reset activity to `Idle`
- [ ] Temporary UI: show activity state in a status line

**Entry criteria**: Phase 2 complete
**Exit criteria**: Bell in unfocused PTY shows indicator; switching clears it
**Enables**: Phase 4

### Phase 4: Sidebar + channel model (`bz-5w9.4`)

Replace "PTY list" with proper channel model. Add sidebar.

- [ ] Channel struct with id, name, ptys, focused_pty
- [ ] TOML config for channel definitions
- [ ] Sidebar widget (left column, configurable width)
- [ ] Sidebar renders: channel names, activity indicators, focus highlight
- [ ] `Ctrl+B` leader key mode
- [ ] `Ctrl+B` + `j/k` or arrow keys to navigate channels
- [ ] `Ctrl+B` + `Enter` to focus channel (or just navigate directly)

**Entry criteria**: Phase 3 complete
**Exit criteria**: Sidebar shows channels with activity indicators, navigation works
**Enables**: Phase 5

### Phase 5: Floating panes (`bz-5w9.5`)

Channel picker as floating overlay.

- [ ] Overlay rendering layer (draw on top of main content)
- [ ] Picker widget: channel list with fuzzy filter
- [ ] `Ctrl+K` opens picker
- [ ] Type to filter, Enter to select, Esc to cancel
- [ ] MRU ordering in picker
- [ ] Close picker on selection

**Entry criteria**: Phase 4 complete
**Exit criteria**: Ctrl+K opens picker, can fuzzy-search and switch channels
**Enables**: Phase 6

### Phase 6: Polish pass (`bz-5w9.6`)

Make it daily-driveable.

- [ ] Test with Claude Code, iamb, vim, tmux
- [ ] Fix terminal emulation edge cases as discovered
- [ ] Smooth resize behavior
- [ ] Error handling: PTY crash recovery, graceful degradation
- [ ] Mouse support: click sidebar to switch channels
- [ ] Config: channel definitions, leader key customization
- [ ] Startup: spawn configured PTYs per channel

**Entry criteria**: Phase 5 complete
**Exit criteria**: Can use bz for real work without hitting major bugs
**Enables**: Deprecation of bash+zellij POC

## Migration Strategy

No migration needed. This is greenfield development alongside the existing POC.

1. Build new `bz` binary in `src/`
2. Test in parallel with POC
3. When ready, switch to new binary
4. Archive/delete bash scripts and KDL configs

The POC's `~/.bz/` state files are not worth migrating. Channel history will reset.

## Risks

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| tui-term not mature enough | Medium | Medium | Fallback to raw portable-pty + vt100; same underlying libs |
| Terminal emulation edge cases | High | Low | Test early with target apps; vt100 is battle-tested |
| Async complexity | Low | Medium | Keep it simple; tokio::select! handles our use case |
| Scope creep (add tmux features) | Medium | High | Strict adherence to definition; defer to "out of scope" |
| Performance with high output | Low | Medium | Profile if issues arise; vt100 is designed for this |

## Open Decisions

None. All decisions resolved in definition or architectural decisions above.

tui-term maturity is a "try it and see" — not a decision to make upfront.

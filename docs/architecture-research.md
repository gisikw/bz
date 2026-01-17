# Architecture Research: TUI Foundation for bz

Research doc evaluating options for bz's core architecture. Context: we have a working POC using bash scripts + zellij + tmux. Question: is this the right foundation to polish, or should we pivot?

## Requirements Recap

From README, bz needs to:
- Embed multiple terminal sessions (Claude Code, OpenCode, iamb matrix client, user shells)
- Provide Slack-like channel navigation with activity indicators
- Support floating panes (mobile-friendly hamburger menu)
- Manage "persona attention" - pausing/resuming agent work across channels
- Feel polished, not cobbled together

The key technical requirements that drive this decision:
1. **PTY embedding** - run real terminal applications inside our UI
2. **Activity detection** - know when a pane has output (for unread indicators)
3. **Input routing** - send keystrokes to the correct embedded terminal
4. **Layout control** - sidebar, floating panes, responsive design

## Option 1: Continue with Bash-Driven Zellij

**What we have now**: KDL layouts + bash scripts for sidebar/picker + file-based state

### Capabilities
- Layout system works (tabs as channels, sidebar pane)
- Tab switching via `zellij action go-to-tab-name`
- Floating panes exist (picker works)
- tmux provides persistent sessions inside tabs

### Limitations (from painpoints.md)
- **No activity detection**: Zellij has no API for terminal bell, tab activity state, or output detection. Our Claude Code notification hook is a workaround specific to CC.
- **Keybind pass-through issues**: Multiple keybinds fail through Safari PWA → Termius → zellij → tmux stack
- **Floating pane chrome**: Can't fully hide PIN indicator, script path shows as name, `pane_frames false` doesn't apply
- **Declarative/imperative mismatch**: Config is KDL but sizing requires `zellij action` calls
- **Limited query capabilities**: Can't query current tab, pane states, etc.
- **Two multiplexers**: zellij + tmux = cognitive overhead, potential keybind conflicts

### Verdict
We've hit the ceiling. The activity detection gap alone is a blocker for the "unread indicators" feature that's core to the Slack-like UX. We'd be building increasingly fragile workarounds.

**Recommendation**: Don't invest more here.

---

## Option 2: Zellij WASM Plugin Architecture

**Hypothesis**: A plugin has deeper access to zellij internals and could solve our problems.

### Plugin API Capabilities

After reviewing [zellij-tile docs](https://docs.rs/zellij-tile/latest/zellij_tile/) and the [events](https://zellij.dev/documentation/plugin-api-events)/[commands](https://zellij.dev/documentation/plugin-api-commands.html) documentation:

**Plugins CAN:**
- Subscribe to `PaneUpdate`, `TabUpdate` events (get pane titles, commands, exit codes)
- Send keys to panes (`write_chars`, `write_to_pane_id`)
- Control panes/tabs extensively (open, close, resize, focus, rename)
- Edit scrollback (`edit_scrollback_for_pane_with_id`)
- Render custom UI (plugins are rendered as panes)
- Intercept all keypresses (`intercept_key_presses`)
- Run background workers (`ZellijWorker` trait)

**Plugins CANNOT:**
- **Read pane content directly** (only scrollback via editing)
- **Detect terminal bell**
- **Get notified on pane output**
- **Query activity state**

### The Problem

The plugin API gives us better control over layout and navigation, but it doesn't expose the one thing we need most: **knowing when something happened in a pane we're not looking at**.

The `PaneUpdate` event tells you about pane metadata (title, command, exit code) but not "this pane had new output." There's no `PaneActivity` or `TerminalBell` event.

### What a Plugin Could Improve
- Replace bash scripts with a compiled plugin (faster, no file-based state)
- Custom sidebar rendering with full control
- Better keybind handling (intercept at plugin level)
- Remove floating pane chrome issues (render our own UI)

### What It Can't Fix
- Activity detection - still requires out-of-band hacks (notification hooks per app)
- We'd still need tmux inside for persistent sessions (or trust zellij's session persistence)

### Development Cost
- Rust + WASM build pipeline
- Learn plugin API
- Hot-reload workflow exists but still slower than bash iteration
- Limited ecosystem (fewer examples than ratatui)

### Verdict
Plugins solve some polish issues but don't address the core gap. We'd be paying a migration cost without gaining the key capability we need.

**Recommendation**: Not worth the pivot. Same fundamental limitations.

---

## Option 3: Custom TUI with Ratatui

**Hypothesis**: Build our own terminal UI that embeds PTY sessions.

### The Stack

```
┌─────────────────────────────────────┐
│            bz (ratatui)             │
│  ┌─────────┐  ┌───────────────────┐ │
│  │ sidebar │  │    PTY widget     │ │
│  │         │  │  ┌─────────────┐  │ │
│  │#channel │  │  │ vt100 parser│  │ │
│  │#channel │  │  │ ↕ PTY ↕     │  │ │
│  │         │  │  │ child proc  │  │ │
│  └─────────┘  └───────────────────┘ │
└─────────────────────────────────────┘
```

### Key Libraries

**[ratatui](https://ratatui.rs/)** - TUI framework, widgets, event loop
- Mature, well-documented, large ecosystem
- 30k+ GitHub stars, active development

**[tui-term](https://crates.io/crates/tui-term)** - PTY widget for ratatui
- Latest: v0.3.0 (Jan 2026)
- Status: "active development, work in progress"
- Uses vt100 for parsing, portable-pty for cross-platform PTY
- Examples exist but limited documentation

**[portable-pty](https://crates.io/crates/portable-pty)** - Cross-platform PTY abstraction
- From WezTerm author, production-proven
- v0.9.0 (Feb 2025), 213k+ monthly downloads
- Handles Unix PTY and Windows ConPTY

**[vt100](https://crates.io/crates/vt100)** - Terminal parser
- "The terminal parser component of a graphical terminal emulator"
- Designed for exactly our use case (programs like tmux/screen)
- Provides in-memory representation of terminal contents

### What We Gain

1. **Activity detection**: We control the PTY read loop. We know exactly when bytes arrive from each child process. Trivial to set "has activity" flags.

2. **Bell detection**: vt100 parses escape sequences. We can detect `\x07` (bell) directly.

3. **Full UI control**: No chrome we can't remove. Pixel-perfect layout. Custom sidebar with whatever indicators we want.

4. **Single binary**: No zellij, no tmux. Just our app.

5. **Input routing**: We control where keystrokes go. No keybind conflicts with outer multiplexers.

### What We Lose

1. **tmux/zellij features**: Session persistence, attach/detach, copy mode, scroll mode - we'd implement what we need or skip what we don't.

2. **Development time**: This is real work. Not "glue some scripts together" but "write a terminal emulator subset."

3. **Edge cases**: Terminal emulation has a long tail of escape sequences, Unicode handling, resize behavior, etc.

### Complexity Assessment

**Do we need full multiplexing?**
No. We need tab-like switching between persistent terminal sessions. No splits within a channel (Claude Code IS the workspace, not a pane within it).

**Do we need copy mode/scroll mode?**
Probably, but Claude Code and iamb handle their own scrolling. For user shells, we'd need scrollback - vt100 provides this.

**What's the minimum viable feature set?**
- Spawn PTY child processes
- Render terminal output (via vt100 → ratatui)
- Route input to focused terminal
- Track activity per terminal
- Sidebar navigation
- Tab switching

This is actually less than "write your own tmux" - more like "write a tabbed terminal emulator."

### Development Cost

- **High initial investment**: 2-4 weeks to get something functional
- **Learning curve**: PTY handling, terminal emulation edge cases
- **Ongoing maintenance**: Less than you'd think - vt100 and portable-pty handle the hard parts

### Existing Art

**[ratterm](https://github.com/hastur-dev/ratterm)** - Split-terminal TUI with PTY + code editor, made with ratatui. Proof this is buildable.

**[mprocs](https://github.com/pvolok/mprocs)** - Process manager with embedded terminals, ratatui-based. Another proof point.

### Verdict
This is the right answer if we're serious about polish. We trade development time for complete control. The activity detection problem vanishes because we're the ones reading from the PTY.

**Recommendation**: Preferred option for a production-quality product.

---

## Option 4: Dark Horses

### 4a: tmux Control Mode

tmux has a [control mode](https://github.com/tmux/tmux/wiki/Control-Mode) (`tmux -CC`) designed for terminal emulator integration (used by iTerm2).

**Pros:**
- tmux handles all terminal multiplexing, session persistence
- Control mode gives programmatic access via text protocol
- Could build ratatui UI that drives tmux

**Cons:**
- Still can't detect activity without polling `tmux list-panes -F "#{pane_active}"`
- Adds IPC complexity (our TUI ↔ tmux ↔ child processes)
- tmux doesn't expose bell/activity events either

**Verdict**: Interesting but doesn't solve the core problem. Adds complexity without clear benefit.

### 4b: WezTerm with Lua

WezTerm is a GPU-accelerated terminal with built-in multiplexing and extensive [Lua scripting](https://wezterm.org/config/lua/wezterm.mux/).

**Pros:**
- Production-grade terminal emulator
- Lua callbacks on keybindings
- Native tabs, panes, splits
- Could customize appearance extensively

**Cons:**
- We'd be building a WezTerm config/plugin, not our own app
- Distribution story: users need WezTerm
- Still lacks activity detection API (checked the mux module docs)
- Can't embed arbitrary ratatui widgets (like our custom sidebar)

**Verdict**: If we only needed terminal multiplexing, this would be great. But we want to embed chat (iamb), which is itself a TUI. WezTerm can run TUIs but can't composite them into a unified interface.

### 4c: Embed iamb/OpenCode Directly

Both iamb (Matrix client) and OpenCode are ratatui-based. Could we import them as libraries rather than running them as subprocesses?

**Pros:**
- No PTY overhead for these apps
- Direct integration, shared state
- Unified event loop

**Cons:**
- They're applications, not libraries. Would require significant refactoring or forking.
- Claude Code is not open source / not embeddable
- User shells still need PTY embedding anyway

**Verdict**: Partial solution. Might be worth exploring for iamb if we go the ratatui route, but doesn't eliminate the need for PTY embedding.

---

## Comparison Matrix

| Capability | Bash+Zellij | Zellij Plugin | Ratatui+PTY | tmux Control |
|------------|-------------|---------------|-------------|--------------|
| Activity detection | ❌ | ❌ | ✅ | ❌ |
| Bell detection | ❌ | ❌ | ✅ | ❌ |
| Full UI control | ❌ | ⚠️ | ✅ | ⚠️ |
| Keybind control | ❌ | ⚠️ | ✅ | ⚠️ |
| Session persistence | ✅ (tmux) | ✅ | ❌ (build it) | ✅ |
| Distribution | ✅ | ⚠️ (zellij) | ✅ | ⚠️ (tmux) |
| Development effort | Low | Medium | High | Medium |
| Polish ceiling | Low | Medium | High | Medium |

---

## Recommendation

**Go with Option 3: Custom TUI with Ratatui.**

### Reasoning

1. **The core problem is activity detection.** Neither zellij nor tmux expose this. Building on either means we're always working around a fundamental gap.

2. **The development cost is bounded.** We're not building tmux. We're building a tabbed terminal with a sidebar. The hard parts (PTY abstraction, terminal parsing) are solved by portable-pty and vt100.

3. **The upside is unlimited polish.** Once we own the stack, every UX improvement is just code. No more "zellij doesn't support this" blockers.

4. **Existing proofs of concept exist.** mprocs and ratterm demonstrate this is achievable.

5. **The POC served its purpose.** We validated the UX concept. Now we're building the real thing.

### Suggested Approach

1. **Start minimal**: One embedded PTY (user shell), sidebar, tab switching, activity indicators
2. **Validate the stack**: Ensure vt100 + portable-pty + ratatui work together smoothly
3. **Add channels incrementally**: Get one working perfectly before adding complexity
4. **Defer persistence**: We can add session save/restore later; it's not core to the UX validation

### Risks

- **Terminal emulation edge cases**: Some programs might render incorrectly. Mitigate by testing with our target apps early (Claude Code, iamb).
- **Performance**: Lots of terminal output could be expensive. Mitigate with profiling; vt100 is designed for this.
- **Scope creep**: Easy to start adding tmux features. Stay focused on what bz actually needs.

---

## Sources

- [Zellij Plugin Tutorial](https://zellij.dev/tutorials/developing-a-rust-plugin/)
- [Zellij Plugin Events](https://zellij.dev/documentation/plugin-api-events)
- [Zellij Plugin Commands](https://zellij.dev/documentation/plugin-api-commands.html)
- [zellij-tile docs](https://docs.rs/zellij-tile/latest/zellij_tile/)
- [Ratatui](https://ratatui.rs/)
- [tui-term crate](https://crates.io/crates/tui-term)
- [portable-pty crate](https://crates.io/crates/portable-pty)
- [vt100 crate](https://docs.rs/vt100/latest/vt100/)
- [mprocs](https://github.com/pvolok/mprocs)
- [ratterm](https://github.com/hastur-dev/ratterm)
- [tmux Control Mode](https://github.com/tmux/tmux/wiki/Control-Mode)
- [WezTerm mux module](https://wezterm.org/config/lua/wezterm.mux/)
- [iamb Matrix client](https://lib.rs/crates/iamb)
- [OpenCode](https://github.com/opencode-ai/opencode)

# bz

A multi-agent coordination TUI. Pronounced "busy."

## What This Is

An office building for agents. Channels are floors. Each floor has a common area (chat) and workspaces (terminals where agents do actual work). You're the building manager with a view of every floor.

Think Slack, but:
- Every participant can have an observable workspace
- Channels contain both communication AND active work sessions
- You live here too — your shell, your neovim, your whatever
- Agents have persistent identities that move between floors, not disposable sessions

## Mental Model

```
bz (the office building)
│
├── #exocortex (floor)
│   ├── Common area (chat)
│   ├── Kevin's workspace (shell)
│   └── Exo's workspace (claude code)
│
├── #wicket (floor)
│   ├── Common area (chat)
│   ├── Kevin's workspace (shell)
│   └── Delegate's workspace
│
├── #fort-nix (floor)
│   ├── Common area (chat)
│   └── Kevin's workspace (shell)
│
└── DMs (@kevin ↔ @exo)
```

Channels generally correspond to repos/projects. Exo primarily works in #exocortex but can be pulled into #wicket when needed.

## Agent Identity

Agents aren't "a Claude session" — they're persistent identities with:
- **Persona**: Who they are (Exo is Kevin's chief of staff)
- **Role**: How they act in context
- **Continuity**: Memory of what they were doing, context when switching

One agent, one attention thread. Exo doesn't simultaneously exist in two places with fragmented context. She's either working in #exocortex or she's been pulled to #wicket — never both.

## Supervision & Interrupts

Each agent has an invisible supervisor (actor model) that:
1. Monitors for @mentions and DMs across all channels
2. Knows what the agent is currently doing (observable workspace)
3. Manages interrupts based on configured urgency
4. Provides context when switching: "You were filing the Q3 report when Kevin pinged you about X"
5. Handles resume logic after the interrupt is resolved

Interruptability is configurable per-agent. Some agents drop everything immediately; others finish their current task first.

## User Observability

The workspaces aren't just for agents — they're for you. At any time you can:
- Watch what any agent is doing in real-time
- Approve actions, pause work, give feedback
- Take over a workspace if needed
- Adjust the trust dial from fully-supervised to autonomous

## Mobile Access

The TUI is the full experience, but chat is accessible from anywhere. Point a Matrix client at the server, and you can:
- Check in on conversations
- Ping agents: "@exo, status update on the migration?"
- Quick capture tasks
- Stay connected without needing terminal access

You won't have workspace visibility, but you'll have comms.

## Technical Stack

- **Rust TUI** with ratatui (custom-built, not wrapping tmux/zellij)
- **Session daemon (bzd)** for persistence across detach/attach
- **Matrix (planned)** as the comms backbone — not bolted-on chat, but the nervous system

## Why "bz"

- Reads as "busy" — the sound of work happening
- Short, typeable: `bz`, `bz stop`, `bz --takeover`
- Originally from "bitizen" (Tiny Tower reference)

## Status

TUI validated and daily-driven. Session persistence working. Comms layer next.

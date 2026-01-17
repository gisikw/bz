# bz

A multi-agent coordination TUI. Pronounced "busy."

## What This Is

An office building for agents. Channels are floors. Each floor has a break room (chat) and workspaces (shells where agents actually do things). You're the building manager, watching the whole tower from the lobby.

Think Slack, but:
- For your terminal
- Channels contain both communication AND active work sessions
- You live here too — spin up neovim, run commands, whatever
- Mobile-friendly via floating panes that collapse to a hamburger

## Mental Model

```
bz
├── #fort-nix
│   ├── chat (IRC/Matrix TUI)
│   ├── workspace: claude
│   └── workspace: opencode
├── #exocortex
│   ├── chat
│   └── workspace: claude
├── #wicket
│   ├── chat
│   └── workspace: claude
└── @kevin ↔ @exo (DMs)
```

The sidebar shows all channels. Unread indicators. Bold when there's activity. Cmd+k to switch. On mobile, the sidebar collapses to a floating 󰍜 that expands on tap.

## Persona Supervision

One persona, one attention thread. Exo can exist in #exocortex and #fort-nix, but not simultaneously typing in both — that's uncanny.

When you @-mention someone or DM them:
1. Persona supervisor catches the interrupt
2. Pauses their current work (with saved context)
3. Switches their attention to the new channel
4. Injects context: "You were doing X, User needs you here"
5. After responding, supervisor decides: resume previous work or stay

Interrupt and resumption behavior can be per-persona.

## Technical Direction

- **Zellij** as the multiplexer base (not tmux — clean slate on bindings, better floating panes)
- **IRC or Matrix** as chat transport (prefer standards over rolling our own)
- **Declarative layouts** via zellij KDL files
- Channels = zellij sessions, workspaces = windows within sessions

## Why "bz"

- Reads as "busy" — the sound of work happening
- Short, typeable, `bz switch #fort-nix` feels right
- Tiny Tower reference that nobody will get

## Status

Concept. The TUI feel is the thing to validate first — if it doesn't feel right, the transport layer doesn't matter.

#!/usr/bin/env bash
# bz help - show keybindings

cat << 'EOF'

  bz keybindings
  ──────────────

  Ctrl+k        Switch channel (picker)
  Ctrl+q Ctrl+q Quit
  Ctrl+q Ctrl+x Quit dev loop
  Ctrl+/        Show this help

  Press any key to close.

EOF

read -n 1 -s
zellij action close-pane

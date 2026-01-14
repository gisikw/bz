#!/usr/bin/env bash
# bz channel picker - minimal fzf-based channel switcher

# Channel list (hardcoded for now)
CHANNELS=("fort-nix" "exocortex" "wicket" "bz")

# State file
STATE_FILE="${HOME}/.bz/current"

# Run fzf with minimal chrome
selected=$(printf '%s\n' "${CHANNELS[@]}" | fzf \
    --prompt=" " \
    --pointer=">" \
    --no-info \
    --no-separator \
    --no-scrollbar \
    --border=none \
    --margin=0 \
    --padding=0 \
    --height=6 \
    --layout=reverse \
    --bind="tab:accept,enter:accept,esc:abort" \
    --color=bg+:-1,pointer:white,prompt:dim \
    2>/dev/null) || true

# If something was selected, background the switch with delay
if [[ -n "$selected" ]]; then
    echo "$selected" > "$STATE_FILE"
    nohup sh -c "sleep 0.1; zellij action go-to-tab-name '$selected'" >/dev/null 2>&1 &
fi

# Close self while still focused
zellij action close-pane

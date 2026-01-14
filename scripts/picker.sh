#!/usr/bin/env bash
# bz channel picker - fzf-based channel switcher

set -e

# Channel list (hardcoded for now)
CHANNELS=("fort-nix" "exocortex" "wicket" "bz")

# State file
STATE_FILE="${HOME}/.bz/current"

# Get current channel for fzf default
current=$(cat "$STATE_FILE" 2>/dev/null || echo "fort-nix")

# Run fzf with channel list
# --height and --layout for floating appearance
# --prompt for branding
selected=$(printf '%s\n' "${CHANNELS[@]}" | fzf \
    --prompt="switch to: " \
    --height=~50% \
    --layout=reverse \
    --border=rounded \
    --no-info \
    --query="" \
    --select-1 \
    --exit-0 \
    --bind="tab:accept,enter:accept" \
    || true)

# If something was selected, switch to it
if [[ -n "$selected" ]]; then
    # Update state file
    echo "$selected" > "$STATE_FILE"
    # Switch zellij tab
    zellij action go-to-tab-name "$selected"
fi

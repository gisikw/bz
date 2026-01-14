#!/usr/bin/env bash
# bz sidebar - displays channel list with current channel highlighted

set -e

# Channel list (hardcoded for now)
CHANNELS=("fort-nix" "exocortex" "wicket" "bz")

# State file location
STATE_DIR="${HOME}/.bz"
STATE_FILE="${STATE_DIR}/current"

# Ensure state directory exists
mkdir -p "$STATE_DIR"

# Initialize state file if it doesn't exist
if [[ ! -f "$STATE_FILE" ]]; then
    echo "fort-nix" > "$STATE_FILE"
fi

# ANSI codes
BOLD='\033[1m'
DIM='\033[2m'
RESET='\033[0m'

render() {
    # Clear screen and move to top
    printf '\033[2J\033[H'

    # Read current channel
    local current
    current=$(cat "$STATE_FILE" 2>/dev/null || echo "fort-nix")

    # Print header with some padding
    echo ""

    # Print each channel
    for channel in "${CHANNELS[@]}"; do
        if [[ "$channel" == "$current" ]]; then
            # Current channel - bold with indicator
            printf " ${BOLD}#%s${RESET}\n" "$channel"
        else
            # Other channels - dimmed
            printf " ${DIM}#%s${RESET}\n" "$channel"
        fi
    done
}

# Initial render
render

# Watch for changes and re-render
# Using a simple poll since inotifywait may not be available
while true; do
    sleep 0.5
    render
done

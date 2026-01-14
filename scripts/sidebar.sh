#!/usr/bin/env bash
# bz sidebar - displays channel list with current channel highlighted

# Channel list (hardcoded for now)
CHANNELS=("fort-nix" "exocortex" "wicket" "bz")

# State file location
STATE_FILE="${HOME}/.bz/current"

# Ensure state directory exists
mkdir -p "${HOME}/.bz"

# Initialize state file if it doesn't exist
if [[ ! -f "$STATE_FILE" ]]; then
    echo "fort-nix" > "$STATE_FILE"
fi

# ANSI codes
BOLD='\033[1m'
DIM='\033[2m'
RESET='\033[0m'
BG_ACTIVE='\033[48;5;237m'  # Dark grey background
HIDE_CURSOR='\033[?25l'
SHOW_CURSOR='\033[?25h'

# Hide cursor for cleaner look
printf "$HIDE_CURSOR"
trap "printf '$SHOW_CURSOR'" EXIT

# Clear screen once at start
printf '\033[2J'

render() {
    # Move to top-left, don't clear (prevents flicker)
    printf '\033[H'

    # Read current channel
    local current
    current=$(cat "$STATE_FILE" 2>/dev/null || echo "fort-nix")

    # Header
    printf '\n'
    printf " ${DIM}CHANNELS${RESET}\n"
    printf '\n'

    # Print each channel (sidebar is 16 chars wide)
    for channel in "${CHANNELS[@]}"; do
        if [[ "$channel" == "$current" ]]; then
            # Current channel - grey background, full width
            printf "${BG_ACTIVE} #%-13s${RESET}\n" "$channel"
        else
            # Other channels - dimmed
            printf " ${DIM}#%-13s${RESET}\n" "$channel"
        fi
    done
}

# Initial render
render
last_state=$(cat "$STATE_FILE" 2>/dev/null)

# Only re-render when state changes
while true; do
    sleep 0.3
    current_state=$(cat "$STATE_FILE" 2>/dev/null)
    if [[ "$current_state" != "$last_state" ]]; then
        render
        last_state="$current_state"
    fi
done

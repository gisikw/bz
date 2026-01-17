#!/usr/bin/env bash
# Claude Code notification hook for bz
# Marks the channel as having activity based on cwd

# Read JSON from stdin
input=$(cat)

# Skip idle_prompt - we handle idle state via Stop hook
notification_type=$(echo "$input" | jq -r '.notification_type // empty')
if [[ "$notification_type" == "idle_prompt" ]]; then
    exit 0
fi

# Extract cwd from JSON
cwd=$(echo "$input" | jq -r '.cwd // empty')

if [[ -z "$cwd" ]]; then
    exit 0
fi

# Map cwd to channel name
# TODO: Make this configurable
channel=""
case "$cwd" in
    /home/dev/Projects/fort-nix*) channel="fort-nix" ;;
    /home/dev/Projects/exocortex*) channel="exocortex" ;;
    /home/dev/Projects/wicket*) channel="wicket" ;;
    /home/dev/Projects/bz*) channel="bz" ;;
esac

if [[ -n "$channel" ]]; then
    # Get the directory where this script lives
    SCRIPT_DIR="$(dirname "$(realpath "${BASH_SOURCE[0]}")")"
    "$SCRIPT_DIR/activity.sh" "$channel"
fi

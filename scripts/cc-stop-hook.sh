#!/usr/bin/env bash
# Claude Code stop hook for bz
# Marks the channel as idle/ready when Claude finishes responding

IDLE_FILE="${HOME}/.bz/idle"
mkdir -p "${HOME}/.bz"

# Read JSON from stdin
input=$(cat)

# Extract transcript_path and derive project from it
# Path format: ~/.claude/projects/.../session.jsonl
transcript_path=$(echo "$input" | jq -r '.transcript_path // empty')

if [[ -z "$transcript_path" ]]; then
    exit 0
fi

# Map transcript path to channel name based on project directory
# The path contains the project path encoded
channel=""
case "$transcript_path" in
    *fort-nix*) channel="fort-nix" ;;
    *exocortex*) channel="exocortex" ;;
    *wicket*) channel="wicket" ;;
    *"/bz/"*|*"-bz-"*) channel="bz" ;;
esac

if [[ -n "$channel" ]]; then
    # Don't add if already present
    if ! grep -qx "$channel" "$IDLE_FILE" 2>/dev/null; then
        echo "$channel" >> "$IDLE_FILE"
    fi
fi

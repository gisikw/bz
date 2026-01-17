#!/usr/bin/env bash
# bz activity - mark a channel as having activity
# Usage: activity.sh <channel>
#        activity.sh --clear <channel>

ACTIVITY_FILE="${HOME}/.bz/activity"
mkdir -p "${HOME}/.bz"

if [[ "$1" == "--clear" ]]; then
    # Clear activity for a channel
    channel="$2"
    if [[ -z "$channel" ]]; then
        echo "Usage: activity.sh --clear <channel>" >&2
        exit 1
    fi
    if [[ -f "$ACTIVITY_FILE" ]]; then
        grep -v "^${channel}$" "$ACTIVITY_FILE" > "${ACTIVITY_FILE}.tmp" 2>/dev/null || true
        mv "${ACTIVITY_FILE}.tmp" "$ACTIVITY_FILE"
    fi
else
    # Add activity for a channel
    channel="$1"
    if [[ -z "$channel" ]]; then
        echo "Usage: activity.sh <channel>" >&2
        exit 1
    fi
    # Don't add if already present
    if ! grep -qx "$channel" "$ACTIVITY_FILE" 2>/dev/null; then
        echo "$channel" >> "$ACTIVITY_FILE"
    fi
fi

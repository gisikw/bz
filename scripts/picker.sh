#!/usr/bin/env bash
# bz channel picker - minimal fzf-based channel switcher

# Prevent re-triggering while already open
LOCK_FILE="${HOME}/.bz/picker.lock"
if [[ -f "$LOCK_FILE" ]]; then
    zellij action close-pane
    exit 0
fi
touch "$LOCK_FILE"
trap "rm -f '$LOCK_FILE'" EXIT

# Resize floating pane to be compact and unpin it
if [[ -n "$ZELLIJ_PANE_ID" ]]; then
    zellij action change-floating-pane-coordinates \
        --pane-id "$ZELLIJ_PANE_ID" \
        --width 70 --height 8 \
        --x "40%" --y "30%" \
        --pinned false
fi

# Set terminal background to grey and clear screen
printf '\033[48;5;237m\033[2J\033[H'

# Channel list (hardcoded for now)
CHANNELS=("fort-nix" "exocortex" "wicket" "bz")

# State files
STATE_FILE="${HOME}/.bz/current"
HISTORY_FILE="${HOME}/.bz/history"
ACTIVITY_FILE="${HOME}/.bz/activity"
IDLE_FILE="${HOME}/.bz/idle"

# Build MRU-sorted channel list
# Channels in history come first (most recent at top), then any not in history
get_sorted_channels() {
    local sorted=()

    # First, add channels from history (preserves MRU order)
    if [[ -f "$HISTORY_FILE" ]]; then
        while IFS= read -r ch; do
            # Only add if it's a valid channel
            for valid in "${CHANNELS[@]}"; do
                if [[ "$ch" == "$valid" ]]; then
                    sorted+=("$ch")
                    break
                fi
            done
        done < "$HISTORY_FILE"
    fi

    # Then add any channels not yet in history
    for ch in "${CHANNELS[@]}"; do
        local found=0
        for s in "${sorted[@]}"; do
            [[ "$ch" == "$s" ]] && found=1 && break
        done
        [[ $found -eq 0 ]] && sorted+=("$ch")
    done

    printf '%s\n' "${sorted[@]}"
}

# Run fzf with minimal chrome, start with second item selected
selected=$(get_sorted_channels | sed 's/^/#/' | fzf \
    --prompt=" " \
    --pointer=">" \
    --no-info \
    --no-separator \
    --no-scrollbar \
    --no-header \
    --border=none \
    --margin=0,1 \
    --padding=0 \
    --height=6 \
    --layout=reverse \
    --bind="load:down,tab:accept,enter:accept,esc:abort" \
    --color=bg:237,bg+:237,pointer:white,prompt:dim \
    2>/dev/null) || true

# Strip # prefix from selection
selected="${selected#\#}"

# If something was selected, update state and history, then switch
if [[ -n "$selected" ]]; then
    echo "$selected" > "$STATE_FILE"

    # Update history: prepend selected, remove duplicates
    {
        echo "$selected"
        grep -v "^${selected}$" "$HISTORY_FILE" 2>/dev/null || true
    } > "${HISTORY_FILE}.tmp" && mv "${HISTORY_FILE}.tmp" "$HISTORY_FILE"

    # Clear activity and idle for this channel
    if [[ -f "$ACTIVITY_FILE" ]]; then
        grep -v "^${selected}$" "$ACTIVITY_FILE" > "${ACTIVITY_FILE}.tmp" 2>/dev/null || true
        mv "${ACTIVITY_FILE}.tmp" "$ACTIVITY_FILE"
    fi
    if [[ -f "$IDLE_FILE" ]]; then
        grep -v "^${selected}$" "$IDLE_FILE" > "${IDLE_FILE}.tmp" 2>/dev/null || true
        mv "${IDLE_FILE}.tmp" "$IDLE_FILE"
    fi

    nohup sh -c "sleep 0.1; zellij action go-to-tab-name '$selected'" >/dev/null 2>&1 &
fi

# Close self while still focused
zellij action close-pane

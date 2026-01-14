#!/usr/bin/env bash
# bz channel picker - minimal fzf-based channel switcher

# Channel list (hardcoded for now)
CHANNELS=("fort-nix" "exocortex" "wicket" "bz")

# State files
STATE_FILE="${HOME}/.bz/current"
HISTORY_FILE="${HOME}/.bz/history"

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
    --border=none \
    --margin=0 \
    --padding=0 \
    --height=6 \
    --layout=reverse \
    --bind="load:down,tab:accept,enter:accept,esc:abort" \
    --color=bg+:-1,pointer:white,prompt:dim \
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

    nohup sh -c "sleep 0.1; zellij action go-to-tab-name '$selected'" >/dev/null 2>&1 &
fi

# Close self while still focused
zellij action close-pane

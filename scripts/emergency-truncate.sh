#!/bin/bash
# Emergency session truncation script
# Usage: emergency-truncate.sh [keep_lines] [session_id]

KEEP_LINES=${1:-50}
AETHERVAULT_HOME="${AETHERVAULT_HOME:-}"
if [ -z "$AETHERVAULT_HOME" ]; then
    if [ -d "$HOME/.aethervault" ]; then
        AETHERVAULT_HOME="$HOME/.aethervault"
    elif [ -d "$HOME/.aethervault" ]; then
        AETHERVAULT_HOME="$HOME/.aethervault"
    else
        AETHERVAULT_HOME="$HOME/.aethervault"
    fi
fi

AGENT_DIR="$AETHERVAULT_HOME/agents/main"
SESSIONS_JSON="$AGENT_DIR/sessions/sessions.json"

# Get session ID (from arg or from sessions.json)
if [ -n "$2" ]; then
    SESSION_ID="$2"
else
    SESSION_ID=$(jq -r '."agent:main:main".sessionId // empty' "$SESSIONS_JSON" 2>/dev/null)
fi

if [ -z "$SESSION_ID" ]; then
    echo "ERROR: Could not determine session ID"
    exit 1
fi

SESSION_FILE="$AGENT_DIR/sessions/${SESSION_ID}.jsonl"

if [ ! -f "$SESSION_FILE" ]; then
    echo "ERROR: Session file not found: $SESSION_FILE"
    exit 1
fi

# Get current stats
TOTAL=$(wc -l < "$SESSION_FILE")
SIZE=$(du -h "$SESSION_FILE" | cut -f1)

echo "=== Emergency Truncation ==="
echo "Session: $SESSION_ID"
echo "Current: $TOTAL lines, $SIZE"
echo "Keeping: $KEEP_LINES lines"

if [ $TOTAL -le $((KEEP_LINES + 5)) ]; then
    echo "Session already small enough, no truncation needed"
    exit 0
fi

# Create timestamped backup
BACKUP="${SESSION_FILE}.backup.$(date +%s)"
cp "$SESSION_FILE" "$BACKUP"
echo "Backup: $BACKUP"

# Truncate: keep session header (line 1) + last N lines
TMP="${SESSION_FILE}.tmp"
head -1 "$SESSION_FILE" > "$TMP"
tail -n $KEEP_LINES "$SESSION_FILE" >> "$TMP"

# Add truncation notice
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%S.000Z")
echo "{\"type\":\"message\",\"timestamp\":\"$TIMESTAMP\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"[SYSTEM: Context emergency-truncated from $TOTAL to $KEEP_LINES messages due to overflow. Backup saved. Use /memory or session-logs for older context.]\"}]}}" >> "$TMP"

# Apply truncation
mv "$TMP" "$SESSION_FILE"

# Report
NEW_TOTAL=$(wc -l < "$SESSION_FILE")
NEW_SIZE=$(du -h "$SESSION_FILE" | cut -f1)
echo "After: $NEW_TOTAL lines, $NEW_SIZE"
echo "Removed: $((TOTAL - KEEP_LINES)) messages"
echo "=== Done ==="

---
name: emergency-compact
description: Emergency context compaction - truncate older messages and force compaction when hitting context limits.
metadata: {"aethervault":{"emoji":"🗜️","requires":{"bins":["jq"]}}}
---

# Emergency Compact

Force context compaction by truncating older messages when the normal compaction fails or context overflow persists.

## When to Use

Use this skill when:
- Context overflow (413 "Prompt is too long") keeps happening
- Normal auto-compaction fails
- Session is stuck at high context usage
- User explicitly asks to force compaction or reduce context

## Quick Emergency Procedure

### Step 1: Check Current Context Status

```bash
# Find current session ID
AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
if [ ! -d "$AETHERVAULT_HOME" ] && [ -d "$HOME/.aethervault" ]; then
  AETHERVAULT_HOME="$HOME/.aethervault"
fi
AGENT_DIR="$AETHERVAULT_HOME/agents/main"
SESSION_ID=$(jq -r '."agent:main:main".sessionId' "$AGENT_DIR/sessions/sessions.json")
SESSION_FILE="$AGENT_DIR/sessions/${SESSION_ID}.jsonl"

# Count messages and estimate size
echo "Session: $SESSION_ID"
wc -l "$SESSION_FILE"
du -h "$SESSION_FILE"
```

### Step 2: Create Truncated Backup and Force Reduce

```bash
# Backup current session
cp "$SESSION_FILE" "${SESSION_FILE}.backup.$(date +%s)"

# Keep only last N messages (adjust N as needed - start with 50)
KEEP_LINES=50
TOTAL=$(wc -l < "$SESSION_FILE")
SKIP=$((TOTAL - KEEP_LINES))

if [ $SKIP -gt 0 ]; then
  # Create truncated version: keep header (first line) + last N lines
  head -1 "$SESSION_FILE" > "${SESSION_FILE}.tmp"
  tail -n $KEEP_LINES "$SESSION_FILE" >> "${SESSION_FILE}.tmp"
  mv "${SESSION_FILE}.tmp" "$SESSION_FILE"
  echo "Truncated from $TOTAL to $((KEEP_LINES + 1)) lines"
else
  echo "Session already small enough ($TOTAL lines)"
fi
```

### Step 3: Add Truncation Notice

```bash
# Add a notice that context was truncated
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%S.000Z")
NOTICE='{"type":"message","timestamp":"'"$TIMESTAMP"'","message":{"role":"user","content":[{"type":"text","text":"[SYSTEM: Context was emergency-truncated due to overflow. Older messages were removed to allow conversation to continue. Use /memory or session-logs skill to access older context if needed.]"}]}}'
echo "$NOTICE" >> "$SESSION_FILE"
```

## Alternative: Aggressive Truncation (Nuclear Option)

If the above doesn't work, keep only the essential:

```bash
# Keep only session header and last 10 messages
KEEP_LINES=10
head -1 "$SESSION_FILE" > "${SESSION_FILE}.tmp"
tail -n $KEEP_LINES "$SESSION_FILE" >> "${SESSION_FILE}.tmp"
mv "${SESSION_FILE}.tmp" "$SESSION_FILE"

# Add notice
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%S.000Z")
echo '{"type":"message","timestamp":"'"$TIMESTAMP"'","message":{"role":"user","content":[{"type":"text","text":"[SYSTEM: Emergency truncation performed - context reduced to last 10 messages. Use /memory for context recovery.]"}]}}' >> "$SESSION_FILE"
```

## After Truncation

1. **Wait a moment** - The session file watcher will pick up changes
2. **Send any message** - This will trigger the session to reload
3. **Compaction should now succeed** - With reduced context, auto-compact works

## Prevention Tips

- Set `contextWindow` lower than actual limit (e.g., 150K for 200K limit)
- Use `/memory` to checkpoint important context before it's compacted
- For long sessions, periodically use `/new` to start fresh

## Troubleshooting

If truncation doesn't help:
1. Check if session file is locked: `ls -la ${SESSION_FILE}.lock`
2. Remove lock if stale: `rm ${SESSION_FILE}.lock`
3. Restart aethervault: `systemctl restart aethervault`

## One-Liner for Quick Fix

```bash
AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}" && [ -d "$HOME/.aethervault" ] && [ ! -d "$AETHERVAULT_HOME" ] && AETHERVAULT_HOME="$HOME/.aethervault" && AGENT_DIR="$AETHERVAULT_HOME/agents/main" && SESSION_ID=$(jq -r '."agent:main:main".sessionId' "$AGENT_DIR/sessions/sessions.json") && SF="$AGENT_DIR/sessions/${SESSION_ID}.jsonl" && cp "$SF" "$SF.bak" && (head -1 "$SF"; tail -50 "$SF") > "$SF.tmp" && mv "$SF.tmp" "$SF" && echo "Truncated to 50 messages"
```

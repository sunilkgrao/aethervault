#!/bin/bash
# AetherVault model_hook wrapper for Codex CLI
# Reads AgentHookRequest JSON from stdin, passes to Python hook.
#
# Safety: enforces a hard timeout, traps signals to clean up children,
# and exits cleanly on any failure.

set -euo pipefail

HOOK_TIMEOUT=${CODEX_HOOK_TIMEOUT:-150}  # 2.5 min (slightly above Python 2 min)
PYTHON_HOOK="/root/.aethervault/hooks/codex-model-hook.py"
LOG_TAG="[codex-hook]"

# --- Signal handling: forward signals to child, then exit ---
CHILD_PID=""

cleanup() {
    local sig="${1:-TERM}"
    if [ -n "$CHILD_PID" ] && kill -0 "$CHILD_PID" 2>/dev/null; then
        # Kill the entire process group of the child
        kill -"$sig" -- -"$CHILD_PID" 2>/dev/null || true
        # Give it a moment, then force-kill
        sleep 1
        kill -0 "$CHILD_PID" 2>/dev/null && kill -9 -- -"$CHILD_PID" 2>/dev/null || true
    fi
}

on_signal() {
    cleanup TERM
    exit 143  # 128 + 15 (SIGTERM)
}

on_timeout() {
    echo "{\"message\":{\"role\":\"assistant\",\"content\":\"(Codex hook timed out after ${HOOK_TIMEOUT}s)\",\"tool_calls\":[]}}"
    cleanup KILL
    exit 124
}

trap on_signal SIGTERM SIGINT SIGHUP
trap on_timeout SIGALRM

# --- Validate Python hook exists ---
if [ ! -f "$PYTHON_HOOK" ]; then
    echo "{\"message\":{\"role\":\"assistant\",\"content\":\"(Error: $PYTHON_HOOK not found)\",\"tool_calls\":[]}}"
    exit 1
fi

# --- Run with timeout ---
# Use setsid so the Python process gets its own process group (for clean kill)
setsid python3 "$PYTHON_HOOK" &
CHILD_PID=$!

# Background timer for hard timeout
(
    sleep "$HOOK_TIMEOUT"
    kill -ALRM $$ 2>/dev/null || true
) &
TIMER_PID=$!

# Wait for child
wait "$CHILD_PID" 2>/dev/null
EXIT_CODE=$?

# Cancel timer
kill "$TIMER_PID" 2>/dev/null || true
wait "$TIMER_PID" 2>/dev/null || true

# If child died with a signal, emit a valid JSON error response
if [ $EXIT_CODE -gt 128 ]; then
    SIG=$((EXIT_CODE - 128))
    echo "{\"message\":{\"role\":\"assistant\",\"content\":\"(Codex hook killed by signal $SIG)\",\"tool_calls\":[]}}"
fi

exit $EXIT_CODE

#!/bin/bash
# AetherVault model_hook wrapper for Codex CLI
# Reads AgentHookRequest JSON from stdin, passes to Python hook.
#
# NO hard timeout. Codex tasks can run for hours or days.
# The Rust caller monitors for zombie processes; this script just
# forwards signals and lets the Python hook run until it finishes.

set -euo pipefail

PYTHON_HOOK="/root/.aethervault/hooks/codex-model-hook.py"

# --- Signal handling: forward signals to child, then exit ---
CHILD_PID=""

cleanup() {
    local sig="${1:-TERM}"
    if [ -n "$CHILD_PID" ] && kill -0 "$CHILD_PID" 2>/dev/null; then
        kill -"$sig" -- -"$CHILD_PID" 2>/dev/null || true
        sleep 1
        kill -0 "$CHILD_PID" 2>/dev/null && kill -9 -- -"$CHILD_PID" 2>/dev/null || true
    fi
}

on_signal() {
    cleanup TERM
    exit 143  # 128 + 15 (SIGTERM)
}

trap on_signal SIGTERM SIGINT SIGHUP

# --- Validate Python hook exists ---
if [ ! -f "$PYTHON_HOOK" ]; then
    echo "{\"message\":{\"role\":\"assistant\",\"content\":\"(Error: $PYTHON_HOOK not found)\",\"tool_calls\":[]}}"
    exit 1
fi

# --- Buffer stdin first, then pipe to Python (no timeout) ---
INPUT=$(cat)

echo "$INPUT" | python3 "$PYTHON_HOOK" &
CHILD_PID=$!
wait "$CHILD_PID"
EXIT_CODE=$?

# If killed by signal, emit a valid JSON error response
if [ $EXIT_CODE -gt 128 ]; then
    SIG=$((EXIT_CODE - 128))
    echo "{\"message\":{\"role\":\"assistant\",\"content\":\"(Codex hook killed by signal $SIG)\",\"tool_calls\":[]}}"
fi

exit $EXIT_CODE

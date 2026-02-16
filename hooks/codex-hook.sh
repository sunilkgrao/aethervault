#!/bin/bash
# AetherVault model_hook wrapper for Codex CLI
# Reads AgentHookRequest JSON from stdin, passes to Python hook.
#
# Safety: enforces a hard timeout, traps signals to clean up children,
# and exits cleanly on any failure.

set -euo pipefail

HOOK_TIMEOUT=${CODEX_HOOK_TIMEOUT:-150}  # 2.5 min default
PYTHON_HOOK="/root/.aethervault/hooks/codex-model-hook.py"
LOG_TAG="[codex-hook]"

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

# --- Buffer stdin first, then pipe to Python ---
# setsid + background disconnects stdin from the child. Instead, read stdin
# into a variable and pipe it explicitly. This preserves process-group
# isolation for clean kills while keeping the data flowing.
INPUT=$(cat)

echo "$INPUT" | timeout --signal=KILL "$HOOK_TIMEOUT" python3 "$PYTHON_HOOK"
EXIT_CODE=$?

# If killed by signal, emit a valid JSON error response
if [ $EXIT_CODE -gt 128 ]; then
    SIG=$((EXIT_CODE - 128))
    echo "{\"message\":{\"role\":\"assistant\",\"content\":\"(Codex hook killed by signal $SIG)\",\"tool_calls\":[]}}"
fi

exit $EXIT_CODE

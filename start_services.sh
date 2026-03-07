#!/bin/bash
# AetherVault Service Starter
# Starts only the optional provider adapters you explicitly enable.
# (Script filename retained as start_services.sh for backward compatibility.)

# Load secure environment
AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="${AETHERVAULT_ENV:-$AETHERVAULT_HOME/.env}"
if [ ! -f "$ENV_FILE" ] && [ -f "$AETHERVAULT_HOME/env" ]; then
    ENV_FILE="$AETHERVAULT_HOME/env"
fi
if [ -f "$ENV_FILE" ]; then
    source "$ENV_FILE"
fi
export PATH=/opt/google-cloud-sdk/bin:$PATH
LOG_DIR="${AETHERVAULT_LOG_DIR:-$AETHERVAULT_HOME/logs}"
mkdir -p "$LOG_DIR"

# Configurable ports (match proxy defaults)
VERTEX_PORT="${VERTEX_PROXY_PORT:-11436}"
MOONSHOT_PORT="${MOONSHOT_PROXY_PORT:-11437}"
LLAMA_PORT="${LLAMA_PROXY_PORT:-11434}"
LLAMA_SSH_PORT="${LLAMA_SSH_PORT:-2222}"
LLAMA_SSH_USER="${LLAMA_SSH_USER:-user}"
ENABLE_VERTEX_PROXY="${ENABLE_VERTEX_PROXY:-0}"
ENABLE_MOONSHOT_PROXY="${ENABLE_MOONSHOT_PROXY:-0}"
ENABLE_LLAMA_TUNNEL="${ENABLE_LLAMA_TUNNEL:-0}"

start_python_service() {
    local enable_flag="$1"
    local script_name="$2"
    local log_name="$3"
    local display_name="$4"
    local port="$5"
    if [ "$enable_flag" != "1" ]; then
        echo "Skipping $display_name (set corresponding ENABLE_* flag to 1 to start it)"
        return
    fi
    pkill -f "$script_name" 2>/dev/null || true
    sleep 1
    cd "$SCRIPT_DIR" && nohup python3 "$script_name" > "$LOG_DIR/$log_name" 2>&1 &
    echo "Started $display_name on 127.0.0.1:$port"
}

start_python_service "$ENABLE_VERTEX_PROXY" "vertex_proxy.py" "vertex_proxy.log" "Vertex AI proxy" "$VERTEX_PORT"
start_python_service "$ENABLE_MOONSHOT_PROXY" "moonshot_proxy.py" "moonshot_proxy.log" "Moonshot proxy" "$MOONSHOT_PORT"

# Start SSH tunnel to Windows (localhost only) if explicitly enabled
if [ "$ENABLE_LLAMA_TUNNEL" = "1" ] && ss -tlnp 2>/dev/null | grep -q "$LLAMA_SSH_PORT"; then
    ssh -o StrictHostKeyChecking=no -o ServerAliveInterval=60 -f -N \
        -L "127.0.0.1:${LLAMA_PORT}:172.31.64.1:${LLAMA_PORT}" \
        -p "$LLAMA_SSH_PORT" "${LLAMA_SSH_USER}@localhost"
    echo "Started SSH tunnel on 127.0.0.1:$LLAMA_PORT"
elif [ "$ENABLE_LLAMA_TUNNEL" = "1" ]; then
    echo "Skipped llama tunnel: nothing is listening on localhost:$LLAMA_SSH_PORT"
else
    echo "Skipping llama tunnel (set ENABLE_LLAMA_TUNNEL=1 to start it)"
fi

echo "Optional adapter startup complete"

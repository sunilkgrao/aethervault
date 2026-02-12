#!/bin/bash
# AetherVault Service Starter
# Launches proxy services and SSH tunnels for the AetherVault stack.
# (Script filename retained as start_services.sh for backward compatibility.)

# Load secure environment
# NOTE: AETHERVAULT_HOME is a legacy env var; prefer AETHERVAULT_HOME in new deployments.
AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
ENV_FILE="${AETHERVAULT_ENV:-$AETHERVAULT_HOME/env}"
if [ -f "$ENV_FILE" ]; then
    source "$ENV_FILE"
fi
export PATH=/opt/google-cloud-sdk/bin:$PATH

# Configurable ports (match proxy defaults)
VERTEX_PORT="${VERTEX_PROXY_PORT:-11436}"
MOONSHOT_PORT="${MOONSHOT_PROXY_PORT:-11435}"
LLAMA_PORT="${LLAMA_PROXY_PORT:-11434}"
LLAMA_SSH_PORT="${LLAMA_SSH_PORT:-2222}"
LLAMA_SSH_USER="${LLAMA_SSH_USER:-user}"

# Start Vertex AI proxy (localhost only)
pkill -f vertex_proxy.py 2>/dev/null || true
sleep 1
cd "$HOME" && nohup python3 vertex_proxy.py > /var/log/vertex_proxy.log 2>&1 &
echo "Started Vertex AI proxy on 127.0.0.1:$VERTEX_PORT"

# Start moonshot proxy (localhost only)
pkill -f moonshot_proxy.py 2>/dev/null || true
sleep 1
cd "$HOME" && nohup python3 moonshot_proxy.py > /var/log/moonshot_proxy.log 2>&1 &
echo "Started Moonshot proxy on 127.0.0.1:$MOONSHOT_PORT"

# Start SSH tunnel to Windows (localhost only) - if Windows is connected
if ss -tlnp | grep -q "$LLAMA_SSH_PORT"; then
    ssh -o StrictHostKeyChecking=no -o ServerAliveInterval=60 -f -N \
        -L "127.0.0.1:${LLAMA_PORT}:172.31.64.1:${LLAMA_PORT}" \
        -p "$LLAMA_SSH_PORT" "${LLAMA_SSH_USER}@localhost"
    echo "Started SSH tunnel on 127.0.0.1:$LLAMA_PORT"
fi

echo "AetherVault services started"

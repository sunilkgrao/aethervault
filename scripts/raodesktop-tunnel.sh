#!/usr/bin/env bash
# raodesktop-tunnel.sh — Self-healing reverse-SSH tunnel health checker
#
# Run on the droplet (cron every 5 min or systemd timer):
#   */5 * * * * /root/aethervault/scripts/raodesktop-tunnel.sh >> /var/log/raodesktop-tunnel.log 2>&1
#
# Also callable by the agent via `exec` tool to manually trigger reconnection.
#
# How it works:
#   1. Checks if the reverse tunnel on port 2222 is alive
#   2. If down, attempts to reach Windows via Tailscale to request tunnel restart
#   3. Logs all actions for debugging

set -euo pipefail

TUNNEL_PORT=2222
TUNNEL_USER="sunil"
TUNNEL_HOST="localhost"
CONNECT_TIMEOUT=5
LOG_PREFIX="[raodesktop-tunnel]"

# Tailscale IP of the Windows machine (raoDesktop)
TAILSCALE_IP="${RAODESKTOP_TAILSCALE_IP:-100.109.33.54}"
TAILSCALE_SSH_PORT="${RAODESKTOP_TAILSCALE_SSH_PORT:-22}"

log() {
    echo "$(date '+%Y-%m-%d %H:%M:%S') $LOG_PREFIX $*"
}

check_tunnel() {
    ssh -p "$TUNNEL_PORT" \
        -o ConnectTimeout="$CONNECT_TIMEOUT" \
        -o StrictHostKeyChecking=no \
        -o BatchMode=yes \
        "$TUNNEL_USER@$TUNNEL_HOST" echo ok 2>/dev/null
}

request_tunnel_via_tailscale() {
    log "Attempting to reach raoDesktop via Tailscale ($TAILSCALE_IP) to restart tunnel..."

    # Try to reach WSL via Tailscale and run autossh
    ssh -o ConnectTimeout=10 \
        -o StrictHostKeyChecking=no \
        -o BatchMode=yes \
        -p "$TAILSCALE_SSH_PORT" \
        "$TUNNEL_USER@$TAILSCALE_IP" \
        'bash -l -c "nohup autossh -M 0 -f -N -R 2222:localhost:22 root@aethervault.app -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o StrictHostKeyChecking=no 2>/dev/null &"' \
        2>/dev/null
}

# --- Main ---

log "Checking tunnel health on port $TUNNEL_PORT..."

if result=$(check_tunnel); then
    log "Tunnel is UP (got: $result)"
    exit 0
fi

log "Tunnel is DOWN — attempting recovery..."

# Strategy 1: Reach Windows via Tailscale and restart tunnel
if request_tunnel_via_tailscale; then
    log "Tunnel restart requested via Tailscale. Waiting 10s for connection..."
    sleep 10
    if check_tunnel; then
        log "Tunnel recovered successfully!"
        exit 0
    else
        log "Tunnel still down after Tailscale restart attempt."
    fi
else
    log "Could not reach raoDesktop via Tailscale."
fi

log "FAILED: Tunnel could not be recovered automatically."
log "Manual intervention needed: start WSL on raoDesktop and run:"
log "  autossh -M 0 -f -N -R 2222:localhost:22 root@aethervault.app -o ServerAliveInterval=30 -o ServerAliveCountMax=3"
exit 1

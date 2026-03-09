#!/usr/bin/env bash
set -euo pipefail

REMOTE_HOST="${REMOTE_HOST:-root@167.172.140.221}"
REMOTE_SCRIPT="${REMOTE_SCRIPT:-/root/.openclaw/workspace/relationship-intel/assertion_battery.py}"

ssh "$REMOTE_HOST" "python3 $REMOTE_SCRIPT --json"

#!/bin/bash
# =============================================================================
# AetherVault Evening Check-In — Cron Wrapper
# =============================================================================
#
# Loads environment variables and runs the proactive check-in script.
#
# Usage:
#     bash /root/.aethervault/hooks/proactive-checkin.sh
#
# Cron schedule (add via `crontab -e` — do NOT install automatically):
#     # Evening check-in: 8 PM daily
#     0 20 * * * /root/.aethervault/hooks/proactive-checkin.sh
#
# Log output goes to /var/log/aethervault/checkin.log
# =============================================================================

set -euo pipefail

AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
ENV_FILE="${AETHERVAULT_HOME}/.env"
LOG_DIR="${AETHERVAULT_LOG_DIR:-/var/log/aethervault}"
LOG_FILE="$LOG_DIR/checkin.log"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Ensure log directory exists
mkdir -p "$LOG_DIR"

# Load environment variables (skip comments and blank lines)
if [ -f "$ENV_FILE" ]; then
    set -a
    while IFS='=' read -r key value; do
        [[ "$key" =~ ^[[:space:]]*# ]] && continue
        [[ -z "$key" ]] && continue
        value="${value%\"}"
        value="${value#\"}"
        value="${value%\'}"
        value="${value#\'}"
        export "$key=$value"
    done < "$ENV_FILE"
    set +a
else
    echo "[$(date)] ERROR: Environment file not found at $ENV_FILE" >> "$LOG_FILE"
    exit 1
fi

cd "$AETHERVAULT_HOME"

echo "" >> "$LOG_FILE"
echo "======================================" >> "$LOG_FILE"
echo "[$(date)] Starting evening check-in" >> "$LOG_FILE"
echo "======================================" >> "$LOG_FILE"

CHECKIN_SCRIPT="${AETHERVAULT_HOME}/hooks/proactive-checkin.py"
if [ ! -f "$CHECKIN_SCRIPT" ]; then
    CHECKIN_SCRIPT="${SCRIPT_DIR}/proactive-checkin.py"
fi
python3 "$CHECKIN_SCRIPT" >> "$LOG_FILE" 2>&1
EXIT_CODE=$?

if [ $EXIT_CODE -ne 0 ]; then
    echo "[$(date)] Evening check-in exited with code $EXIT_CODE" >> "$LOG_FILE"
fi

exit $EXIT_CODE

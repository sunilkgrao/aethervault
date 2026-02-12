#!/bin/bash
# =============================================================================
# AetherVault Morning Briefing — Cron Wrapper
# =============================================================================
#
# Loads environment variables from the AetherVault .env file and runs the
# morning briefing Python script with proper logging.
#
# Usage:
#     bash /root/.aethervault/hooks/morning-briefing.sh
#
# Cron schedule (add via `crontab -e` — do NOT install automatically):
#     # Morning briefing: 8 AM weekdays, 9 AM weekends (Eastern Time)
#     0 8 * * 1-5 /root/.aethervault/hooks/morning-briefing.sh
#     0 9 * * 0,6 /root/.aethervault/hooks/morning-briefing.sh
#
#     # Evening check-in: 8 PM daily
#     0 20 * * * /root/.aethervault/hooks/proactive-checkin.sh
#
# Log output goes to /var/log/aethervault/briefing.log
# =============================================================================

set -euo pipefail

AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
ENV_FILE="${AETHERVAULT_HOME}/.env"
LOG_DIR="${AETHERVAULT_LOG_DIR:-/var/log/aethervault}"
LOG_FILE="$LOG_DIR/briefing.log"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Ensure log directory exists
mkdir -p "$LOG_DIR"

# Load environment variables (skip comments and blank lines)
if [ -f "$ENV_FILE" ]; then
    set -a
    while IFS='=' read -r key value; do
        # Skip comments and blank lines
        [[ "$key" =~ ^[[:space:]]*# ]] && continue
        [[ -z "$key" ]] && continue
        # Strip surrounding quotes from value
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

# Change to the AetherVault directory
cd "$AETHERVAULT_HOME"

# Run the briefing script with logging
echo "" >> "$LOG_FILE"
echo "======================================" >> "$LOG_FILE"
echo "[$(date)] Starting morning briefing" >> "$LOG_FILE"
echo "======================================" >> "$LOG_FILE"

BRIEFING_SCRIPT="${AETHERVAULT_HOME}/hooks/morning-briefing.py"
if [ ! -f "$BRIEFING_SCRIPT" ]; then
    BRIEFING_SCRIPT="${SCRIPT_DIR}/morning-briefing.py"
fi
python3 "$BRIEFING_SCRIPT" >> "$LOG_FILE" 2>&1
EXIT_CODE=$?

if [ $EXIT_CODE -ne 0 ]; then
    echo "[$(date)] Morning briefing exited with code $EXIT_CODE" >> "$LOG_FILE"
fi

exit $EXIT_CODE

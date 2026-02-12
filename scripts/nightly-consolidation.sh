#!/bin/bash
# =============================================================================
# AetherVault Nightly Consolidation Wrapper
# =============================================================================
#
# Bash wrapper for nightly-consolidation.py. Designed to be called by cron.
#
# Cron entry (runs at 3 AM daily):
#   0 3 * * * /root/.aethervault/hooks/nightly-consolidation.sh
#
# What it does:
#   1. Sources the .env file for API keys
#   2. Runs the Python consolidation script
#   3. Logs all output to /var/log/aethervault/consolidation.log
#   4. Rotates logs if they grow too large
#
# Manual usage:
#   bash scripts/nightly-consolidation.sh              # Run for today
#   bash scripts/nightly-consolidation.sh --dry-run    # Preview only
#   bash scripts/nightly-consolidation.sh --date 2026-02-10  # Specific date
#
# =============================================================================

set -uo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
ENV_FILE="${AETHERVAULT_HOME}/.env"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PYTHON_SCRIPT="${SCRIPT_DIR}/nightly-consolidation.py"
LOG_DIR="/var/log/aethervault"
LOG_FILE="${LOG_DIR}/consolidation.log"
MAX_LOG_SIZE_MB=50
PYTHON="${PYTHON:-python3}"

# ---------------------------------------------------------------------------
# Functions
# ---------------------------------------------------------------------------

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"
}

rotate_log() {
    if [ ! -f "$LOG_FILE" ]; then
        return
    fi
    local size_kb
    size_kb=$(du -k "$LOG_FILE" 2>/dev/null | cut -f1)
    local max_kb=$((MAX_LOG_SIZE_MB * 1024))
    if [ "${size_kb:-0}" -ge "$max_kb" ]; then
        local timestamp
        timestamp=$(date +%Y%m%d-%H%M%S)
        mv "$LOG_FILE" "${LOG_FILE}.${timestamp}"
        # Keep only the 5 most recent rotated logs
        ls -t "${LOG_FILE}".* 2>/dev/null | tail -n +6 | xargs rm -f 2>/dev/null || true
        log "Rotated consolidation log (was ${size_kb}KB)"
    fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

# Ensure log directory exists
mkdir -p "$LOG_DIR"

# Rotate log if needed
rotate_log

# Start logging
exec >> "$LOG_FILE" 2>&1

log "=========================================="
log "Nightly Consolidation Starting"
log "=========================================="

# Source .env for API keys
if [ -f "$ENV_FILE" ]; then
    log "Sourcing ${ENV_FILE}"
    set -a
    # shellcheck disable=SC1090
    source "$ENV_FILE"
    set +a
else
    log "WARNING: ${ENV_FILE} not found. ANTHROPIC_API_KEY must be set in environment."
fi

# Verify API key is available
if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    log "ERROR: ANTHROPIC_API_KEY is not set. Cannot proceed."
    exit 1
fi

# Verify Python script exists
if [ ! -f "$PYTHON_SCRIPT" ]; then
    # Also check the hooks location (where cron expects it)
    ALT_SCRIPT="${AETHERVAULT_HOME}/hooks/nightly-consolidation.py"
    if [ -f "$ALT_SCRIPT" ]; then
        PYTHON_SCRIPT="$ALT_SCRIPT"
    else
        log "ERROR: Python script not found at ${PYTHON_SCRIPT} or ${ALT_SCRIPT}"
        exit 1
    fi
fi

# Verify Python is available
if ! command -v "$PYTHON" &>/dev/null; then
    log "ERROR: ${PYTHON} not found in PATH"
    exit 1
fi

# Run the consolidation
log "Running: ${PYTHON} ${PYTHON_SCRIPT} $*"
"$PYTHON" "$PYTHON_SCRIPT" "$@"
EXIT_CODE=$?

if [ $EXIT_CODE -eq 0 ]; then
    log "Consolidation completed successfully"
else
    log "Consolidation exited with code ${EXIT_CODE}"
fi

log "=========================================="
log "Nightly Consolidation Finished (exit=${EXIT_CODE})"
log "=========================================="

exit $EXIT_CODE

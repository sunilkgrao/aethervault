#!/bin/bash
# =============================================================================
# AetherVault Monitoring Script
# =============================================================================
#
# Runs on the DigitalOcean droplet to continuously monitor the bot service.
# Tracks memory usage, CPU, and watches journalctl for crashes/panics.
#
# Usage:
#     # Copy to droplet and run:
#     scp scripts/aethervault-monitor.sh root@<DROPLET_IP>:/tmp/
#     ssh root@<DROPLET_IP> 'bash /tmp/aethervault-monitor.sh'
#
#     # Or run with custom settings:
#     INTERVAL=5 DURATION=3600 SERVICE=aethervault bash /tmp/aethervault-monitor.sh
#
#     # Run in background:
#     nohup bash /tmp/aethervault-monitor.sh &
#
# Output:
#     /tmp/aethervault-monitor.log       - Main monitoring log (TSV)
#     /tmp/aethervault-crashes.log       - Crash/panic entries
#     /tmp/aethervault-monitor-summary.md - Final summary report
#
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

INTERVAL="${INTERVAL:-10}"                          # Polling interval in seconds
DURATION="${DURATION:-0}"                           # Total duration (0 = run until killed)
SERVICE="${SERVICE:-}"                              # Service name (auto-detect if empty)
LOG_FILE="${LOG_FILE:-/tmp/aethervault-monitor.log}"
CRASH_LOG="${CRASH_LOG:-/tmp/aethervault-crashes.log}"
SUMMARY_FILE="${SUMMARY_FILE:-/tmp/aethervault-monitor-summary.md}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
DIM='\033[2m'
NC='\033[0m'

# ---------------------------------------------------------------------------
# Functions
# ---------------------------------------------------------------------------

log_info()  { echo -e "${GREEN}[INFO]${NC}  $(date '+%H:%M:%S') $1"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC}  $(date '+%H:%M:%S') $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $(date '+%H:%M:%S') $1"; }

detect_service() {
    # Try aethervault first, then aethervault
    for name in aethervault aethervault; do
        if systemctl is-active "$name" &>/dev/null; then
            echo "$name"
            return 0
        fi
    done
    # Check if unit exists even if not active
    for name in aethervault aethervault; do
        if systemctl cat "$name" &>/dev/null 2>&1; then
            echo "$name"
            return 0
        fi
    done
    return 1
}

get_service_pid() {
    systemctl show "$SERVICE" --property=MainPID --value 2>/dev/null || echo "0"
}

get_process_memory_kb() {
    local pid="$1"
    if [ "$pid" -gt 0 ] && [ -f "/proc/$pid/status" ]; then
        grep -i 'VmRSS' "/proc/$pid/status" 2>/dev/null | awk '{print $2}' || echo "0"
    else
        # Fallback: find the main node process
        ps -C node -o rss= --sort=-rss 2>/dev/null | head -1 | tr -d ' ' || echo "0"
    fi
}

get_process_cpu() {
    local pid="$1"
    if [ "$pid" -gt 0 ]; then
        ps -p "$pid" -o %cpu= 2>/dev/null | tr -d ' ' || echo "0.0"
    else
        ps -C node -o %cpu= --sort=-rss 2>/dev/null | head -1 | tr -d ' ' || echo "0.0"
    fi
}

get_system_memory() {
    # Returns: total_mb used_mb available_mb
    free -m 2>/dev/null | awk '/^Mem:/ {print $2, $3, $7}' || echo "0 0 0"
}

get_swap_usage() {
    # Returns: total_mb used_mb
    free -m 2>/dev/null | awk '/^Swap:/ {print $2, $3}' || echo "0 0"
}

get_disk_usage() {
    # Returns usage percentage for root partition
    df -h / 2>/dev/null | awk 'NR==2 {print $5}' | tr -d '%' || echo "0"
}

get_open_fds() {
    local pid="$1"
    if [ "$pid" -gt 0 ] && [ -d "/proc/$pid/fd" ]; then
        ls -1 "/proc/$pid/fd" 2>/dev/null | wc -l || echo "0"
    else
        echo "0"
    fi
}

check_crashes() {
    # Check journalctl for crash indicators since last check
    local since_seconds="${1:-$INTERVAL}"
    local crash_patterns="panic|segfault|SIGSEGV|SIGABRT|fatal error|"
    crash_patterns+="JavaScript heap out of memory|ENOMEM|OOMKiller|"
    crash_patterns+="unhandled exception|Error: connect ECONNREFUSED|"
    crash_patterns+="SIGKILL|oom-kill"

    journalctl -u "$SERVICE" --since "${since_seconds} seconds ago" \
        --no-pager 2>/dev/null \
        | grep -iE "$crash_patterns" 2>/dev/null || true
}

check_restarts() {
    # Count how many times the service restarted in the last hour
    journalctl -u "$SERVICE" --since "1 hour ago" --no-pager 2>/dev/null \
        | grep -c "Started\|Stopped\|Main process exited" 2>/dev/null || echo "0"
}

# ---------------------------------------------------------------------------
# Initialization
# ---------------------------------------------------------------------------

echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  AetherVault Service Monitor${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Auto-detect service if not set
if [ -z "$SERVICE" ]; then
    log_info "Auto-detecting service name..."
    SERVICE=$(detect_service) || true
    if [ -z "$SERVICE" ]; then
        log_error "Could not detect service. Set SERVICE=<name> and retry."
        exit 1
    fi
fi
log_info "Monitoring service: $SERVICE"
log_info "Interval: ${INTERVAL}s"
if [ "$DURATION" -gt 0 ]; then
    log_info "Duration: ${DURATION}s ($(( DURATION / 60 )) min)"
else
    log_info "Duration: indefinite (Ctrl+C to stop)"
fi
log_info "Log file: $LOG_FILE"
log_info "Crash log: $CRASH_LOG"

# Check service is present
if ! systemctl cat "$SERVICE" &>/dev/null 2>&1; then
    log_error "Service '$SERVICE' not found on this system."
    exit 1
fi

# Check service status
SVC_STATUS=$(systemctl is-active "$SERVICE" 2>/dev/null || true)
if [ "$SVC_STATUS" != "active" ]; then
    log_warn "Service is not active (status: $SVC_STATUS). Monitoring anyway."
fi

# Initialize log files
echo "# AetherVault Monitor Log - $(date -u '+%Y-%m-%dT%H:%M:%SZ')" > "$LOG_FILE"
echo "# Service: $SERVICE | Interval: ${INTERVAL}s" >> "$LOG_FILE"
echo -e "timestamp\trss_mb\tcpu_pct\tsys_mem_used_mb\tsys_mem_avail_mb\tswap_used_mb\tdisk_pct\topen_fds\tcrash_count\trestart_count" >> "$LOG_FILE"

: > "$CRASH_LOG"  # Truncate crash log

# Counters
TOTAL_SAMPLES=0
TOTAL_CRASHES=0
MAX_RSS_MB=0
MIN_RSS_MB=999999
START_TIME=$(date +%s)
LAST_CRASH_CHECK=$START_TIME

# Trap for clean exit
cleanup() {
    echo ""
    log_info "Shutting down monitor..."
    generate_summary
    log_info "Summary written to: $SUMMARY_FILE"
    log_info "Total samples: $TOTAL_SAMPLES"
    log_info "Total crash lines: $TOTAL_CRASHES"
    exit 0
}
trap cleanup SIGINT SIGTERM

# ---------------------------------------------------------------------------
# Summary generation
# ---------------------------------------------------------------------------

generate_summary() {
    local end_time=$(date +%s)
    local elapsed=$(( end_time - START_TIME ))
    local elapsed_min=$(( elapsed / 60 ))

    cat > "$SUMMARY_FILE" << HEREDOC
# AetherVault Monitor Summary

**Generated:** $(date -u '+%Y-%m-%dT%H:%M:%SZ')
**Service:** $SERVICE
**Duration:** ${elapsed_min} minutes (${elapsed}s)
**Samples:** $TOTAL_SAMPLES

## Resource Usage

| Metric | Value |
|--------|-------|
| Peak RSS | ${MAX_RSS_MB} MB |
| Min RSS | $([ "$MIN_RSS_MB" -eq 999999 ] && echo "N/A" || echo "${MIN_RSS_MB} MB") |
| Total Crash Lines | $TOTAL_CRASHES |

## Crash Log

$(if [ -s "$CRASH_LOG" ]; then
    echo '```'
    head -50 "$CRASH_LOG"
    echo '```'
else
    echo "No crashes detected."
fi)

## Raw Data

See \`$LOG_FILE\` for per-sample TSV data.

---
*Generated by aethervault-monitor.sh*
HEREDOC
}

# ---------------------------------------------------------------------------
# Main monitoring loop
# ---------------------------------------------------------------------------

log_info "Starting monitoring loop..."
echo ""
printf "${DIM}%-20s %8s %6s %10s %10s %8s %5s %6s %6s %7s${NC}\n" \
    "TIMESTAMP" "RSS_MB" "CPU%" "SYS_USED" "SYS_AVAIL" "SWAP_MB" "DISK%" "FDs" "CRASH" "RESTRT"
echo -e "${DIM}$(printf '%0.s-' {1..100})${NC}"

while true; do
    NOW=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
    NOW_EPOCH=$(date +%s)

    # Check if duration exceeded
    if [ "$DURATION" -gt 0 ]; then
        ELAPSED=$(( NOW_EPOCH - START_TIME ))
        if [ "$ELAPSED" -ge "$DURATION" ]; then
            log_info "Duration reached. Stopping."
            break
        fi
    fi

    # Get service PID
    PID=$(get_service_pid)

    # Collect metrics
    RSS_KB=$(get_process_memory_kb "$PID")
    RSS_MB=$(( RSS_KB / 1024 ))
    CPU_PCT=$(get_process_cpu "$PID")

    read SYS_TOTAL SYS_USED SYS_AVAIL <<< "$(get_system_memory)"
    read SWAP_TOTAL SWAP_USED <<< "$(get_swap_usage)"
    DISK_PCT=$(get_disk_usage)
    OPEN_FDS=$(get_open_fds "$PID")

    # Check for crashes (look at the last interval plus a buffer)
    CRASH_LINES=$(check_crashes "$(( INTERVAL + 5 ))")
    CRASH_COUNT=0
    if [ -n "$CRASH_LINES" ]; then
        CRASH_COUNT=$(echo "$CRASH_LINES" | wc -l)
        TOTAL_CRASHES=$(( TOTAL_CRASHES + CRASH_COUNT ))
        echo "$CRASH_LINES" >> "$CRASH_LOG"
        # Alert
        echo ""
        log_error "CRASH/ERROR DETECTED ($CRASH_COUNT lines):"
        echo "$CRASH_LINES" | head -5 | while read -r line; do
            echo -e "  ${RED}$line${NC}"
        done
        echo ""
    fi

    RESTART_COUNT=$(check_restarts)

    # Update max/min
    if [ "$RSS_MB" -gt "$MAX_RSS_MB" ]; then
        MAX_RSS_MB=$RSS_MB
    fi
    if [ "$RSS_MB" -gt 0 ] && [ "$RSS_MB" -lt "$MIN_RSS_MB" ]; then
        MIN_RSS_MB=$RSS_MB
    fi

    # Write to log
    echo -e "${NOW}\t${RSS_MB}\t${CPU_PCT}\t${SYS_USED}\t${SYS_AVAIL}\t${SWAP_USED}\t${DISK_PCT}\t${OPEN_FDS}\t${CRASH_COUNT}\t${RESTART_COUNT}" >> "$LOG_FILE"

    # Print to terminal
    # Color RSS based on usage
    if [ "$RSS_MB" -gt 1500 ]; then
        RSS_COLOR="$RED"
    elif [ "$RSS_MB" -gt 800 ]; then
        RSS_COLOR="$YELLOW"
    else
        RSS_COLOR="$GREEN"
    fi

    # Color crash count
    if [ "$CRASH_COUNT" -gt 0 ]; then
        CRASH_COLOR="$RED"
    else
        CRASH_COLOR="$GREEN"
    fi

    printf "%-20s ${RSS_COLOR}%7d${NC} %6s %9d %10d %8d %5s %6s ${CRASH_COLOR}%5d${NC} %7s\n" \
        "$(date '+%H:%M:%S')" \
        "$RSS_MB" \
        "$CPU_PCT" \
        "$SYS_USED" \
        "$SYS_AVAIL" \
        "$SWAP_USED" \
        "${DISK_PCT}%" \
        "$OPEN_FDS" \
        "$CRASH_COUNT" \
        "$RESTART_COUNT"

    TOTAL_SAMPLES=$(( TOTAL_SAMPLES + 1 ))

    # Check if service died
    CURRENT_STATUS=$(systemctl is-active "$SERVICE" 2>/dev/null || true)
    if [ "$CURRENT_STATUS" != "active" ] && [ "$SVC_STATUS" = "active" ]; then
        log_error "SERVICE WENT DOWN! Status: $CURRENT_STATUS"
        echo "$(date -u '+%Y-%m-%dT%H:%M:%SZ') SERVICE DOWN: $CURRENT_STATUS" >> "$CRASH_LOG"
    fi
    SVC_STATUS="$CURRENT_STATUS"

    sleep "$INTERVAL"
done

cleanup

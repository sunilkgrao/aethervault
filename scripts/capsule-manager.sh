#!/bin/bash
# AetherVault Capsule Manager — Production-Grade Capsule Orchestration
#
# Usage:
#   capsule-manager.sh status          — Report capsule capacity, frame breakdown, and health
#   capsule-manager.sh check           — Exit 0 if healthy, exit 1 if over capacity (for cron)
#   capsule-manager.sh archive         — Archive current capsule with timestamp
#   capsule-manager.sh rebuild         — Archive + create fresh capsule from migration data
#   capsule-manager.sh rotate-logs     — Show options for agent-log rotation
#   capsule-manager.sh compact         — Compact the working capsule (reclaim dead space)
#   capsule-manager.sh verify          — Full integrity check with doctor
#   capsule-manager.sh watchdog        — Cron-safe: check health, alert on issues, auto-compact
#
# Architecture:
#   /root/.aethervault/memory.mv2           — Working capsule (agent reads/writes)
#   /root/.aethervault/archive/             — Archived capsules (read-only, timestamped)
#   /root/.aethervault/migration-data/      — Original migration source data (if available)
#   /root/.aethervault/logs/                — Capsule manager logs
#
# Capacity tiers (determined by WAL size in binary):
#   Free  (WAL < 4 MiB):  200 MiB
#   Dev   (WAL >= 4 MiB):   2 GiB
#   Enterprise (WAL >= 16 MiB): 10 GiB
#
# Production capsule has 8 MiB WAL → Dev tier → 2 GiB effective limit
# Tier::Free constant is 200 MiB (patched from original 50 MiB)

set -uo pipefail

# All paths derive from AETHERVAULT_HOME (default: ~/.aethervault)
AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
MV2="${CAPSULE_PATH:-$AETHERVAULT_HOME/memory.mv2}"
ARCHIVE_DIR="$AETHERVAULT_HOME/archive"
LOG_DIR="$AETHERVAULT_HOME/logs"
AV="${AETHERVAULT_BIN:-/usr/local/bin/aethervault}"

# Capacity limits — these match the patched binary
TIER_FREE_LIMIT=$((200 * 1024 * 1024))    # 200 MiB
TIER_DEV_LIMIT=$((2 * 1024 * 1024 * 1024)) # 2 GiB
WAL_SIZE_MEDIUM=$((4 * 1024 * 1024))       # 4 MiB threshold for Dev tier

# Thresholds
WARN_PCT=80
CRITICAL_PCT=95
MAX_ARCHIVES=10  # Keep at most this many archive capsules

# Telegram alerting — read chat_id from config (not hardcoded)
ENV_FILE="$AETHERVAULT_HOME/.env"
ALERT_CHAT_ID=""
BRIEFING_CONFIG="$AETHERVAULT_HOME/config/briefing.json"
if [ -f "$BRIEFING_CONFIG" ]; then
    ALERT_CHAT_ID=$(python3 -c "import json; print(json.load(open('$BRIEFING_CONFIG')).get('chat_id',''))" 2>/dev/null || echo "")
fi

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"; }

log_to_file() {
    mkdir -p "$LOG_DIR"
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1" >> "$LOG_DIR/capsule-manager.log"
}

# Determine the effective capacity limit based on WAL size (mirrors Rust tier logic)
get_effective_limit() {
    local capsule="${1:-$MV2}"
    local doctor_out
    doctor_out=$("$AV" doctor --dry-run "$capsule" 2>&1)
    local wal_size
    wal_size=$(echo "$doctor_out" | grep -oP 'wal_size=\K[0-9]+' || echo "0")
    if [ "$wal_size" -ge "$((16 * 1024 * 1024))" ]; then
        echo "$((10 * 1024 * 1024 * 1024))"  # Enterprise: 10 GiB
    elif [ "$wal_size" -ge "$WAL_SIZE_MEDIUM" ]; then
        echo "$TIER_DEV_LIMIT"  # Dev: 2 GiB
    else
        echo "$TIER_FREE_LIMIT"  # Free: 200 MiB
    fi
}

get_data_size() {
    local status_out
    status_out=$("$AV" doctor --dry-run "$1" 2>&1)
    local footer_offset
    footer_offset=$(echo "$status_out" | grep -oP 'footer_offset=\K[0-9]+' || echo "0")
    if [ "$footer_offset" = "0" ]; then
        stat -c%s "$1" 2>/dev/null || stat -f%z "$1" 2>/dev/null
    else
        echo "$footer_offset"
    fi
}

get_wal_size() {
    local doctor_out
    doctor_out=$("$AV" doctor --dry-run "$1" 2>&1)
    echo "$doctor_out" | grep -oP 'wal_size=\K[0-9]+' || echo "0"
}

get_frame_count() {
    "$AV" doctor --dry-run "$1" 2>&1 | grep -oP 'frames=\K[0-9]+' || \
    "$AV" status "$1" 2>&1 | grep "frames:" | awk '{print $2}' || echo "0"
}

get_collection_count() {
    local capsule="$1" collection="$2"
    local result
    result=$("$AV" search "$capsule" "the" --collection "$collection" --limit 9999 2>/dev/null | grep -c "aethervault://$collection" 2>/dev/null) || result=0
    echo "${result:-0}"
}

send_telegram_alert() {
    local message="$1"
    if [ -z "$ALERT_CHAT_ID" ]; then return; fi
    if [ ! -f "$ENV_FILE" ]; then return; fi
    local bot_token
    bot_token=$(grep TELEGRAM_BOT_TOKEN "$ENV_FILE" | cut -d= -f2)
    if [ -z "$bot_token" ]; then return; fi
    curl -s -X POST "https://api.telegram.org/bot${bot_token}/sendMessage" \
        -d "chat_id=${ALERT_CHAT_ID}" \
        -d "text=${message}" \
        -d "parse_mode=Markdown" >/dev/null 2>&1 || true
}

format_bytes() {
    local bytes="$1"
    if [ "$bytes" -ge $((1024 * 1024 * 1024)) ]; then
        echo "$((bytes / 1024 / 1024 / 1024)) GiB"
    elif [ "$bytes" -ge $((1024 * 1024)) ]; then
        echo "$((bytes / 1024 / 1024)) MiB"
    elif [ "$bytes" -ge 1024 ]; then
        echo "$((bytes / 1024)) KiB"
    else
        echo "$bytes B"
    fi
}

cmd_status() {
    if [ ! -f "$MV2" ]; then
        log "ERROR: Capsule not found at $MV2"
        exit 1
    fi

    local file_size data_size frames wal_size capacity_limit
    file_size=$(stat -c%s "$MV2" 2>/dev/null || stat -f%z "$MV2" 2>/dev/null)
    data_size=$(get_data_size "$MV2")
    frames=$(get_frame_count "$MV2")
    wal_size=$(get_wal_size "$MV2")
    capacity_limit=$(get_effective_limit "$MV2")

    # Determine tier name
    local tier_name="Free"
    if [ "$wal_size" -ge $((16 * 1024 * 1024)) ]; then
        tier_name="Enterprise"
    elif [ "$wal_size" -ge "$WAL_SIZE_MEDIUM" ]; then
        tier_name="Dev"
    fi

    echo "=============================================="
    echo "  AetherVault Capsule Status"
    echo "  $(date -u)"
    echo "=============================================="
    echo ""
    echo "Capsule:        $MV2"
    echo "File size:      $(format_bytes "$file_size") ($file_size bytes)"
    echo "Data size:      $(format_bytes "$data_size") ($data_size bytes)"
    echo "WAL size:       $(format_bytes "$wal_size")"
    echo "Tier:           $tier_name"
    echo "Capacity limit: $(format_bytes "$capacity_limit") ($capacity_limit bytes)"
    echo "Frames:         $frames"
    echo ""

    # Capacity gauge
    local pct=$((data_size * 100 / capacity_limit))
    local warn_threshold=$((capacity_limit * WARN_PCT / 100))
    local crit_threshold=$((capacity_limit * CRITICAL_PCT / 100))

    if [ "$data_size" -gt "$capacity_limit" ]; then
        local over=$((data_size - capacity_limit))
        echo "CAPACITY: EXCEEDED by $(format_bytes "$over")"
        echo "  Agent write-access: BLOCKED"
        echo "  Bridge logging:    BROKEN (Telegram turns not persisted)"
        echo "  Read-only access:  OK (search/query still work)"
    elif [ "$data_size" -gt "$crit_threshold" ]; then
        local remaining=$((capacity_limit - data_size))
        echo "CAPACITY: CRITICAL — ${pct}% used, $(format_bytes "$remaining") remaining"
        echo "  Agent write-access: OK (will be blocked soon)"
        echo "  Action needed:     Archive or compact to free space"
    elif [ "$data_size" -gt "$warn_threshold" ]; then
        local remaining=$((capacity_limit - data_size))
        echo "CAPACITY: WARNING — ${pct}% used, $(format_bytes "$remaining") remaining"
        echo "  Agent write-access: OK (approaching limit)"
    else
        local remaining=$((capacity_limit - data_size))
        echo "CAPACITY: HEALTHY — ${pct}% used, $(format_bytes "$remaining") remaining"
        echo "  Agent write-access: OK"
    fi
    echo ""

    # Frame breakdown by collection
    echo "Frame breakdown by collection:"
    local total_counted=0
    for coll in people roam-notes aethervault-memory agent-log; do
        local count
        count=$(get_collection_count "$MV2" "$coll")
        echo "  $coll: $count"
        total_counted=$((total_counted + count))
    done
    echo "  other/config: $((frames - total_counted))"
    echo ""

    # Archives
    if [ -d "$ARCHIVE_DIR" ]; then
        echo "Archives:"
        ls -lh "$ARCHIVE_DIR"/*.mv2 2>/dev/null | awk '{print "  " $NF " (" $5 ")"}'
        local archive_count
        archive_count=$(ls "$ARCHIVE_DIR"/*.mv2 2>/dev/null | wc -l)
        echo "  Total archives: $archive_count"
    else
        echo "Archives: none (no archive directory)"
    fi
    echo ""

    # Bridge status
    echo "Bridge status:"
    if systemctl is-active --quiet aethervault 2>/dev/null; then
        echo "  aethervault.service: ACTIVE"
        local uptime
        uptime=$(systemctl show aethervault --property=ActiveEnterTimestamp 2>/dev/null | cut -d= -f2)
        echo "  Started: $uptime"
    else
        echo "  aethervault.service: INACTIVE"
    fi
    echo ""

    # Disk space
    echo "Disk space ($AETHERVAULT_HOME/):"
    du -sh "$AETHERVAULT_HOME/" 2>/dev/null | awk '{print "  Total: " $1}'
    du -sh "$AETHERVAULT_HOME/archive/" 2>/dev/null | awk '{print "  Archives: " $1}'
    df -h / | tail -1 | awk '{print "  Volume: " $4 " free of " $2}'
}

cmd_check() {
    if [ ! -f "$MV2" ]; then
        echo "MISSING: Capsule not found at $MV2"
        exit 2
    fi

    local data_size capacity_limit
    data_size=$(get_data_size "$MV2")
    capacity_limit=$(get_effective_limit "$MV2")
    local pct=$((data_size * 100 / capacity_limit))
    local warn_threshold=$((capacity_limit * WARN_PCT / 100))
    local crit_threshold=$((capacity_limit * CRITICAL_PCT / 100))

    if [ "$data_size" -gt "$capacity_limit" ]; then
        echo "OVER_CAPACITY: $(format_bytes "$data_size") / $(format_bytes "$capacity_limit") (${pct}%)"
        exit 1
    elif [ "$data_size" -gt "$crit_threshold" ]; then
        echo "CRITICAL: $(format_bytes "$data_size") / $(format_bytes "$capacity_limit") (${pct}%)"
        exit 1
    elif [ "$data_size" -gt "$warn_threshold" ]; then
        echo "WARNING: $(format_bytes "$data_size") / $(format_bytes "$capacity_limit") (${pct}%)"
        exit 0
    else
        echo "HEALTHY: $(format_bytes "$data_size") / $(format_bytes "$capacity_limit") (${pct}%)"
        exit 0
    fi
}

cmd_verify() {
    log "Running full integrity check on $MV2"
    if [ ! -f "$MV2" ]; then
        log "ERROR: Capsule not found"
        exit 1
    fi

    local result
    result=$("$AV" doctor --dry-run "$MV2" 2>&1)
    echo "$result"

    if echo "$result" | grep -q "Clean"; then
        log "Integrity: PASS"
        exit 0
    else
        log "Integrity: ISSUES FOUND"
        log_to_file "INTEGRITY_ISSUE: $(echo "$result" | grep -E 'finding|error' | head -5)"
        exit 1
    fi
}

cmd_compact() {
    log "Compacting working capsule"

    # Check bridge status
    local bridge_was_active=false
    if systemctl is-active --quiet aethervault 2>/dev/null; then
        bridge_was_active=true
        log "Stopping bridge for exclusive access..."
        systemctl stop aethervault
        sleep 2
    fi

    local before_size
    before_size=$(stat -c%s "$MV2" 2>/dev/null || stat -f%z "$MV2" 2>/dev/null)

    log "Before: $(format_bytes "$before_size")"
    "$AV" compact "$MV2" 2>&1

    local after_size
    after_size=$(stat -c%s "$MV2" 2>/dev/null || stat -f%z "$MV2" 2>/dev/null)
    local saved=$((before_size - after_size))

    log "After:  $(format_bytes "$after_size")"
    log "Saved:  $(format_bytes "$saved")"

    # Verify integrity after compaction
    local verify
    verify=$("$AV" doctor --dry-run "$MV2" 2>&1)
    if echo "$verify" | grep -q "Clean"; then
        log "Post-compact integrity: OK"
    else
        log "WARNING: Post-compact integrity issues"
        echo "$verify" | head -5
    fi

    if $bridge_was_active; then
        log "Restarting bridge..."
        systemctl start aethervault
    fi

    log_to_file "COMPACT: $(format_bytes "$before_size") -> $(format_bytes "$after_size") (saved $(format_bytes "$saved"))"
}

cmd_archive() {
    log "Archiving current capsule"

    # Stop the bridge for exclusive access
    local bridge_was_active=false
    if systemctl is-active --quiet aethervault 2>/dev/null; then
        bridge_was_active=true
        systemctl stop aethervault 2>/dev/null || true
        sleep 2
    fi

    mkdir -p "$ARCHIVE_DIR"

    local timestamp
    timestamp=$(date +%Y%m%d-%H%M%S)
    local archive_path="$ARCHIVE_DIR/memory-${timestamp}.mv2"

    log "Copying $MV2 → $archive_path"
    cp "$MV2" "$archive_path"
    local archive_size
    archive_size=$(stat -c%s "$archive_path" 2>/dev/null || stat -f%z "$archive_path" 2>/dev/null)
    log "Archived: $archive_path ($(format_bytes "$archive_size"))"

    # Compact the archive for storage efficiency
    log "Compacting archive..."
    "$AV" compact "$archive_path" 2>&1 | grep -E "status|duration" || true

    # Verify archive integrity
    local verify
    verify=$("$AV" doctor --dry-run "$archive_path" 2>&1)
    if echo "$verify" | grep -q "Clean"; then
        log "Archive integrity: OK"
    else
        log "WARNING: Archive may have issues"
        echo "$verify" | head -5
    fi

    # Prune old archives if we have too many
    cmd_prune_archives

    echo ""
    echo "Archive complete: $archive_path"
    echo "To create a fresh working capsule, run: $0 rebuild"
    echo ""

    log_to_file "ARCHIVE: $archive_path ($(format_bytes "$archive_size"))"

    # Restart bridge
    if $bridge_was_active; then
        systemctl start aethervault 2>/dev/null || true
    fi
}

cmd_prune_archives() {
    if [ ! -d "$ARCHIVE_DIR" ]; then return; fi

    local archive_count
    archive_count=$(ls "$ARCHIVE_DIR"/memory-*.mv2 2>/dev/null | wc -l)

    if [ "$archive_count" -gt "$MAX_ARCHIVES" ]; then
        local to_remove=$((archive_count - MAX_ARCHIVES))
        log "Pruning $to_remove old archive(s) (keeping $MAX_ARCHIVES most recent)"
        ls -t "$ARCHIVE_DIR"/memory-*.mv2 | tail -"$to_remove" | while read -r old; do
            log "  Removing: $old"
            rm -f "$old"
            log_to_file "PRUNE: removed $old"
        done
    fi
}

cmd_rebuild() {
    log "Rebuilding working capsule via merge (preserves data, resets WAL for Dev tier)"

    # Stop bridge for exclusive access
    local bridge_was_active=false
    if systemctl is-active --quiet aethervault 2>/dev/null; then
        bridge_was_active=true
        systemctl stop aethervault 2>/dev/null || true
        sleep 2
    fi

    # Archive current capsule first
    mkdir -p "$ARCHIVE_DIR"
    local timestamp
    timestamp=$(date +%Y%m%d-%H%M%S)
    local archive_path="$ARCHIVE_DIR/memory-${timestamp}.mv2"
    cp "$MV2" "$archive_path"
    log "Archived current capsule to $archive_path"

    # Save config before merge (the full agent config with system prompt, subagents, etc.)
    local config_tmp="/tmp/rebuild-config-$$.json"
    if "$AV" config "$MV2" get --key index --raw > "$config_tmp" 2>/dev/null && [ -s "$config_tmp" ]; then
        log "Config saved ($(wc -c < "$config_tmp") bytes)"
    else
        log "WARNING: Could not extract config from current capsule"
        echo "{}" > "$config_tmp"
    fi

    # Merge into fresh capsule — deduplicates, drops dead/corrupt frames, resets WAL.
    # Corrupt frames are skipped automatically (logged to stderr).
    local tmp_empty="/tmp/rebuild-empty-$$.mv2"
    local tmp_rebuilt="/tmp/rebuild-merged-$$.mv2"
    rm -f "$tmp_empty" "$tmp_rebuilt" 2>/dev/null
    "$AV" init "$tmp_empty" 2>/dev/null

    log "Merging capsule (dedup + fresh WAL)..."
    local merge_stderr="/tmp/rebuild-merge-stderr-$$.log"
    if ! "$AV" merge "$MV2" "$tmp_empty" "$tmp_rebuilt" --json 2>"$merge_stderr"; then
        local err_msg
        err_msg=$(cat "$merge_stderr" 2>/dev/null)
        log "ERROR: Merge failed: $err_msg"
        rm -f "$tmp_empty" "$tmp_rebuilt" "$config_tmp" "$merge_stderr"
        if $bridge_was_active; then systemctl start aethervault 2>/dev/null || true; fi
        return 1
    fi

    # Log skipped corrupt frames if any
    local skipped_count
    skipped_count=$(grep -c "skipping corrupt frame" "$merge_stderr" 2>/dev/null || echo "0")
    if [ "$skipped_count" -gt 0 ]; then
        log "Skipped $skipped_count corrupt frames during merge"
    fi
    rm -f "$merge_stderr"

    # Restore config to new capsule
    if [ -s "$config_tmp" ] && [ "$(cat "$config_tmp")" != "{}" ]; then
        "$AV" config "$tmp_rebuilt" set --key index --file "$config_tmp" 2>/dev/null
        log "Config restored to rebuilt capsule"
    else
        log "WARNING: No config to restore — new capsule has no agent config"
    fi

    # Verify new capsule
    local verify
    verify=$("$AV" doctor --dry-run "$tmp_rebuilt" 2>&1)
    if ! echo "$verify" | grep -qE "Clean|findings=0"; then
        log "WARNING: Rebuilt capsule has integrity issues, aborting swap"
        rm -f "$tmp_empty" "$tmp_rebuilt" "$config_tmp"
        if $bridge_was_active; then systemctl start aethervault 2>/dev/null || true; fi
        return 1
    fi

    # Report size reduction
    local old_size new_size
    old_size=$(stat -c%s "$MV2" 2>/dev/null || stat -f%z "$MV2" 2>/dev/null)
    new_size=$(stat -c%s "$tmp_rebuilt" 2>/dev/null || stat -f%z "$tmp_rebuilt" 2>/dev/null)
    log "Size: $(format_bytes "$old_size") -> $(format_bytes "$new_size")"

    # Check new WAL/tier — ABORT if Free tier (200 MiB limit is too small)
    local new_wal tier_name
    new_wal=$("$AV" doctor --dry-run "$tmp_rebuilt" 2>&1 | grep -oP 'wal_size=\K[0-9]+' || echo "0")
    if [ "$new_wal" -ge "$WAL_SIZE_MEDIUM" ]; then
        tier_name="Dev"
        log "Tier: Dev (WAL $(format_bytes "$new_wal") >= 4 MiB) — 2 GiB capacity"
    else
        tier_name="Free"
        log "ABORT: Rebuilt capsule has WAL $(format_bytes "$new_wal") — Free tier (200 MiB). Too small."
        log "Restoring original capsule."
        rm -f "$tmp_rebuilt" "$tmp_empty" "$config_tmp"
        if $bridge_was_active; then
            systemctl reset-failed aethervault 2>/dev/null
            systemctl start aethervault 2>/dev/null || true
        fi
        send_telegram_alert "REBUILD ABORTED: New capsule is Free tier (200 MiB). Original capsule preserved."
        return 1
    fi

    # Swap in rebuilt capsule (with rollback on failure)
    mv "$MV2" "${MV2}.pre-rebuild"
    if ! mv "$tmp_rebuilt" "$MV2"; then
        log "ERROR: Failed to swap rebuilt capsule into place. Restoring original."
        mv "${MV2}.pre-rebuild" "$MV2"
        rm -f "$tmp_empty" "$config_tmp"
        if $bridge_was_active; then
            systemctl reset-failed aethervault 2>/dev/null
            systemctl start aethervault 2>/dev/null || true
        fi
        return 1
    fi
    rm -f "${MV2}.pre-rebuild" "$tmp_empty" "$config_tmp"

    # Prune old archives
    cmd_prune_archives

    # Restart bridge (with retry)
    if $bridge_was_active; then
        systemctl reset-failed aethervault 2>/dev/null
        if ! systemctl start aethervault 2>/dev/null; then
            log "WARNING: Bridge failed to start after rebuild"
            send_telegram_alert "REBUILD WARNING: Bridge failed to restart. Manual intervention needed."
        fi
    fi

    log_to_file "REBUILD: $(format_bytes "$old_size") -> $(format_bytes "$new_size"), WAL=$new_wal, tier=$tier_name"
    send_telegram_alert "CAPSULE REBUILT: $(format_bytes "$old_size") -> $(format_bytes "$new_size"). $tier_name tier."
    log "Rebuild complete"
    cmd_status
}

cmd_restore() {
    local archive_name="${1:-}"
    if [ -z "$archive_name" ]; then
        echo "Usage: $0 restore <archive-filename>"
        echo ""
        echo "Available archives:"
        ls -lh "$ARCHIVE_DIR"/memory-*.mv2 2>/dev/null | awk '{print "  " $NF " (" $5 ")"}'
        exit 1
    fi

    local archive_path="$ARCHIVE_DIR/$archive_name"
    if [ ! -f "$archive_path" ]; then
        # Try with .mv2 extension
        archive_path="$ARCHIVE_DIR/${archive_name}.mv2"
    fi
    if [ ! -f "$archive_path" ]; then
        log "ERROR: Archive not found: $archive_name"
        exit 1
    fi

    # Verify archive integrity before restoring
    log "Verifying archive integrity..."
    local verify
    verify=$("$AV" doctor --dry-run "$archive_path" 2>&1)
    if ! echo "$verify" | grep -q "Clean"; then
        log "ERROR: Archive has integrity issues, aborting restore"
        echo "$verify" | head -5
        exit 1
    fi
    log "Archive integrity: OK"

    # Stop bridge
    systemctl stop aethervault 2>/dev/null || true
    sleep 2

    # Backup current capsule
    local timestamp
    timestamp=$(date +%Y%m%d-%H%M%S)
    if [ -f "$MV2" ]; then
        log "Backing up current capsule..."
        cp "$MV2" "${MV2}.pre-restore-${timestamp}"
    fi

    # Restore
    log "Restoring from $archive_path..."
    cp "$archive_path" "$MV2"
    log "Restored."

    # Restart bridge
    systemctl start aethervault 2>/dev/null || true

    log_to_file "RESTORE: from $archive_path"
    cmd_status
}

cmd_rotate_logs() {
    # Rotate the capsule: archive current + merge rebuild (drops bloat, keeps data)
    # This is the primary mechanism for preventing capacity overflow.
    # The merge operation deduplicates frames and creates a fresh WAL,
    # which typically reduces size by 50-90% (agent logs are the main bloat).

    local data_size capacity_limit pct
    data_size=$(get_data_size "$MV2")
    capacity_limit=$(get_effective_limit "$MV2")
    pct=$((data_size * 100 / capacity_limit))

    log "Capsule rotation: $(format_bytes "$data_size") / $(format_bytes "$capacity_limit") (${pct}%)"
    log "Merge will deduplicate frames and create fresh WAL (Dev tier)"

    # Delegate to rebuild which does archive + merge + config restore + restart
    cmd_rebuild
}

cmd_watchdog() {
    # Cron-safe watchdog: check health, log, alert, auto-compact if needed
    mkdir -p "$LOG_DIR"

    if [ ! -f "$MV2" ]; then
        log_to_file "WATCHDOG: CRITICAL — capsule missing at $MV2"
        send_telegram_alert "CAPSULE MISSING: $MV2 not found on aethervault server"
        exit 2
    fi

    local data_size capacity_limit pct
    data_size=$(get_data_size "$MV2")
    capacity_limit=$(get_effective_limit "$MV2")
    pct=$((data_size * 100 / capacity_limit))

    local bridge_status="inactive"
    if systemctl is-active --quiet aethervault 2>/dev/null; then
        bridge_status="active"
    fi

    # Log every check
    log_to_file "WATCHDOG: ${pct}% ($(format_bytes "$data_size") / $(format_bytes "$capacity_limit")) bridge=$bridge_status"

    # Check for capacity issues
    if [ "$data_size" -gt "$capacity_limit" ]; then
        log_to_file "WATCHDOG: ALERT — OVER CAPACITY — triggering emergency rebuild"
        send_telegram_alert "CAPSULE OVER CAPACITY: ${pct}%. Triggering emergency archive+rebuild..."
        # Fall through to critical handler which does archive+rebuild
    fi

    local crit_threshold=$((capacity_limit * CRITICAL_PCT / 100))
    if [ "$data_size" -gt "$crit_threshold" ]; then
        log_to_file "WATCHDOG: ALERT — CRITICAL capacity (${pct}%)"
        send_telegram_alert "CAPSULE CRITICAL: ${pct}% capacity used. Auto-archiving and rebuilding..."
        # Auto-archive + merge rebuild at critical threshold
        # Step 1: Stop bridge for exclusive access
        local bridge_was_active=false
        if systemctl is-active --quiet aethervault 2>/dev/null; then
            bridge_was_active=true
            systemctl stop aethervault
            sleep 2
        fi
        # Step 2: Archive current capsule
        mkdir -p "$ARCHIVE_DIR"
        local ts=$(date +%Y%m%d-%H%M%S)
        cp "$MV2" "${ARCHIVE_DIR}/memory-${ts}.mv2"
        log_to_file "WATCHDOG: Archived to memory-${ts}.mv2"
        # Step 3: Save config before merge
        local config_tmp="/tmp/watchdog-config-$$.json"
        "$AV" config "$MV2" get --key index --raw > "$config_tmp" 2>/dev/null || echo "{}" > "$config_tmp"
        # Step 4: Merge into fresh capsule (drops dead/corrupt frames, resets WAL)
        local tmp_empty="/tmp/watchdog-empty-$$.mv2"
        local tmp_rebuilt="/tmp/watchdog-rebuilt-$$.mv2"
        "$AV" init "$tmp_empty" 2>/dev/null
        if "$AV" merge "$MV2" "$tmp_empty" "$tmp_rebuilt" 2>/dev/null; then
            # Step 5: Restore config
            if [ -s "$config_tmp" ] && [ "$(cat "$config_tmp")" != "{}" ]; then
                "$AV" config "$tmp_rebuilt" set --key index --file "$config_tmp" 2>/dev/null
            fi
            # Step 6: Swap in rebuilt capsule
            mv "$MV2" "${MV2}.pre-rebuild"
            mv "$tmp_rebuilt" "$MV2"
            rm -f "${MV2}.pre-rebuild" "$config_tmp"
            local new_size=$(stat -c%s "$MV2" 2>/dev/null || stat -f%z "$MV2" 2>/dev/null)
            log_to_file "WATCHDOG: Rebuilt capsule: $(format_bytes "$data_size") -> $(format_bytes "$new_size")"
            send_telegram_alert "CAPSULE REBUILT: $(format_bytes "$data_size") -> $(format_bytes "$new_size"). Archive saved."
        else
            log_to_file "WATCHDOG: Merge failed, falling back to compact"
            rm -f "$config_tmp"
            cmd_compact
        fi
        rm -f "$tmp_empty" "$tmp_rebuilt"
        # Step 6: Restart bridge
        if [ "$bridge_was_active" = true ]; then
            systemctl start aethervault
        fi
    fi

    local warn_threshold=$((capacity_limit * WARN_PCT / 100))
    if [ "$data_size" -gt "$warn_threshold" ]; then
        log_to_file "WATCHDOG: WARNING — ${pct}% capacity used"
    fi

    # Check bridge health
    if [ "$bridge_status" = "inactive" ]; then
        log_to_file "WATCHDOG: WARNING — bridge is not running"
        # Auto-restart bridge if it should be running
        if systemctl is-enabled --quiet aethervault 2>/dev/null; then
            log_to_file "WATCHDOG: Auto-restarting bridge..."
            systemctl start aethervault
            send_telegram_alert "BRIDGE RESTART: aethervault.service was down, auto-restarted"
        fi
    fi

    # Check disk space
    local disk_pct
    disk_pct=$(df / | awk 'NR==2 {print $5}' | tr -d '%')
    if [ "$disk_pct" -ge 90 ]; then
        log_to_file "WATCHDOG: ALERT — disk at ${disk_pct}%"
        send_telegram_alert "DISK CRITICAL: ${disk_pct}% full on aethervault server. Free space needed."
    elif [ "$disk_pct" -ge 85 ]; then
        log_to_file "WATCHDOG: WARNING — disk at ${disk_pct}%"
    fi

    # Check proxy processes
    if ! pgrep -f "vertex_proxy.py" >/dev/null 2>&1; then
        log_to_file "WATCHDOG: WARNING — vertex_proxy.py not running"
    fi
    if ! pgrep -f "moonshot_proxy.py" >/dev/null 2>&1; then
        log_to_file "WATCHDOG: WARNING — moonshot_proxy.py not running"
    fi

    # Check integrity weekly (every Sunday at the cron interval)
    if [ "$(date +%u)" = "7" ] && [ "$(date +%H)" = "04" ]; then
        local verify
        verify=$("$AV" doctor --dry-run "$MV2" 2>&1)
        if ! echo "$verify" | grep -q "Clean"; then
            log_to_file "WATCHDOG: INTEGRITY ISSUE detected"
            send_telegram_alert "CAPSULE INTEGRITY: doctor found issues. Run 'capsule-manager.sh verify' for details."
        else
            log_to_file "WATCHDOG: Weekly integrity check PASSED"
        fi
    fi

    # Prune old archives
    cmd_prune_archives

    echo "OK: ${pct}% disk=${disk_pct}% bridge=$bridge_status"
}

# --- Main ---
case "${1:-status}" in
    status)       cmd_status ;;
    check)        cmd_check ;;
    verify)       cmd_verify ;;
    compact)      cmd_compact ;;
    archive)      cmd_archive ;;
    rebuild)      cmd_rebuild ;;
    restore)      cmd_restore "${2:-}" ;;
    rotate-logs|rotate) cmd_rotate_logs ;;
    watchdog)     cmd_watchdog ;;
    prune)        cmd_prune_archives ;;
    *)
        echo "Usage: $0 {status|check|verify|compact|archive|rebuild|restore|rotate-logs|watchdog}"
        echo ""
        echo "  status       Full capsule status report with capacity, frames, bridge"
        echo "  check        Exit 0 if healthy, exit 1 if over/critical capacity (for cron)"
        echo "  verify       Full integrity check with aethervault doctor"
        echo "  compact      Compact working capsule to reclaim dead space"
        echo "  archive      Archive current capsule with timestamp"
        echo "  rebuild      Archive + create fresh capsule from migration data"
        echo "  restore      Restore capsule from archive"
        echo "  rotate-logs  Show options for agent-log rotation"
        echo "  watchdog     Cron-safe health check with auto-healing and alerts"
        echo "  prune        Remove old archives (keep $MAX_ARCHIVES most recent)"
        exit 1
        ;;
esac

#!/usr/bin/env bash
# Blue-Green Binary Upgrade for AetherVault
# Compiles to the inactive slot, smoke tests, swaps symlink, restarts.
# Usage: upgrade.sh [--branch BRANCH] [--skip-tests]
set -euo pipefail

DEPLOY_DIR="/opt/aethervault"
ACTIVE_FILE="${DEPLOY_DIR}/active"
SYMLINK="/usr/local/bin/aethervault"
REPO_DIR="/root/aethervault"
LOG_FILE="${DEPLOY_DIR}/upgrade.log"
HEALTH_CHECK_SECONDS=30
HEALTH_CHECK_INTERVAL=5

BRANCH="main"
SKIP_TESTS=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --branch) BRANCH="$2"; shift 2 ;;
        --skip-tests) SKIP_TESTS=true; shift ;;
        *) echo "Unknown arg: $1" >&2; exit 1 ;;
    esac
done

log() {
    local msg="[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] $*"
    echo "$msg" | tee -a "$LOG_FILE"
}

die() {
    log "FATAL: $*"
    exit 1
}

# Ensure directory structure exists
mkdir -p "${DEPLOY_DIR}/blue" "${DEPLOY_DIR}/green"

# Determine active/inactive slots
if [[ -f "$ACTIVE_FILE" ]]; then
    ACTIVE=$(cat "$ACTIVE_FILE")
else
    # First run: initialize from current binary
    ACTIVE="blue"
    if [[ -f "$SYMLINK" ]] && [[ ! -L "$SYMLINK" ]]; then
        # Current binary is a regular file, copy to blue slot
        cp "$SYMLINK" "${DEPLOY_DIR}/blue/aethervault"
    elif [[ -L "$SYMLINK" ]]; then
        ACTIVE=$(basename "$(dirname "$(readlink -f "$SYMLINK")")")
    fi
    echo "$ACTIVE" > "$ACTIVE_FILE"
    log "Initialized active slot: $ACTIVE"
fi

if [[ "$ACTIVE" == "blue" ]]; then
    INACTIVE="green"
else
    INACTIVE="blue"
fi

log "=== Upgrade started: branch=$BRANCH, active=$ACTIVE, target=$INACTIVE ==="

# Step 1: Pull latest code
log "Pulling branch $BRANCH..."
cd "$REPO_DIR"
git fetch origin "$BRANCH" || die "git fetch failed"
git checkout "$BRANCH" || die "git checkout failed"
git reset --hard "origin/$BRANCH" || die "git reset failed"
log "Git pull complete: $(git rev-parse --short HEAD)"

# Step 2: Compile to inactive slot
log "Building release binary..."
cargo build --release 2>&1 | tail -20 | tee -a "$LOG_FILE"
if [[ ${PIPESTATUS[0]} -ne 0 ]]; then
    die "cargo build --release failed"
fi

BUILT_BINARY="${REPO_DIR}/target/release/aethervault"
if [[ ! -f "$BUILT_BINARY" ]]; then
    die "Binary not found at $BUILT_BINARY"
fi

cp "$BUILT_BINARY" "${DEPLOY_DIR}/${INACTIVE}/aethervault"
chmod +x "${DEPLOY_DIR}/${INACTIVE}/aethervault"
log "Binary copied to ${INACTIVE} slot"

# Step 3: Smoke test
if [[ "$SKIP_TESTS" == "false" ]]; then
    log "Running smoke test..."
    if timeout 5 "${DEPLOY_DIR}/${INACTIVE}/aethervault" --version > /dev/null 2>&1; then
        log "Smoke test passed"
    else
        die "Smoke test failed: binary did not respond to --version within 5s"
    fi
else
    log "Smoke test skipped (--skip-tests)"
fi

# Step 4: Backup and swap symlink atomically
log "Swapping symlink: $SYMLINK -> ${DEPLOY_DIR}/${INACTIVE}/aethervault"
ln -sfn "${DEPLOY_DIR}/${INACTIVE}/aethervault" "${SYMLINK}.tmp"
mv -f "${SYMLINK}.tmp" "$SYMLINK"
echo "$INACTIVE" > "$ACTIVE_FILE"
log "Symlink swapped. Active slot is now: $INACTIVE"

# Step 5: Restart the service
log "Restarting aethervault service..."
systemctl restart aethervault || die "systemctl restart failed"

# Step 6: Health check (30 seconds)
log "Monitoring for ${HEALTH_CHECK_SECONDS}s..."
ELAPSED=0
while [[ $ELAPSED -lt $HEALTH_CHECK_SECONDS ]]; do
    sleep "$HEALTH_CHECK_INTERVAL"
    ELAPSED=$((ELAPSED + HEALTH_CHECK_INTERVAL))
    if ! systemctl is-active --quiet aethervault; then
        log "Service crashed after ${ELAPSED}s! Rolling back..."
        # Rollback: swap back to previous slot
        ln -sfn "${DEPLOY_DIR}/${ACTIVE}/aethervault" "${SYMLINK}.tmp"
        mv -f "${SYMLINK}.tmp" "$SYMLINK"
        echo "$ACTIVE" > "$ACTIVE_FILE"
        systemctl restart aethervault
        die "Rolled back to $ACTIVE slot after crash"
    fi
    log "Health check ${ELAPSED}/${HEALTH_CHECK_SECONDS}s: OK"
done

log "=== Upgrade complete: $(${SYMLINK} --version 2>/dev/null || echo 'version unknown') ==="

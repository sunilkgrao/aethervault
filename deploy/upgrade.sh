#!/usr/bin/env bash
# Blue-Green Binary Upgrade for AetherVault
#
# MANDATORY PIPELINE (no exceptions):
#   1. All changes MUST be committed and pushed to GitHub FIRST
#   2. This script pulls from the remote repo (source of truth)
#   3. Refuses to build if local repo has uncommitted changes
#   4. Binary is ALWAYS built from the clean repo state
#   5. Swap binary, graceful restart, health check
#
# The restart + health check run in a detached background process so they
# survive the service restart (which kills the calling agent process).
# Usage: upgrade.sh [--branch BRANCH] [--skip-tests]
set -euo pipefail

# Source Rust toolchain (cargo may not be in PATH for systemd services)
# shellcheck disable=SC1091
[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"

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

# Step 1: Pull latest code from git (the source of truth)
log "Pulling branch $BRANCH..."
cd "$REPO_DIR"
git fetch origin "$BRANCH" || die "git fetch failed"
git checkout "$BRANCH" || die "git checkout failed"
git reset --hard "origin/$BRANCH" || die "git reset failed"

LOCAL_HEAD=$(git rev-parse HEAD)
REMOTE_HEAD=$(git rev-parse "origin/$BRANCH")
if [[ "$LOCAL_HEAD" != "$REMOTE_HEAD" ]]; then
    die "Local HEAD ($LOCAL_HEAD) != remote ($REMOTE_HEAD). Commit and push first!"
fi

# Verify repo is clean — refuse to build from dirty state
DIRTY=$(git status --porcelain 2>/dev/null)
if [[ -n "$DIRTY" ]]; then
    die "Repo has uncommitted changes. Commit and push to GitHub first!\n$DIRTY"
fi

log "Git sync verified: $(git rev-parse --short HEAD) (clean, matches origin/$BRANCH)"

# Step 2: Compile from clean repo (NEVER from local modifications)
log "Building release binary..."
cargo build --release 2>&1 | tail -20 | tee -a "$LOG_FILE"
if [[ ${PIPESTATUS[0]} -ne 0 ]]; then
    die "cargo build --release failed"
fi

BUILT_BINARY="${REPO_DIR}/target/release/aethervault"
if [[ ! -f "$BUILT_BINARY" ]]; then
    die "Binary not found at $BUILT_BINARY"
fi

# Remove old binary first — if the service is still running from this slot
# (e.g. after a prior swap without restart), Linux returns ETXTBSY on cp.
# Unlinking is safe: the running process keeps its file handle.
rm -f "${DEPLOY_DIR}/${INACTIVE}/aethervault"
cp "$BUILT_BINARY" "${DEPLOY_DIR}/${INACTIVE}/aethervault"
chmod +x "${DEPLOY_DIR}/${INACTIVE}/aethervault"
log "Binary copied to ${INACTIVE} slot"

# Step 2b: Ensure runtime dependencies (npm tools the binary shells out to)
log "Checking runtime dependencies..."
if ! command -v agent-browser &>/dev/null; then
    log "Installing agent-browser (browser automation CLI)..."
    npm install -g agent-browser 2>&1 | tail -3 | tee -a "$LOG_FILE"
    agent-browser install 2>&1 | tail -3 | tee -a "$LOG_FILE"
    log "agent-browser installed: $(agent-browser --version 2>/dev/null || echo 'unknown')"
else
    # Ensure browser binaries are present even if the npm package is installed
    if ! agent-browser --session _healthcheck open about:blank &>/dev/null; then
        log "agent-browser installed but browser missing, reinstalling..."
        agent-browser install 2>&1 | tail -3 | tee -a "$LOG_FILE"
    fi
    log "agent-browser OK: $(agent-browser --version 2>/dev/null)"
fi

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

# Step 4: Swap symlink atomically
log "Swapping symlink: $SYMLINK -> ${DEPLOY_DIR}/${INACTIVE}/aethervault"
ln -sfn "${DEPLOY_DIR}/${INACTIVE}/aethervault" "${SYMLINK}.tmp"
mv -f "${SYMLINK}.tmp" "$SYMLINK"
echo "$INACTIVE" > "$ACTIVE_FILE"
log "Symlink swapped. Active slot is now: $INACTIVE"

# Step 5: Detach restart + health check into a background process.
# systemctl restart kills the aethervault service (which is our parent),
# so the health check must survive independently via setsid/nohup.
log "Spawning detached restart + health check..."
setsid bash -c "
    LOG_FILE='${LOG_FILE}'
    DEPLOY_DIR='${DEPLOY_DIR}'
    SYMLINK='${SYMLINK}'
    ACTIVE='${ACTIVE}'
    INACTIVE='${INACTIVE}'
    HEALTH_CHECK_SECONDS=${HEALTH_CHECK_SECONDS}
    HEALTH_CHECK_INTERVAL=${HEALTH_CHECK_INTERVAL}

    log() {
        local msg=\"[\$(date -u '+%Y-%m-%dT%H:%M:%SZ')] \$*\"
        echo \"\$msg\" >> \"\$LOG_FILE\"
    }

    # Graceful shutdown: send SIGTERM and wait for in-progress agent runs
    # to finish their capsule commits. The bridge catches SIGTERM, stops
    # accepting new messages, waits up to 60s for active runs, then exits.
    # This prevents mid-commit capsule corruption.
    log 'Sending SIGTERM for graceful shutdown (up to 90s)...'
    systemctl stop aethervault --no-block
    # Wait for the service to stop gracefully (up to 90s)
    STOP_WAIT=0
    while systemctl is-active --quiet aethervault && [[ \$STOP_WAIT -lt 90 ]]; do
        sleep 2
        STOP_WAIT=\$((STOP_WAIT + 2))
        log \"Waiting for graceful shutdown... \${STOP_WAIT}s\"
    done
    if systemctl is-active --quiet aethervault; then
        log 'Service did not stop gracefully after 90s, force killing...'
        systemctl kill -s SIGKILL aethervault
        sleep 1
    fi
    log 'Service stopped. Starting with new binary...'
    systemctl start aethervault

    log \"Monitoring for \${HEALTH_CHECK_SECONDS}s...\"
    ELAPSED=0
    while [[ \$ELAPSED -lt \$HEALTH_CHECK_SECONDS ]]; do
        sleep \$HEALTH_CHECK_INTERVAL
        ELAPSED=\$((ELAPSED + HEALTH_CHECK_INTERVAL))
        if ! systemctl is-active --quiet aethervault; then
            log \"Service crashed after \${ELAPSED}s! Rolling back to \${ACTIVE}...\"
            ln -sfn \"\${DEPLOY_DIR}/\${ACTIVE}/aethervault\" \"\${SYMLINK}.tmp\"
            mv -f \"\${SYMLINK}.tmp\" \"\$SYMLINK\"
            echo \"\$ACTIVE\" > \"\${DEPLOY_DIR}/active\"
            systemctl restart aethervault
            log \"FATAL: Rolled back to \$ACTIVE slot after crash\"
            exit 1
        fi
        log \"Health check \${ELAPSED}/\${HEALTH_CHECK_SECONDS}s: OK\"
    done

    log \"=== Upgrade complete: \$(\$SYMLINK --version 2>/dev/null || echo 'version unknown') ===\"
" >> "$LOG_FILE" 2>&1 &

# Give the background process a moment to start
sleep 1
log "Detached health check PID: $!"
echo "Upgrade binary swapped successfully. Service restart + health check running in background (PID $!)."

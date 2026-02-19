#!/usr/bin/env bash
# Autonomous Self-Improvement Loop (SICA-style)
# Triggered by systemd timer every 6h
set -euo pipefail

source /root/.cargo/env
WORKSPACE="/root/.aethervault"
REPO="/root/aethervault"
MV2="${WORKSPACE}/capsule.mv2"
LOG="${WORKSPACE}/data/self-improve-log.jsonl"
LOCK="/tmp/aethervault-self-improve.lock"
DATE=$(date -u +%Y%m%d-%H%M%S)
MAX_SCAN_STEPS=128
MAX_IMPL_STEPS=196

# Flock: only one improvement cycle at a time
exec 200>"$LOCK"
flock -n 200 || { echo "Another improvement cycle running"; exit 0; }

mkdir -p "$(dirname "$LOG")"

log() { echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] $*" | tee -a "$LOG"; }

die() { log "FATAL: $*"; exit 1; }

# Safety: don't run if service is down
systemctl is-active --quiet aethervault || die "Service not running, skipping"

# Safety: don't run if repo has uncommitted changes
cd "$REPO"
if [[ -n "$(git status --porcelain 2>/dev/null)" ]]; then
    log "WARN: Repo dirty, stashing before improvement cycle"
    git stash push -m "self-improve-${DATE}" || die "git stash failed"
fi

# ═══════════════════════════════════════════
# PHASE 1: SITUATIONAL AWARENESS (Scan)
# ═══════════════════════════════════════════
log "=== Phase 1: Scanning for improvements ==="

SCAN_PROMPT=$(cat <<'PROMPT'
You are running an autonomous self-improvement cycle. Your job is to analyze
your own source code and identify the single highest-impact improvement.

Steps:
1. Read recent capsule reflections: use memory_search for "error" and "failure"
2. Check recent git log: exec "cd /root/aethervault && git log --oneline -20"
3. Read the main source files that handle tool execution and agent logic
4. Analyze: what patterns cause errors? What's brittle? What's missing?
5. Check /root/.aethervault/data/self-improve-log.jsonl for past improvements (avoid repeats)

Output EXACTLY this JSON (nothing else before or after):
{
  "target_file": "src/filename.rs",
  "description": "One-line description of what to change",
  "rationale": "Why this is the highest-impact improvement",
  "risk": "low|medium|high",
  "estimated_lines_changed": <number>,
  "category": "reliability|performance|capability|safety|ux"
}

Rules:
- Only propose changes to files in src/ (Rust source)
- Estimated lines changed must be < 50 (small, focused changes only)
- Risk must be "low" or "medium" — never propose high-risk changes autonomously
- Never propose changes to deploy/, upgrade.sh, or systemd configs
- Never propose removing safety checks or approval gates
- Prefer reliability and safety improvements over new features
PROMPT
)

SCAN_OUTPUT=$(timeout 600 aethervault agent \
    --mv2 "$MV2" \
    --session "self-improve-scan-${DATE}" \
    --max-steps "$MAX_SCAN_STEPS" \
    --model-hook builtin:sonnet \
    --prompt "$SCAN_PROMPT" 2>&1) || {
    log "Phase 1 failed (timeout or error)"
    exit 1
}

# Extract JSON from scan output
SCAN_JSON=$(echo "$SCAN_OUTPUT" | grep -oP '\{[^{}]*"target_file"[^{}]*\}' | head -1)
if [[ -z "$SCAN_JSON" ]]; then
    log "Phase 1: No valid improvement proposal found. Output: $(echo "$SCAN_OUTPUT" | tail -5)"
    exit 0
fi

TARGET_FILE=$(echo "$SCAN_JSON" | python3 -c "import sys,json; print(json.load(sys.stdin)['target_file'])")
DESCRIPTION=$(echo "$SCAN_JSON" | python3 -c "import sys,json; print(json.load(sys.stdin)['description'])")
RISK=$(echo "$SCAN_JSON" | python3 -c "import sys,json; print(json.load(sys.stdin)['risk'])")
CATEGORY=$(echo "$SCAN_JSON" | python3 -c "import sys,json; print(json.load(sys.stdin)['category'])")

log "Phase 1 result: [$CATEGORY/$RISK] $TARGET_FILE — $DESCRIPTION"

# Safety gate: reject high-risk proposals
if [[ "$RISK" == "high" ]]; then
    log "REJECTED: High-risk proposal. Skipping."
    exit 0
fi

# Safety gate: only allow src/ files
if [[ ! "$TARGET_FILE" =~ ^src/ ]]; then
    log "REJECTED: Target file '$TARGET_FILE' not in src/. Skipping."
    exit 0
fi

# ═══════════════════════════════════════════
# PHASE 2: IMPLEMENTATION
# ═══════════════════════════════════════════
log "=== Phase 2: Implementing improvement ==="

IMPL_PROMPT=$(cat <<PROMPT
You are implementing an autonomous self-improvement. The scan phase identified:

Target: $TARGET_FILE
Change: $DESCRIPTION
Category: $CATEGORY

Instructions:
1. Read the target file: exec "cat /root/aethervault/$TARGET_FILE"
2. Implement the change using fs_write or exec with sed
3. Run: exec "cd /root/aethervault && cargo check" — the change MUST compile
4. If cargo check fails, fix the issue or revert and report failure
5. Run: exec "cd /root/aethervault && cargo test" — existing tests must pass
6. If tests fail, fix or revert

Output EXACTLY this JSON when done:
{
  "status": "success|failed|reverted",
  "files_changed": ["list", "of", "files"],
  "lines_added": <number>,
  "lines_removed": <number>,
  "cargo_check": "pass|fail",
  "cargo_test": "pass|fail|skipped",
  "summary": "What was actually changed"
}

Rules:
- Do NOT touch any file outside src/
- Do NOT change deploy/, upgrade.sh, systemd configs, or .env files
- Keep changes minimal and focused (< 50 lines diff)
- If cargo check fails after 2 attempts, revert ALL changes and report status: "reverted"
PROMPT
)

# Snapshot current HEAD so we can revert if needed
PRE_HEAD=$(cd "$REPO" && git rev-parse HEAD)

IMPL_OUTPUT=$(timeout 900 aethervault agent \
    --mv2 "$MV2" \
    --session "self-improve-impl-${DATE}" \
    --max-steps "$MAX_IMPL_STEPS" \
    --model-hook builtin:claude \
    --prompt "$IMPL_PROMPT" 2>&1) || {
    log "Phase 2 failed (timeout or error). Reverting."
    cd "$REPO" && git checkout -- .
    exit 1
}

# Extract implementation result
IMPL_JSON=$(echo "$IMPL_OUTPUT" | grep -oP '\{[^{}]*"status"[^{}]*\}' | tail -1)
IMPL_STATUS=$(echo "$IMPL_JSON" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status','unknown'))" 2>/dev/null || echo "unknown")

if [[ "$IMPL_STATUS" != "success" ]]; then
    log "Phase 2: Implementation $IMPL_STATUS. Reverting any changes."
    cd "$REPO" && git checkout -- .
    exit 0
fi

# Double-check: cargo check ourselves (don't trust agent output alone)
log "Phase 2: Verifying cargo check independently..."
cd "$REPO"
if ! cargo check 2>&1 | tail -5; then
    log "Phase 2: Independent cargo check FAILED. Reverting."
    git checkout -- .
    exit 1
fi

# ═══════════════════════════════════════════
# PHASE 3: VALIDATION (Regression Battery)
# ═══════════════════════════════════════════
log "=== Phase 3: Running validation battery ==="

if [[ -x "$REPO/scripts/self-improve-validate.sh" ]]; then
    if ! timeout 300 "$REPO/scripts/self-improve-validate.sh" 2>&1 | tee -a "$LOG"; then
        log "Phase 3: Validation FAILED. Reverting."
        cd "$REPO" && git checkout -- .
        exit 1
    fi
else
    log "Phase 3: No validation script found, relying on cargo check + cargo test"
    cd "$REPO"
    if ! cargo test 2>&1 | tail -10; then
        log "Phase 3: cargo test FAILED. Reverting."
        git checkout -- .
        exit 1
    fi
fi

# ═══════════════════════════════════════════
# PHASE 4: DEPLOYMENT
# ═══════════════════════════════════════════
log "=== Phase 4: Deploying improvement ==="

cd "$REPO"
git add -A
git commit -m "self-improve(${CATEGORY}): ${DESCRIPTION}

Autonomous improvement cycle ${DATE}
Risk: ${RISK}
$(echo "$IMPL_JSON" | python3 -c "import sys,json; d=json.load(sys.stdin); print(f'Files: {\", \".join(d.get(\"files_changed\",[]))}')" 2>/dev/null || true)
" || {
    log "Phase 4: Nothing to commit (no changes). Skipping."
    exit 0
}

git push origin main || {
    log "Phase 4: git push failed. Reverting commit."
    git reset --soft HEAD~1
    git checkout -- .
    exit 1
}

# Call self_upgrade via agent (it needs the tool framework)
UPGRADE_OUTPUT=$(timeout 120 aethervault agent \
    --mv2 "$MV2" \
    --session "self-improve-deploy-${DATE}" \
    --max-steps 8 \
    --prompt "Call self_upgrade with branch main. Report the result." 2>&1) || {
    log "Phase 4: self_upgrade timed out. Check /opt/aethervault/upgrade.log"
}

log "Phase 4: Deploy initiated. Waiting for health check..."
sleep 35  # Wait for blue-green health check (30s) + buffer

if systemctl is-active --quiet aethervault; then
    log "Phase 4: Service healthy after deploy"
else
    log "Phase 4: Service NOT healthy — upgrade.sh should auto-rollback"
    exit 1
fi

# ═══════════════════════════════════════════
# PHASE 5: ARCHIVE
# ═══════════════════════════════════════════
log "=== Phase 5: Archiving improvement record ==="

RECORD=$(python3 -c "
import json, datetime
print(json.dumps({
    'timestamp': '${DATE}',
    'target_file': '${TARGET_FILE}',
    'description': '${DESCRIPTION}',
    'category': '${CATEGORY}',
    'risk': '${RISK}',
    'status': 'deployed',
    'git_commit': '$(cd "$REPO" && git rev-parse --short HEAD)',
    'pre_head': '${PRE_HEAD:0:8}'
}))
")
echo "$RECORD" >> "$LOG"

# Store in capsule memory for future scan phases to find
aethervault agent \
    --mv2 "$MV2" \
    --session "self-improve-archive-${DATE}" \
    --max-steps 4 \
    --prompt "Use reflect tool to store: Self-improvement deployed — ${DESCRIPTION} (${CATEGORY}, risk:${RISK}, commit:$(cd "$REPO" && git rev-parse --short HEAD))" \
    2>/dev/null || true

log "=== Self-improvement cycle complete: ${DESCRIPTION} ==="

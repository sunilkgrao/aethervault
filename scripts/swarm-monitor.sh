#!/usr/bin/env bash
# Swarm Monitor — check task status, retry failures, notify on completion
# Runs every 10 minutes via systemd timer. Deterministic — no AI tokens consumed.
set -euo pipefail

SWARM_DB="${AETHERVAULT_HOME:-/root/.aethervault}/swarm.sqlite"
REPO_DIR="/root/aethervault"
AV_BIN="/root/aethervault/target/release/aethervault"

# Load env for Telegram notifications
ENV_FILE="${AETHERVAULT_HOME:-/root/.aethervault}/.env"
if [[ -f "$ENV_FILE" ]]; then
    # shellcheck disable=SC1090
    source "$ENV_FILE"
fi

NOTIFY_CHAT_ID="${TELEGRAM_CHAT_ID:-}"
TELEGRAM_TOKEN="${TELEGRAM_BOT_TOKEN:-}"

log() { echo "[swarm-monitor $(date -u +%H:%M:%S)] $*"; }

notify_telegram() {
    local text="$1"
    if [[ -n "$TELEGRAM_TOKEN" && -n "$NOTIFY_CHAT_ID" ]]; then
        curl -s -X POST "https://api.telegram.org/bot${TELEGRAM_TOKEN}/sendMessage" \
            -d chat_id="$NOTIFY_CHAT_ID" \
            -d text="$text" \
            -d parse_mode="Markdown" \
            --max-time 10 >/dev/null 2>&1 || true
    fi
}

# Check if DB exists
if [[ ! -f "$SWARM_DB" ]]; then
    log "No swarm DB at $SWARM_DB — nothing to monitor"
    exit 0
fi

# ─────────────────────────────────────────────────────────
# 1. Check "pr_open" tasks — CI status via gh
# ─────────────────────────────────────────────────────────
log "Checking pr_open tasks..."
sqlite3 "$SWARM_DB" "SELECT id, name, pr_number, retry_count, max_retries, agent_backend FROM swarm_tasks WHERE status='pr_open' AND pr_number IS NOT NULL" 2>/dev/null | while IFS='|' read -r task_id task_name pr_num retry_count max_retries agent_backend; do
    [[ -z "$pr_num" ]] && continue
    log "  Checking PR #$pr_num ($task_name)..."

    # Get CI check states
    ci_states=$(gh pr checks "$pr_num" --json state -q '.[].state' 2>/dev/null || echo "UNKNOWN")
    unique_states=$(echo "$ci_states" | sort -u | tr '\n' ',' | sed 's/,$//')

    if echo "$ci_states" | grep -qE '^(SUCCESS|NEUTRAL|SKIPPED)$' && ! echo "$ci_states" | grep -qE '^(FAILURE|ERROR|PENDING)$'; then
        # All checks passing → move to reviewing
        log "  PR #$pr_num CI passing — moving to reviewing"
        sqlite3 "$SWARM_DB" "UPDATE swarm_tasks SET status='reviewing', ci_status='passing', updated_at=datetime('now') WHERE id='$task_id'"
        notify_telegram "✅ Swarm task *$task_name* (PR #$pr_num) — CI passing, ready for review"

    elif echo "$ci_states" | grep -qE '^(FAILURE|ERROR)$'; then
        # CI failing
        log "  PR #$pr_num CI failing (states: $unique_states)"
        sqlite3 "$SWARM_DB" "UPDATE swarm_tasks SET ci_status='failing', updated_at=datetime('now') WHERE id='$task_id'"

        if [[ "$retry_count" -lt "$max_retries" ]]; then
            # Adaptive retry: get error output, rewrite prompt, respawn
            new_count=$((retry_count + 1))
            log "  Retrying ($new_count/$max_retries)..."

            # Get CI error details
            error_output=$(gh pr checks "$pr_num" --json 'name,state,output' 2>/dev/null | head -c 2000 || echo "Could not fetch error details")

            # Get original prompt
            original_prompt=$(sqlite3 "$SWARM_DB" "SELECT prompt FROM swarm_tasks WHERE id='$task_id'" 2>/dev/null)

            # Build retry prompt
            retry_prompt="$original_prompt

PREVIOUS ATTEMPT FAILED (attempt $new_count/$max_retries):
$error_output

Fix the issues above and try again. The branch already exists — make your changes on the existing branch, commit, and force-push."

            # Update DB
            sqlite3 "$SWARM_DB" "UPDATE swarm_tasks SET retry_count=$new_count, error_context='$(echo "$error_output" | head -c 500 | sed "s/'/''/g")', status='running', updated_at=datetime('now') WHERE id='$task_id'"

            # Get worktree path (reuse existing)
            wt_path=$(sqlite3 "$SWARM_DB" "SELECT worktree_path FROM swarm_tasks WHERE id='$task_id'" 2>/dev/null)

            # Spawn agent via AV CLI if available, otherwise use claude directly
            if [[ -x "$AV_BIN" ]]; then
                "$AV_BIN" agent --prompt "$retry_prompt" ${wt_path:+--cwd "$wt_path"} &
            fi

            notify_telegram "🔄 Swarm task *$task_name* CI failed (attempt $new_count/$max_retries). Retrying..."
        else
            # Exhausted retries
            sqlite3 "$SWARM_DB" "UPDATE swarm_tasks SET status='failed', updated_at=datetime('now') WHERE id='$task_id'"
            notify_telegram "❌ Swarm task *$task_name* failed after $max_retries attempts. Manual review needed: PR #$pr_num"
        fi
    else
        log "  PR #$pr_num CI pending (states: $unique_states)"
        sqlite3 "$SWARM_DB" "UPDATE swarm_tasks SET ci_status='pending', updated_at=datetime('now') WHERE id='$task_id'"
    fi
done

# ─────────────────────────────────────────────────────────
# 2. Check "reviewing" tasks — review status
# ─────────────────────────────────────────────────────────
log "Checking reviewing tasks..."
sqlite3 "$SWARM_DB" "SELECT id, name, pr_number, agent_backend FROM swarm_tasks WHERE status='reviewing' AND pr_number IS NOT NULL" 2>/dev/null | while IFS='|' read -r task_id task_name pr_num agent_backend; do
    [[ -z "$pr_num" ]] && continue
    log "  Checking review status for PR #$pr_num ($task_name)..."

    review_decision=$(gh pr view "$pr_num" --json reviewDecision -q '.reviewDecision' 2>/dev/null || echo "")

    case "$review_decision" in
        APPROVED)
            log "  PR #$pr_num approved!"
            sqlite3 "$SWARM_DB" "UPDATE swarm_tasks SET status='done', review_status='approved', updated_at=datetime('now') WHERE id='$task_id'"
            notify_telegram "🎉 Swarm task *$task_name* — PR #$pr_num approved and ready for merge!"
            ;;
        CHANGES_REQUESTED)
            log "  PR #$pr_num has changes requested"
            sqlite3 "$SWARM_DB" "UPDATE swarm_tasks SET review_status='changes_requested', updated_at=datetime('now') WHERE id='$task_id'"
            notify_telegram "📝 Swarm task *$task_name* — PR #$pr_num has changes requested"
            ;;
        *)
            log "  PR #$pr_num review pending"
            ;;
    esac
done

# ─────────────────────────────────────────────────────────
# 3. Multi-model code review for newly-reviewing tasks
# ─────────────────────────────────────────────────────────
# When CI passes, spawn a cross-model review before notifying human
sqlite3 "$SWARM_DB" "SELECT id, name, pr_number, agent_backend FROM swarm_tasks WHERE status='reviewing' AND review_status IS NULL AND pr_number IS NOT NULL" 2>/dev/null | while IFS='|' read -r task_id task_name pr_num agent_backend; do
    [[ -z "$pr_num" ]] && continue

    # Determine reviewer backend (cross-model)
    if [[ "$agent_backend" == "codex" ]]; then
        reviewer="swarm-reviewer-claude"
    else
        reviewer="swarm-reviewer-codex"
    fi

    log "  Spawning $reviewer for PR #$pr_num"
    if [[ -x "$AV_BIN" ]]; then
        review_prompt="Review PR #$pr_num. Run: gh pr diff $pr_num to read the changes. Focus on logic errors, missing error handling, race conditions, edge cases, security issues, and test coverage. Post your review via: gh pr review $pr_num --comment --body 'YOUR REVIEW'"
        "$AV_BIN" agent --prompt "$review_prompt" --subagent "$reviewer" &
    fi

    sqlite3 "$SWARM_DB" "UPDATE swarm_tasks SET review_status='pending', updated_at=datetime('now') WHERE id='$task_id'"
done

# ─────────────────────────────────────────────────────────
# 4. Cleanup — remove worktrees for done/failed tasks older than 7 days
# ─────────────────────────────────────────────────────────
log "Cleanup pass..."
sqlite3 "$SWARM_DB" "SELECT id, worktree_path FROM swarm_tasks WHERE status IN ('done', 'failed') AND worktree_path IS NOT NULL AND updated_at < datetime('now', '-7 days')" 2>/dev/null | while IFS='|' read -r task_id wt_path; do
    if [[ -d "$wt_path" ]]; then
        log "  Cleaning up worktree: $wt_path"
        cd "$REPO_DIR"
        git worktree remove "$wt_path" --force 2>/dev/null || rm -rf "$wt_path"
        sqlite3 "$SWARM_DB" "UPDATE swarm_tasks SET worktree_path=NULL, updated_at=datetime('now') WHERE id='$task_id'"
    fi
done

# Prune old completed tasks from DB (>30 days)
pruned=$(sqlite3 "$SWARM_DB" "DELETE FROM swarm_tasks WHERE status IN ('done', 'failed') AND updated_at < datetime('now', '-30 days'); SELECT changes()" 2>/dev/null || echo "0")
if [[ "$pruned" -gt 0 ]]; then
    log "Pruned $pruned old tasks from DB"
fi

log "Done."

#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DB_PATH="${DB_PATH:-/root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite}"
MEMORY_DIR="${MEMORY_DIR:-/root/.openclaw/workspace/memory}"
STATE_PATH="${STATE_PATH:-/root/.openclaw/workspace/relationship-intel/sync_state.json}"
ACCOUNT_EMAIL="${ACCOUNT_EMAIL:-sunil@tribble.ai}"
TOP_N="${TOP_N:-250}"
GMAIL_QUERY="${GMAIL_QUERY:-in:anywhere}"
GMAIL_LOOKBACK_DAYS="${GMAIL_LOOKBACK_DAYS:-14}"
GMAIL_MAX_MESSAGES="${GMAIL_MAX_MESSAGES:-750}"
CALENDAR_LOOKBACK_DAYS="${CALENDAR_LOOKBACK_DAYS:-30}"
CALENDAR_FUTURE_DAYS="${CALENDAR_FUTURE_DAYS:-180}"
DRIVE_LOOKBACK_DAYS="${DRIVE_LOOKBACK_DAYS:-30}"
DRIVE_BODY_LIMIT="${DRIVE_BODY_LIMIT:-80}"
RUN_WHATSAPP="${RUN_WHATSAPP:-0}"
RUN_SLACK="${RUN_SLACK:-0}"
SLACK_ARCHIVE_DIR="${SLACK_ARCHIVE_DIR:-}"
DRY_RUN="${DRY_RUN:-0}"
PAUSE_GATEWAY="${PAUSE_GATEWAY:-1}"
GATEWAY_SERVICE="${GATEWAY_SERVICE:-openclaw-gateway.service}"

gateway_was_active=0
resume_gateway() {
  if [[ "$gateway_was_active" == "1" ]]; then
    systemctl --user start "$GATEWAY_SERVICE" >/dev/null 2>&1 || true
  fi
}

if [[ "$DRY_RUN" != "1" && "$PAUSE_GATEWAY" == "1" ]]; then
  if systemctl --user is-active --quiet "$GATEWAY_SERVICE"; then
    gateway_was_active=1
    trap resume_gateway EXIT
    systemctl --user stop "$GATEWAY_SERVICE"
  fi
fi

if [[ "$RUN_WHATSAPP" == "1" && -x "$ROOT_DIR/whatsapp_history_sync.sh" ]]; then
  "$ROOT_DIR/whatsapp_history_sync.sh"
fi

if [[ "$RUN_SLACK" == "1" && -n "$SLACK_ARCHIVE_DIR" ]]; then
  python3 "$ROOT_DIR/relationship_intel.py" \
    --db "$DB_PATH" \
    import-slack-archive \
    --archive-dir "$SLACK_ARCHIVE_DIR" \
    --top-n "$TOP_N"
fi

args=(
  python3 "$ROOT_DIR/relationship_intel.py"
  --db "$DB_PATH"
  sync-incremental
  --account-email "$ACCOUNT_EMAIL"
  --state-path "$STATE_PATH"
  --gmail-query "$GMAIL_QUERY"
  --gmail-lookback-days "$GMAIL_LOOKBACK_DAYS"
  --gmail-max-messages "$GMAIL_MAX_MESSAGES"
  --calendar-lookback-days "$CALENDAR_LOOKBACK_DAYS"
  --calendar-future-days "$CALENDAR_FUTURE_DAYS"
  --drive-lookback-days "$DRIVE_LOOKBACK_DAYS"
  --drive-body-limit "$DRIVE_BODY_LIMIT"
  --top-n "$TOP_N"
  --json
)

if [[ "$DRY_RUN" == "1" ]]; then
  args+=(--dry-run)
fi

"${args[@]}"

if [[ "$DRY_RUN" != "1" ]]; then
  "$ROOT_DIR/relationship_sync.sh"
fi

if [[ "$gateway_was_active" == "1" ]]; then
  resume_gateway
  trap - EXIT
fi

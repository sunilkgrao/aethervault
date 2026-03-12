#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DB_PATH="${DB_PATH:-/root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite}"
MEMORY_DIR="${MEMORY_DIR:-/root/.openclaw/workspace/memory}"
AUTH_DIR="${AUTH_DIR:-/root/.openclaw/credentials/whatsapp/default}"
ACCOUNT_ID="${ACCOUNT_ID:-default}"
TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-75}"
TOP_N="${TOP_N:-220}"
RUNTIME_DIR="${RUNTIME_DIR:-$ROOT_DIR/runtime}"
STAMP="$(date -u +%Y%m%d-%H%M%S)"
EXPORT_FILE="${EXPORT_FILE:-$RUNTIME_DIR/whatsapp-history-$STAMP.ndjson}"
IMPORT_REPORT="${IMPORT_REPORT:-$RUNTIME_DIR/whatsapp-history-$STAMP.import.json}"
ALLOW_QR="${ALLOW_QR:-0}"
QR_FILE="${QR_FILE:-$RUNTIME_DIR/whatsapp-login-$STAMP.qr.txt}"
OPENCLAW_SERVICE="${OPENCLAW_SERVICE:-openclaw-gateway.service}"
STOP_GATEWAY="${STOP_GATEWAY:-1}"

service_ctl() {
  systemctl --user "$@"
}

mkdir -p "$RUNTIME_DIR"

gateway_was_active=0
if [[ "$STOP_GATEWAY" == "1" ]] && service_ctl is-active --quiet "$OPENCLAW_SERVICE"; then
  gateway_was_active=1
  service_ctl stop "$OPENCLAW_SERVICE"
fi

cleanup() {
  if [[ "$gateway_was_active" == "1" ]]; then
    service_ctl start "$OPENCLAW_SERVICE"
  fi
}
trap cleanup EXIT

node "$ROOT_DIR/whatsapp_history_sync.mjs" \
  --auth-dir "$AUTH_DIR" \
  --account-id "$ACCOUNT_ID" \
  --timeout-seconds "$TIMEOUT_SECONDS" \
  --allow-qr "$ALLOW_QR" \
  --qr-file "$QR_FILE" \
  --output "$EXPORT_FILE"

python3 "$ROOT_DIR/relationship_intel.py" \
  --db "$DB_PATH" \
  import-whatsapp \
  --source-file "$EXPORT_FILE" \
  --memory-dir "$MEMORY_DIR" \
  --top-n "$TOP_N" \
  --json > "$IMPORT_REPORT"

"$ROOT_DIR/relationship_sync.sh"

echo "$IMPORT_REPORT"

#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AUTH_ROOT="${AUTH_ROOT:-/root/.openclaw/credentials/whatsapp}"
LIVE_AUTH_DIR="${LIVE_AUTH_DIR:-$AUTH_ROOT/default}"
STAMP="$(date -u +%Y%m%d-%H%M%S)"
BACKUP_AUTH_DIR="${BACKUP_AUTH_DIR:-$AUTH_ROOT/default.backup-$STAMP}"
TEMP_AUTH_DIR="${TEMP_AUTH_DIR:-$AUTH_ROOT/default.relink-$STAMP}"
OPENCLAW_SERVICE="${OPENCLAW_SERVICE:-openclaw-gateway.service}"
TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-300}"
RUNTIME_DIR="${RUNTIME_DIR:-$ROOT_DIR/runtime}"
QR_FILE="${QR_FILE:-$RUNTIME_DIR/whatsapp-relink-$STAMP.qr.txt}"
EXPORT_FILE="${EXPORT_FILE:-$RUNTIME_DIR/whatsapp-relink-$STAMP.ndjson}"
IMPORT_REPORT="${IMPORT_REPORT:-$RUNTIME_DIR/whatsapp-relink-$STAMP.import.json}"

service_ctl() {
  systemctl --user "$@"
}

mkdir -p "$RUNTIME_DIR"

restore_live_auth() {
  rm -rf "$LIVE_AUTH_DIR"
  if [[ -d "$BACKUP_AUTH_DIR" ]]; then
    cp -a "$BACKUP_AUTH_DIR" "$LIVE_AUTH_DIR"
  fi
}

service_ctl stop "$OPENCLAW_SERVICE" || true

if [[ -d "$LIVE_AUTH_DIR" ]]; then
  cp -a "$LIVE_AUTH_DIR" "$BACKUP_AUTH_DIR"
fi

rm -rf "$TEMP_AUTH_DIR"
mkdir -p "$TEMP_AUTH_DIR"

sync_ok=0
if AUTH_DIR="$TEMP_AUTH_DIR" \
  ALLOW_QR=1 \
  STOP_GATEWAY=0 \
  TIMEOUT_SECONDS="$TIMEOUT_SECONDS" \
  QR_FILE="$QR_FILE" \
  EXPORT_FILE="$EXPORT_FILE" \
  IMPORT_REPORT="$IMPORT_REPORT" \
  "$ROOT_DIR/whatsapp_history_sync.sh"; then
  sync_ok=1
fi

if [[ "$sync_ok" == "1" ]]; then
  rm -rf "$LIVE_AUTH_DIR"
  mv "$TEMP_AUTH_DIR" "$LIVE_AUTH_DIR"
else
  restore_live_auth
  rm -rf "$TEMP_AUTH_DIR"
fi

service_ctl start "$OPENCLAW_SERVICE"

if [[ "$sync_ok" != "1" ]]; then
  echo "relink failed; restored previous auth" >&2
  exit 1
fi

echo "$IMPORT_REPORT"

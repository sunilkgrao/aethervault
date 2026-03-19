#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 4 ]]; then
  echo "usage: $0 TARGET_DS9 [LCARS_PORT] [UI_PORT] [CHAT_PORT]" >&2
  exit 1
fi

TARGET_DS9="$1"
LCARS_PORT="${2:-${LCARS_PORT:-3000}}"
UI_PORT="${3:-${UI_PORT:-5173}}"
CHAT_PORT="${4:-${CHAT_PORT:-3001}}"
UI_DIR="$TARGET_DS9/lcars/ui"
ENV_LOCAL="$UI_DIR/.env.local"
LCARS_ENV_LOCAL="$TARGET_DS9/lcars/.env.local"
TRIBBLE_CHAT_ENV="$TARGET_DS9/tribble-chat/.env"

if [[ ! -d "$UI_DIR" || ! -d "$TARGET_DS9/lcars" || ! -d "$TARGET_DS9/tribble-chat" ]]; then
  echo "lcars/ui or companion directories not found under $TARGET_DS9" >&2
  exit 1
fi

mkdir -p "$UI_DIR"

write_kv() {
  local path="$1"
  shift
  mkdir -p "$(dirname "$path")"
  local tmp
  tmp="$(mktemp)"
  if [[ -f "$path" ]]; then
    cp "$path" "$tmp"
  else
    : >"$tmp"
  fi
  for key in "$@"; do
    grep -vE "^${key}=" "$tmp" >"${tmp}.next" || true
    mv "${tmp}.next" "$tmp"
  done
  cat >>"$tmp"
  mv "$tmp" "$path"
}

write_kv "$ENV_LOCAL" \
  VITE_WEBCHAT_API_URL \
  VITE_WEBCHAT_DOMAIN \
  VITE_LOCAL_API_ORIGIN <<EOF
VITE_WEBCHAT_API_URL=ws://localhost:${CHAT_PORT}/api/chat
VITE_WEBCHAT_DOMAIN=localhost:${CHAT_PORT}
VITE_LOCAL_API_ORIGIN=http://localhost:${LCARS_PORT}
EOF

write_kv "$LCARS_ENV_LOCAL" \
  APP_PATH \
  WEBCHAT_DOMAIN <<EOF
APP_PATH=http://localhost:${UI_PORT}
WEBCHAT_DOMAIN=localhost:${CHAT_PORT}
EOF

write_kv "$TRIBBLE_CHAT_ENV" \
  APP_PATH <<EOF
APP_PATH=http://localhost:${UI_PORT}
EOF

printf 'wrote %s\n' "$ENV_LOCAL"
grep -E '^(VITE_WEBCHAT_API_URL|VITE_WEBCHAT_DOMAIN|VITE_LOCAL_API_ORIGIN)=' "$ENV_LOCAL"
printf 'wrote %s\n' "$LCARS_ENV_LOCAL"
grep -E '^(APP_PATH|WEBCHAT_DOMAIN)=' "$LCARS_ENV_LOCAL"
printf 'wrote %s\n' "$TRIBBLE_CHAT_ENV"
grep -E '^(APP_PATH)=' "$TRIBBLE_CHAT_ENV"

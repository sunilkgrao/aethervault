#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 TARGET_DS9" >&2
  exit 1
fi

TARGET_DS9="$1"
UI_DIR="$TARGET_DS9/lcars/ui"
ENV_LOCAL="$UI_DIR/.env.local"

if [[ ! -d "$UI_DIR" ]]; then
  echo "lcars/ui directory not found under $TARGET_DS9" >&2
  exit 1
fi

mkdir -p "$UI_DIR"
tmp="$(mktemp)"

if [[ -f "$ENV_LOCAL" ]]; then
  grep -vE '^(VITE_WEBCHAT_API_URL|VITE_WEBCHAT_DOMAIN|VITE_LOCAL_DEV_AUTH_[A-Z_]+)=' "$ENV_LOCAL" >"$tmp" || true
else
  : >"$tmp"
fi

{
  echo "VITE_WEBCHAT_API_URL=ws://localhost:3001/api/chat"
  echo "VITE_WEBCHAT_DOMAIN=localhost:3001"
} >>"$tmp"

mv "$tmp" "$ENV_LOCAL"

printf 'wrote %s\n' "$ENV_LOCAL"
grep -E '^(VITE_WEBCHAT_API_URL|VITE_WEBCHAT_DOMAIN)=' "$ENV_LOCAL"

#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 TARGET_DS9 [EMAIL]" >&2
  exit 1
fi

TARGET_DS9="$1"
EMAIL="${2:-sunil@tribble.ai}"
TOKEN_DOTTED="local-dev-token.${EMAIL//@/__at__}"
TOKEN_COLON="local-dev-token:${EMAIL}"

UI_ENV_LOCAL="$TARGET_DS9/lcars/ui/.env.local"
LCARS_ENV_LOCAL="$TARGET_DS9/lcars/.env.local"
TRIBBLE_CHAT_ENV="$TARGET_DS9/tribble-chat/.env"

if [[ ! -d "$TARGET_DS9/lcars/ui" ]]; then
  echo "lcars/ui directory not found under $TARGET_DS9" >&2
  exit 1
fi

if [[ ! -d "$TARGET_DS9/tribble-chat" ]]; then
  echo "tribble-chat directory not found under $TARGET_DS9" >&2
  exit 1
fi

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

write_kv "$UI_ENV_LOCAL" \
  VITE_LOCAL_DEV_AUTH_BYPASS \
  VITE_LOCAL_DEV_AUTH_EMAIL <<EOF
VITE_LOCAL_DEV_AUTH_BYPASS=true
VITE_LOCAL_DEV_AUTH_EMAIL=${EMAIL}
EOF

write_kv "$LCARS_ENV_LOCAL" \
  LOCAL_DEV_AUTH_ENABLED \
  LOCAL_DEV_AUTH_EMAIL \
  LOCAL_DEV_AUTH_TOKEN <<EOF
LOCAL_DEV_AUTH_ENABLED=true
LOCAL_DEV_AUTH_EMAIL=${EMAIL}
LOCAL_DEV_AUTH_TOKEN=${TOKEN_DOTTED}
EOF

write_kv "$TRIBBLE_CHAT_ENV" \
  LOCAL_DEV_AUTH_ENABLED \
  LOCAL_DEV_AUTH_EMAIL \
  LOCAL_DEV_AUTH_TOKEN <<EOF
LOCAL_DEV_AUTH_ENABLED=true
LOCAL_DEV_AUTH_EMAIL=${EMAIL}
LOCAL_DEV_AUTH_TOKEN=${TOKEN_DOTTED}
EOF

cat <<EOF
wrote ${UI_ENV_LOCAL}
wrote ${LCARS_ENV_LOCAL}
wrote ${TRIBBLE_CHAT_ENV}
email=${EMAIL}
token_dotted=${TOKEN_DOTTED}
token_colon=${TOKEN_COLON}

warning: this only prepares local env flags.
It assumes the DS9 checkout already contains the local-auth-bypass implementation in UI/backend source.
Never commit those bypass source changes in a branch or PR. Keep them local-only for testing.
EOF

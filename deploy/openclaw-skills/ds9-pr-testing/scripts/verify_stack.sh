#!/usr/bin/env bash
set -euo pipefail

AUTH_TOKEN="${AUTH_TOKEN:-}"
REQUIRED_PORTS="${REQUIRED_PORTS:-50051 50061 3000 3091 7072 3001}"
UI_PORT="${UI_PORT:-5173}"
DATABASE_URL="${DATABASE_URL:-postgres://tribbledev@localhost:5432/postgres}"
VERIFY_CHAT_READY="${VERIFY_CHAT_READY:-0}"
TARGET_DS9="${TARGET_DS9:-}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -n "$TARGET_DS9" && -x "$SCRIPT_DIR/preflight_local_infra.sh" ]]; then
  echo "== local infra preflight =="
  if ! ALLOW_FOREIGN_STACK="${ALLOW_FOREIGN_STACK:-0}" bash "$SCRIPT_DIR/preflight_local_infra.sh" "$TARGET_DS9"; then
    missing=1
  fi
  echo
fi

echo "== listeners =="
lsof_args=()
for port in $REQUIRED_PORTS; do
  lsof_args+=("-iTCP:${port}")
done
lsof_args+=("-iTCP:${UI_PORT}" "-iTCP:5174" "-sTCP:LISTEN")
lsof -nP "${lsof_args[@]}" || true

missing=0
for port in $REQUIRED_PORTS; do
  if ! lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "missing listener on port $port" >&2
    missing=1
  fi
done

if ! lsof -nP -iTCP:"$UI_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "missing listener on UI port $UI_PORT" >&2
  missing=1
fi

if lsof -nP -iTCP:5174 -sTCP:LISTEN >/dev/null 2>&1; then
  echo "warning: unexpected listener on port 5174; authenticated local auth may break if the browser uses that origin" >&2
fi

echo
echo "== positronic-files health =="
curl -fsS http://127.0.0.1:7072/api/health
echo

echo
echo "== local ds9 db =="
if ! DATABASE_URL="$DATABASE_URL" bash "$SCRIPT_DIR/verify_local_ds9_db.sh"; then
  missing=1
fi
echo

echo
echo "== lcars ui origin =="
curl -I -s "http://localhost:${UI_PORT}" | head -n 1 || true
echo

if [[ -n "$AUTH_TOKEN" ]]; then
  echo
  echo "== lcars user detail =="
  curl -i -s http://127.0.0.1:3000/api/user_detail \
    -H "Authorization: Bearer $AUTH_TOKEN"
  echo
  echo
  echo "== lcars projects =="
  curl -s http://127.0.0.1:3000/api/projects \
    -H "Authorization: Bearer $AUTH_TOKEN"
  echo
fi

if [[ "$VERIFY_CHAT_READY" == "1" ]]; then
  echo
  echo "== authenticated chat readiness =="
  if [[ -z "$TARGET_DS9" ]]; then
    echo "VERIFY_CHAT_READY=1 requires TARGET_DS9=/path/to/ds9" >&2
    missing=1
  elif ! bash "$SCRIPT_DIR/assert_chat_ready_via_cdp.sh" "$TARGET_DS9" "http://localhost:${UI_PORT}"; then
    missing=1
  fi
  echo
fi

exit "$missing"

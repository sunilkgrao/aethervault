#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 TARGET_DS9 server|ui" >&2
  exit 1
fi

TARGET_DS9="$1"
MODE="$2"
LCARS_DIR="$TARGET_DS9/lcars"
UI_DIR="$LCARS_DIR/ui"
LCARS_PORT="${LCARS_PORT:-3000}"
UI_PORT="${UI_PORT:-5173}"
UI_HOST="${UI_HOST:-localhost}"

if [[ ! -d "$LCARS_DIR" || ! -d "$UI_DIR" ]]; then
  echo "missing lcars/ui under $TARGET_DS9" >&2
  exit 1
fi

kill_port() {
  local port="$1"
  lsof -ti tcp:"$port" | xargs kill -9 2>/dev/null || true
}

case "$MODE" in
  server)
    cd "$LCARS_DIR"
    nvm use "$(cat ../.node-version)" >/dev/null
    echo "starting lcars backend from $LCARS_DIR on http://localhost:${LCARS_PORT}"
    exec env PORT="$LCARS_PORT" npm run dev:server
    ;;
  ui)
    kill_port "$UI_PORT"
    cd "$UI_DIR"
    echo "starting lcars ui from $UI_DIR on http://${UI_HOST}:${UI_PORT} -> http://localhost:${LCARS_PORT}"
    exec env VITE_LOCAL_API_ORIGIN="http://localhost:${LCARS_PORT}" npm run dev -- --host "$UI_HOST" --port "$UI_PORT" --strictPort
    ;;
  *)
    echo "invalid mode: $MODE (expected server or ui)" >&2
    exit 1
    ;;
esac

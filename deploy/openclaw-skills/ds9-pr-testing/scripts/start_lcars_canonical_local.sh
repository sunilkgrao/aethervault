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
    echo "starting lcars backend from $LCARS_DIR"
    exec npm run dev:server
    ;;
  ui)
    # Auth0 local dev only works reliably when the UI origin is the canonical
    # localhost callback origin. Clear stale Vite listeners first so strictPort
    # can enforce that origin.
    kill_port 5173
    kill_port 5174
    cd "$UI_DIR"
    echo "starting lcars ui from $UI_DIR on http://localhost:5173"
    exec npm run dev -- --host localhost --port 5173 --strictPort
    ;;
  *)
    echo "invalid mode: $MODE (expected server or ui)" >&2
    exit 1
    ;;
esac

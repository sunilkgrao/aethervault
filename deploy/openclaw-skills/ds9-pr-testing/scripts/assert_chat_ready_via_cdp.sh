#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 TARGET_DS9 [BASE_URL]" >&2
  exit 1
fi

TARGET_DS9="$1"
BASE_URL="${2:-http://localhost:5173}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$TARGET_DS9"
UI_DIR="$TARGET_DS9/lcars/ui"
SCREENSHOT_PATH="${ARTIFACT_DIR:-/tmp/ds9-pr-test-artifacts}/chat-ready.png"

if [[ ! -d "$UI_DIR" ]]; then
  echo "lcars/ui directory not found under $TARGET_DS9" >&2
  exit 1
fi

if [[ ! -d "$ROOT_DIR/node_modules/playwright" && ! -d "$ROOT_DIR/node_modules/playwright-core" && ! -d "$UI_DIR/node_modules/playwright" && ! -d "$UI_DIR/node_modules/playwright-core" ]]; then
  echo "playwright is not installed under $ROOT_DIR/node_modules or $UI_DIR/node_modules" >&2
  exit 1
fi

bash "$SCRIPT_DIR/ensure_windows_chrome_cdp_bridge.sh" >/tmp/linus_cdp_bridge.log
ENDPOINT="$(tr -d '\r\n' </tmp/linus_chrome_cdp_endpoint.txt)"

if [[ -z "$ENDPOINT" ]]; then
  echo "failed to resolve CDP endpoint" >&2
  exit 1
fi

mkdir -p "$(dirname "$SCREENSHOT_PATH")"

(
  cd "$ROOT_DIR"
  node "$SCRIPT_DIR/check_chat_ready_via_cdp.js" "$ENDPOINT" "$BASE_URL" "$SCREENSHOT_PATH"
)

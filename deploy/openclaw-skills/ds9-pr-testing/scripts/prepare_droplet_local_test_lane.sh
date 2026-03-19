#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 TARGET_DS9 [EMAIL]" >&2
  exit 1
fi

TARGET_DS9="$1"
EMAIL="${2:-sunil@tribble.ai}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

LCARS_PORT="${LCARS_PORT:-$("${SCRIPT_DIR}/find_available_port.sh" 3300 3400 3500 3600 3700 3800)}"
CHAT_PORT="${CHAT_PORT:-$("${SCRIPT_DIR}/find_available_port.sh" 3001 3301 3401 3501 3601 3901)}"
POSITRONIC_FILES_PORT="${POSITRONIC_FILES_PORT:-$("${SCRIPT_DIR}/find_available_port.sh" 7072 7172 7272 7372)}"
UI_PORT="${UI_PORT:-$("${SCRIPT_DIR}/find_available_port.sh" 5173 5273 5373 5473 5573)}"

bash "$SCRIPT_DIR/apply_local_only_auth_overlay.sh" "$TARGET_DS9" "$EMAIL"
bash "$SCRIPT_DIR/enable_local_auth_bypass_env.sh" "$TARGET_DS9" "$EMAIL"
bash "$SCRIPT_DIR/prepare_lcars_local_chat_env.sh" "$TARGET_DS9" "$LCARS_PORT" "$UI_PORT" "$CHAT_PORT"

cat <<EOF
droplet_test_lane_ready=1
target_ds9=${TARGET_DS9}
email=${EMAIL}
lcars_port=${LCARS_PORT}
chat_port=${CHAT_PORT}
positronic_files_port=${POSITRONIC_FILES_PORT}
ui_port=${UI_PORT}

export LCARS_PORT=${LCARS_PORT}
export CHAT_PORT=${CHAT_PORT}
export POSITRONIC_FILES_PORT=${POSITRONIC_FILES_PORT}
export UI_PORT=${UI_PORT}
export AUTH_TOKEN=local-dev-token.${EMAIL//@/__at__}

next_steps:
  1. start lcars server with LCARS_PORT=${LCARS_PORT}
  2. start lcars ui with UI_PORT=${UI_PORT}
  3. verify with verify_stack.sh using AUTH_TOKEN and the same ports

important:
  - these DS9 worktree edits are local-only and must never be committed
  - run revert_local_only_auth_overlay.sh before preparing a PR branch
EOF

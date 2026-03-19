#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 TARGET_DS9" >&2
  exit 1
fi

TARGET_DS9="$1"
ALLOW_FOREIGN_STACK="${ALLOW_FOREIGN_STACK:-0}"
LCARS_PORT="${LCARS_PORT:-3000}"
CHAT_PORT="${CHAT_PORT:-3001}"
POSITRONIC_FILES_PORT="${POSITRONIC_FILES_PORT:-7072}"
UI_PORT="${UI_PORT:-5173}"
UI_FALLBACK_PORT="${UI_FALLBACK_PORT:-5174}"
CHECK_PORTS="${CHECK_PORTS:-50051 50061 ${LCARS_PORT} 3091 ${POSITRONIC_FILES_PORT} ${CHAT_PORT} ${UI_PORT} ${UI_FALLBACK_PORT}}"
MIN_WATCHERS="${MIN_WATCHERS:-524288}"
MIN_INSTANCES="${MIN_INSTANCES:-1024}"

if [[ ! -d "$TARGET_DS9" ]]; then
  echo "TARGET_DS9 does not exist: $TARGET_DS9" >&2
  exit 1
fi

if [[ ! -d "$TARGET_DS9/.git" && ! -f "$TARGET_DS9/.git" ]]; then
  echo "TARGET_DS9 is not a git checkout: $TARGET_DS9" >&2
  exit 1
fi

foreign=0

echo "== target checkout =="
printf 'target_ds9=%s\n' "$TARGET_DS9"
git -C "$TARGET_DS9" rev-parse --abbrev-ref HEAD 2>/dev/null | sed 's/^/branch=/'
printf 'lcars_port=%s\n' "$LCARS_PORT"
printf 'chat_port=%s\n' "$CHAT_PORT"
printf 'positronic_files_port=%s\n' "$POSITRONIC_FILES_PORT"
printf 'ui_port=%s\n' "$UI_PORT"
echo

echo "== canonical port owners =="
for port in $CHECK_PORTS; do
  if ! lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
    continue
  fi

  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    pid="$(awk '{print $2}' <<<"$line")"
    cmd="$(awk '{print $1}' <<<"$line")"
    cwd=""
    if [[ -n "$pid" && -d "/proc/$pid" ]]; then
      cwd="$(readlink "/proc/$pid/cwd" 2>/dev/null || true)"
    fi
    printf 'port=%s pid=%s cmd=%s cwd=%s\n' "$port" "$pid" "$cmd" "${cwd:-unknown}"
    if [[ -n "$cwd" && "$cwd" != "$TARGET_DS9"* ]]; then
      foreign=1
    fi
  done < <(lsof -nP -iTCP:"$port" -sTCP:LISTEN | awk 'NR>1')
done
echo

if [[ "$foreign" == "1" ]]; then
  echo "warning: one or more canonical DS9 ports are owned by a process outside $TARGET_DS9" >&2
  echo "This usually means a different DS9 checkout or stale source stack is occupying the branch-testing ports." >&2
  echo "Decide explicitly:" >&2
  echo "  1. stop the foreign stack and start the branch stack, or" >&2
  echo "  2. set ALLOW_FOREIGN_STACK=1 if you intentionally want to reuse the existing stack" >&2
  if [[ "$ALLOW_FOREIGN_STACK" != "1" ]]; then
    exit 1
  fi
fi

if [[ "$(uname -s)" == "Linux" ]] && command -v sysctl >/dev/null 2>&1; then
  echo "== linux dev limits =="
  current_watchers="$(sysctl -n fs.inotify.max_user_watches 2>/dev/null || echo 0)"
  current_instances="$(sysctl -n fs.inotify.max_user_instances 2>/dev/null || echo 0)"
  current_nofile="$(ulimit -n 2>/dev/null || echo 0)"
  printf 'fs.inotify.max_user_watches=%s\n' "$current_watchers"
  printf 'fs.inotify.max_user_instances=%s\n' "$current_instances"
  printf 'ulimit_nofile=%s\n' "$current_nofile"
  if (( current_watchers < MIN_WATCHERS )); then
    echo "warning: inotify max_user_watches is low; repeated Vite/lcars restarts may die with ENOSPC" >&2
    echo "suggested: sudo sysctl -w fs.inotify.max_user_watches=$MIN_WATCHERS" >&2
  fi
  if (( current_instances < MIN_INSTANCES )); then
    echo "warning: inotify max_user_instances is low; repeated dev servers may die with ENOSPC" >&2
    echo "suggested: sudo sysctl -w fs.inotify.max_user_instances=$MIN_INSTANCES" >&2
  fi
  echo
fi

echo "== install surface =="
for dir in "$TARGET_DS9/node_modules" "$TARGET_DS9/lcars/node_modules" "$TARGET_DS9/lcars/ui/node_modules"; do
  printf '%s=%s\n' "$dir" "$( [[ -d "$dir" ]] && echo present || echo missing )"
done
echo

echo "== guidance =="
echo "If this bug is tied to a spreadsheet/E2E questionnaire, local content_detail rows alone are not enough."
echo "You also need the matching e2e_workbook/e2e_sheet/e2e_answer_entry data shape."
echo "If that exact shape is missing locally, prefer an approved readonly production-data clone over hand-building partial workbook state."

#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: $0 SOURCE_DS9 TARGET_DS9 [BRANCH_OR_REF]" >&2
  exit 1
fi

SOURCE_DS9="$1"
TARGET_DS9="$2"
BRANCH_OR_REF="${3:-}"

if [[ ! -d "$SOURCE_DS9/.git" && ! -f "$SOURCE_DS9/.git" ]]; then
  echo "SOURCE_DS9 is not a git checkout: $SOURCE_DS9" >&2
  exit 1
fi

mkdir -p "$TARGET_DS9"

if [[ -n "$BRANCH_OR_REF" && ! -e "$TARGET_DS9/.git" ]]; then
  (
    cd "$SOURCE_DS9"
    git fetch origin
    git worktree add "$TARGET_DS9" "$BRANCH_OR_REF"
  )
fi

if [[ ! -e "$TARGET_DS9/.git" ]]; then
  echo "TARGET_DS9 is not a git checkout and no branch/ref was supplied: $TARGET_DS9" >&2
  exit 1
fi

copied=0
while IFS= read -r -d '' rel; do
  rel="${rel#./}"
  mkdir -p "$TARGET_DS9/$(dirname "$rel")"
  cp "$SOURCE_DS9/$rel" "$TARGET_DS9/$rel"
  printf 'copied %s\n' "$rel"
  copied=$((copied + 1))
done < <(
  cd "$SOURCE_DS9" &&
    find . \
      \( -path './node_modules' -o -path './.git' -o -path './.claude' \) -prune -o \
      \( -name '.env' -o -name '.env.*' -o -name 'local.settings.json' -o -name 'local.settings.*.json' \) \
      ! -name '*.sample' \
      -type f \
      -print0
)

printf 'copied_count=%s\n' "$copied"

for rel in .node-version lcars/.node-version Q/.node-version; do
  if [[ -f "$TARGET_DS9/$rel" ]]; then
    printf '%s=%s\n' "$rel" "$(cat "$TARGET_DS9/$rel")"
  fi
done

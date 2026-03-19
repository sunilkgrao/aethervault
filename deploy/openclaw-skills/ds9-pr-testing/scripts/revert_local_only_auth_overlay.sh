#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 TARGET_DS9" >&2
  exit 1
fi

TARGET_DS9="$1"
BACKUP_ROOT="$TARGET_DS9/.linus-local-only-auth-backup"

if [[ ! -d "$BACKUP_ROOT" ]]; then
  echo "no local-only auth overlay backup found at $BACKUP_ROOT" >&2
  exit 1
fi

while IFS= read -r -d '' backup_path; do
  rel="${backup_path#"$BACKUP_ROOT"/}"
  target_path="$TARGET_DS9/$rel"
  mkdir -p "$(dirname "$target_path")"
  cp "$backup_path" "$target_path"
  printf 'restored %s\n' "$rel"
done < <(find "$BACKUP_ROOT" -type f -print0)

if [[ ! -f "$BACKUP_ROOT/lcars/ui/src/dev/localAuth0.tsx" ]]; then
  rm -f "$TARGET_DS9/lcars/ui/src/dev/localAuth0.tsx"
fi

rm -rf "$BACKUP_ROOT"
echo "reverted local-only auth overlay in $TARGET_DS9"

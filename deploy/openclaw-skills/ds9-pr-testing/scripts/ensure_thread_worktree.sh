#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 || $# -gt 4 ]]; then
  echo "usage: $0 SOURCE_DS9 WORKTREE_ROOT THREAD_KEY [ISSUE_SLUG]" >&2
  exit 1
fi

SOURCE_DS9="$1"
WORKTREE_ROOT="$2"
THREAD_KEY_RAW="$3"
ISSUE_SLUG_RAW="${4:-issue}"

sanitize() {
  printf '%s' "$1" \
    | tr '[:upper:]' '[:lower:]' \
    | sed -E 's/[^a-z0-9]+/-/g; s/^-+//; s/-+$//; s/-+/-/g'
}

THREAD_KEY="$(sanitize "$THREAD_KEY_RAW")"
ISSUE_SLUG="$(sanitize "$ISSUE_SLUG_RAW")"

if [[ -z "$THREAD_KEY" ]]; then
  echo "THREAD_KEY must contain at least one alphanumeric character" >&2
  exit 1
fi

if [[ -z "$ISSUE_SLUG" ]]; then
  ISSUE_SLUG="issue"
fi

if [[ ! -d "$SOURCE_DS9/.git" && ! -f "$SOURCE_DS9/.git" ]]; then
  echo "SOURCE_DS9 is not a git checkout: $SOURCE_DS9" >&2
  exit 1
fi

BRANCH_NAME="linus/${THREAD_KEY}/${ISSUE_SLUG}"
TARGET_DS9="${WORKTREE_ROOT}/${THREAD_KEY}-${ISSUE_SLUG}"

mkdir -p "$WORKTREE_ROOT"

(
  cd "$SOURCE_DS9"
  git fetch origin
  git worktree prune

  if [[ -e "$TARGET_DS9/.git" ]]; then
    :
  elif git show-ref --verify --quiet "refs/heads/${BRANCH_NAME}"; then
    git worktree add "$TARGET_DS9" "$BRANCH_NAME"
  else
    git worktree add -b "$BRANCH_NAME" "$TARGET_DS9" origin/main
  fi
)

git -C "$TARGET_DS9" branch --set-upstream-to=origin/main "$BRANCH_NAME" >/dev/null 2>&1 || true

printf 'source_ds9=%s\n' "$SOURCE_DS9"
printf 'worktree_root=%s\n' "$WORKTREE_ROOT"
printf 'thread_key=%s\n' "$THREAD_KEY"
printf 'issue_slug=%s\n' "$ISSUE_SLUG"
printf 'branch_name=%s\n' "$BRANCH_NAME"
printf 'target_ds9=%s\n' "$TARGET_DS9"
printf 'base_ref=origin/main\n'
printf 'head_commit=%s\n' "$(git -C "$TARGET_DS9" rev-parse HEAD)"

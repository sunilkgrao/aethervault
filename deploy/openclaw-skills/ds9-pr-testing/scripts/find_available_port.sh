#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 PORT [PORT...]" >&2
  exit 1
fi

for port in "$@"; do
  if ! lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "$port"
    exit 0
  fi
done

echo "no candidate ports are free: $*" >&2
exit 1

#!/usr/bin/env bash
set -euo pipefail

AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
AETHERVAULT_BIN="${AETHERVAULT_BIN:-/usr/local/bin/aethervault}"
WORKSPACE="${AETHERVAULT_WORKSPACE:-$AETHERVAULT_HOME/workspace}"
MV2="${CAPSULE_PATH:-${AETHERVAULT_MV2:-$AETHERVAULT_HOME/memory.mv2}}"
REPO_DIR="${AETHERVAULT_REPO:-$(cd "$(dirname "$0")/.." && pwd)}"

exec "$AETHERVAULT_BIN" swarm-monitor --workspace "$WORKSPACE" --mv2 "$MV2" --repo "$REPO_DIR" "$@"

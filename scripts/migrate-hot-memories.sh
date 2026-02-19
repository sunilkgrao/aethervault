#!/usr/bin/env bash
set -euo pipefail
source /root/.cargo/env 2>/dev/null || true

MV2="${AETHERVAULT_MV2:-/root/.aethervault/memory.mv2}"
JSONL="${AETHERVAULT_HOT_MEMORIES:-/root/.aethervault/data/hot-memories.jsonl}"

if [ ! -f "$JSONL" ]; then
    echo "No hot-memories.jsonl found at $JSONL — nothing to migrate."
    exit 0
fi

echo "Backing up capsule..."
cp "$MV2" "${MV2}.pre-migration"

echo "Running migration..."
aethervault migrate-hot-memories --jsonl "$JSONL" "$MV2"

echo "Archiving original JSONL..."
mv "$JSONL" "${JSONL}.migrated"

echo "Migration complete. Original JSONL backed up to ${JSONL}.migrated"

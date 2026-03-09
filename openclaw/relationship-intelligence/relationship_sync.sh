#!/usr/bin/env bash
set -euo pipefail

DB_PATH="${DB_PATH:-/root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite}"
MEMORY_DIR="${MEMORY_DIR:-/root/.openclaw/workspace/memory}"
TOP_N="${TOP_N:-220}"
MEMORY_FILE="${MEMORY_FILE:-/root/.openclaw/workspace/MEMORY.md}"
PROMPT_RECONNECT_LIMIT="${PROMPT_RECONNECT_LIMIT:-5}"
PROMPT_LOOP_LIMIT="${PROMPT_LOOP_LIMIT:-5}"

python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db "$DB_PATH" \
  render \
  --memory-dir "$MEMORY_DIR" \
  --top-n "$TOP_N"

tmp_block="$(mktemp)"
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db "$DB_PATH" \
  prompt-block \
  --reconnect-limit "$PROMPT_RECONNECT_LIMIT" \
  --loop-limit "$PROMPT_LOOP_LIMIT" > "$tmp_block"

python3 - "$MEMORY_FILE" "$tmp_block" <<'PY'
import sys
from pathlib import Path

memory_path = Path(sys.argv[1])
block_path = Path(sys.argv[2])
start = "<!-- REL_INTEL:START -->"
end = "<!-- REL_INTEL:END -->"
block = block_path.read_text(encoding="utf-8").rstrip()
wrapped = f"{start}\n{block}\n{end}"
text = memory_path.read_text(encoding="utf-8")
if start in text and end in text:
    before, _, rest = text.partition(start)
    _, _, after = rest.partition(end)
    text = before.rstrip() + "\n\n" + wrapped + "\n" + after.lstrip("\n")
else:
    text = text.rstrip() + "\n\n" + wrapped + "\n"
memory_path.write_text(text, encoding="utf-8")
PY

rm -f "$tmp_block"

openclaw memory index --agent main --force

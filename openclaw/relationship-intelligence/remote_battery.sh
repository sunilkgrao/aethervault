#!/usr/bin/env bash
set -euo pipefail

REMOTE_HOST="${REMOTE_HOST:-root@167.172.140.221}"
REMOTE_OPENCLAW_HOME="${REMOTE_OPENCLAW_HOME:-/root/.openclaw}"

ssh "$REMOTE_HOST" 'bash -s' <<'REMOTE'
set -euo pipefail

REMOTE_OPENCLAW_HOME="${REMOTE_OPENCLAW_HOME:-/root/.openclaw}"
STAMP="$(date +%Y%m%d-%H%M%S)"
BASE="/tmp/openclaw-battery-$STAMP"
HOME_ROOT="$BASE/home"
OPENCLAW_HOME="$HOME_ROOT/.openclaw"
REPORT_DIR="$REMOTE_OPENCLAW_HOME/reports"
REPORT_PATH="$REPORT_DIR/openclaw-relationship-battery-$STAMP.md"

mkdir -p "$OPENCLAW_HOME" "$OPENCLAW_HOME/agents/main/agent" "$OPENCLAW_HOME/agents/main/sessions" "$REPORT_DIR"
cp "$REMOTE_OPENCLAW_HOME/openclaw.json" "$OPENCLAW_HOME/"
cp -R "$REMOTE_OPENCLAW_HOME/workspace" "$OPENCLAW_HOME/"
cp "$REMOTE_OPENCLAW_HOME/agents/main/agent/auth-profiles.json" "$OPENCLAW_HOME/agents/main/agent/"
if [ -f "$REMOTE_OPENCLAW_HOME/models.json" ]; then
  cp "$REMOTE_OPENCLAW_HOME/models.json" "$OPENCLAW_HOME/"
fi

python3 - "$OPENCLAW_HOME/openclaw.json" "$OPENCLAW_HOME/workspace" <<'PY'
import json
import sys

config_path = sys.argv[1]
workspace_path = sys.argv[2]
with open(config_path, "r", encoding="utf-8") as fh:
    obj = json.load(fh)
obj.setdefault("agents", {}).setdefault("defaults", {})["workspace"] = workspace_path
for channel in ("telegram", "slack"):
    obj.setdefault("channels", {}).setdefault(channel, {})["enabled"] = False
with open(config_path, "w", encoding="utf-8") as fh:
    json.dump(obj, fh, indent=2)
PY

printf "{}\n" > "$OPENCLAW_HOME/agents/main/sessions/sessions.json"
export HOME="$HOME_ROOT"
export ANTHROPIC_API_KEY="$(jq -r '.profiles["anthropic:default"].key' "$REMOTE_OPENCLAW_HOME/agents/main/agent/auth-profiles.json")"

run_case() {
  local session_id="$1"
  local slug="$2"
  local prompt="$3"
  local output_json="$BASE/$slug.json"
  openclaw agent --local --json --session-id "$session_id" -m "$prompt" > "$output_json"
}

run_case "rel-battery-family" "01-parents" "Who are my parents?"
run_case "rel-battery-family" "02-parent-travel" "I need to find flights for my parents to visit me."
run_case "rel-battery-rhaine" "03-rhaine" "Who is Rhaine and when should I use her?"
run_case "rel-battery-network" "04-reconnect" "Who should I reach out to this week and why?"

python3 - "$BASE" "$REPORT_PATH" <<'PY'
import json
import sys
from pathlib import Path

base = Path(sys.argv[1])
report_path = Path(sys.argv[2])
cases = [
    ("parents_identity", "01-parents.json"),
    ("parent_travel_discovery", "02-parent-travel.json"),
    ("rhaine_role", "03-rhaine.json"),
    ("weekly_reconnects", "04-reconnect.json"),
]

lines = [
    "# OpenClaw Relationship Battery",
    "",
]
for name, filename in cases:
    payload = json.loads((base / filename).read_text(encoding="utf-8"))
    text = ""
    if payload.get("payloads"):
        text = payload["payloads"][0].get("text") or ""
    duration = payload.get("meta", {}).get("durationMs")
    model = payload.get("meta", {}).get("agentMeta", {}).get("model")
    stop = payload.get("meta", {}).get("stopReason")
    lines.extend(
        [
            f"## {name}",
            "",
            f"- model: {model or 'unknown'}",
            f"- duration_ms: {duration or 'unknown'}",
            f"- stop_reason: {stop or 'unknown'}",
            "",
            text.strip() or "_No text payload returned._",
            "",
        ]
    )

report_path.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8")
print(report_path)
PY

rm -rf "$BASE"
REMOTE

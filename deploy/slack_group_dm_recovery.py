#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import subprocess
import time
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


MASTER_ENV = Path("/root/.secrets/master.env")
STATE_PATH = Path("/root/.openclaw/slack-group-dm-recovery-state.json")
POLL_SECONDS = 15
RECENT_WINDOW_SECS = 3600
HISTORY_LIMIT = 30
MAX_GROUP_MESSAGES = 12


def log(message: str) -> None:
    print(message, flush=True)


def load_env_file(path: Path) -> None:
    if not path.exists():
        return
    for raw in path.read_text(errors="ignore").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        os.environ.setdefault(key, value)


def slack_api(method: str, params: dict[str, str]) -> dict[str, Any]:
    token = os.environ["SLACK_BOT_TOKEN"]
    query = urllib.parse.urlencode(params)
    req = urllib.request.Request(
        f"https://slack.com/api/{method}?{query}",
        headers={"Authorization": f"Bearer {token}"},
    )
    with urllib.request.urlopen(req, timeout=60) as response:
        data = json.load(response)
    if not data.get("ok"):
        raise RuntimeError(f"Slack API {method} failed: {data}")
    return data


def slack_post_message(channel: str, text: str) -> str:
    token = os.environ["SLACK_BOT_TOKEN"]
    payload = json.dumps(
        {
            "channel": channel,
            "text": text,
            "unfurl_links": False,
            "unfurl_media": False,
        }
    ).encode()
    req = urllib.request.Request(
        "https://slack.com/api/chat.postMessage",
        data=payload,
        method="POST",
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json; charset=utf-8",
        },
    )
    with urllib.request.urlopen(req, timeout=60) as response:
        data = json.load(response)
    if not data.get("ok"):
        raise RuntimeError(f"Slack API chat.postMessage failed: {data}")
    return str(data["ts"])


def load_state() -> dict[str, Any]:
    if not STATE_PATH.exists():
        return {"handled": {}}
    try:
        return json.loads(STATE_PATH.read_text())
    except Exception:
        return {"handled": {}}


def save_state(state: dict[str, Any]) -> None:
    STATE_PATH.parent.mkdir(parents=True, exist_ok=True)
    STATE_PATH.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n")


def ts_value(value: str | None) -> float:
    if not value:
        return 0.0
    try:
        return float(value)
    except Exception:
        return 0.0


def auth_test() -> dict[str, Any]:
    return slack_api("auth.test", {})


def discover_mpim_ids() -> list[str]:
    ids: set[str] = set()
    cursor = ""
    while True:
        params = {
            "types": "mpim",
            "exclude_archived": "true",
            "limit": "200",
        }
        if cursor:
            params["cursor"] = cursor
        listed = slack_api("conversations.list", params)
        for convo in listed.get("channels", []):
            if convo.get("is_mpim") and convo.get("is_member") and convo.get("id"):
                ids.add(str(convo["id"]))
        cursor = str(listed.get("response_metadata", {}).get("next_cursor") or "")
        if not cursor:
            break
    return sorted(ids)


def latest_group_dm_mention(channel_id: str, bot_user_id: str) -> dict[str, Any] | None:
    history = slack_api("conversations.history", {"channel": channel_id, "limit": str(HISTORY_LIMIT)})
    now = time.time()
    for message in history.get("messages", []):
        if not isinstance(message, dict):
            continue
        if message.get("user") == bot_user_id or message.get("bot_id"):
            continue
        ts = str(message.get("ts") or "")
        if not ts:
            continue
        try:
            if float(ts) < now - RECENT_WINDOW_SECS:
                continue
        except Exception:
            continue
        text = str(message.get("text") or "")
        if f"<@{bot_user_id}>" not in text:
            continue
        return message
    return None


def build_group_dm_prompt(channel_id: str, channel_name: str, messages: list[dict[str, Any]], latest: dict[str, Any]) -> str:
    lines = [
        "You are Linus responding in a Slack group DM.",
        "This is a private small-group conversation, so be direct and useful.",
        "If the latest request asks you to use Codex or Claude, actually do that rather than only describing what you would do.",
        "",
        f"Slack group DM: {channel_name or channel_id}",
        f"Latest message ts: {latest.get('ts', '')}",
        "",
        "Recent conversation:",
    ]
    for item in messages[:MAX_GROUP_MESSAGES]:
        sender = item.get("user") or item.get("bot_id") or "unknown"
        text = str(item.get("text") or "").strip().replace("\n", " ")
        if text:
            lines.append(f"- {sender}: {text[:500]}")
    lines.extend(
        [
            "",
            "Latest user request:",
            str(latest.get("text") or "").strip(),
            "",
            "Reply with exactly the Slack message Linus should send next.",
        ]
    )
    return "\n".join(lines)


def run_agent(prompt: str) -> str:
    proc = subprocess.run(
        [
            "openclaw",
            "agent",
            "--agent",
            "main",
            "--local",
            "--message",
            prompt,
            "--json",
            "--timeout",
            "86400",
        ],
        capture_output=True,
        text=True,
        env=os.environ.copy(),
        timeout=86500,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or proc.stdout.strip() or f"agent failed rc={proc.returncode}")
    data = json.loads(proc.stdout)
    payloads = data.get("payloads", [])
    texts = [str(item.get("text") or "").strip() for item in payloads if isinstance(item, dict) and str(item.get("text") or "").strip()]
    if not texts:
        raise RuntimeError("agent returned no text payload")
    return "\n\n".join(texts)


def main() -> int:
    load_env_file(MASTER_ENV)
    bot_user_id = str(auth_test().get("user_id") or "")
    if not bot_user_id:
        raise SystemExit("Could not resolve Slack bot user id")
    log(f"group-dm-recovery: bot_user_id={bot_user_id}")
    while True:
        state = load_state()
        handled = state.setdefault("handled", {})
        for channel_id in discover_mpim_ids():
            try:
                info = slack_api("conversations.info", {"channel": channel_id})
                channel_name = str(info.get("channel", {}).get("name") or channel_id)
                latest = latest_group_dm_mention(channel_id, bot_user_id)
                if not latest:
                    continue
                latest_ts = str(latest["ts"])
                state_key = f"mpim:{channel_id}"
                if ts_value(handled.get(state_key)) >= ts_value(latest_ts):
                    continue
                history = slack_api("conversations.history", {"channel": channel_id, "limit": str(HISTORY_LIMIT)})
                messages = list(reversed(history.get("messages", [])))
                log(f"group-dm-recovery: handling channel={channel_name} ts={latest_ts}")
                prompt = build_group_dm_prompt(channel_id, channel_name, messages, latest)
                response = run_agent(prompt)
                sent_ts = slack_post_message(channel_id, response)
                handled[state_key] = latest_ts
                save_state(state)
                log(f"group-dm-recovery: posted reply ts={sent_ts}")
            except Exception as err:
                log(f"group-dm-recovery: channel {channel_id} failed: {err}")
        time.sleep(POLL_SECONDS)


if __name__ == "__main__":
    raise SystemExit(main())

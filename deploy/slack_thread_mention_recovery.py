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
SESSIONS_JSON = Path("/root/.openclaw/agents/main/sessions/sessions.json")
STATE_PATH = Path("/root/.openclaw/slack-thread-mention-recovery-state.json")
MEDIA_PACKET_SCRIPT = Path(
    "/root/.openclaw/workspace/skills/slack-media-analysis/scripts/build_slack_media_packet.py"
)
MEDIA_PACKET_DIR = Path("/tmp/openclaw-slack-thread-recovery")
POLL_SECONDS = 15
RECENT_WINDOW_SECS = 3600
HISTORY_LIMIT = 30
MAX_THREAD_MESSAGES = 12
MAX_GROUP_MESSAGES = 12
MAX_PACKET_SUMMARY_CHARS = 4000


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


def slack_post_message(channel: str, text: str, thread_ts: str | None = None) -> str:
    token = os.environ["SLACK_BOT_TOKEN"]
    body: dict[str, Any] = {
        "channel": channel,
        "text": text,
        "unfurl_links": False,
        "unfurl_media": False,
    }
    if thread_ts:
        body["thread_ts"] = thread_ts
    payload = json.dumps(body).encode()
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


def discover_channel_ids() -> list[str]:
    channels: set[str] = set()
    if SESSIONS_JSON.exists():
        try:
            data = json.loads(SESSIONS_JSON.read_text())
            session_map = data.get("sessions", data)
            if not isinstance(session_map, dict):
                session_map = {}
            for meta in session_map.values():
                if not isinstance(meta, dict):
                    continue
                for value in (meta.get("lastTo"), meta.get("lastRoute", {}).get("to"), meta.get("route", {}).get("to")):
                    if isinstance(value, str) and value.startswith("channel:"):
                        channels.add(value.split(":", 1)[1])
        except Exception as err:
            log(f"thread-recovery: failed to parse sessions.json: {err}")
    return sorted(channels)


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


def auth_test() -> dict[str, Any]:
    return slack_api("auth.test", {})


def recent_thread_candidates(channel_id: str, bot_user_id: str) -> list[dict[str, Any]]:
    history = slack_api("conversations.history", {"channel": channel_id, "limit": str(HISTORY_LIMIT)})
    now = time.time()
    matches: list[dict[str, Any]] = []
    for root in history.get("messages", []):
        root_ts = root.get("ts")
        latest_reply = root.get("latest_reply")
        if not isinstance(root_ts, str) or not isinstance(latest_reply, str):
            continue
        try:
            if float(latest_reply) < now - RECENT_WINDOW_SECS:
                continue
        except Exception:
            continue
        if latest_reply == root_ts:
            continue
        replies = slack_api(
            "conversations.replies",
            {
                "channel": channel_id,
                "ts": root_ts,
                "latest": latest_reply,
                "oldest": latest_reply,
                "inclusive": "true",
                "limit": "1",
            },
        )
        reply = None
        for item in replies.get("messages", []):
            if item.get("ts") == latest_reply:
                reply = item
                break
        if reply is None and replies.get("messages"):
            reply = replies["messages"][-1]
        if not isinstance(reply, dict):
            continue
        if reply.get("user") == bot_user_id or reply.get("bot_id"):
            continue
        text = str(reply.get("text") or "")
        if f"<@{bot_user_id}>" not in text:
            continue
        matches.append({"root": root, "reply": reply})
    return matches


def thread_messages(channel_id: str, thread_ts: str) -> list[dict[str, Any]]:
    replies = slack_api(
        "conversations.replies",
        {"channel": channel_id, "ts": thread_ts, "inclusive": "true", "limit": "100"},
    )
    return list(replies.get("messages", []))


def maybe_build_media_summary(channel_id: str, thread_ts: str) -> str:
    if not MEDIA_PACKET_SCRIPT.exists():
        return ""
    out_dir = MEDIA_PACKET_DIR / f"{channel_id}-{thread_ts.replace('.', '-')}"
    out_dir.parent.mkdir(parents=True, exist_ok=True)
    proc = subprocess.run(
        [
            "python3",
            str(MEDIA_PACKET_SCRIPT),
            "--channel",
            channel_id,
            "--thread-ts",
            thread_ts,
            "--out",
            str(out_dir),
        ],
        capture_output=True,
        text=True,
        timeout=600,
    )
    if proc.returncode != 0:
        log(f"thread-recovery: media packet build failed for {channel_id}/{thread_ts}: {proc.stderr.strip()}")
        return ""
    summary = out_dir / "summary.txt"
    if not summary.exists():
        return ""
    text = summary.read_text(errors="ignore").strip()
    if len(text) > MAX_PACKET_SUMMARY_CHARS:
        text = text[:MAX_PACKET_SUMMARY_CHARS].rstrip() + "\n[truncated]"
    return text


def build_prompt(channel_id: str, channel_name: str, root: dict[str, Any], reply: dict[str, Any], thread: list[dict[str, Any]], media_summary: str) -> str:
    lines = [
        "You are Linus responding in an existing Slack thread.",
        "Keep the reply concise and professional for a shared engineering channel.",
        "If the latest user request asks you to investigate with Codex or Claude, do that rather than just describing what you would do.",
        "",
        f"Slack channel: #{channel_name or channel_id}",
        f"Thread root ts: {root.get('ts', '')}",
        f"Latest message ts: {reply.get('ts', '')}",
        "",
        "Thread summary:",
    ]
    for item in thread[-MAX_THREAD_MESSAGES:]:
        sender = item.get("user") or item.get("bot_id") or "unknown"
        text = str(item.get("text") or "").strip().replace("\n", " ")
        if text:
            lines.append(f"- {sender}: {text[:500]}")
    if media_summary:
        lines.extend(["", "Media packet summary:", media_summary])
    lines.extend(
        [
            "",
            "Latest user request:",
            str(reply.get("text") or "").strip(),
            "",
            "Reply with exactly the Slack thread message Linus should send next.",
        ]
    )
    return "\n".join(lines)


def run_agent(prompt: str) -> str:
    env = os.environ.copy()
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
        env=env,
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


def process_candidate(channel_id: str, channel_name: str, root: dict[str, Any], reply: dict[str, Any]) -> str:
    thread_ts = str(root["ts"])
    thread = thread_messages(channel_id, thread_ts)
    media_summary = maybe_build_media_summary(channel_id, thread_ts)
    prompt = build_prompt(channel_id, channel_name, root, reply, thread, media_summary)
    return run_agent(prompt)


def recent_group_dm_mentions(channel_id: str, bot_user_id: str) -> list[dict[str, Any]]:
    history = slack_api("conversations.history", {"channel": channel_id, "limit": str(HISTORY_LIMIT)})
    now = time.time()
    matches: list[dict[str, Any]] = []
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
        matches.append(message)
        break
    return matches


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


def process_group_dm_candidate(channel_id: str, channel_name: str, latest: dict[str, Any]) -> str:
    history = slack_api("conversations.history", {"channel": channel_id, "limit": str(HISTORY_LIMIT)})
    messages = list(reversed(history.get("messages", [])))
    prompt = build_group_dm_prompt(channel_id, channel_name, messages, latest)
    return run_agent(prompt)


def main() -> int:
    load_env_file(MASTER_ENV)
    auth = auth_test()
    bot_user_id = str(auth.get("user_id") or "")
    if not bot_user_id:
        raise SystemExit("Could not resolve Slack bot user id")
    log(f"thread-recovery: bot_user_id={bot_user_id}")
    while True:
        state = load_state()
        handled = state.setdefault("handled", {})
        channel_ids = discover_channel_ids()
        if channel_ids:
            log(f"thread-recovery: scanning {len(channel_ids)} channels")
        for channel_id in discover_mpim_ids():
            try:
                info = slack_api("conversations.info", {"channel": channel_id})
                channel_name = str(info.get("channel", {}).get("name") or channel_id)
                for message in recent_group_dm_mentions(channel_id, bot_user_id):
                    message_ts = str(message["ts"])
                    state_key = f"mpim:{channel_id}"
                    if ts_value(handled.get(state_key)) >= ts_value(message_ts):
                        continue
                    log(f"thread-recovery: handling group dm mention channel={channel_name} ts={message_ts}")
                    response = process_group_dm_candidate(channel_id, channel_name, message)
                    sent_ts = slack_post_message(channel_id, response)
                    handled[state_key] = message_ts
                    save_state(state)
                    log(f"thread-recovery: posted group dm reply ts={sent_ts}")
            except Exception as err:
                log(f"thread-recovery: group dm {channel_id} failed: {err}")
        for channel_id in channel_ids:
            try:
                info = slack_api("conversations.info", {"channel": channel_id})
                channel_name = str(info.get("channel", {}).get("name") or channel_id)
                for candidate in recent_thread_candidates(channel_id, bot_user_id):
                    root = candidate["root"]
                    reply = candidate["reply"]
                    reply_ts = str(reply["ts"])
                    state_key = f"{channel_id}:{root['ts']}"
                    if handled.get(state_key) == reply_ts:
                        continue
                    log(f"thread-recovery: handling missed mention channel={channel_name} thread={root['ts']} ts={reply_ts}")
                    response = process_candidate(channel_id, channel_name, root, reply)
                    sent_ts = slack_post_message(channel_id, response, thread_ts=str(root["ts"]))
                    handled[state_key] = reply_ts
                    save_state(state)
                    log(f"thread-recovery: posted reply ts={sent_ts}")
            except Exception as err:
                log(f"thread-recovery: channel {channel_id} failed: {err}")
        time.sleep(POLL_SECONDS)


if __name__ == "__main__":
    raise SystemExit(main())

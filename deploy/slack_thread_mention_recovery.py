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
OPENCLAW_DIST = Path("/usr/lib/node_modules/openclaw/dist")
SESSIONS_JSON = Path("/root/.openclaw/agents/main/sessions/sessions.json")
STATE_PATH = Path("/root/.openclaw/slack-thread-mention-recovery-state.json")
MEDIA_PACKET_SCRIPT = Path(
    "/root/.openclaw/workspace/skills/slack-media-analysis/scripts/build_slack_media_packet.py"
)
MEDIA_PACKET_DIR = Path("/tmp/openclaw-slack-thread-recovery")
POLL_SECONDS = 15
RECENT_WINDOW_SECS = 3600
RECOVERY_DELAY_SECS = 20
HISTORY_LIMIT = 30
MAX_THREAD_MESSAGES = 12
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


def auth_test() -> dict[str, Any]:
    return slack_api("auth.test", {})


def gateway_has_builtin_thread_poll() -> bool:
    if not OPENCLAW_DIST.exists():
        return False
    for path in OPENCLAW_DIST.glob("*.js"):
        try:
            text = path.read_text(errors="ignore")
        except Exception:
            continue
        if "const threadMentionPoll = async () => {" in text or "slack thread mention poll armed" in text:
            return True
    return False


def is_bot_authored(message: dict[str, Any], bot_user_id: str) -> bool:
    return bool(message.get("bot_id")) or message.get("user") == bot_user_id


def has_bot_reply_after(messages: list[dict[str, Any]], after_ts: str, bot_user_id: str) -> bool:
    after = ts_value(after_ts)
    for message in messages:
        if not isinstance(message, dict):
            continue
        if not is_bot_authored(message, bot_user_id):
            continue
        if ts_value(str(message.get("ts") or "")) > after:
            return True
    return False


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
            if float(latest_reply) > now - RECOVERY_DELAY_SECS:
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
        if is_bot_authored(reply, bot_user_id):
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
        "Reply only in this same Slack thread, in this same Slack channel. Never move the conversation to another channel.",
        "Keep the reply concise and professional for a shared engineering channel.",
        "Treat shared Slack as an engineering/product surface only.",
        "Do not use or reveal private owner context, family details, household details, health details, personal emails, addresses, birthdays, pets, or unrelated personal information.",
        "If the latest user request asks you to investigate with Codex or Claude, do that rather than just describing what you would do.",
        "Do not send stream-of-consciousness updates, exploration traces, or repeated status pings like 'let me check' or 'now let me try'.",
        "Use at most one concise status or blocker update, or one concise final answer. Prefer the final answer when possible.",
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
    return texts[-1]


def process_candidate(channel_id: str, channel_name: str, root: dict[str, Any], reply: dict[str, Any], thread: list[dict[str, Any]]) -> str:
    thread_ts = str(root["ts"])
    media_summary = maybe_build_media_summary(channel_id, thread_ts)
    prompt = build_prompt(channel_id, channel_name, root, reply, thread, media_summary)
    return run_agent(prompt)


def main() -> int:
    load_env_file(MASTER_ENV)
    auth = auth_test()
    bot_user_id = str(auth.get("user_id") or "")
    if not bot_user_id:
        raise SystemExit("Could not resolve Slack bot user id")
    log(f"thread-recovery: bot_user_id={bot_user_id}")
    while True:
        if gateway_has_builtin_thread_poll():
            log("thread-recovery: builtin gateway thread poll detected; worker idle")
            time.sleep(max(POLL_SECONDS, 300))
            continue
        state = load_state()
        handled = state.setdefault("handled", {})
        channel_ids = discover_channel_ids()
        if channel_ids:
            log(f"thread-recovery: scanning {len(channel_ids)} channels")
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
                    thread = thread_messages(channel_id, str(root["ts"]))
                    if has_bot_reply_after(thread, reply_ts, bot_user_id):
                        handled[state_key] = reply_ts
                        save_state(state)
                        log(
                            f"thread-recovery: skipping channel={channel_name} thread={root['ts']} ts={reply_ts} because bot already replied"
                        )
                        continue
                    log(f"thread-recovery: handling missed mention channel={channel_name} thread={root['ts']} ts={reply_ts}")
                    response = process_candidate(channel_id, channel_name, root, reply, thread)
                    sent_ts = slack_post_message(channel_id, response, thread_ts=str(root["ts"]))
                    handled[state_key] = reply_ts
                    save_state(state)
                    log(f"thread-recovery: posted reply ts={sent_ts}")
            except Exception as err:
                log(f"thread-recovery: channel {channel_id} failed: {err}")
        time.sleep(POLL_SECONDS)


if __name__ == "__main__":
    raise SystemExit(main())

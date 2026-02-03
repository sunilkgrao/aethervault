#!/usr/bin/env python3
"""Telegram long-polling bridge (stdlib-only)."""
import json
import os
import time
import urllib.parse
import urllib.request

from agent_runner import run_with_subagents

TOKEN = os.environ.get("TELEGRAM_BOT_TOKEN")
if not TOKEN:
    raise SystemExit("Missing TELEGRAM_BOT_TOKEN")

API_BASE = f"https://api.telegram.org/bot{TOKEN}"
POLL_TIMEOUT = int(os.environ.get("TELEGRAM_POLL_TIMEOUT", "50"))


def api_get(method, params=None):
    params = params or {}
    url = f"{API_BASE}/{method}?{urllib.parse.urlencode(params)}"
    with urllib.request.urlopen(url, timeout=POLL_TIMEOUT + 5) as resp:
        return json.loads(resp.read().decode("utf-8"))


def api_post(method, payload):
    data = urllib.parse.urlencode(payload).encode("utf-8")
    req = urllib.request.Request(f"{API_BASE}/{method}", data=data, method="POST")
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.loads(resp.read().decode("utf-8"))


def main():
    offset = 0
    while True:
        try:
            updates = api_get("getUpdates", {"timeout": POLL_TIMEOUT, "offset": offset})
            if not updates.get("ok"):
                time.sleep(2)
                continue
            for update in updates.get("result", []):
                offset = max(offset, update.get("update_id", 0) + 1)
                message = update.get("message") or update.get("edited_message")
                if not message:
                    continue
                text = message.get("text") or message.get("caption")
                if not text:
                    continue
                chat_id = message["chat"]["id"]
                session = f"telegram:{chat_id}"
                response, _ = run_with_subagents(text, session)
                api_post("sendMessage", {"chat_id": chat_id, "text": response})
        except Exception:  # noqa: BLE001
            time.sleep(2)


if __name__ == "__main__":
    main()

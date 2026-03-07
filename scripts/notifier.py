#!/usr/bin/env python3
"""Shared outbound notification helpers for AetherVault lifecycle scripts."""

import json
import os
import urllib.error
import urllib.request


TELEGRAM_BOT_TOKEN = os.environ.get("TELEGRAM_BOT_TOKEN", "")
TELEGRAM_CHAT_ID = os.environ.get(
    "TELEGRAM_CHAT_ID",
    os.environ.get("AETHERVAULT_TELEGRAM_CHAT_ID", ""),
)


def split_message(text: str, max_length: int = 4000) -> list[str]:
    if len(text) <= max_length:
        return [text]

    chunks = []
    current = ""
    for line in text.split("\n"):
        if len(current) + len(line) + 1 > max_length:
            if current:
                chunks.append(current.strip())
            current = line + "\n"
        else:
            current += line + "\n"
    if current.strip():
        chunks.append(current.strip())
    return chunks if chunks else [text[:max_length]]


def _send_chunk(message: str, parse_mode: str | None, disable_web_page_preview: bool) -> dict:
    if not TELEGRAM_BOT_TOKEN or not TELEGRAM_CHAT_ID:
        raise RuntimeError("Telegram is not configured")

    payload = {
        "chat_id": TELEGRAM_CHAT_ID,
        "text": message,
        "disable_web_page_preview": disable_web_page_preview,
    }
    if parse_mode:
        payload["parse_mode"] = parse_mode

    req = urllib.request.Request(
        f"https://api.telegram.org/bot{TELEGRAM_BOT_TOKEN}/sendMessage",
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=15) as resp:
        return json.loads(resp.read().decode("utf-8"))


def send_telegram(
    message: str,
    *,
    log,
    parse_mode: str | None = "Markdown",
    disable_web_page_preview: bool = True,
    max_length: int = 4000,
) -> bool:
    if not TELEGRAM_BOT_TOKEN:
        log("TELEGRAM_BOT_TOKEN not set")
        return False
    if not TELEGRAM_CHAT_ID:
        log("TELEGRAM_CHAT_ID not set")
        return False

    for index, chunk in enumerate(split_message(message, max_length=max_length), start=1):
        try:
            result = _send_chunk(chunk, parse_mode, disable_web_page_preview)
            if result.get("ok"):
                log(f"Telegram message sent (chunk {index})")
                continue
            log(f"Telegram API returned not-ok: {result}")
        except urllib.error.HTTPError as exc:
            body = exc.read().decode("utf-8", errors="replace")
            log(f"Telegram API error {exc.code}: {body[:300]}")
        except Exception as exc:
            log(f"Telegram send failed: {exc}")
            return False

        if parse_mode:
            try:
                result = _send_chunk(chunk, None, disable_web_page_preview)
                if result.get("ok"):
                    log(f"Telegram plain-text fallback succeeded (chunk {index})")
                    continue
                log(f"Telegram plain fallback failed: {result}")
            except Exception as exc:
                log(f"Telegram plain fallback failed: {exc}")
        return False

    return True

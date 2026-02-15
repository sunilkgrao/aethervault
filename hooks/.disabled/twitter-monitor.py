#!/usr/bin/env python3
"""AetherVault Twitter Monitor — polls Grok x_search for topics and sends Telegram alerts."""

import json
import os
import sys
import time
import logging
import signal
from datetime import datetime, timezone
from pathlib import Path

import requests

# --- Paths ---
CONFIG_PATH = Path("/root/.aethervault/config/twitter-monitor.json")
SEEN_PATH = Path("/root/.aethervault/data/seen-tweets.json")
ENV_PATH = Path("/root/.aethervault/.env")

# --- Logging ---
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
)
log = logging.getLogger("twitter-monitor")

# --- Globals ---
GROK_ENDPOINT = "https://api.x.ai/v1/responses"
GROK_MODEL = "grok-4-1-fast-non-reasoning"
TELEGRAM_API = "https://api.telegram.org/bot{token}/{method}"

shutdown_requested = False


def handle_signal(signum, frame):
    global shutdown_requested
    log.info("Received signal %s, shutting down gracefully...", signum)
    shutdown_requested = True


signal.signal(signal.SIGTERM, handle_signal)
signal.signal(signal.SIGINT, handle_signal)


def load_env():
    """Load env vars from .env file (simple KEY=VALUE format)."""
    if ENV_PATH.exists():
        for line in ENV_PATH.read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                key, _, value = line.partition("=")
                os.environ.setdefault(key.strip(), value.strip())


def load_config():
    """Load monitor config from JSON."""
    with open(CONFIG_PATH) as f:
        return json.load(f)


def save_config(config):
    """Save config back (e.g., after discovering chat_id)."""
    with open(CONFIG_PATH, "w") as f:
        json.dump(config, f, indent=2)
    log.info("Config saved to %s", CONFIG_PATH)


def load_seen():
    """Load set of previously seen tweet URLs."""
    if SEEN_PATH.exists():
        try:
            data = json.loads(SEEN_PATH.read_text())
            return set(data.get("urls", []))
        except (json.JSONDecodeError, KeyError):
            return set()
    return set()


def save_seen(seen_set):
    """Persist seen URLs. Keep only the most recent 2000 to avoid unbounded growth."""
    urls = list(seen_set)
    if len(urls) > 2000:
        urls = urls[-2000:]
    SEEN_PATH.write_text(json.dumps({"urls": urls, "updated": datetime.now(timezone.utc).isoformat()}))


def discover_chat_id(token):
    """Try to discover chat_id from Telegram getUpdates."""
    log.info("Attempting chat_id discovery via getUpdates...")
    url = TELEGRAM_API.format(token=token, method="getUpdates")
    try:
        resp = requests.get(url, params={"limit": 10}, timeout=15)
        resp.raise_for_status()
        data = resp.json()
        results = data.get("result", [])
        for update in reversed(results):
            msg = update.get("message") or update.get("channel_post") or {}
            chat = msg.get("chat", {})
            chat_id = chat.get("id")
            if chat_id:
                log.info("Discovered chat_id: %s (chat: %s)", chat_id, chat.get("title") or chat.get("first_name"))
                return chat_id
    except Exception as e:
        log.warning("getUpdates failed: %s", e)

    log.warning("Could not discover chat_id from getUpdates. Will retry next cycle.")
    return None


def send_telegram(token, chat_id, text):
    """Send a message via Telegram Bot API."""
    url = TELEGRAM_API.format(token=token, method="sendMessage")
    payload = {
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "HTML",
        "disable_web_page_preview": True,
    }
    try:
        resp = requests.post(url, json=payload, timeout=15)
        resp.raise_for_status()
        log.info("Telegram message sent to chat %s", chat_id)
        return True
    except Exception as e:
        log.error("Failed to send Telegram message: %s", e)
        return False


def search_grok(api_key, query, handles=None):
    """Call Grok Responses API with x_search tool."""
    tool = {"type": "x_search"}
    if handles:
        tool["allowed_x_handles"] = handles

    payload = {
        "model": GROK_MODEL,
        "input": [
            {
                "role": "user",
                "content": (
                    f"Search Twitter/X for the most recent and noteworthy posts about: {query}\n\n"
                    "For each significant post found, provide:\n"
                    "- The author handle\n"
                    "- A brief summary of the post\n"
                    "- The URL to the post (https://x.com/... format)\n\n"
                    "Focus on posts from the last few hours. If nothing notable, say 'No significant posts found.'"
                ),
            }
        ],
        "tools": [tool],
    }

    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {api_key}",
    }

    try:
        resp = requests.post(GROK_ENDPOINT, headers=headers, json=payload, timeout=90)
        resp.raise_for_status()
        result = resp.json()

        # Extract text from Responses API output
        texts = []
        for item in result.get("output", []):
            if item.get("type") == "message" and item.get("role") == "assistant":
                for content in item.get("content", []):
                    if content.get("type") == "output_text":
                        texts.append(content["text"])

        if texts:
            return "\n\n".join(texts)
        return None
    except requests.exceptions.HTTPError as e:
        detail = ""
        try:
            detail = e.response.text[:500]
        except Exception:
            pass
        log.error("Grok API error for query '%s': %s %s", query, e, detail)
        return None
    except Exception as e:
        log.error("Grok search failed for query '%s': %s", query, e)
        return None


def extract_urls(text):
    """Extract x.com/twitter.com URLs from text."""
    import re
    urls = set()
    for match in re.finditer(r'https?://(?:x\.com|twitter\.com)/\S+', text):
        url = match.group(0).rstrip('.,;:!?)"\'>]')
        urls.add(url)
    return urls


def process_topic(api_key, topic, handles, seen):
    """Search for a topic and return (alert_text, new_urls) if noteworthy."""
    query = topic["query"]
    priority = topic.get("priority", "medium")
    log.info("Searching: '%s' (priority: %s)", query, priority)

    result_text = search_grok(api_key, query, handles if priority == "high" else None)
    if not result_text:
        return None, set()

    # Check for "no significant posts" indicator
    lower = result_text.lower()
    if "no significant posts found" in lower or "no noteworthy" in lower or "no recent" in lower:
        log.info("No significant posts for '%s'", query)
        return None, set()

    # Extract URLs and deduplicate
    found_urls = extract_urls(result_text)
    new_urls = found_urls - seen

    if not new_urls and found_urls:
        log.info("All %d URLs already seen for '%s'", len(found_urls), query)
        return None, set()

    # Build alert
    priority_tag = {"high": "🔴", "medium": "🟡", "low": "🟢"}.get(priority, "⚪")
    alert = (
        f"{priority_tag} <b>Twitter Alert: {query}</b>\n\n"
        f"{result_text[:3000]}"
    )

    return alert, new_urls


def run_cycle(config, seen):
    """Run one monitoring cycle across all topics."""
    api_key = os.environ.get("XAI_API_KEY") or os.environ.get("GROK_API_KEY")
    token = os.environ.get("TELEGRAM_BOT_TOKEN")
    chat_id = config.get("chat_id")

    if not api_key:
        log.error("No XAI_API_KEY or GROK_API_KEY in environment")
        return seen

    if not token:
        log.error("No TELEGRAM_BOT_TOKEN in environment")
        return seen

    # Try to discover chat_id if not set
    if not chat_id:
        chat_id = discover_chat_id(token)
        if chat_id:
            config["chat_id"] = chat_id
            save_config(config)
        else:
            log.warning("No chat_id available. Skipping Telegram alerts this cycle.")

    topics = config.get("topics", [])
    handles = config.get("handles", [])
    alerts_sent = 0

    for topic in topics:
        if shutdown_requested:
            break

        alert_text, new_urls = process_topic(api_key, topic, handles, seen)

        if alert_text and chat_id:
            if send_telegram(token, chat_id, alert_text):
                alerts_sent += 1
            seen.update(new_urls)
        elif alert_text:
            log.info("Alert generated but no chat_id to send to:\n%s", alert_text[:200])
            seen.update(new_urls)

        # Brief pause between queries to avoid rate limits
        if not shutdown_requested:
            time.sleep(3)

    save_seen(seen)
    log.info("Cycle complete. Alerts sent: %d. Tracked URLs: %d", alerts_sent, len(seen))
    return seen


def main():
    log.info("AetherVault Twitter Monitor starting up")
    load_env()

    if not CONFIG_PATH.exists():
        log.error("Config not found at %s", CONFIG_PATH)
        sys.exit(1)

    config = load_config()
    seen = load_seen()
    interval = config.get("poll_interval_minutes", 15) * 60

    log.info(
        "Config loaded: %d topics, %d handles, interval=%dm, chat_id=%s",
        len(config.get("topics", [])),
        len(config.get("handles", [])),
        config.get("poll_interval_minutes", 15),
        config.get("chat_id") or "auto-discover",
    )

    # Run first cycle immediately
    seen = run_cycle(config, seen)

    while not shutdown_requested:
        log.info("Sleeping %d minutes until next cycle...", interval // 60)
        # Sleep in small increments so we can respond to signals
        sleep_end = time.time() + interval
        while time.time() < sleep_end and not shutdown_requested:
            time.sleep(5)

        if not shutdown_requested:
            # Reload config each cycle (allows hot-reconfiguration)
            try:
                config = load_config()
            except Exception as e:
                log.warning("Failed to reload config: %s", e)
            seen = run_cycle(config, seen)

    log.info("Twitter Monitor shut down cleanly.")


if __name__ == "__main__":
    main()

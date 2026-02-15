#!/usr/bin/env python3
"""
AetherVault Proactive Evening Check-In
=======================================

A lighter companion to the morning briefing. Runs in the evening to surface
anything that needs attention before winding down: unaddressed emails,
incomplete tasks, and a brief status nudge.

Usage:
    python3 /root/.aethervault/hooks/proactive-checkin.py

    # Or with the bash wrapper for cron:
    bash /root/.aethervault/hooks/proactive-checkin.sh

Environment variables (loaded from /root/.aethervault/.env):
    TELEGRAM_BOT_TOKEN   - Telegram bot API token
    ANTHROPIC_API_KEY    - Anthropic API key for Claude

Cron schedule (add via `crontab -e`):
    # Evening check-in: 8 PM daily
    0 20 * * * /root/.aethervault/hooks/proactive-checkin.sh
"""

import json
import os
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timedelta
from pathlib import Path

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

TELEGRAM_BOT_TOKEN = os.environ.get("TELEGRAM_BOT_TOKEN", "")
TELEGRAM_CHAT_ID = os.environ.get("TELEGRAM_CHAT_ID", "")
ANTHROPIC_API_KEY = os.environ.get("ANTHROPIC_API_KEY", "")
CLAUDE_API_URL = os.environ.get("CLAUDE_API_URL", "http://127.0.0.1:11436/v1/messages")
CLAUDE_MODEL = os.environ.get("CLAUDE_MODEL", "claude-sonnet-4-5")
OWNER_NAME = os.environ.get("OWNER_NAME", "the user")

AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
AETHERVAULT_DIR = Path(AETHERVAULT_HOME)
DAILY_SUMMARIES_DIR = AETHERVAULT_DIR / "workspace" / "daily-summaries"
KNOWLEDGE_GRAPH_SCRIPT = AETHERVAULT_DIR / "hooks" / "knowledge-graph.py"
KNOWLEDGE_GRAPH_FILE = AETHERVAULT_DIR / "data" / "knowledge-graph.json"


def log(msg: str):
    """Print a timestamped log message."""
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{ts}] {msg}", flush=True)


# ---------------------------------------------------------------------------
# Data Gathering
# ---------------------------------------------------------------------------

def gather_emails_since_morning() -> tuple:
    """Fetch recent emails — focus on what came in today."""
    try:
        result = subprocess.run(
            ["himalaya", "list", "-s", "10"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode != 0:
            return "", False
        output = result.stdout.strip()
        if not output:
            return "No emails today.", True
        log(f"Fetched {len(output.splitlines())} email lines")
        return output, True
    except FileNotFoundError:
        log("Himalaya CLI not found")
        return "", False
    except Exception as e:
        log(f"Email fetch failed: {e}")
        return "", False


def gather_todays_briefing() -> tuple:
    """Read this morning's briefing to know what was planned."""
    try:
        today = datetime.now().strftime("%Y-%m-%d")
        briefing_path = DAILY_SUMMARIES_DIR / f"briefing-{today}.md"
        if not briefing_path.exists():
            log("No morning briefing found for today")
            return "", False
        content = briefing_path.read_text().strip()
        if len(content) > 2000:
            content = content[:2000] + "\n... (truncated)"
        log(f"Today's briefing loaded ({len(content)} chars)")
        return content, True
    except Exception as e:
        log(f"Briefing read failed: {e}")
        return "", False


def gather_active_tasks() -> tuple:
    """Query knowledge graph for active tasks/projects."""
    try:
        result = subprocess.run(
            [
                "python3",
                str(KNOWLEDGE_GRAPH_SCRIPT),
                "query",
                "--type", "project",
            ],
            capture_output=True,
            text=True,
            timeout=15,
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip(), True
        # Fallback: read knowledge graph directly
        return _read_kg_tasks_fallback()
    except Exception:
        return _read_kg_tasks_fallback()


def _read_kg_tasks_fallback() -> tuple:
    """Read the knowledge graph JSON directly for active tasks."""
    try:
        if not KNOWLEDGE_GRAPH_FILE.exists():
            return "", False
        with open(KNOWLEDGE_GRAPH_FILE, "r") as f:
            data = json.load(f)
        tasks = []
        for node in data.get("nodes", []):
            node_data = node.get("data", node)
            node_type = node_data.get("type", "")
            props = node_data.get("properties", {})
            status = props.get("status", "")
            name = node_data.get("name", node_data.get("id", "unknown"))
            if node_type == "project" and status in ("active", "in-progress", ""):
                tasks.append(f"- {name}")
        if not tasks:
            return "No active tasks found.", True
        return "Active tasks:\n" + "\n".join(tasks), True
    except Exception as e:
        log(f"KG fallback failed: {e}")
        return "", False


def check_system_health() -> tuple:
    """Quick check on AetherVault system status."""
    health_items = []
    try:
        # Check if the bridge service is running
        result = subprocess.run(
            ["systemctl", "is-active", "aethervault"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        bridge_status = result.stdout.strip()
        health_items.append(f"Bridge: {bridge_status}")
    except Exception:
        pass

    try:
        # Check disk usage
        result = subprocess.run(
            ["df", "-h", "/"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if result.returncode == 0:
            lines = result.stdout.strip().splitlines()
            if len(lines) >= 2:
                parts = lines[1].split()
                if len(parts) >= 5:
                    health_items.append(f"Disk: {parts[4]} used ({parts[3]} free)")
    except Exception:
        pass

    if health_items:
        return "System: " + " | ".join(health_items), True
    return "", False


# ---------------------------------------------------------------------------
# Claude API — Generate the check-in
# ---------------------------------------------------------------------------

def generate_checkin(context: dict) -> str:
    """Call Claude API to generate the evening check-in."""
    if not ANTHROPIC_API_KEY:
        log("ANTHROPIC_API_KEY not set — using fallback")
        return _fallback_checkin(context)

    today = datetime.now()
    day_name = today.strftime("%A")
    is_weekend = today.weekday() >= 5

    sections = []

    if context.get("morning_briefing"):
        sections.append(f"THIS MORNING'S BRIEFING:\n{context['morning_briefing']}")

    if context.get("emails"):
        sections.append(f"EMAIL INBOX (recent):\n{context['emails']}")

    if context.get("tasks"):
        sections.append(f"ACTIVE TASKS:\n{context['tasks']}")

    if context.get("system_health"):
        sections.append(f"SYSTEM STATUS:\n{context['system_health']}")

    unavailable = context.get("unavailable", [])
    if unavailable:
        sections.append(f"UNAVAILABLE: {', '.join(unavailable)}")

    compiled = "\n\n---\n\n".join(sections) if sections else "Limited data available."

    system_prompt = (
        f"You are AetherVault, {OWNER_NAME}'s personal AI assistant. "
        "You're doing a casual evening check-in. Be brief, warm, and genuinely helpful. "
        "Think of it like a trusted friend who happens to be organized."
    )

    user_prompt = f"""Write a brief evening check-in message for {day_name} evening.
{"Keep it especially casual — it's the weekend." if is_weekend else ""}

Format for Telegram Markdown (single *asterisks* for bold).
Keep it SHORT — 100-200 words max. This is a quick nudge, not a report.

Structure:
1. Casual opener (one line — "Hey" energy, not formal)
2. Quick hits — any emails that still need a response, or tasks that didn't get done
3. One-liner on system health if anything is notable
4. Sign off — a chill closing, like "anything else before you call it?" vibe

If nothing needs attention, say so and keep it to 2-3 sentences.

Context:

{compiled}"""

    request_body = json.dumps({
        "model": CLAUDE_MODEL,
        "max_tokens": 512,
        "system": system_prompt,
        "messages": [
            {"role": "user", "content": user_prompt}
        ],
    }).encode("utf-8")

    req = urllib.request.Request(
        CLAUDE_API_URL,
        data=request_body,
        headers={
            "Content-Type": "application/json",
            "x-api-key": ANTHROPIC_API_KEY,
            "anthropic-version": "2023-06-01",
        },
        method="POST",
    )

    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            data = json.loads(resp.read().decode("utf-8"))
        content_blocks = data.get("content", [])
        text_parts = [b["text"] for b in content_blocks if b.get("type") == "text"]
        checkin = "\n".join(text_parts)
        if not checkin:
            return _fallback_checkin(context)
        log(f"Check-in generated ({len(checkin)} chars)")
        return checkin
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        log(f"Claude API error {e.code}: {body[:500]}")
        return _fallback_checkin(context)
    except Exception as e:
        log(f"Claude API failed: {e}")
        return _fallback_checkin(context)


def _fallback_checkin(context: dict) -> str:
    """Basic check-in without Claude."""
    lines = [
        f"*Evening Check-in — {datetime.now().strftime('%A, %B %d')}*",
        "",
        "Hey, quick status before you wind down:",
        "",
    ]
    if context.get("emails"):
        lines.append("*Inbox*")
        # Just show first few lines
        email_lines = context["emails"].splitlines()[:5]
        lines.extend(email_lines)
        lines.append("")
    if context.get("tasks"):
        lines.append("*Active Tasks*")
        lines.append(context["tasks"])
        lines.append("")
    if context.get("system_health"):
        lines.append(f"_{context['system_health']}_")
        lines.append("")
    lines.append("Anything else you need?")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Telegram
# ---------------------------------------------------------------------------

def send_telegram(message: str) -> bool:
    """Send a message via Telegram Bot API."""
    if not TELEGRAM_BOT_TOKEN:
        log("TELEGRAM_BOT_TOKEN not set")
        return False

    url = f"https://api.telegram.org/bot{TELEGRAM_BOT_TOKEN}/sendMessage"

    payload = json.dumps({
        "chat_id": TELEGRAM_CHAT_ID,
        "text": message,
        "parse_mode": "Markdown",
        "disable_web_page_preview": True,
    }).encode("utf-8")

    req = urllib.request.Request(
        url,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )

    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            result = json.loads(resp.read().decode("utf-8"))
        if result.get("ok"):
            log("Telegram message sent")
            return True
        log(f"Telegram not-ok: {result}")
        # Retry without Markdown
        return _send_plain(message)
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        log(f"Telegram error {e.code}: {body[:300]}")
        return _send_plain(message)
    except Exception as e:
        log(f"Telegram send failed: {e}")
        return False


def _send_plain(message: str) -> bool:
    """Retry without Markdown formatting."""
    url = f"https://api.telegram.org/bot{TELEGRAM_BOT_TOKEN}/sendMessage"
    payload = json.dumps({
        "chat_id": TELEGRAM_CHAT_ID,
        "text": message,
        "disable_web_page_preview": True,
    }).encode("utf-8")

    req = urllib.request.Request(
        url,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )

    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            result = json.loads(resp.read().decode("utf-8"))
        if result.get("ok"):
            log("Sent as plain text fallback")
            return True
        return False
    except Exception as e:
        log(f"Plain send also failed: {e}")
        return False


# ---------------------------------------------------------------------------
# Save check-in
# ---------------------------------------------------------------------------

def save_checkin(checkin: str) -> str:
    """Save the evening check-in to the daily summaries directory."""
    try:
        DAILY_SUMMARIES_DIR.mkdir(parents=True, exist_ok=True)
        today = datetime.now().strftime("%Y-%m-%d")
        filepath = DAILY_SUMMARIES_DIR / f"checkin-{today}.md"

        header = f"# Evening Check-in — {datetime.now().strftime('%A, %B %d, %Y')}\n\n"
        header += f"Generated at {datetime.now().strftime('%H:%M:%S')}\n\n---\n\n"

        filepath.write_text(header + checkin)
        log(f"Check-in saved to {filepath}")
        return str(filepath)
    except Exception as e:
        log(f"Failed to save check-in: {e}")
        return ""


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    log("=" * 50)
    log("AetherVault Evening Check-In")
    log("=" * 50)

    if not TELEGRAM_BOT_TOKEN:
        log("WARNING: TELEGRAM_BOT_TOKEN not set")
    if not ANTHROPIC_API_KEY:
        log("WARNING: ANTHROPIC_API_KEY not set")

    context = {"unavailable": []}

    log("Gathering emails...")
    emails, ok = gather_emails_since_morning()
    if ok:
        context["emails"] = emails
    else:
        context["unavailable"].append("email")

    log("Reading morning briefing...")
    briefing, ok = gather_todays_briefing()
    if ok:
        context["morning_briefing"] = briefing
    else:
        context["unavailable"].append("morning briefing")

    log("Gathering active tasks...")
    tasks, ok = gather_active_tasks()
    if ok:
        context["tasks"] = tasks
    else:
        context["unavailable"].append("tasks")

    log("Checking system health...")
    health, ok = check_system_health()
    if ok:
        context["system_health"] = health

    # Generate check-in
    log("Generating check-in via Claude...")
    checkin = generate_checkin(context)

    if not checkin:
        log("ERROR: Failed to generate check-in")
        sys.exit(1)

    # Save
    save_checkin(checkin)

    # Send
    if TELEGRAM_BOT_TOKEN:
        log("Sending via Telegram...")
        sent = send_telegram(checkin)
        if not sent:
            log("WARNING: Telegram send failed")
    else:
        log("Skipping Telegram (no bot token)")

    log("Evening check-in complete")


if __name__ == "__main__":
    main()

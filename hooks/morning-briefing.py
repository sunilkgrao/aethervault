#!/usr/bin/env python3
"""
AetherVault Morning Briefing Generator
=======================================

Gathers context from multiple sources (email, calendar, knowledge graph, weather,
yesterday's summary) and uses Claude to generate a personalized morning briefing.
Sends the briefing via Telegram and saves it locally.

Designed to run as a cron job on the AetherVault DigitalOcean droplet.

Usage:
    python3 /root/.aethervault/hooks/morning-briefing.py

    # Or with the bash wrapper for cron:
    bash /root/.aethervault/hooks/morning-briefing.sh

Environment variables (loaded from /root/.aethervault/.env):
    TELEGRAM_BOT_TOKEN   - Telegram bot API token
    ANTHROPIC_API_KEY    - Anthropic API key for Claude

Cron schedule (add via `crontab -e`):
    # Morning briefing: 8 AM weekdays, 9 AM weekends
    0 8 * * 1-5 /root/.aethervault/hooks/morning-briefing.sh
    0 9 * * 0,6 /root/.aethervault/hooks/morning-briefing.sh
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

WEATHER_LOCATION = os.environ.get("WEATHER_LOCATION", "")
WEATHER_URL = f"https://wttr.in/{WEATHER_LOCATION}?format=3" if WEATHER_LOCATION else ""


def log(msg: str):
    """Print a timestamped log message."""
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{ts}] {msg}", flush=True)


# ---------------------------------------------------------------------------
# Data Gathering — each function returns (str, bool) where bool = success
# ---------------------------------------------------------------------------

def gather_weather() -> tuple:
    """Fetch current weather via wttr.in."""
    if not WEATHER_URL:
        log("WEATHER_LOCATION not set — skipping weather")
        return "", False
    try:
        req = urllib.request.Request(
            WEATHER_URL,
            headers={"User-Agent": "curl/7.88.0"},
        )
        with urllib.request.urlopen(req, timeout=10) as resp:
            weather = resp.read().decode("utf-8").strip()
        log(f"Weather: {weather}")
        return weather, True
    except Exception as e:
        log(f"Weather fetch failed: {e}")
        return "", False


def gather_emails() -> tuple:
    """Fetch the last 5 emails via Himalaya CLI."""
    try:
        result = subprocess.run(
            ["himalaya", "list", "-s", "5"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode != 0:
            log(f"Himalaya failed (rc={result.returncode}): {result.stderr.strip()}")
            return "", False
        output = result.stdout.strip()
        if not output:
            return "No recent emails.", True
        log(f"Fetched {len(output.splitlines())} email lines")
        return output, True
    except FileNotFoundError:
        log("Himalaya CLI not found — skipping emails")
        return "", False
    except subprocess.TimeoutExpired:
        log("Himalaya timed out")
        return "", False
    except Exception as e:
        log(f"Email fetch failed: {e}")
        return "", False


def gather_active_projects() -> tuple:
    """Query the knowledge graph for active projects."""
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
        if result.returncode != 0:
            # Fallback: try reading the knowledge graph file directly
            return _read_knowledge_graph_fallback()
        output = result.stdout.strip()
        if not output:
            return "No active projects found.", True
        log(f"Active projects fetched ({len(output)} chars)")
        return output, True
    except FileNotFoundError:
        return _read_knowledge_graph_fallback()
    except subprocess.TimeoutExpired:
        log("Knowledge graph query timed out")
        return _read_knowledge_graph_fallback()
    except Exception as e:
        log(f"Knowledge graph query failed: {e}")
        return _read_knowledge_graph_fallback()


def _read_knowledge_graph_fallback() -> tuple:
    """Read the knowledge graph JSON directly to extract active projects."""
    try:
        if not KNOWLEDGE_GRAPH_FILE.exists():
            log("Knowledge graph file not found")
            return "", False
        with open(KNOWLEDGE_GRAPH_FILE, "r") as f:
            data = json.load(f)
        # Extract nodes that look like active projects
        projects = []
        nodes = data.get("nodes", [])
        for node in nodes:
            node_data = node.get("data", node)
            node_type = node_data.get("type", "")
            props = node_data.get("properties", {})
            status = props.get("status", "")
            name = node_data.get("name", node_data.get("id", "unknown"))
            if node_type == "project" and status in ("active", "in-progress", ""):
                desc = props.get("description", "no description")
                projects.append(f"- {name}: {desc}")
        if not projects:
            return "No active projects found in knowledge graph.", True
        output = "Active projects:\n" + "\n".join(projects)
        log(f"Extracted {len(projects)} projects from knowledge graph")
        return output, True
    except Exception as e:
        log(f"Knowledge graph fallback failed: {e}")
        return "", False


def gather_calendar() -> tuple:
    """Check today's calendar events via gcalcli or Google Calendar API."""
    try:
        result = subprocess.run(
            ["gcalcli", "agenda", "--nocolor", "--tsv",
             datetime.now().strftime("%Y-%m-%d"),
             (datetime.now() + timedelta(days=1)).strftime("%Y-%m-%d")],
            capture_output=True,
            text=True,
            timeout=15,
        )
        if result.returncode != 0:
            log(f"gcalcli failed (rc={result.returncode}): {result.stderr.strip()}")
            return "", False
        output = result.stdout.strip()
        if not output:
            return "No calendar events today.", True
        log(f"Calendar: {len(output.splitlines())} events")
        return output, True
    except FileNotFoundError:
        log("gcalcli not found — skipping calendar")
        return "", False
    except subprocess.TimeoutExpired:
        log("gcalcli timed out")
        return "", False
    except Exception as e:
        log(f"Calendar fetch failed: {e}")
        return "", False


def gather_yesterday_summary() -> tuple:
    """Read yesterday's daily summary if it exists."""
    try:
        yesterday = (datetime.now() - timedelta(days=1)).strftime("%Y-%m-%d")
        summary_path = DAILY_SUMMARIES_DIR / f"summary-{yesterday}.md"
        if not summary_path.exists():
            # Try alternate naming conventions
            for pattern in [f"{yesterday}.md", f"daily-{yesterday}.md", f"briefing-{yesterday}.md"]:
                alt_path = DAILY_SUMMARIES_DIR / pattern
                if alt_path.exists():
                    summary_path = alt_path
                    break
        if not summary_path.exists():
            log(f"No summary found for {yesterday}")
            return "", False
        content = summary_path.read_text().strip()
        # Truncate if too long to keep context window reasonable
        if len(content) > 3000:
            content = content[:3000] + "\n... (truncated)"
        log(f"Yesterday's summary loaded ({len(content)} chars)")
        return content, True
    except Exception as e:
        log(f"Yesterday summary fetch failed: {e}")
        return "", False


# ---------------------------------------------------------------------------
# Claude API — Generate the briefing
# ---------------------------------------------------------------------------

def generate_briefing(context: dict) -> str:
    """Call Claude API to generate the morning briefing."""
    if not ANTHROPIC_API_KEY:
        log("ERROR: ANTHROPIC_API_KEY not set")
        return _fallback_briefing(context)

    today = datetime.now()
    day_name = today.strftime("%A")
    date_str = today.strftime("%B %d, %Y")
    is_weekend = today.weekday() >= 5

    # Build context sections
    sections = []

    if context.get("weather"):
        sections.append(f"WEATHER:\n{context['weather']}")

    if context.get("emails"):
        sections.append(f"RECENT EMAILS (last 5):\n{context['emails']}")

    if context.get("projects"):
        sections.append(f"ACTIVE PROJECTS:\n{context['projects']}")

    if context.get("calendar"):
        sections.append(f"TODAY'S CALENDAR:\n{context['calendar']}")

    if context.get("yesterday_summary"):
        sections.append(f"YESTERDAY'S SUMMARY:\n{context['yesterday_summary']}")

    # Note unavailable sources
    unavailable = context.get("unavailable", [])
    if unavailable:
        sections.append(f"DATA SOURCES UNAVAILABLE: {', '.join(unavailable)}")

    compiled_context = "\n\n---\n\n".join(sections) if sections else "No data sources were available."

    system_prompt = (
        f"You are the AetherVault personal AI assistant for {OWNER_NAME}. "
        "You write concise, warm morning briefings. Your tone is like a trusted "
        "chief of staff — efficient, personal, never sycophantic or cheesy. "
        "You highlight what matters and skip what doesn't."
    )

    user_prompt = f"""Write a morning briefing for {day_name}, {date_str}.
{"It's the weekend, so keep it lighter." if is_weekend else ""}

Format the briefing for Telegram using Markdown:
- Use *bold* for section headers (Telegram uses single asterisks for bold)
- Use bullet points with - or bullet characters
- Keep it concise — aim for 200-400 words max
- No fluff, no corporate speak

Structure:
1. A brief, warm greeting (one line, reference the weather if available)
2. *Schedule* — today's calendar events (or "clear day" if none)
3. *Inbox* — emails that need attention (skip routine ones)
4. *Active Projects* — brief status on what's moving
5. *From Yesterday* — anything carried over or noteworthy
6. A closing line — something genuine and motivating, not a fortune cookie

Here is today's context:

{compiled_context}"""

    request_body = json.dumps({
        "model": CLAUDE_MODEL,
        "max_tokens": 1024,
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
        # Extract text from response
        content_blocks = data.get("content", [])
        text_parts = []
        for block in content_blocks:
            if block.get("type") == "text":
                text_parts.append(block["text"])
        briefing = "\n".join(text_parts)
        if not briefing:
            log("Claude returned empty response")
            return _fallback_briefing(context)
        log(f"Briefing generated ({len(briefing)} chars)")
        return briefing
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        log(f"Claude API error {e.code}: {body[:500]}")
        return _fallback_briefing(context)
    except Exception as e:
        log(f"Claude API call failed: {e}")
        return _fallback_briefing(context)


def _fallback_briefing(context: dict) -> str:
    """Generate a basic briefing without Claude if the API is unavailable."""
    today = datetime.now()
    lines = [
        f"*Morning Briefing — {today.strftime('%A, %B %d')}*",
        "",
    ]
    if context.get("weather"):
        lines.append(f"{context['weather']}")
        lines.append("")
    if context.get("calendar"):
        lines.append("*Schedule*")
        lines.append(context["calendar"])
        lines.append("")
    if context.get("emails"):
        lines.append("*Inbox*")
        lines.append(context["emails"])
        lines.append("")
    if context.get("projects"):
        lines.append("*Active Projects*")
        lines.append(context["projects"])
        lines.append("")
    if context.get("unavailable"):
        lines.append(f"_Data unavailable: {', '.join(context['unavailable'])}_")
        lines.append("")
    lines.append("_(Briefing generated without Claude — API was unavailable)_")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Telegram — Send the briefing
# ---------------------------------------------------------------------------

def send_telegram(message: str) -> bool:
    """Send a message via Telegram Bot API with Markdown formatting."""
    if not TELEGRAM_BOT_TOKEN:
        log("ERROR: TELEGRAM_BOT_TOKEN not set")
        return False

    url = f"https://api.telegram.org/bot{TELEGRAM_BOT_TOKEN}/sendMessage"

    # Telegram has a 4096 character limit per message
    # If the briefing is too long, split it
    chunks = _split_message(message, max_length=4000)

    success = True
    for i, chunk in enumerate(chunks):
        payload = json.dumps({
            "chat_id": TELEGRAM_CHAT_ID,
            "text": chunk,
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
            if not result.get("ok"):
                log(f"Telegram API returned not-ok: {result}")
                # Retry without Markdown if parsing failed
                success = _send_telegram_plain(chunk)
            else:
                log(f"Telegram message sent (chunk {i + 1}/{len(chunks)})")
        except urllib.error.HTTPError as e:
            body = e.read().decode("utf-8", errors="replace")
            log(f"Telegram API error {e.code}: {body[:300]}")
            # Retry without Markdown parsing
            success = _send_telegram_plain(chunk)
        except Exception as e:
            log(f"Telegram send failed: {e}")
            success = False

    return success


def _send_telegram_plain(message: str) -> bool:
    """Retry sending without Markdown if formatting causes issues."""
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
            log("Telegram message sent (plain text fallback)")
            return True
        log(f"Telegram plain send also failed: {result}")
        return False
    except Exception as e:
        log(f"Telegram plain send failed: {e}")
        return False


def _split_message(text: str, max_length: int = 4000) -> list:
    """Split a message into chunks that fit within Telegram's character limit."""
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


# ---------------------------------------------------------------------------
# Save briefing to disk
# ---------------------------------------------------------------------------

def save_briefing(briefing: str) -> str:
    """Save the briefing as a markdown file in the daily summaries directory."""
    try:
        DAILY_SUMMARIES_DIR.mkdir(parents=True, exist_ok=True)
        today = datetime.now().strftime("%Y-%m-%d")
        filepath = DAILY_SUMMARIES_DIR / f"briefing-{today}.md"

        header = f"# Morning Briefing — {datetime.now().strftime('%A, %B %d, %Y')}\n\n"
        header += f"Generated at {datetime.now().strftime('%H:%M:%S')}\n\n---\n\n"

        filepath.write_text(header + briefing)
        log(f"Briefing saved to {filepath}")
        return str(filepath)
    except Exception as e:
        log(f"Failed to save briefing: {e}")
        return ""


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    log("=" * 50)
    log("AetherVault Morning Briefing")
    log("=" * 50)

    # Validate required env vars
    if not TELEGRAM_BOT_TOKEN:
        log("WARNING: TELEGRAM_BOT_TOKEN not set — briefing will be saved but not sent")
    if not ANTHROPIC_API_KEY:
        log("WARNING: ANTHROPIC_API_KEY not set — will use fallback briefing format")

    # Gather all context sources
    context = {"unavailable": []}

    log("Gathering weather...")
    weather, ok = gather_weather()
    if ok:
        context["weather"] = weather
    else:
        context["unavailable"].append("weather")

    log("Gathering emails...")
    emails, ok = gather_emails()
    if ok:
        context["emails"] = emails
    else:
        context["unavailable"].append("email")

    log("Gathering active projects...")
    projects, ok = gather_active_projects()
    if ok:
        context["projects"] = projects
    else:
        context["unavailable"].append("knowledge graph")

    log("Gathering calendar...")
    calendar, ok = gather_calendar()
    if ok:
        context["calendar"] = calendar
    else:
        context["unavailable"].append("calendar")

    log("Gathering yesterday's summary...")
    yesterday, ok = gather_yesterday_summary()
    if ok:
        context["yesterday_summary"] = yesterday
    else:
        context["unavailable"].append("yesterday's summary")

    # Generate briefing
    log("Generating briefing via Claude...")
    briefing = generate_briefing(context)

    if not briefing:
        log("ERROR: Failed to generate briefing")
        sys.exit(1)

    # Save to disk
    save_path = save_briefing(briefing)

    # Send via Telegram
    if TELEGRAM_BOT_TOKEN:
        log("Sending via Telegram...")
        sent = send_telegram(briefing)
        if not sent:
            log("WARNING: Telegram send failed — briefing saved locally")
    else:
        log("Skipping Telegram (no bot token)")

    log("Morning briefing complete")
    if save_path:
        log(f"Saved: {save_path}")


if __name__ == "__main__":
    main()

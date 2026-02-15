#!/usr/bin/env python3
"""
AetherVault Nightly Consolidation Script
=========================================

Runs as a cron job each night to:
1. Read today's agent logs from the capsule
2. Summarize conversations using Claude API
3. Extract new facts about the user
4. Update MEMORY.md with new facts
5. Update the knowledge graph with new entities/relations
6. Write a daily summary to workspace/daily-summaries/

Usage:
    # Run directly (requires ANTHROPIC_API_KEY in environment or .env):
    python3 scripts/nightly-consolidation.py

    # Or via the bash wrapper (preferred for cron):
    bash scripts/nightly-consolidation.sh

    # Dry run (no writes, just print what would happen):
    python3 scripts/nightly-consolidation.py --dry-run

    # Process a specific date (default: today):
    python3 scripts/nightly-consolidation.py --date 2026-02-10

Architecture:
    Capsule:         /root/.aethervault/memory.mv2
    Knowledge graph: /root/.aethervault/data/knowledge-graph.json
    MEMORY.md:       /root/.aethervault/workspace/MEMORY.md
    Daily summaries: /root/.aethervault/workspace/daily-summaries/
    Agent logs:      queried via `aethervault query` against agent-log collection
"""

import argparse
import datetime
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
CAPSULE_PATH = os.environ.get("CAPSULE_PATH", os.path.join(AETHERVAULT_HOME, "memory.mv2"))
AETHERVAULT_BIN = os.environ.get("AETHERVAULT_BIN", "/usr/local/bin/aethervault")
KNOWLEDGE_GRAPH_PATH = os.path.join(AETHERVAULT_HOME, "data", "knowledge-graph.json")
KNOWLEDGE_GRAPH_HOOK = os.path.join(AETHERVAULT_HOME, "hooks", "knowledge-graph.py")
MEMORY_MD_PATH = os.path.join(AETHERVAULT_HOME, "workspace", "MEMORY.md")
DAILY_SUMMARIES_DIR = os.path.join(AETHERVAULT_HOME, "workspace", "daily-summaries")
ENV_FILE = os.path.join(AETHERVAULT_HOME, ".env")
LOG_DIR = os.environ.get("AETHERVAULT_LOG_DIR", "/var/log/aethervault")
OWNER_NAME = os.environ.get("OWNER_NAME", "the user")

CLAUDE_MODEL = os.environ.get("CLAUDE_MODEL", "claude-sonnet-4-5")
CLAUDE_API_URL = os.environ.get("CLAUDE_API_URL", "http://127.0.0.1:11436/v1/messages")
CLAUDE_API_VERSION = os.environ.get("CLAUDE_API_VERSION", "2023-06-01")
MAX_LOG_TOKENS = 80000  # rough char budget for log content sent to Claude
MAX_RETRIES = 3
RETRY_DELAY_SECONDS = 5


# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------

def log(msg: str, level: str = "INFO"):
    ts = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] [{level}] {msg}"
    print(line, flush=True)


def log_error(msg: str):
    log(msg, level="ERROR")


def log_warn(msg: str):
    log(msg, level="WARN")


# ---------------------------------------------------------------------------
# Environment
# ---------------------------------------------------------------------------

def load_env():
    """Load environment variables from .env file if present."""
    if os.path.isfile(ENV_FILE):
        try:
            with open(ENV_FILE, "r") as f:
                for line in f:
                    line = line.strip()
                    if not line or line.startswith("#"):
                        continue
                    if "=" in line:
                        key, _, value = line.partition("=")
                        key = key.strip()
                        value = value.strip().strip('"').strip("'")
                        if key and value:
                            os.environ.setdefault(key, value)
        except OSError as e:
            log_warn(f"Could not read {ENV_FILE}: {e}")


def get_api_key() -> str:
    key = os.environ.get("ANTHROPIC_API_KEY", "")
    if not key:
        log_error("ANTHROPIC_API_KEY not set. Source .env or export it.")
        sys.exit(1)
    return key


# ---------------------------------------------------------------------------
# Capsule query
# ---------------------------------------------------------------------------

def query_agent_logs(target_date: str, limit: int = 100) -> str:
    """
    Query today's agent logs from the capsule.

    Uses `aethervault query` to pull frames from the agent-log collection.
    The target_date is used as the query string to match relevant frames.
    """
    if not os.path.isfile(AETHERVAULT_BIN):
        log_warn(f"aethervault binary not found at {AETHERVAULT_BIN}, trying PATH")
        binary = "aethervault"
    else:
        binary = AETHERVAULT_BIN

    if not os.path.isfile(CAPSULE_PATH):
        log_error(f"Capsule not found at {CAPSULE_PATH}")
        return ""

    cmd = [
        binary, "query", CAPSULE_PATH,
        "--collection", "agent-log",
        "--query", target_date,
        "--limit", str(limit),
    ]

    log(f"Querying agent logs: {' '.join(cmd)}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=60,
        )
        if result.returncode != 0:
            log_warn(f"aethervault query returned {result.returncode}: {result.stderr.strip()}")
        output = result.stdout.strip()
        if not output:
            log_warn("No agent logs returned for today")
        else:
            log(f"Retrieved {len(output)} chars of agent log data")
        return output
    except FileNotFoundError:
        log_error(f"aethervault binary not found: {binary}")
        return ""
    except subprocess.TimeoutExpired:
        log_error("aethervault query timed out after 60s")
        return ""
    except Exception as e:
        log_error(f"Failed to query agent logs: {e}")
        return ""


# ---------------------------------------------------------------------------
# Claude API
# ---------------------------------------------------------------------------

def call_claude(api_key: str, system_prompt: str, user_message: str,
                max_tokens: int = 4096) -> str:
    """
    Call the Claude Messages API (non-streaming) and return the text response.
    Retries on transient errors.
    """
    payload = {
        "model": CLAUDE_MODEL,
        "max_tokens": max_tokens,
        "system": system_prompt,
        "messages": [
            {"role": "user", "content": user_message}
        ],
    }

    headers = {
        "Content-Type": "application/json",
        "x-api-key": api_key,
        "anthropic-version": CLAUDE_API_VERSION,
    }

    data = json.dumps(payload).encode("utf-8")

    for attempt in range(1, MAX_RETRIES + 1):
        try:
            req = urllib.request.Request(
                CLAUDE_API_URL,
                data=data,
                headers=headers,
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=120) as resp:
                body = json.loads(resp.read().decode("utf-8"))

            # Extract text from content blocks
            content_blocks = body.get("content", [])
            text_parts = []
            for block in content_blocks:
                if block.get("type") == "text":
                    text_parts.append(block["text"])
            result = "\n".join(text_parts)

            usage = body.get("usage", {})
            log(f"Claude API call OK: input={usage.get('input_tokens', '?')} "
                f"output={usage.get('output_tokens', '?')}")
            return result

        except urllib.error.HTTPError as e:
            err_body = ""
            try:
                err_body = e.read().decode("utf-8", errors="replace")[:500]
            except Exception:
                pass
            log_error(f"Claude API HTTP {e.code} (attempt {attempt}/{MAX_RETRIES}): {err_body}")
            if e.code in (429, 500, 502, 503, 529) and attempt < MAX_RETRIES:
                wait = RETRY_DELAY_SECONDS * attempt
                log(f"Retrying in {wait}s...")
                time.sleep(wait)
                continue
            return ""

        except urllib.error.URLError as e:
            log_error(f"Claude API URL error (attempt {attempt}/{MAX_RETRIES}): {e.reason}")
            if attempt < MAX_RETRIES:
                time.sleep(RETRY_DELAY_SECONDS * attempt)
                continue
            return ""

        except Exception as e:
            log_error(f"Claude API unexpected error (attempt {attempt}/{MAX_RETRIES}): {e}")
            if attempt < MAX_RETRIES:
                time.sleep(RETRY_DELAY_SECONDS * attempt)
                continue
            return ""

    return ""


# ---------------------------------------------------------------------------
# Summarization
# ---------------------------------------------------------------------------

SUMMARIZE_SYSTEM = """\
You are a consolidation assistant for a personal AI assistant called Linus (AetherVault).
Your job is to analyze a day's worth of agent conversation logs and produce a structured summary.

Respond with ONLY valid JSON matching this schema (no markdown fences, no extra text):

{
  "topics_discussed": ["topic1", "topic2"],
  "tasks_completed": ["task1", "task2"],
  "tasks_in_progress": ["task1"],
  "decisions_made": ["decision1"],
  "user_mood": "brief description of user's apparent mood/energy",
  "key_quotes": ["notable quote or request from user"],
  "new_facts": [
    {
      "category": "preference|person|project|event|opinion|plan|habit|health|work",
      "fact": "concise fact to remember about the user",
      "confidence": "high|medium|low"
    }
  ],
  "entities": [
    {
      "name": "entity name",
      "type": "person|project|company|tool|place|concept",
      "description": "brief description"
    }
  ],
  "relations": [
    {
      "from": "entity1",
      "relation": "relationship type (e.g. works_on, knows, uses, part_of)",
      "to": "entity2"
    }
  ],
  "summary_paragraph": "A 2-4 sentence narrative summary of the day's interactions."
}

Guidelines:
- Extract ONLY facts that are clearly stated or strongly implied by the user
- For new_facts, focus on durable information (preferences, people, plans) not ephemeral chatter
- Mark confidence as "low" for anything inferred rather than directly stated
- For entities, include people mentioned by name, projects discussed, tools used
- For relations, capture how entities connect (e.g. "User works_on ProjectX")
- The user's name is """ + OWNER_NAME + """
- If the logs are empty or contain no meaningful content, return minimal JSON with empty arrays
"""


def summarize_logs(api_key: str, logs: str, target_date: str) -> dict:
    """Send logs to Claude for summarization. Returns parsed JSON dict."""
    # Truncate logs if too long to keep within token budget
    if len(logs) > MAX_LOG_TOKENS:
        log(f"Truncating logs from {len(logs)} to {MAX_LOG_TOKENS} chars")
        logs = logs[:MAX_LOG_TOKENS] + "\n\n[... truncated ...]"

    user_msg = (
        f"Here are the agent conversation logs for {target_date}.\n"
        f"Analyze them and produce the structured JSON summary.\n\n"
        f"--- BEGIN LOGS ---\n{logs}\n--- END LOGS ---"
    )

    raw = call_claude(api_key, SUMMARIZE_SYSTEM, user_msg, max_tokens=4096)
    if not raw:
        log_error("Claude returned empty response for summarization")
        return {}

    # Parse JSON from response - handle potential markdown fences
    cleaned = raw.strip()
    if cleaned.startswith("```"):
        # Strip markdown code fences
        lines = cleaned.split("\n")
        # Remove first line (```json or ```) and last line (```)
        if lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip() == "```":
            lines = lines[:-1]
        cleaned = "\n".join(lines)

    try:
        data = json.loads(cleaned)
        log("Successfully parsed consolidation JSON")
        return data
    except json.JSONDecodeError as e:
        log_error(f"Failed to parse Claude's JSON response: {e}")
        log_error(f"Raw response (first 500 chars): {raw[:500]}")
        return {}


# ---------------------------------------------------------------------------
# Idempotency check
# ---------------------------------------------------------------------------

def already_processed(target_date: str) -> bool:
    """Check if we've already written a summary for this date."""
    summary_path = os.path.join(DAILY_SUMMARIES_DIR, f"{target_date}.md")
    if os.path.isfile(summary_path):
        log(f"Summary already exists at {summary_path} - skipping to avoid duplicates")
        return True
    return False


# ---------------------------------------------------------------------------
# Update MEMORY.md
# ---------------------------------------------------------------------------

def read_memory_md() -> str:
    """Read current MEMORY.md content, or return empty string."""
    if not os.path.isfile(MEMORY_MD_PATH):
        return ""
    try:
        with open(MEMORY_MD_PATH, "r") as f:
            return f.read()
    except OSError as e:
        log_warn(f"Could not read MEMORY.md: {e}")
        return ""


def update_memory_md(new_facts: list, target_date: str, dry_run: bool = False):
    """
    Append new facts to MEMORY.md, organized by category.
    Each fact is tagged with the date to prevent re-addition.

    Idempotency: we check if facts with the same date tag already exist.
    """
    if not new_facts:
        log("No new facts to add to MEMORY.md")
        return

    current_content = read_memory_md()
    date_tag = f"[{target_date}]"

    # Filter out facts that appear to already be in the file (by content + date)
    facts_to_add = []
    for fact_obj in new_facts:
        fact_text = fact_obj.get("fact", "").strip()
        if not fact_text:
            continue
        # Check if this fact (or something very similar) already exists with this date
        if date_tag in current_content and fact_text.lower() in current_content.lower():
            log(f"Skipping duplicate fact: {fact_text[:60]}...")
            continue
        facts_to_add.append(fact_obj)

    if not facts_to_add:
        log("All facts already present in MEMORY.md - nothing to add")
        return

    # Group facts by category
    by_category = {}
    for fact_obj in facts_to_add:
        cat = fact_obj.get("category", "general").capitalize()
        fact_text = fact_obj.get("fact", "")
        confidence = fact_obj.get("confidence", "medium")
        by_category.setdefault(cat, []).append((fact_text, confidence))

    # Build the block to append
    lines = [
        "",
        f"## Consolidation Notes {date_tag}",
        "",
    ]
    for category, facts in sorted(by_category.items()):
        lines.append(f"### {category}")
        for fact_text, confidence in facts:
            conf_marker = "" if confidence == "high" else f" ({confidence} confidence)"
            lines.append(f"- {fact_text}{conf_marker}")
        lines.append("")

    block = "\n".join(lines)

    if dry_run:
        log("DRY RUN - would append to MEMORY.md:")
        print(block)
        return

    # Initialize MEMORY.md if it doesn't exist
    if not current_content:
        header = f"# Memory\n\nFacts about {OWNER_NAME}, extracted from conversations.\n"
        current_content = header

    try:
        with open(MEMORY_MD_PATH, "a") as f:
            f.write(block)
        log(f"Appended {len(facts_to_add)} new facts to MEMORY.md")
    except OSError as e:
        log_error(f"Failed to write MEMORY.md: {e}")


# ---------------------------------------------------------------------------
# Update knowledge graph
# ---------------------------------------------------------------------------

def update_knowledge_graph(entities: list, relations: list,
                           target_date: str, dry_run: bool = False):
    """
    Update the knowledge graph by calling knowledge-graph.py or
    directly modifying the JSON file.

    Tries the hook script first. Falls back to direct JSON manipulation.
    """
    if not entities and not relations:
        log("No entities or relations to add to knowledge graph")
        return

    # Try using the hook script
    if os.path.isfile(KNOWLEDGE_GRAPH_HOOK):
        _update_kg_via_hook(entities, relations, target_date, dry_run)
    else:
        _update_kg_direct(entities, relations, target_date, dry_run)


def _update_kg_via_hook(entities: list, relations: list,
                        target_date: str, dry_run: bool):
    """Update knowledge graph by calling knowledge-graph.py hook."""
    for entity in entities:
        name = entity.get("name", "").strip()
        etype = entity.get("type", "concept").strip()
        desc = entity.get("description", "").strip()
        if not name:
            continue

        cmd = [
            sys.executable, KNOWLEDGE_GRAPH_HOOK,
            "add-entity",
            "--name", name,
            "--type", etype,
        ]
        if desc:
            cmd.extend(["--attrs", json.dumps({"description": desc})])

        if dry_run:
            log(f"DRY RUN - would run: {' '.join(cmd)}")
            continue

        try:
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
            if result.returncode != 0:
                log_warn(f"knowledge-graph.py add-entity failed for '{name}': "
                         f"{result.stderr.strip()}")
            else:
                log(f"Added entity: {name} ({etype})")
        except Exception as e:
            log_warn(f"Failed to add entity '{name}': {e}")

    for relation in relations:
        from_ent = relation.get("from", "").strip()
        rel_type = relation.get("relation", "").strip()
        to_ent = relation.get("to", "").strip()
        if not (from_ent and rel_type and to_ent):
            continue

        cmd = [
            sys.executable, KNOWLEDGE_GRAPH_HOOK,
            "add-relation",
            "--from", from_ent,
            "--relation", rel_type,
            "--to", to_ent,
        ]

        if dry_run:
            log(f"DRY RUN - would run: {' '.join(cmd)}")
            continue

        try:
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
            if result.returncode != 0:
                log_warn(f"knowledge-graph.py add-relation failed for "
                         f"'{from_ent} -> {to_ent}': {result.stderr.strip()}")
            else:
                log(f"Added relation: {from_ent} --[{rel_type}]--> {to_ent}")
        except Exception as e:
            log_warn(f"Failed to add relation '{from_ent} -> {to_ent}': {e}")


def _update_kg_direct(entities: list, relations: list,
                      target_date: str, dry_run: bool):
    """Fallback: directly manipulate the knowledge-graph.json file."""
    log("knowledge-graph.py hook not found, updating JSON directly")

    # Load existing graph (NetworkX node-link format uses "nodes" and "links")
    graph = {"directed": True, "multigraph": False, "graph": {}, "nodes": [], "links": []}
    if os.path.isfile(KNOWLEDGE_GRAPH_PATH):
        try:
            with open(KNOWLEDGE_GRAPH_PATH, "r") as f:
                graph = json.load(f)
        except (json.JSONDecodeError, OSError) as e:
            log_warn(f"Could not read knowledge graph: {e}")
            graph = {"directed": True, "multigraph": False, "graph": {}, "nodes": [], "links": []}

    if "nodes" not in graph:
        graph["nodes"] = []
    if "links" not in graph:
        graph["links"] = []

    existing_entity_names = set()
    for node in graph["nodes"]:
        name = node.get("name", node.get("id", "")).lower()
        if name:
            existing_entity_names.add(name)

    existing_relations = set()
    for link in graph["links"]:
        key = (
            str(link.get("source", "")).lower(),
            link.get("relation", "").lower(),
            str(link.get("target", "")).lower(),
        )
        existing_relations.add(key)

    entities_added = 0
    relations_added = 0
    now_ts = datetime.datetime.now(datetime.timezone.utc).isoformat()

    for entity in entities:
        name = entity.get("name", "").strip()
        if not name or name.lower() in existing_entity_names:
            continue
        entry = {
            "id": name,
            "type": entity.get("type", "concept"),
            "name": name,
            "properties": {"description": entity.get("description", "")},
            "created_at": now_ts,
            "updated_at": now_ts,
        }
        if dry_run:
            log(f"DRY RUN - would add entity: {name}")
        else:
            graph["nodes"].append(entry)
            existing_entity_names.add(name.lower())
            entities_added += 1

    for relation in relations:
        from_ent = relation.get("from", "").strip()
        rel_type = relation.get("relation", "").strip()
        to_ent = relation.get("to", "").strip()
        if not (from_ent and rel_type and to_ent):
            continue
        key = (from_ent.lower(), rel_type.lower(), to_ent.lower())
        if key in existing_relations:
            continue
        entry = {
            "source": from_ent,
            "target": to_ent,
            "relation": rel_type,
            "confidence": 0.8,
            "created_at": now_ts,
        }
        if dry_run:
            log(f"DRY RUN - would add relation: {from_ent} --[{rel_type}]--> {to_ent}")
        else:
            graph["links"].append(entry)
            existing_relations.add(key)
            relations_added += 1

    if dry_run:
        return

    if entities_added == 0 and relations_added == 0:
        log("No new entities or relations to add (all duplicates)")
        return

    # Update metadata
    if "metadata" not in graph:
        graph["metadata"] = {}
    graph["metadata"]["last_consolidation"] = target_date
    graph["metadata"]["last_updated"] = datetime.datetime.now().isoformat()

    # Ensure parent directory exists
    os.makedirs(os.path.dirname(KNOWLEDGE_GRAPH_PATH), exist_ok=True)

    try:
        with open(KNOWLEDGE_GRAPH_PATH, "w") as f:
            json.dump(graph, f, indent=2)
        log(f"Knowledge graph updated: +{entities_added} entities, "
            f"+{relations_added} relations")
    except OSError as e:
        log_error(f"Failed to write knowledge graph: {e}")


# ---------------------------------------------------------------------------
# Write daily summary
# ---------------------------------------------------------------------------

def write_daily_summary(summary_data: dict, logs_raw: str,
                        target_date: str, dry_run: bool = False):
    """Write a markdown daily summary to the daily-summaries directory."""
    os.makedirs(DAILY_SUMMARIES_DIR, exist_ok=True)
    summary_path = os.path.join(DAILY_SUMMARIES_DIR, f"{target_date}.md")

    topics = summary_data.get("topics_discussed", [])
    tasks_done = summary_data.get("tasks_completed", [])
    tasks_wip = summary_data.get("tasks_in_progress", [])
    decisions = summary_data.get("decisions_made", [])
    mood = summary_data.get("user_mood", "unknown")
    quotes = summary_data.get("key_quotes", [])
    new_facts = summary_data.get("new_facts", [])
    entities = summary_data.get("entities", [])
    relations = summary_data.get("relations", [])
    paragraph = summary_data.get("summary_paragraph", "No summary available.")

    lines = [
        f"# Daily Summary: {target_date}",
        "",
        f"*Generated by nightly-consolidation at "
        f"{datetime.datetime.now().strftime('%Y-%m-%d %H:%M:%S')}*",
        "",
        "## Overview",
        "",
        paragraph,
        "",
        f"**User mood/energy:** {mood}",
        "",
    ]

    if topics:
        lines.append("## Topics Discussed")
        lines.append("")
        for t in topics:
            lines.append(f"- {t}")
        lines.append("")

    if tasks_done:
        lines.append("## Tasks Completed")
        lines.append("")
        for t in tasks_done:
            lines.append(f"- {t}")
        lines.append("")

    if tasks_wip:
        lines.append("## Tasks In Progress")
        lines.append("")
        for t in tasks_wip:
            lines.append(f"- {t}")
        lines.append("")

    if decisions:
        lines.append("## Decisions Made")
        lines.append("")
        for d in decisions:
            lines.append(f"- {d}")
        lines.append("")

    if quotes:
        lines.append("## Notable Quotes")
        lines.append("")
        for q in quotes:
            lines.append(f"> {q}")
            lines.append("")

    if new_facts:
        lines.append("## New Facts Extracted")
        lines.append("")
        for fact in new_facts:
            cat = fact.get("category", "general")
            text = fact.get("fact", "")
            conf = fact.get("confidence", "medium")
            lines.append(f"- **[{cat}]** {text} _{conf} confidence_")
        lines.append("")

    if entities:
        lines.append("## Entities Discovered")
        lines.append("")
        lines.append("| Name | Type | Description |")
        lines.append("|------|------|-------------|")
        for e in entities:
            lines.append(f"| {e.get('name', '')} | {e.get('type', '')} "
                         f"| {e.get('description', '')} |")
        lines.append("")

    if relations:
        lines.append("## Relations Discovered")
        lines.append("")
        for r in relations:
            lines.append(f"- {r.get('from', '?')} --[{r.get('relation', '?')}]--> "
                         f"{r.get('to', '?')}")
        lines.append("")

    lines.append("---")
    lines.append("*Generated by nightly-consolidation.py*")

    content = "\n".join(lines) + "\n"

    if dry_run:
        log(f"DRY RUN - would write {len(content)} chars to {summary_path}")
        print(content)
        return

    try:
        with open(summary_path, "w") as f:
            f.write(content)
        log(f"Daily summary written to {summary_path}")
    except OSError as e:
        log_error(f"Failed to write daily summary: {e}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="AetherVault Nightly Consolidation - summarize and extract from daily logs",
    )
    parser.add_argument(
        "--date",
        default=None,
        help="Target date in YYYY-MM-DD format (default: today)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print what would happen without writing any files",
    )
    parser.add_argument(
        "--log-limit",
        type=int,
        default=100,
        help="Max number of log frames to query (default: 100)",
    )
    args = parser.parse_args()

    # Determine target date
    if args.date:
        target_date = args.date
        # Validate format
        try:
            datetime.datetime.strptime(target_date, "%Y-%m-%d")
        except ValueError:
            log_error(f"Invalid date format: {target_date} (expected YYYY-MM-DD)")
            sys.exit(1)
    else:
        target_date = datetime.date.today().strftime("%Y-%m-%d")

    log(f"=== AetherVault Nightly Consolidation for {target_date} ===")

    if args.dry_run:
        log("DRY RUN mode enabled - no files will be written")

    # Load environment
    load_env()
    api_key = get_api_key()

    # Idempotency check
    if not args.dry_run and already_processed(target_date):
        log("Already processed today. Use --date to reprocess or delete the existing summary.")
        sys.exit(0)

    # Step 1: Query agent logs
    log("Step 1/5: Querying agent logs from capsule...")
    logs_raw = query_agent_logs(target_date, limit=args.log_limit)
    if not logs_raw:
        log_warn("No logs found for today. Writing empty summary and exiting.")
        empty_summary = {
            "topics_discussed": [],
            "tasks_completed": [],
            "tasks_in_progress": [],
            "decisions_made": [],
            "user_mood": "no interactions today",
            "key_quotes": [],
            "new_facts": [],
            "entities": [],
            "relations": [],
            "summary_paragraph": f"No agent interactions recorded for {target_date}.",
        }
        write_daily_summary(empty_summary, "", target_date, dry_run=args.dry_run)
        log("Done (no logs to process)")
        sys.exit(0)

    # Step 2: Summarize with Claude
    log("Step 2/5: Summarizing conversations with Claude...")
    summary_data = summarize_logs(api_key, logs_raw, target_date)
    if not summary_data:
        log_error("Failed to get summary from Claude. Writing raw log summary.")
        summary_data = {
            "topics_discussed": ["consolidation failed - see raw logs"],
            "tasks_completed": [],
            "tasks_in_progress": [],
            "decisions_made": [],
            "user_mood": "unknown (API failure)",
            "key_quotes": [],
            "new_facts": [],
            "entities": [],
            "relations": [],
            "summary_paragraph": (
                f"Nightly consolidation for {target_date} failed to contact Claude API. "
                f"Raw logs were {len(logs_raw)} characters. Manual review recommended."
            ),
        }
        write_daily_summary(summary_data, logs_raw, target_date, dry_run=args.dry_run)
        sys.exit(1)

    # Step 3: Update MEMORY.md
    log("Step 3/5: Updating MEMORY.md with new facts...")
    new_facts = summary_data.get("new_facts", [])
    update_memory_md(new_facts, target_date, dry_run=args.dry_run)

    # Step 4: Update knowledge graph
    log("Step 4/5: Updating knowledge graph...")
    entities = summary_data.get("entities", [])
    relations = summary_data.get("relations", [])
    update_knowledge_graph(entities, relations, target_date, dry_run=args.dry_run)

    # Step 5: Write daily summary
    log("Step 5/5: Writing daily summary...")
    write_daily_summary(summary_data, logs_raw, target_date, dry_run=args.dry_run)

    # Final stats
    log(f"Consolidation complete for {target_date}:")
    log(f"  Topics:    {len(summary_data.get('topics_discussed', []))}")
    log(f"  Tasks:     {len(summary_data.get('tasks_completed', []))} completed, "
        f"{len(summary_data.get('tasks_in_progress', []))} in progress")
    log(f"  Facts:     {len(new_facts)} extracted")
    log(f"  Entities:  {len(entities)}")
    log(f"  Relations: {len(relations)}")
    log("=== Done ===")


if __name__ == "__main__":
    main()

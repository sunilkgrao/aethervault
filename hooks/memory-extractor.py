#!/usr/bin/env python3
"""
AetherVault Real-Time Memory Extraction Hook
=============================================

Runs every 5 minutes via cron to extract facts from recent conversations.
This is the "hot path" — catches high-importance facts in near-real-time
instead of waiting for the nightly consolidation (24-hour delay).

Implements:
- Mem0's two-phase pipeline: extract candidate facts, then reconcile (ADD/UPDATE/DELETE/NOOP)
- LangMem's dual-path: hot path (importance >= 5) here, cold path via nightly consolidation
- Generative Agents importance scoring (1-10 scale)
- FadeMem decay metadata (exponential decay with importance-modulated rate)
- Zep-style bi-temporal timestamps (t_valid / t_invalid)

Usage:
    python3 memory-extractor.py              # normal cron operation
    python3 memory-extractor.py --dry-run    # print without writing
    python3 memory-extractor.py --window 15  # look back 15 minutes
    python3 memory-extractor.py --force      # ignore last-processed marker
"""

import argparse
import datetime
import fcntl
import json
import os
import subprocess
import sys
import tempfile

# Shared module (same directory)
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hot_memory_store import (
    AETHERVAULT_HOME, CAPSULE_PATH, AETHERVAULT_BIN, OWNER_NAME,
    PROMOTE_THRESHOLD,
    log, log_error, log_warn,
    load_env, get_api_key, call_claude, parse_claude_json, send_telegram,
    read_hot_memories, write_hot_memories, append_hot_memory,
    invalidate_hot_memory, update_hot_memory,
    check_disk_space, cleanup_temp_files, rotate_archive, prune_invalidated,
    record_failure, record_success, atomic_write_json,
    search_capsule, search_hot_memories_text,
    hot_memory_lock, hot_memory_unlock,
)

# ---------------------------------------------------------------------------
# Extractor-specific configuration
# ---------------------------------------------------------------------------

KNOWLEDGE_GRAPH_PATH = os.path.join(AETHERVAULT_HOME, "data", "knowledge-graph.json")
KNOWLEDGE_GRAPH_HOOK = os.path.join(AETHERVAULT_HOME, "hooks", "knowledge-graph.py")
MARKER_PATH = os.path.join(AETHERVAULT_HOME, "data", "extractor-marker.json")
PID_FILE = os.path.join(AETHERVAULT_HOME, "data", "extractor.pid")

DEFAULT_WINDOW_MINUTES = 10
HOT_PATH_IMPORTANCE_THRESHOLD = 5
MAX_LOG_CHARS = 30000
MAX_ADDITIONS_PER_HOUR = 5
MIN_FACT_LENGTH = 20

COMPONENT_NAME = "memory-extractor"
DAILY_DIGEST_PATH = os.path.join(AETHERVAULT_HOME, "data", "extractor-daily-digest.json")


# ---------------------------------------------------------------------------
# Instance locking (prevent overlapping cron runs)
# ---------------------------------------------------------------------------

_pid_fd = None

def acquire_instance_lock():
    global _pid_fd
    os.makedirs(os.path.dirname(PID_FILE), exist_ok=True)
    try:
        _pid_fd = os.open(PID_FILE, os.O_CREAT | os.O_WRONLY)
        fcntl.flock(_pid_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        os.ftruncate(_pid_fd, 0)
        os.write(_pid_fd, str(os.getpid()).encode())
    except (OSError, BlockingIOError):
        log("Another extractor instance is running, exiting")
        sys.exit(0)


# ---------------------------------------------------------------------------
# Marker tracking
# ---------------------------------------------------------------------------

def read_marker() -> str:
    if os.path.isfile(MARKER_PATH):
        try:
            with open(MARKER_PATH, "r") as f:
                data = json.load(f)
                return data.get("last_processed", "")
        except (json.JSONDecodeError, OSError) as e:
            log_warn(f"Marker file corrupted or unreadable: {e}")
    return ""


def write_marker(timestamp: str):
    try:
        atomic_write_json(MARKER_PATH, {
            "last_processed": timestamp,
            "updated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        })
    except OSError as e:
        log_warn(f"Could not write marker: {e}")


# ---------------------------------------------------------------------------
# Capsule query
# ---------------------------------------------------------------------------

def query_recent_logs(window_minutes: int):
    """Returns str on success (may be empty), None on error.
    Retries up to 3 times with backoff on capsule lock contention."""
    import time as _time
    if not os.path.isfile(AETHERVAULT_BIN):
        binary = "aethervault"
    else:
        binary = AETHERVAULT_BIN

    if not os.path.isfile(CAPSULE_PATH):
        log_error(f"Capsule not found at {CAPSULE_PATH}")
        return None

    now = datetime.datetime.now()
    query_str = now.strftime("%Y-%m-%d")

    cmd = [binary, "query", "--collection", "agent-log", "--limit", "30",
           CAPSULE_PATH, query_str]

    max_retries = 3
    for attempt in range(1, max_retries + 1):
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
            if result.returncode != 0:
                stderr = result.stderr.strip()
                # Lock contention is transient — retry with backoff
                if "Lock" in stderr or "exclusive access" in stderr or "in use" in stderr:
                    if attempt < max_retries:
                        wait = 5 * attempt
                        log(f"Capsule locked (attempt {attempt}/{max_retries}), retrying in {wait}s...")
                        _time.sleep(wait)
                        continue
                    else:
                        log(f"Capsule still locked after {max_retries} attempts — agent is busy, skipping this cycle")
                        return ""  # Return empty (not None) to avoid failure alert
                log_warn(f"aethervault query returned {result.returncode}: {stderr}")
                return None
            output = result.stdout.strip()
            if not output:
                log("No recent agent logs found")
                return ""
            if len(output) > MAX_LOG_CHARS:
                output = output[-MAX_LOG_CHARS:]
                log(f"Truncated logs to last {MAX_LOG_CHARS} chars")
            else:
                log(f"Retrieved {len(output)} chars of recent logs")
            return output
        except FileNotFoundError:
            log_error(f"aethervault binary not found: {binary}")
            return None
        except subprocess.TimeoutExpired:
            if attempt < max_retries:
                log(f"aethervault query timed out (attempt {attempt}/{max_retries}), retrying...")
                _time.sleep(5 * attempt)
                continue
            log_error("aethervault query timed out after all retries")
            return None
        except Exception as e:
            log_error(f"Failed to query logs: {e}")
            return None
    return None


# ---------------------------------------------------------------------------
# Fact extraction (Phase 1)
# ---------------------------------------------------------------------------

EXTRACT_SYSTEM = """\
You are a memory extraction agent for a personal AI assistant.
Your job is to extract durable, important facts from recent conversation logs.

Respond with ONLY valid JSON (no markdown fences):

{
  "facts": [
    {
      "fact": "concise declarative statement",
      "category": "preference|person|project|event|plan|health|work|opinion|habit|location|relationship",
      "importance": 8,
      "entities": ["entity1", "entity2"],
      "temporal": "2026-02-12"
    }
  ]
}

Importance scale (1-10):
  1-2: Mundane (greetings, small talk, acknowledgments)
  3-4: Mildly useful (mentioned a tool, asked a generic question)
  5-6: Moderately useful (stated a preference, mentioned a plan)
  7-8: Important (changed a preference, new relationship, project decision)
  9-10: Critical (major life event, job change, relationship change, health issue)

Rules:
- Extract ONLY facts with importance >= """ + str(HOT_PATH_IMPORTANCE_THRESHOLD) + """
- Focus on NEW information, not things already discussed before
- Be precise and factual -- no speculation or inference beyond what's clearly stated
- Include temporal context when available (dates, relative timing)
- Each fact should be self-contained (understandable without conversation context)
- The user's name is """ + OWNER_NAME + """
- If no important new facts exist, return {"facts": []}
"""


def extract_facts(api_key: str, logs: str, marker_timestamp: str = ""):
    """Returns list on success (may be empty), None on API/parse failure."""
    context_lines = []
    if marker_timestamp:
        context_lines.append(
            f"IMPORTANT: Only extract facts from events AFTER {marker_timestamp}. "
            f"Ignore older context that was already processed."
        )
    # Include existing hot memory facts so Claude avoids re-extracting them
    existing_facts = _get_existing_fact_summaries()
    if existing_facts:
        context_lines.append(
            f"ALREADY KNOWN FACTS (do NOT re-extract these):\n{existing_facts}"
        )
    context_block = "\n".join(context_lines) + "\n\n" if context_lines else ""
    user_msg = (
        f"{context_block}"
        f"Extract important new facts from these recent conversation logs.\n\n"
        f"--- BEGIN LOGS ---\n{logs}\n--- END LOGS ---"
    )

    raw = call_claude(api_key, EXTRACT_SYSTEM, user_msg, max_tokens=2048)
    if not raw:
        log_error("Claude API returned empty response for extraction")
        return None

    try:
        data = parse_claude_json(raw)
        facts = data.get("facts", [])
        if not isinstance(facts, list):
            log_error(f"Expected 'facts' to be a list, got {type(facts).__name__}")
            return None
        log(f"Extracted {len(facts)} candidate facts")
        return facts
    except (json.JSONDecodeError, ValueError) as e:
        log_error(f"Failed to parse extraction JSON: {e}")
        log_error(f"Raw response: {raw[:300]}")
        return None


def _get_existing_fact_summaries(max_chars: int = 1000) -> str:
    """Return a compact summary of existing hot memory facts for the extraction prompt."""
    memories = read_hot_memories()
    if not memories:
        return ""
    facts = []
    total = 0
    for mem in memories:
        if mem.get("metadata", {}).get("t_invalid"):
            continue
        fact = mem.get("fact", "").strip()
        if fact and total + len(fact) < max_chars:
            facts.append(f"- {fact}")
            total += len(fact) + 3
    return "\n".join(facts)


# ---------------------------------------------------------------------------
# Validation (Phase 2.5 — self-review before commit)
# ---------------------------------------------------------------------------

def validate_candidate(candidate: dict, existing_hot_facts: list) -> tuple:
    """Pre-commit quality gate. Returns (keep: bool, reason: str).

    Checks:
    1. Fact text quality (length, not a question, declarative)
    2. Tighter duplicate pre-check against hot memories (word overlap > 60%)
    3. Entity enrichment (extract capitalized words if entities empty)
    """
    fact_text = str(candidate.get("fact", "")).strip()

    # Quality gate: too short
    if len(fact_text) < MIN_FACT_LENGTH:
        return False, f"fact too short ({len(fact_text)} chars < {MIN_FACT_LENGTH})"

    # Quality gate: questions aren't facts
    if fact_text.rstrip().endswith("?"):
        return False, "fact is a question, not a statement"

    # Tighter duplicate pre-check: word overlap > 60% with any existing hot fact
    fact_words = set(fact_text.lower().split())
    for existing in existing_hot_facts:
        existing_words = set(existing.lower().split())
        if not fact_words or not existing_words:
            continue
        overlap = len(fact_words & existing_words)
        smaller = min(len(fact_words), len(existing_words))
        if smaller > 0 and overlap / smaller > 0.6:
            return False, f"too similar to existing: '{existing[:60]}...'"

    # Entity enrichment: if entities list is empty, extract capitalized words
    entities = candidate.get("entities", [])
    if not entities:
        enriched = [w for w in fact_text.split()
                    if len(w) > 1 and w[0].isupper() and w not in ("The", "This", "That", "User")]
        if enriched:
            candidate["entities"] = enriched[:5]

    return True, "passed"


def _count_recent_additions() -> int:
    """Count how many facts were added in the last hour (rate limiting)."""
    memories = read_hot_memories()
    if not memories:
        return 0
    one_hour_ago = (
        datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=1)
    ).isoformat()
    count = 0
    for mem in memories:
        created = mem.get("metadata", {}).get("created_at", "")
        if created > one_hour_ago:
            count += 1
    return count


# ---------------------------------------------------------------------------
# Reconciliation (Phase 3)
# ---------------------------------------------------------------------------

def search_existing_memories(query: str) -> list:
    """Search BOTH capsule and hot memories for existing facts.

    Bug fix: previously only searched capsule's aethervault-memory collection,
    so facts added to hot-memories.jsonl were invisible to reconciliation,
    causing the same facts to be ADD'd repeatedly.
    """
    # Search capsule (cold storage)
    capsule_results = search_capsule(query, collections=["aethervault-memory"], limit=3)
    # Search hot memories (warm storage) — this is the critical addition
    hot_results = search_hot_memories_text(query, limit=3)
    # Combine, deduplicated
    combined = list(capsule_results)
    for r in hot_results:
        if r not in combined:
            combined.append(r)
    return combined[:5]


RECONCILE_SYSTEM = """\
You are a memory reconciliation agent. Given a NEW fact and EXISTING memories,
determine what operation to perform.

Respond with ONLY valid JSON (no markdown fences):

{
  "operation": "ADD|UPDATE|DELETE|NOOP",
  "reason": "brief explanation",
  "updated_fact": "the reconciled fact text (only for UPDATE)",
  "delete_target": "the existing fact text to invalidate (only for DELETE)"
}

Rules:
- ADD: The fact is genuinely new -- no existing memory covers it
- UPDATE: An existing memory exists but needs updating (e.g., preference changed)
- DELETE: The new fact contradicts/invalidates an existing memory (e.g., "I no longer like X")
- NOOP: The fact is already known or too similar to existing memories
- For UPDATE, provide the corrected/merged fact text in updated_fact
- For DELETE, identify which existing memory should be invalidated in delete_target
- Be conservative: prefer NOOP over ADD for marginal facts
"""


def reconcile_fact(api_key: str, candidate: dict, existing: list) -> dict:
    if not existing:
        return {"operation": "ADD", "reason": "no existing memories found"}

    existing_text = "\n".join(f"- {m}" for m in existing[:5])
    user_msg = (
        f"NEW FACT: {candidate['fact']}\n"
        f"(Category: {candidate.get('category', 'general')}, "
        f"Importance: {candidate.get('importance', 5)})\n\n"
        f"EXISTING MEMORIES:\n{existing_text}"
    )

    raw = call_claude(api_key, RECONCILE_SYSTEM, user_msg, max_tokens=256)
    if not raw:
        return {"operation": "ADD", "reason": "API fallback"}

    try:
        return parse_claude_json(raw)
    except (json.JSONDecodeError, ValueError):
        return {"operation": "ADD", "reason": "parse fallback"}


# ---------------------------------------------------------------------------
# Memory metadata builder
# ---------------------------------------------------------------------------

def build_memory_metadata(candidate: dict) -> dict:
    now_iso = datetime.datetime.now(datetime.timezone.utc).isoformat()
    importance = candidate.get("importance", 5)
    importance_norm = importance / 10.0
    return {
        "category": candidate.get("category", "general"),
        "importance": importance,
        "importance_normalized": round(importance_norm, 2),
        "created_at": now_iso,
        "last_accessed": now_iso,
        "access_count": 0,
        "decay_strength": 1.0,
        "decay_layer": "ltm" if importance_norm >= PROMOTE_THRESHOLD else "stm",
        "t_valid": candidate.get("temporal", now_iso),
        "t_invalid": None,
        "source": "hot-path-extractor",
        "entities": candidate.get("entities", []),
    }


# ---------------------------------------------------------------------------
# Knowledge graph update (bi-temporal)
# ---------------------------------------------------------------------------

JUNK_ENTITIES = {"user", "cron", "automation", "monitoring", r"D:\VMs\macOS"}

CATEGORY_RELATION_MAP = {
    "preference": "prefers",
    "person": "knows",
    "relationship": "related-to",
    "project": "works-on",
    "work": "works-on",
    "location": "located-at",
    "event": "part-of",
    "plan": "works-on",
    "health": "has",
    "opinion": "prefers",
    "habit": "has",
}


def update_knowledge_graph_bitemporal(entities: list, fact_text: str,
                                       category: str, dry_run: bool = False):
    if not entities:
        return

    if not os.path.isfile(KNOWLEDGE_GRAPH_HOOK):
        return

    # Filter out junk entities
    clean_entities = [
        e.strip() for e in entities
        if e and e.strip() and e.strip().lower() not in {j.lower() for j in JUNK_ENTITIES}
    ]
    if not clean_entities:
        return

    type_mapping = {
        "person": "person", "relationship": "person",
        "project": "project", "work": "project",
        "location": "location", "event": "event",
    }
    entity_type = type_mapping.get(category, "concept")

    # Phase 1: Add all entities
    for entity_name in clean_entities:
        cmd = [
            sys.executable, KNOWLEDGE_GRAPH_HOOK,
            "add-entity",
            "--name", entity_name,
            "--type", entity_type,
        ]
        if dry_run:
            log(f"DRY RUN: {' '.join(cmd)}")
            continue
        try:
            subprocess.run(cmd, capture_output=True, text=True, timeout=15)
        except Exception as e:
            log_warn(f"KG entity add failed for '{entity_name}': {e}")

    # Phase 2: Create relations between entity pairs
    if len(clean_entities) >= 2:
        relation = CATEGORY_RELATION_MAP.get(category, "related-to")
        # Use first entity as source, relate it to each subsequent entity
        source = clean_entities[0]
        for target in clean_entities[1:]:
            if source.lower() == target.lower():
                continue
            cmd = [
                sys.executable, KNOWLEDGE_GRAPH_HOOK,
                "add-relation",
                "--from", source,
                "--relation", relation,
                "--to", target,
                "--confidence", "0.8",
            ]
            if dry_run:
                log(f"DRY RUN: {' '.join(cmd)}")
                continue
            try:
                result = subprocess.run(cmd, capture_output=True, text=True, timeout=15)
                if result.returncode == 0:
                    log(f"KG relation: {source} --[{relation}]--> {target}")
                else:
                    log_warn(f"KG relation failed: {result.stderr.strip()}")
            except Exception as e:
                log_warn(f"KG relation add failed for '{source}' -> '{target}': {e}")

    if not dry_run and os.path.isfile(KNOWLEDGE_GRAPH_PATH):
        _ensure_bitemporal_fields()


def _ensure_bitemporal_fields():
    try:
        # Guard against corrupt/huge KG file
        file_size = os.path.getsize(KNOWLEDGE_GRAPH_PATH)
        if file_size > 50 * 1024 * 1024:  # 50MB safety limit
            log_warn(f"Knowledge graph too large ({file_size / 1024 / 1024:.1f}MB), skipping bitemporal update")
            return
        with open(KNOWLEDGE_GRAPH_PATH, "r") as f:
            graph = json.load(f)
    except (json.JSONDecodeError, OSError):
        return

    modified = False
    now_iso = datetime.datetime.now(datetime.timezone.utc).isoformat()
    for link in graph.get("edges", []):
        for field, default in [("t_valid", link.get("created_at", now_iso)),
                               ("t_invalid", None),
                               ("t_created", link.get("created_at", now_iso)),
                               ("t_expired", None)]:
            if field not in link:
                link[field] = default
                modified = True

    if modified:
        try:
            atomic_write_json(KNOWLEDGE_GRAPH_PATH, graph)
            log("Added bi-temporal fields to knowledge graph edges")
        except OSError as e:
            log_warn(f"Could not update KG: {e}")


# ---------------------------------------------------------------------------
# Main extraction pipeline
# ---------------------------------------------------------------------------

def run_extraction(window_minutes: int, force: bool, dry_run: bool):
    load_env()
    api_key = get_api_key()

    # Startup housekeeping: clean temp files, rotate archive, prune invalidated
    cleanup_temp_files()
    rotate_archive()
    prune_invalidated(max_age_hours=48.0)

    # Disk space guard
    if not check_disk_space():
        record_failure(COMPONENT_NAME, "Disk space critically low")
        return

    # Check marker
    if not force:
        last_processed = read_marker()
        if last_processed:
            try:
                last_ts = datetime.datetime.fromisoformat(last_processed.replace("Z", "+00:00"))
                now = datetime.datetime.now(datetime.timezone.utc)
                minutes_since = (now - last_ts).total_seconds() / 60
                if minutes_since < 3:
                    log(f"Last processed {minutes_since:.1f}m ago, skipping (< 3m)")
                    return
            except (ValueError, TypeError):
                pass

    # Query recent logs
    log(f"Querying agent logs (window: {window_minutes}m)...")
    logs = query_recent_logs(window_minutes)
    if logs is None:
        log_error("Failed to query logs, marker NOT advanced")
        record_failure(COMPONENT_NAME, "Failed to query agent logs")
        return
    if not logs:
        log("No recent logs to process")
        write_marker(datetime.datetime.now(datetime.timezone.utc).isoformat())
        record_success(COMPONENT_NAME)
        return

    # Phase 1: Extract candidate facts
    last_processed = read_marker() if not force else ""
    log("Phase 1: Extracting candidate facts...")
    candidates = extract_facts(api_key, logs, marker_timestamp=last_processed)
    if candidates is None:
        log_error("Extraction failed, marker NOT advanced")
        record_failure(COMPONENT_NAME, "Claude API extraction failed")
        return
    if not candidates:
        log("No facts extracted")
        write_marker(datetime.datetime.now(datetime.timezone.utc).isoformat())
        record_success(COMPONENT_NAME)
        return

    # Filter by importance threshold
    hot_candidates = [c for c in candidates
                      if c.get("importance", 0) >= HOT_PATH_IMPORTANCE_THRESHOLD]
    log(f"  {len(candidates)} total candidates, {len(hot_candidates)} above "
        f"importance threshold ({HOT_PATH_IMPORTANCE_THRESHOLD})")

    if not hot_candidates:
        log("No high-importance facts to process")
        write_marker(datetime.datetime.now(datetime.timezone.utc).isoformat())
        record_success(COMPONENT_NAME)
        return

    # Phase 2: Self-validation (quality gate + duplicate pre-check)
    log("Phase 2: Validating candidates...")
    existing_hot_facts = [
        m.get("fact", "") for m in read_hot_memories()
        if not m.get("metadata", {}).get("t_invalid") and m.get("fact")
    ]
    validated = []
    for candidate in hot_candidates:
        keep, reason = validate_candidate(candidate, existing_hot_facts)
        if keep:
            validated.append(candidate)
        else:
            log(f"  REJECTED: {str(candidate.get('fact', ''))[:60]}... ({reason})")
    log(f"  {len(validated)}/{len(hot_candidates)} passed validation")

    if not validated:
        log("No candidates survived validation")
        write_marker(datetime.datetime.now(datetime.timezone.utc).isoformat())
        record_success(COMPONENT_NAME)
        return

    # Rate limiting: cap additions per hour to prevent extraction bursts
    recent_adds = _count_recent_additions()
    budget = max(0, MAX_ADDITIONS_PER_HOUR - recent_adds)
    if budget == 0:
        log(f"Rate limit: {recent_adds} facts added in last hour (max {MAX_ADDITIONS_PER_HOUR}), deferring")
        write_marker(datetime.datetime.now(datetime.timezone.utc).isoformat())
        record_success(COMPONENT_NAME)
        return
    if len(validated) > budget:
        log(f"Rate limit: capping from {len(validated)} to {budget} candidates (budget remaining)")
        validated = validated[:budget]

    # Phase 3: Reconcile each fact against existing memories
    log("Phase 3: Reconciling against existing memories...")
    added = 0
    updated = 0
    deleted = 0
    skipped = 0
    errors = 0

    for candidate in validated:
        if not isinstance(candidate, dict):
            log_warn(f"Skipping non-dict candidate: {type(candidate).__name__}")
            continue
        fact_text = str(candidate.get("fact", "")).strip()
        if not fact_text:
            continue

        existing = search_existing_memories(fact_text)
        result = reconcile_fact(api_key, candidate, existing)
        operation = result.get("operation", "NOOP")
        reason = result.get("reason", "")
        is_api_fallback = reason in ("API fallback", "parse fallback")

        if operation == "ADD":
            if is_api_fallback:
                log_warn(f"  SKIP (API fallback, not adding blind): {fact_text[:60]}...")
                errors += 1
                continue
            metadata = build_memory_metadata(candidate)
            if dry_run:
                log(f"  DRY RUN ADD: {fact_text[:80]}... (importance={candidate.get('importance')})")
            else:
                append_hot_memory(fact_text, metadata)
                update_knowledge_graph_bitemporal(
                    candidate.get("entities", []), fact_text,
                    candidate.get("category", "general"),
                )
                log(f"  ADD: {fact_text[:80]}...")
            added += 1

        elif operation == "UPDATE":
            updated_text = result.get("updated_fact", fact_text)
            metadata = build_memory_metadata(candidate)
            metadata["updated_from"] = fact_text
            if dry_run:
                log(f"  DRY RUN UPDATE: {updated_text[:80]}...")
            else:
                update_hot_memory(fact_text, updated_text, metadata)
                log(f"  UPDATE: {updated_text[:80]}...")
            updated += 1

        elif operation == "DELETE":
            delete_target = result.get("delete_target", "")
            if dry_run:
                log(f"  DRY RUN DELETE: invalidate '{delete_target[:60]}...'")
            else:
                if delete_target:
                    invalidate_hot_memory(delete_target)
                else:
                    log_warn(f"  DELETE without target for: {fact_text[:60]}...")
            deleted += 1

        else:  # NOOP
            log(f"  NOOP: {fact_text[:60]}... ({result.get('reason', 'duplicate')})")
            skipped += 1

    # Marker advancement — only when no API errors
    if not dry_run:
        if errors == 0:
            write_marker(datetime.datetime.now(datetime.timezone.utc).isoformat())
            record_success(COMPONENT_NAME)
        else:
            log_warn(f"Had {errors} reconciliation errors, marker NOT advanced")
            record_failure(COMPONENT_NAME,
                           f"{errors} reconciliation API errors")

    log(f"Extraction complete: +{added} added, ~{updated} updated, "
        f"-{deleted} deleted, ={skipped} skipped, !{errors} errors")

    # Accumulate daily digest instead of per-run notifications
    if not dry_run and (added + updated + deleted) > 0:
        _accumulate_daily_digest(added, updated, deleted, skipped)


# ---------------------------------------------------------------------------
# Daily digest (accumulate per-run stats, send once daily)
# ---------------------------------------------------------------------------

def _accumulate_daily_digest(added: int, updated: int, deleted: int, skipped: int):
    """Accumulate extraction stats. Send Telegram digest once per day."""
    today = datetime.datetime.now().strftime("%Y-%m-%d")
    data = {}
    if os.path.isfile(DAILY_DIGEST_PATH):
        try:
            with open(DAILY_DIGEST_PATH, "r") as f:
                data = json.load(f)
        except (json.JSONDecodeError, OSError):
            data = {}

    # Reset if new day
    if data.get("date") != today:
        data = {"date": today, "added": 0, "updated": 0, "deleted": 0,
                "skipped": 0, "runs": 0, "sent": False}

    data["added"] = data.get("added", 0) + added
    data["updated"] = data.get("updated", 0) + updated
    data["deleted"] = data.get("deleted", 0) + deleted
    data["skipped"] = data.get("skipped", 0) + skipped
    data["runs"] = data.get("runs", 0) + 1

    # Send digest once per day (after accumulating at least one change)
    # Trigger at first run after 20:00 local time (evening summary)
    hour = datetime.datetime.now().hour
    if not data.get("sent") and hour >= 20 and data["added"] + data["updated"] > 0:
        # Silent -- no Telegram notification. Data is logged and queryable.
        data["sent"] = True

    try:
        atomic_write_json(DAILY_DIGEST_PATH, data)
    except OSError as e:
        log_warn(f"Could not write daily digest: {e}")


def send_daily_digest():
    """Force-send the daily digest (for manual or cron trigger)."""
    load_env()
    if not os.path.isfile(DAILY_DIGEST_PATH):
        log("No digest data to send")
        return
    try:
        with open(DAILY_DIGEST_PATH, "r") as f:
            data = json.load(f)
    except (json.JSONDecodeError, OSError):
        log("Could not read digest data")
        return

    today = datetime.datetime.now().strftime("%Y-%m-%d")
    date = data.get("date", today)
    total = data.get("added", 0) + data.get("updated", 0)

    if total == 0 and data.get("runs", 0) == 0:
        log("No extraction activity to report")
        return

    # Silent -- digest data logged but not pushed to Telegram.
    log(f"Daily digest: {data.get('runs', 0)} runs, +{data.get('added', 0)} added, "
        f"~{data.get('updated', 0)} updated, -{data.get('deleted', 0)} deleted")
    data["sent"] = True
    try:
        atomic_write_json(DAILY_DIGEST_PATH, data)
    except OSError:
        pass
    log("Daily digest sent")


# ---------------------------------------------------------------------------
# Deduplication (one-shot cleanup)
# ---------------------------------------------------------------------------

def deduplicate_hot_memories():
    """Remove duplicate hot memories, keeping the newest of each unique fact."""
    lock_fd = hot_memory_lock()
    try:
        memories = read_hot_memories()
        if not memories:
            log("No hot memories to deduplicate")
            return

        # Group by normalized fact text (case-insensitive, stripped)
        seen = {}
        for mem in memories:
            fact = mem.get("fact", "").strip().lower()
            if not fact:
                continue
            created = mem.get("metadata", {}).get("created_at", "")
            if fact in seen:
                # Keep the one with the newer created_at
                existing_created = seen[fact].get("metadata", {}).get("created_at", "")
                if created > existing_created:
                    seen[fact] = mem
            else:
                seen[fact] = mem

        deduped = list(seen.values())
        removed = len(memories) - len(deduped)

        if removed > 0:
            write_hot_memories(deduped)
            log(f"Deduplicated: removed {removed} duplicates, kept {len(deduped)} unique memories")
            # Silent -- dedup results logged only.
            log(f"Dedup: removed {removed} duplicates, {len(deduped)} unique remain")
        else:
            log(f"No duplicates found ({len(memories)} memories all unique)")
    finally:
        hot_memory_unlock(lock_fd)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="AetherVault Real-Time Memory Extraction",
    )
    parser.add_argument("--window", type=int, default=DEFAULT_WINDOW_MINUTES,
                        help=f"Look back N minutes (default: {DEFAULT_WINDOW_MINUTES})")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--dedup", action="store_true",
                        help="Deduplicate hot memories and exit")
    parser.add_argument("--send-digest", action="store_true",
                        help="Force-send the daily digest and exit")
    args = parser.parse_args()

    if args.dedup:
        log("=== Deduplicating hot memories ===")
        deduplicate_hot_memories()
        return

    if args.send_digest:
        log("=== Sending daily digest ===")
        send_daily_digest()
        return

    log("=== AetherVault Real-Time Memory Extraction ===")
    if args.dry_run:
        log("DRY RUN mode")

    acquire_instance_lock()

    try:
        run_extraction(args.window, args.force, args.dry_run)
    except KeyboardInterrupt:
        log("Interrupted")
        sys.exit(130)
    except Exception as e:
        log_error(f"Unexpected error: {e}")
        record_failure(COMPONENT_NAME, str(e))
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()

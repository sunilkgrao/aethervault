#!/usr/bin/env python3
"""
AetherVault Hot Memory Store — Shared Module
=============================================

Single source of truth for hot memory file operations, used by:
- memory-extractor.py (cron every 5 min)
- memory-scorer.py (on-demand by agent)
- weekly-reflection.py (cron weekly)
- memory-health.py (health check + self-healing)

Eliminates divergent copies. All hot memory read/write/lock operations
go through this module.
"""

import datetime
import fcntl
import json
import math
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
CAPSULE_PATH = os.environ.get("CAPSULE_PATH", os.path.join(AETHERVAULT_HOME, "memory.mv2"))
AETHERVAULT_BIN = os.environ.get("AETHERVAULT_BIN", "/usr/local/bin/aethervault")
HOT_MEMORY_PATH = os.path.join(AETHERVAULT_HOME, "data", "hot-memories.jsonl")
LOCK_PATH = os.path.join(AETHERVAULT_HOME, "data", "hot-memories.lock")
ARCHIVE_PATH = os.path.join(AETHERVAULT_HOME, "data", "hot-memories-archive.jsonl")
HEALTH_PATH = os.path.join(AETHERVAULT_HOME, "data", "memory-health.json")
FAILURE_PATH = os.path.join(AETHERVAULT_HOME, "data", "extractor-failures.json")
ENV_FILE = os.path.join(AETHERVAULT_HOME, ".env")
OWNER_NAME = os.environ.get("OWNER_NAME", "the user")

# Buffer limits
MAX_HOT_MEMORIES = 200
MAX_PINNED_MEMORIES = 50
MAX_ARCHIVE_LINES = 10000
TEMP_FILE_MAX_AGE_SECONDS = 600  # 10 min
MIN_DISK_FREE_MB = 100  # don't write if less than this free

# FadeMem decay parameters
LAMBDA_BASE = 0.1
MU = 2.0
BETA_LTM = 0.8
BETA_STM = 1.2
PROMOTE_THRESHOLD = 0.7

# Recency decay (Generative Agents paper: 0.995 per game-hour)
RECENCY_DECAY_RATE = 0.995

# Alert thresholds
CONSECUTIVE_FAILURE_ALERT_THRESHOLD = 3
MARKER_STALE_MINUTES = 30

# Claude API config
CLAUDE_API_URL = os.environ.get("CLAUDE_API_URL", "http://127.0.0.1:11436/v1/messages")
CLAUDE_API_VERSION = os.environ.get("CLAUDE_API_VERSION", "2023-06-01")
MAX_RETRIES = 2
RETRY_DELAY_SECONDS = 3


# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------

def log(msg: str, level: str = "INFO"):
    ts = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{ts}] [{level}] {msg}", flush=True)


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
        log_error("ANTHROPIC_API_KEY not set")
        sys.exit(1)
    return key


# ---------------------------------------------------------------------------
# File locking
# ---------------------------------------------------------------------------

LOCK_TIMEOUT_SECONDS = 30  # max time to wait for file lock

def hot_memory_lock(timeout: float = LOCK_TIMEOUT_SECONDS):
    """Acquire advisory lock with timeout (prevents indefinite blocking)."""
    os.makedirs(os.path.dirname(LOCK_PATH), exist_ok=True)
    fd = os.open(LOCK_PATH, os.O_CREAT | os.O_WRONLY)
    deadline = time.monotonic() + timeout
    while True:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return fd
        except (OSError, BlockingIOError):
            if time.monotonic() >= deadline:
                os.close(fd)
                raise TimeoutError(
                    f"Could not acquire hot-memory lock within {timeout}s"
                )
            time.sleep(0.1)


def hot_memory_unlock(fd):
    """Release hot-memories lock."""
    try:
        fcntl.flock(fd, fcntl.LOCK_UN)
    except OSError as e:
        log_warn(f"Failed to unlock: {e}")
    finally:
        try:
            os.close(fd)
        except OSError:
            pass


# ---------------------------------------------------------------------------
# Hot memory read/write (single source of truth)
# ---------------------------------------------------------------------------

def read_hot_memories() -> list:
    """Read hot memories from JSONL file.

    Raises OSError if file exists but cannot be read (prevents silent data loss).
    Skips and logs corrupt JSON lines (partial write recovery).
    """
    if not os.path.isfile(HOT_MEMORY_PATH):
        return []
    # Guard against corrupt/huge files (expect < 5MB for 200 memories)
    try:
        file_size = os.path.getsize(HOT_MEMORY_PATH)
        if file_size > 10 * 1024 * 1024:  # 10MB safety limit
            log_error(f"Hot memories file too large ({file_size / 1024 / 1024:.1f}MB), refusing to load")
            return []
    except OSError:
        pass  # proceed anyway, open() will fail if unreadable
    memories = []
    corrupt_lines = 0
    with open(HOT_MEMORY_PATH, "r") as f:
        for line in f:
            line = line.strip()
            if line:
                try:
                    memories.append(json.loads(line))
                except json.JSONDecodeError:
                    corrupt_lines += 1
                    continue
    if corrupt_lines > 0:
        log_warn(f"Skipped {corrupt_lines} corrupt lines in hot memories")
    return memories


def _archive_evicted(evicted: list):
    """Append evicted memories to archive file before dropping them."""
    if not evicted:
        return
    try:
        os.makedirs(os.path.dirname(ARCHIVE_PATH), exist_ok=True)
        with open(ARCHIVE_PATH, "a") as f:
            now_iso = datetime.datetime.now(datetime.timezone.utc).isoformat()
            for mem in evicted:
                mem.setdefault("metadata", {})["evicted_at"] = now_iso
                f.write(json.dumps(mem) + "\n")
        log(f"Archived {len(evicted)} evicted memories")
    except OSError as e:
        log_warn(f"Could not archive evicted memories: {e}")


def write_hot_memories(memories: list):
    """Write hot memories to JSONL file atomically, maintaining rolling buffer.

    Handles: eviction with archive, pinned memory cap, atomic rename.
    """
    if len(memories) > MAX_HOT_MEMORIES:
        pinned = [m for m in memories if m.get("metadata", {}).get("pinned")]
        unpinned = [m for m in memories if not m.get("metadata", {}).get("pinned")]
        # Cap pinned entries
        if len(pinned) > MAX_PINNED_MEMORIES:
            pinned_evicted = pinned[:-MAX_PINNED_MEMORIES]
            _archive_evicted(pinned_evicted)
            pinned = pinned[-MAX_PINNED_MEMORIES:]
            log_warn(f"Pinned memories exceeded cap ({MAX_PINNED_MEMORIES}), "
                     f"archived {len(pinned_evicted)} oldest")
        # Evict unpinned
        budget = MAX_HOT_MEMORIES - len(pinned)
        if budget > 0 and len(unpinned) > budget:
            evicted = unpinned[:-budget]
            _archive_evicted(evicted)
            unpinned = unpinned[-budget:]
        memories = pinned + unpinned

    os.makedirs(os.path.dirname(HOT_MEMORY_PATH), exist_ok=True)
    try:
        fd, tmp_path = tempfile.mkstemp(
            dir=os.path.dirname(HOT_MEMORY_PATH),
            prefix=".hot-memories-", suffix=".tmp",
        )
        try:
            with os.fdopen(fd, "w") as f:
                for mem in memories:
                    f.write(json.dumps(mem) + "\n")
            os.replace(tmp_path, HOT_MEMORY_PATH)
        except Exception:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass
            raise
    except OSError as e:
        log_error(f"Failed to write hot memories: {e}")


def append_hot_memory(fact_text: str, metadata: dict):
    """Append a single memory (with file locking and duplicate guard).

    Skips appending if an existing non-invalidated memory has the exact same
    fact text (case-insensitive).  This is a last-resort safety net; callers
    should still do their own reconciliation for near-duplicate detection.
    """
    lock_fd = hot_memory_lock()
    try:
        memories = read_hot_memories()
        fact_lower = fact_text.strip().lower()
        for mem in memories:
            existing = mem.get("fact", "").strip().lower()
            if existing == fact_lower and not mem.get("metadata", {}).get("t_invalid"):
                log_warn(f"Duplicate blocked in append_hot_memory: {fact_text[:60]}...")
                return
        memories.append({"fact": fact_text, "metadata": metadata})
        write_hot_memories(memories)
    finally:
        hot_memory_unlock(lock_fd)


def invalidate_hot_memory(target_fact: str):
    """Mark matching hot memories as invalid (bi-temporal DELETE, with locking)."""
    lock_fd = hot_memory_lock()
    try:
        memories = read_hot_memories()
        now_iso = datetime.datetime.now(datetime.timezone.utc).isoformat()
        invalidated = 0
        for mem in memories:
            if target_fact.lower() in mem.get("fact", "").lower():
                mem.setdefault("metadata", {})["t_invalid"] = now_iso
                log(f"  Invalidated: {mem.get('fact', '')[:60]}...")
                invalidated += 1
        if invalidated > 0:
            write_hot_memories(memories)
        else:
            log_warn(f"No memory matched for invalidation: {target_fact[:60]}...")
    finally:
        hot_memory_unlock(lock_fd)


def update_hot_memory(old_fact: str, new_fact: str, metadata: dict):
    """UPDATE: invalidate old entry, add new one (with locking)."""
    lock_fd = hot_memory_lock()
    try:
        memories = read_hot_memories()
        now_iso = datetime.datetime.now(datetime.timezone.utc).isoformat()
        for mem in memories:
            if old_fact.lower() in mem.get("fact", "").lower():
                mem.setdefault("metadata", {})["t_invalid"] = now_iso
        memories.append({"fact": new_fact, "metadata": metadata})
        write_hot_memories(memories)
    finally:
        hot_memory_unlock(lock_fd)


# ---------------------------------------------------------------------------
# Disk space guard
# ---------------------------------------------------------------------------

def check_disk_space() -> bool:
    """Return True if sufficient disk space is available for writes."""
    try:
        stat = os.statvfs(os.path.dirname(HOT_MEMORY_PATH))
        free_mb = (stat.f_bavail * stat.f_frsize) / (1024 * 1024)
        if free_mb < MIN_DISK_FREE_MB:
            log_error(f"Disk critically low: {free_mb:.0f}MB free (min {MIN_DISK_FREE_MB}MB)")
            return False
        return True
    except OSError:
        return True  # can't check, proceed anyway


# ---------------------------------------------------------------------------
# Temp file cleanup
# ---------------------------------------------------------------------------

def cleanup_temp_files():
    """Remove stale .tmp files from the data directory (orphans from crashes)."""
    data_dir = os.path.dirname(HOT_MEMORY_PATH)
    if not os.path.isdir(data_dir):
        return
    now = time.time()
    cleaned = 0
    for name in os.listdir(data_dir):
        if name.endswith(".tmp") and name.startswith("."):
            filepath = os.path.join(data_dir, name)
            try:
                age = now - os.path.getmtime(filepath)
                if age > TEMP_FILE_MAX_AGE_SECONDS:
                    os.unlink(filepath)
                    cleaned += 1
            except OSError:
                continue
    if cleaned > 0:
        log(f"Cleaned {cleaned} stale temp files")


# ---------------------------------------------------------------------------
# Archive rotation
# ---------------------------------------------------------------------------

def rotate_archive():
    """Keep archive file under MAX_ARCHIVE_LINES by truncating oldest entries."""
    if not os.path.isfile(ARCHIVE_PATH):
        return
    try:
        # Guard against corrupt/huge archive (expect < 50MB for 10k lines)
        file_size = os.path.getsize(ARCHIVE_PATH)
        if file_size > 100 * 1024 * 1024:  # 100MB safety limit
            log_error(f"Archive file too large ({file_size / 1024 / 1024:.1f}MB), skipping rotation")
            return
        with open(ARCHIVE_PATH, "r") as f:
            lines = f.readlines()
        if len(lines) <= MAX_ARCHIVE_LINES:
            return
        # Keep only the newest entries
        trimmed = lines[-MAX_ARCHIVE_LINES:]
        dropped = len(lines) - len(trimmed)
        fd, tmp_path = tempfile.mkstemp(
            dir=os.path.dirname(ARCHIVE_PATH),
            prefix=".archive-", suffix=".tmp",
        )
        try:
            with os.fdopen(fd, "w") as f:
                f.writelines(trimmed)
            os.replace(tmp_path, ARCHIVE_PATH)
            log(f"Rotated archive: dropped {dropped} oldest, kept {len(trimmed)}")
        except Exception:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass
            raise
    except OSError as e:
        log_warn(f"Archive rotation failed: {e}")


# ---------------------------------------------------------------------------
# Invalidated memory pruning
# ---------------------------------------------------------------------------

def prune_invalidated(max_age_hours: float = 24.0):
    """Remove memories with t_invalid older than max_age_hours (with locking)."""
    lock_fd = hot_memory_lock()
    try:
        memories = read_hot_memories()
        now = datetime.datetime.now(datetime.timezone.utc)
        cutoff = now - datetime.timedelta(hours=max_age_hours)
        keep = []
        pruned = 0
        for mem in memories:
            t_invalid = mem.get("metadata", {}).get("t_invalid")
            if t_invalid:
                try:
                    inv_dt = datetime.datetime.fromisoformat(
                        t_invalid.replace("Z", "+00:00")
                    )
                    if inv_dt < cutoff:
                        pruned += 1
                        continue  # drop this memory
                except (ValueError, TypeError):
                    pass
            keep.append(mem)
        if pruned > 0:
            write_hot_memories(keep)
            log(f"Pruned {pruned} invalidated memories older than {max_age_hours}h")
    finally:
        hot_memory_unlock(lock_fd)


# ---------------------------------------------------------------------------
# Failure tracking
# ---------------------------------------------------------------------------

def record_failure(component: str, error: str):
    """Record a failure for tracking consecutive errors."""
    os.makedirs(os.path.dirname(FAILURE_PATH), exist_ok=True)
    data = _read_failure_data()
    data.setdefault("failures", {})
    data["failures"].setdefault(component, {"count": 0, "last_error": "", "first_at": ""})
    entry = data["failures"][component]
    entry["count"] += 1
    entry["last_error"] = error[:200]
    entry["last_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    if not entry["first_at"]:
        entry["first_at"] = entry["last_at"]

    _write_failure_data(data)

    # Alert on consecutive failures
    if entry["count"] >= CONSECUTIVE_FAILURE_ALERT_THRESHOLD:
        if entry["count"] == CONSECUTIVE_FAILURE_ALERT_THRESHOLD:
            send_telegram(
                f"[ALERT] {component} has failed {entry['count']} consecutive times.\n"
                f"Last error: {error[:100]}\n"
                f"Since: {entry['first_at']}"
            )


def record_success(component: str):
    """Reset failure counter on success, archiving failure history."""
    data = _read_failure_data()
    if component in data.get("failures", {}):
        prev_count = data["failures"][component].get("count", 0)
        if prev_count >= CONSECUTIVE_FAILURE_ALERT_THRESHOLD:
            send_telegram(
                f"[RECOVERED] {component} recovered after {prev_count} consecutive failures."
            )
        # Archive failure history instead of deleting
        entry = data["failures"][component]
        entry["recovered_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        data.setdefault("history", [])
        data["history"].append(entry)
        # Cap history at 100 entries
        if len(data["history"]) > 100:
            data["history"] = data["history"][-100:]
        del data["failures"][component]
        _write_failure_data(data)


def _read_failure_data() -> dict:
    if os.path.isfile(FAILURE_PATH):
        try:
            with open(FAILURE_PATH, "r") as f:
                return json.load(f)
        except (json.JSONDecodeError, OSError):
            pass
    return {}


def _write_failure_data(data: dict):
    try:
        fd, tmp_path = tempfile.mkstemp(
            dir=os.path.dirname(FAILURE_PATH),
            prefix=".failures-", suffix=".tmp",
        )
        try:
            with os.fdopen(fd, "w") as f:
                json.dump(data, f, indent=2)
            os.replace(tmp_path, FAILURE_PATH)
        except Exception:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass
            raise
    except OSError as e:
        log_warn(f"Could not write failure data: {e}")


# ---------------------------------------------------------------------------
# Atomic file write helper
# ---------------------------------------------------------------------------

def atomic_write_json(filepath: str, data):
    """Write JSON data to a file atomically."""
    os.makedirs(os.path.dirname(filepath), exist_ok=True)
    fd, tmp_path = tempfile.mkstemp(
        dir=os.path.dirname(filepath),
        prefix="." + os.path.basename(filepath) + "-",
        suffix=".tmp",
    )
    try:
        with os.fdopen(fd, "w") as f:
            json.dump(data, f, indent=2)
        os.replace(tmp_path, filepath)
    except Exception:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass
        raise


# ---------------------------------------------------------------------------
# Capsule search
# ---------------------------------------------------------------------------

def search_capsule(query: str, collections: list = None, limit: int = 10) -> list:
    """Search capsule memories across collections."""
    if not os.path.isfile(AETHERVAULT_BIN):
        binary = "aethervault"
    else:
        binary = AETHERVAULT_BIN

    if collections is None:
        collections = ["aethervault-memory", "people", "roam-notes"]

    results = []
    for collection in collections:
        cmd = [
            binary, "query",
            "--collection", collection,
            "--limit", str(limit),
            CAPSULE_PATH,
            query,
        ]
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=15)
            if result.returncode == 0 and result.stdout.strip():
                chunks = [c.strip() for c in result.stdout.strip().split("\n\n")
                          if c.strip()]
                results.extend(chunks[:limit])
        except FileNotFoundError:
            log_warn(f"aethervault binary not found: {binary}")
            break  # no point trying other collections
        except subprocess.TimeoutExpired:
            log_warn(f"Capsule search timed out for collection '{collection}'")
            continue
        except Exception as e:
            log_warn(f"Capsule search error for '{collection}': {e}")
            continue

    return results[:limit]


# ---------------------------------------------------------------------------
# Claude API
# ---------------------------------------------------------------------------

def call_claude(api_key: str, system_prompt: str, user_message: str,
                max_tokens: int = 2048, model: str = None,
                timeout: int = 60) -> str:
    """Call Claude API with retry logic. Returns text response or empty string on failure."""
    if model is None:
        model = os.environ.get("EXTRACTOR_MODEL",
                               os.environ.get("REFLECTION_MODEL", "claude-sonnet-4-5"))
    payload = {
        "model": model,
        "max_tokens": max_tokens,
        "system": system_prompt,
        "messages": [{"role": "user", "content": user_message}],
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
                CLAUDE_API_URL, data=data, headers=headers, method="POST",
            )
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                body = json.loads(resp.read().decode("utf-8"))

            content_blocks = body.get("content", [])
            text_parts = [b["text"] for b in content_blocks if b.get("type") == "text"]
            result = "\n".join(text_parts)

            usage = body.get("usage", {})
            log(f"Claude API: in={usage.get('input_tokens', '?')} "
                f"out={usage.get('output_tokens', '?')}")
            return result

        except urllib.error.HTTPError as e:
            err_body = ""
            try:
                err_body = e.read().decode("utf-8", errors="replace")[:300]
            except Exception:
                pass
            log_error(f"Claude API HTTP {e.code} (attempt {attempt}): {err_body}")
            if e.code in (429, 500, 502, 503, 529) and attempt < MAX_RETRIES:
                time.sleep(RETRY_DELAY_SECONDS * attempt)
                continue
            return ""
        except Exception as e:
            log_error(f"Claude API error (attempt {attempt}): {e}")
            if attempt < MAX_RETRIES:
                time.sleep(RETRY_DELAY_SECONDS * attempt)
                continue
            return ""
    return ""


def parse_claude_json(raw: str) -> dict:
    """Parse JSON from Claude response, stripping markdown fences."""
    cleaned = raw.strip()
    if cleaned.startswith("```"):
        lines = cleaned.split("\n")
        if lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip() == "```":
            lines = lines[:-1]
        cleaned = "\n".join(lines)
    return json.loads(cleaned)


# ---------------------------------------------------------------------------
# Telegram notification
# ---------------------------------------------------------------------------

def send_telegram(text: str):
    """Send a Telegram notification. Logs failure instead of silently swallowing."""
    token = os.environ.get("TELEGRAM_BOT_TOKEN", "")
    chat_id = os.environ.get("TELEGRAM_CHAT_ID", "")

    if not chat_id:
        try:
            cfg_path = os.path.join(AETHERVAULT_HOME, "config", "briefing.json")
            with open(cfg_path) as f:
                cfg = json.load(f)
                chat_id = str(cfg.get("chat_id", ""))
        except Exception:
            pass

    if not token or not chat_id:
        return

    try:
        data = json.dumps({"chat_id": chat_id, "text": text}).encode()
        req = urllib.request.Request(
            f"https://api.telegram.org/bot{token}/sendMessage",
            data=data,
            headers={"Content-Type": "application/json"},
        )
        urllib.request.urlopen(req, timeout=10)
    except Exception as e:
        log_warn(f"Telegram notification failed: {e}")


# ---------------------------------------------------------------------------
# FadeMem decay computation
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# Hot memory text search (for reconciliation)
# ---------------------------------------------------------------------------

def search_hot_memories_text(query: str, limit: int = 5) -> list:
    """Search hot memories by text similarity (case-insensitive substring + word overlap).

    Returns a list of fact strings that match, suitable for feeding into
    the reconciliation phase alongside capsule results.
    """
    if not query or not query.strip():
        return []

    memories = read_hot_memories()
    if not memories:
        return []

    query_lower = query.lower()
    query_words = set(query_lower.split())

    scored = []
    for mem in memories:
        fact = mem.get("fact", "")
        if not fact:
            continue
        # Skip invalidated memories
        if mem.get("metadata", {}).get("t_invalid"):
            continue

        fact_lower = fact.lower()

        # Score: substring match = high, word overlap = proportional
        if query_lower in fact_lower or fact_lower in query_lower:
            score = 1.0
        else:
            fact_words = set(fact_lower.split())
            overlap = len(query_words & fact_words)
            if overlap == 0:
                continue
            score = overlap / max(len(query_words), 1)
            # Require at least 30% word overlap to be relevant
            if score < 0.3:
                continue

        scored.append((score, fact))

    # Sort by score descending, return top N fact texts
    scored.sort(key=lambda x: -x[0])
    return [fact for _, fact in scored[:limit]]


def compute_decay_strength(importance_normalized: float, days_elapsed: float) -> float:
    """FadeMem-style exponential decay with importance-modulated rate."""
    lambda_i = LAMBDA_BASE * math.exp(-MU * importance_normalized)
    beta = BETA_LTM if importance_normalized >= PROMOTE_THRESHOLD else BETA_STM
    if days_elapsed <= 0:
        return 1.0
    strength = math.exp(-lambda_i * (days_elapsed ** beta))
    return max(0.0, min(1.0, strength))


def compute_recency(hours_since_access: float) -> float:
    """Generative Agents recency score: 0.995 ^ hours_since_last_accessed."""
    if hours_since_access <= 0:
        return 1.0
    return RECENCY_DECAY_RATE ** hours_since_access

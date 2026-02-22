#!/usr/bin/env python3
"""
Shared utilities for AetherVault hooks.

Consolidates duplicated code across hook scripts:
- Environment & configuration loading
- Logging
- Telegram notifications
- Claude API calls
- Hook request/response helpers
"""
import datetime
import json
import os
import sys
import time
import urllib.error
import urllib.request

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
ENV_FILE = os.path.join(AETHERVAULT_HOME, ".env")

CLAUDE_API_URL = os.environ.get("CLAUDE_API_URL", "http://127.0.0.1:11436/v1/messages")
CLAUDE_API_VERSION = os.environ.get("CLAUDE_API_VERSION", "2023-06-01")


# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------

def log(msg: str, level: str = "INFO"):
    """Print a timestamped log message to stderr."""
    ts = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{ts}] [{level}] {msg}", flush=True)


def log_error(msg: str):
    log(msg, level="ERROR")


def log_warn(msg: str):
    log(msg, level="WARN")


# ---------------------------------------------------------------------------
# Environment loading
# ---------------------------------------------------------------------------

def load_env():
    """Load all environment variables from .env file into os.environ (setdefault)."""
    if not os.path.isfile(ENV_FILE):
        return
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


def load_env_var(key: str) -> str:
    """Load a single var from environment or .env file."""
    val = os.environ.get(key, "")
    if val:
        return val
    if os.path.exists(ENV_FILE):
        try:
            with open(ENV_FILE) as f:
                for line in f:
                    line = line.strip()
                    if line and not line.startswith('#') and '=' in line:
                        k, _, v = line.partition('=')
                        if k.strip() == key:
                            return v.strip()
        except OSError:
            pass
    return ""


def get_api_key() -> str:
    """Get ANTHROPIC_API_KEY from environment. Exits if not set."""
    key = os.environ.get("ANTHROPIC_API_KEY", "")
    if not key:
        log_error("ANTHROPIC_API_KEY not set")
        sys.exit(1)
    return key


# ---------------------------------------------------------------------------
# Telegram
# ---------------------------------------------------------------------------

def _resolve_chat_id() -> str:
    """Resolve Telegram chat_id from env or briefing config."""
    chat_id = load_env_var("TELEGRAM_CHAT_ID")
    if chat_id:
        return chat_id
    try:
        cfg_path = os.path.join(AETHERVAULT_HOME, "config", "briefing.json")
        with open(cfg_path) as f:
            cfg = json.load(f)
            return str(cfg.get("chat_id", ""))
    except Exception:
        return ""


def send_telegram(text: str):
    """Send a Telegram message (best-effort, fire-and-forget)."""
    token = load_env_var("TELEGRAM_BOT_TOKEN")
    chat_id = _resolve_chat_id()
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
    except Exception:
        pass


def send_typing(chat_id: str = None, token: str = None):
    """Send typing indicator to Telegram."""
    if token is None:
        token = load_env_var("TELEGRAM_BOT_TOKEN")
    if chat_id is None:
        chat_id = _resolve_chat_id()
    if not token or not chat_id:
        return
    try:
        data = json.dumps({"chat_id": chat_id, "action": "typing"}).encode()
        req = urllib.request.Request(
            f"https://api.telegram.org/bot{token}/sendChatAction",
            data=data,
            headers={"Content-Type": "application/json"},
        )
        urllib.request.urlopen(req, timeout=5)
    except Exception:
        pass


# ---------------------------------------------------------------------------
# Claude API
# ---------------------------------------------------------------------------

def call_claude(api_key: str, system_prompt: str, user_message: str,
                max_tokens: int = 2048, model: str = None,
                timeout: int = 60, max_retries: int = 2,
                retry_delay: float = 3) -> str:
    """Call Claude API with retry logic. Returns text response or empty string."""
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

    for attempt in range(1, max_retries + 1):
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
            if e.code in (429, 500, 502, 503, 529) and attempt < max_retries:
                time.sleep(retry_delay * attempt)
                continue
            return ""
        except Exception as e:
            log_error(f"Claude API error (attempt {attempt}): {e}")
            if attempt < max_retries:
                time.sleep(retry_delay * attempt)
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
# Hook request/response helpers
# ---------------------------------------------------------------------------

def extract_last_user_message(messages) -> str:
    """Extract the last user message text from a messages array."""
    if not isinstance(messages, list):
        return ""
    for msg in reversed(messages):
        if not isinstance(msg, dict):
            continue
        if msg.get("role") == "user" and msg.get("content"):
            content = msg["content"]
            if isinstance(content, str):
                return content
            if isinstance(content, list):
                parts = [b.get("text", "") for b in content
                         if isinstance(b, dict) and b.get("type") == "text"]
                return "\n".join(parts)
    return ""


def format_elapsed(seconds) -> str:
    """Format seconds as Xm Ys."""
    m, s = divmod(int(seconds), 60)
    return f"{m}m {s}s"


def make_hook_error_response(error_msg: str) -> str:
    """Create a JSON error response for a hook."""
    return json.dumps({
        "message": {
            "role": "assistant",
            "content": error_msg,
            "tool_calls": []
        }
    })


def make_hook_response(content: str) -> str:
    """Create a JSON success response for a hook."""
    return json.dumps({
        "message": {
            "role": "assistant",
            "content": content,
            "tool_calls": []
        }
    })


# ---------------------------------------------------------------------------
# Rate limit detection
# ---------------------------------------------------------------------------

def is_rate_limit_error(stderr_text: str, exit_code: int, patterns: list = None) -> bool:
    """Detect rate limit from exit code and stderr patterns.

    Args:
        stderr_text: stderr output to check
        exit_code: process exit code
        patterns: optional list of patterns to check (default: common rate limit signals)

    Returns:
        True if rate limit detected, False otherwise
    """
    if patterns is None:
        patterns = [
            "rate limit", "429", "too many requests", "quota exceeded",
            "ratelimiterror", "overloaded", "capacity",
        ]
    if exit_code == 429:
        return True
    lower = stderr_text.lower()
    return any(pat in lower for pat in patterns)


# ---------------------------------------------------------------------------
# Utility helpers
# ---------------------------------------------------------------------------

def tail_file(filepath: str, n_lines: int = 5, max_chars: int = 400) -> str:
    """Read the last N lines of a file, capped at max_chars total.

    Args:
        filepath: path to file
        n_lines: number of lines to read
        max_chars: max characters to return

    Returns:
        tail text or error message
    """
    try:
        file_size = os.path.getsize(filepath)
        if file_size == 0:
            return "(no output yet)"
        read_size = min(file_size, 8192)
        with open(filepath, "r", errors="replace") as f:
            f.seek(max(0, file_size - read_size))
            chunk = f.read(read_size)
        lines = chunk.splitlines()
        tail = [l.rstrip() for l in lines[-n_lines:] if l.strip()]
        if not tail:
            return "(no output yet)"
        result = "\n".join(tail)
        if len(result) > max_chars:
            result = "..." + result[-(max_chars - 3):]
        return result
    except (OSError, IOError):
        return "(output not available)"


def get_file_stats(filepath: str) -> tuple:
    """Get file stats: approximate line count, byte size.

    Args:
        filepath: path to file

    Returns:
        Tuple of (line_count, byte_size)
    """
    try:
        size = os.path.getsize(filepath)
        if size == 0:
            return 0, 0
        if size <= 1024 * 1024:
            with open(filepath, "r", errors="replace") as f:
                lines = sum(1 for _ in f)
        else:
            with open(filepath, "r", errors="replace") as f:
                sample = f.read(8192)
            sample_lines = sample.count("\n") or 1
            avg_line_len = len(sample) / sample_lines
            lines = int(size / avg_line_len) if avg_line_len > 0 else 0
        return lines, size
    except (OSError, IOError):
        return 0, 0


def format_file_size(size_bytes: int) -> str:
    """Format bytes as human-readable size string.

    Args:
        size_bytes: size in bytes

    Returns:
        Formatted size string (e.g., "1.5MB")
    """
    if size_bytes < 1024 * 1024:
        return f"{size_bytes / 1024:.1f}KB"
    else:
        return f"{size_bytes / (1024 * 1024):.1f}MB"

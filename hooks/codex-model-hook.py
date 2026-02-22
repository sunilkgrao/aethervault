#!/usr/bin/env python3
"""
Codex CLI model hook for AetherVault subagent system.
Reads AgentHookRequest JSON from stdin, extracts the user prompt,
runs Codex CLI, and returns an AgentHookResponse on stdout.

Sends periodic Telegram progress updates during long-running Codex sessions
with real-time status: what's happening now, output progress, full prompt.

Process group isolation and stdin/stdout capping are handled by the Rust binary.
This hook only manages its own Codex subprocess lifecycle.
"""
import json
import os
import subprocess
import sys
import tempfile
import threading
import time

# Add hooks directory to path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pool_state
from common import (
    AETHERVAULT_HOME, send_telegram, send_typing, extract_last_user_message,
    format_elapsed, make_hook_response, make_hook_error_response,
)

CODEX_TIMEOUT = None  # No timeout — Codex tasks can run for hours/days
PROGRESS_INTERVAL = 60  # Check every 60 seconds
TEXT_UPDATE_INTERVAL = 120  # Send text update every 2 minutes
PROGRESS_BAR_WIDTH = 14


def tail_file(filepath, n_lines=5, max_chars=400):
    """Read the last N lines of a file, capped at max_chars total."""
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


def count_output_stats(filepath):
    """Get output file stats: approximate line count, byte size."""
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


def parse_progress_line(raw_line):
    """Parse a Codex progress JSON line."""
    try:
        payload = json.loads(raw_line)
    except (json.JSONDecodeError, TypeError, ValueError):
        return None
    if not isinstance(payload, dict):
        return None
    percent = payload.get("percent", payload.get("progress"))
    if percent is None:
        return None
    try:
        percent = float(percent)
    except (TypeError, ValueError):
        return None
    if percent > 1.0 and percent <= 100.0:
        pass
    elif percent <= 1.0:
        percent = percent * 100.0
    else:
        return None
    milestone = (
        payload.get("milestone")
        or payload.get("stage")
        or payload.get("phase")
        or payload.get("status")
        or "progress"
    )
    if not isinstance(milestone, str) or not milestone:
        milestone = "progress"
    message = payload.get("message") or payload.get("text")
    if not isinstance(message, str) or not message.strip():
        message = None
    return milestone, percent, message


def render_progress_bar(percent):
    pct = min(max(percent, 0.0), 100.0)
    filled = int(round((pct / 100.0) * PROGRESS_BAR_WIDTH))
    filled = min(PROGRESS_BAR_WIDTH, max(0, filled))
    return f"{'█' * filled}{'░' * (PROGRESS_BAR_WIDTH - filled)}"


def progress_reporter(full_prompt, output_path, start_time, stop_event):
    """Background thread: sends real-time Telegram updates with actual progress."""
    update_num = 0
    last_line_count = 0
    last_offset = 0
    line_fragment = ""
    latest_percent = None
    latest_milestone = None
    latest_message = None

    while not stop_event.is_set():
        stop_event.wait(PROGRESS_INTERVAL)
        if stop_event.is_set():
            break
        update_num += 1
        elapsed = time.time() - start_time

        try:
            with open(output_path, "r", errors="replace") as f:
                f.seek(last_offset)
                chunk = f.read()
        except (OSError, IOError):
            chunk = ""
        if chunk:
            last_offset += len(chunk)
            combined = f"{line_fragment}{chunk}"
            lines = combined.split("\n")
            line_fragment = ""
            if combined and not combined.endswith("\n") and lines:
                line_fragment = lines.pop()
            for raw_line in lines:
                parsed = parse_progress_line(raw_line.strip())
                if parsed is None:
                    continue
                milestone, percent, message = parsed
                latest_percent = percent
                latest_milestone = milestone
                latest_message = message

        send_typing()

        if (update_num * PROGRESS_INTERVAL) % TEXT_UPDATE_INTERVAL != 0:
            continue

        if latest_percent is not None and latest_milestone:
            bar = render_progress_bar(latest_percent)
            msg = (
                f"[Codex] {format_elapsed(elapsed)} elapsed\n"
                f"Progress: {bar} {latest_percent:.1f}%\n"
                f"Milestone: {latest_milestone}"
            )
            if latest_message:
                msg += f"\n{latest_message}"
            msg += f"\n\nPrompt:\n{full_prompt[:500]}"
            if len(full_prompt) > 500:
                msg += f"\n... ({len(full_prompt)} chars total)"
            send_telegram(msg)
            continue

        line_count, byte_size = count_output_stats(output_path)
        current_activity = tail_file(output_path, n_lines=3, max_chars=300)
        new_lines = line_count - last_line_count
        last_line_count = line_count

        size_str = f"{byte_size / 1024:.1f}KB" if byte_size < 1024 * 1024 else f"{byte_size / (1024*1024):.1f}MB"
        msg_parts = [
            f"[Codex] {format_elapsed(elapsed)} elapsed",
            f"Output: {line_count} lines ({size_str}), +{new_lines} since last update",
            "",
            f"Prompt:\n{full_prompt[:500]}",
        ]
        if len(full_prompt) > 500:
            msg_parts.append(f"... ({len(full_prompt)} chars total)")
        msg_parts.extend(["", f"Current activity:\n{current_activity}"])

        send_telegram("\n".join(msg_parts))


def parse_codex_jsonl(filepath):
    """Parse Codex --json JSONL output file, extracting message text from item.completed events."""
    text_parts = []
    with open(filepath, "r", errors="replace") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
                if not isinstance(event, dict):
                    continue
                event_type = event.get("type", "")
                if event_type == "item.completed":
                    item = event.get("item", {})
                    if isinstance(item, dict) and item.get("text"):
                        text_parts.append(item["text"])
            except (json.JSONDecodeError, TypeError, ValueError):
                text_parts.append(line)
    return "\n".join(text_parts).strip()


RATE_LIMIT_PATTERNS = [
    "rate limit", "429", "too many requests", "quota exceeded", "ratelimiterror",
]


def _is_rate_limit(stderr_text, exit_code):
    """Detect rate limit from exit code and stderr patterns."""
    if exit_code == 429:
        return True
    lower = stderr_text.lower()
    return any(pat in lower for pat in RATE_LIMIT_PATTERNS)


def _run_codex_once(prompt, account):
    """Run Codex CLI once with a specific account. Returns (output, rate_limited)."""
    profile = pool_state.get_account_profile(account)
    config_dir = profile.get("codex_config_dir", "/root/.codex")
    model = profile.get("model", "gpt-5.3-codex-spark")
    reasoning = profile.get("reasoning_effort", "xhigh")

    logs_dir = os.path.join(AETHERVAULT_HOME, "logs")
    os.makedirs(logs_dir, exist_ok=True)
    fd, output_path = tempfile.mkstemp(prefix="codex-output-", suffix=".log",
                                        dir=logs_dir)
    os.close(fd)

    fd_err, stderr_path = tempfile.mkstemp(prefix="codex-stderr-", suffix=".log",
                                            dir=logs_dir)
    os.close(fd_err)

    stop_event = threading.Event()
    start_time = time.time()
    reporter = threading.Thread(
        target=progress_reporter,
        args=(prompt, output_path, start_time, stop_event),
        daemon=True,
    )
    reporter.start()

    try:
        env = os.environ.copy()
        env["CODEX_CONFIG_DIR"] = config_dir

        with open(output_path, "w") as out_f, open(stderr_path, "w") as err_f:
            proc = subprocess.Popen(
                ["codex", "exec",
                 "-m", model,
                 "--dangerously-bypass-approvals-and-sandbox",
                 "--json",
                 "--skip-git-repo-check",
                 "-c", f'model_reasoning_effort="{reasoning}"',
                 prompt],
                stdout=out_f,
                stderr=err_f,
                text=True,
                cwd="/root/quake",
                env=env,
            )

        proc.wait()

        try:
            with open(stderr_path, "r", errors="replace") as f:
                stderr_text = f.read()
        except OSError:
            stderr_text = ""

        if _is_rate_limit(stderr_text, proc.returncode):
            pool_state.mark_rate_limited(account)
            elapsed = time.time() - start_time
            send_telegram(
                f"[Codex] Rate limited (account: {account}) after {format_elapsed(elapsed)}\n"
                f"Prompt:\n{prompt[:300]}"
            )
            return None, True

        output = parse_codex_jsonl(output_path)

        elapsed = time.time() - start_time
        line_count, byte_size = count_output_stats(output_path)
        pool_state.mark_success(account)

        if elapsed > TEXT_UPDATE_INTERVAL:
            size_str = f"{byte_size / 1024:.1f}KB" if byte_size < 1024 * 1024 else f"{byte_size / (1024*1024):.1f}MB"
            status = "completed" if proc.returncode == 0 else f"exited with code {proc.returncode}"
            send_telegram(
                f"[Codex] {status} in {format_elapsed(elapsed)} (account: {account})\n"
                f"Output: {line_count} lines ({size_str})\n\n"
                f"Prompt:\n{prompt[:500]}\n\n"
                f"Final output:\n{tail_file(output_path, n_lines=5, max_chars=400)}"
            )

        return (output if output else "(Codex returned no output)"), False

    except Exception as e:
        send_telegram(f"[Codex] Error: {e}\nPrompt:\n{prompt[:300]}")
        return f"(Codex error: {e})", False

    finally:
        stop_event.set()
        reporter.join(timeout=2)
        for path in (output_path, stderr_path):
            try:
                os.unlink(path)
            except OSError:
                pass


def run_codex(prompt):
    """Run Codex CLI with account rotation and auto-failover on rate limit."""
    tried_accounts = set()

    while True:
        account = pool_state.pick_best_account("codex")
        if account is None or account in tried_accounts:
            if tried_accounts:
                return "(All Codex accounts are rate-limited)"
            else:
                return "(No Codex accounts configured)"

        tried_accounts.add(account)
        result, rate_limited = _run_codex_once(prompt, account)

        if not rate_limited:
            return result


def main():
    try:
        raw_input = sys.stdin.read()
        request = json.loads(raw_input)
    except (json.JSONDecodeError, ValueError) as e:
        print(make_hook_error_response(f"(Error: Invalid JSON input to Codex hook: {e})"))
        return

    messages = request.get("messages", [])
    prompt = extract_last_user_message(messages)

    if not prompt:
        response_text = "(No user prompt found in messages)"
    else:
        response_text = run_codex(prompt)

    print(make_hook_response(response_text))


if __name__ == "__main__":
    main()

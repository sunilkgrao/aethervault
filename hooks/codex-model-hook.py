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
import urllib.request
import urllib.error

CODEX_TIMEOUT = 900  # 15 minutes max per Codex run
PROGRESS_INTERVAL = 60  # Check every 60 seconds
TEXT_UPDATE_INTERVAL = 120  # Send text update every 2 minutes
PROGRESS_BAR_WIDTH = 14
AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
ENV_FILE = os.path.join(AETHERVAULT_HOME, ".env")


def load_env_var(key):
    """Load a var from environment or .env file."""
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


def send_telegram(text):
    """Send a Telegram message (best-effort, never blocks the hook)."""
    token = load_env_var("TELEGRAM_BOT_TOKEN")
    chat_id = load_env_var("TELEGRAM_CHAT_ID")
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
    except Exception:
        pass


def send_typing(chat_id, token):
    """Send typing indicator to Telegram."""
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


def extract_last_user_message(messages):
    """Extract the last user message text from the messages array."""
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


def format_elapsed(seconds):
    """Format seconds as Xm Ys."""
    m, s = divmod(int(seconds), 60)
    return f"{m}m {s}s"


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
    token = load_env_var("TELEGRAM_BOT_TOKEN")
    chat_id = load_env_var("TELEGRAM_CHAT_ID")
    if not chat_id:
        try:
            cfg_path = os.path.join(AETHERVAULT_HOME, "config", "briefing.json")
            with open(cfg_path) as f:
                cfg = json.load(f)
                chat_id = str(cfg.get("chat_id", ""))
        except Exception:
            pass

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

        send_typing(chat_id, token)

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
                # Extract text from completed agent messages
                if event_type == "item.completed":
                    item = event.get("item", {})
                    if isinstance(item, dict) and item.get("text"):
                        text_parts.append(item["text"])
            except (json.JSONDecodeError, TypeError, ValueError):
                # Non-JSON line — might be raw text output, include it
                text_parts.append(line)
    return "\n".join(text_parts).strip()


def run_codex(prompt):
    """Run Codex CLI with streaming output and real-time progress reporting."""
    logs_dir = os.path.join(AETHERVAULT_HOME, "logs")
    os.makedirs(logs_dir, exist_ok=True)
    fd, output_path = tempfile.mkstemp(prefix="codex-output-", suffix=".log",
                                        dir=logs_dir)
    os.close(fd)

    stop_event = threading.Event()
    start_time = time.time()
    reporter = threading.Thread(
        target=progress_reporter,
        args=(prompt, output_path, start_time, stop_event),
        daemon=True,
    )
    reporter.start()

    try:
        with open(output_path, "w") as out_f:
            proc = subprocess.Popen(
                ["codex", "exec",
                 "-m", "gpt-5.3-codex-spark",
                 "--dangerously-bypass-approvals-and-sandbox",
                 "--json",
                 "--skip-git-repo-check",
                 "-c", "model_reasoning_effort=\"xhigh\"",
                 prompt],
                stdout=out_f,
                stderr=subprocess.DEVNULL,
                text=True,
                cwd="/root/quake",
            )

        try:
            proc.wait(timeout=CODEX_TIMEOUT)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)

            elapsed = time.time() - start_time
            line_count, _ = count_output_stats(output_path)
            send_telegram(
                f"[Codex] Timed out after {format_elapsed(elapsed)}\n"
                f"Output: {line_count} lines before timeout\n\n"
                f"Prompt:\n{prompt[:500]}\n\n"
                f"Last activity:\n{tail_file(output_path, n_lines=5, max_chars=400)}"
            )
            try:
                partial = parse_codex_jsonl(output_path)
                return partial or f"(Codex timed out after {CODEX_TIMEOUT // 60} minutes)"
            except OSError:
                return f"(Codex timed out after {CODEX_TIMEOUT // 60} minutes)"

        output = parse_codex_jsonl(output_path)

        elapsed = time.time() - start_time
        line_count, byte_size = count_output_stats(output_path)

        if elapsed > TEXT_UPDATE_INTERVAL:
            size_str = f"{byte_size / 1024:.1f}KB" if byte_size < 1024 * 1024 else f"{byte_size / (1024*1024):.1f}MB"
            status = "completed" if proc.returncode == 0 else f"exited with code {proc.returncode}"
            send_telegram(
                f"[Codex] {status} in {format_elapsed(elapsed)}\n"
                f"Output: {line_count} lines ({size_str})\n\n"
                f"Prompt:\n{prompt[:500]}\n\n"
                f"Final output:\n{tail_file(output_path, n_lines=5, max_chars=400)}"
            )

        return output if output else "(Codex returned no output)"

    except Exception as e:
        send_telegram(f"[Codex] Error: {e}\nPrompt:\n{prompt[:300]}")
        return f"(Codex error: {e})"

    finally:
        stop_event.set()
        reporter.join(timeout=2)
        try:
            os.unlink(output_path)
        except OSError:
            pass


def main():
    try:
        raw_input = sys.stdin.read()
        request = json.loads(raw_input)
    except (json.JSONDecodeError, ValueError) as e:
        print(json.dumps({
            "message": {
                "role": "assistant",
                "content": f"(Error: Invalid JSON input to Codex hook: {e})",
                "tool_calls": []
            }
        }))
        return

    messages = request.get("messages", [])
    prompt = extract_last_user_message(messages)

    if not prompt:
        response_text = "(No user prompt found in messages)"
    else:
        response_text = run_codex(prompt)

    response = {
        "message": {
            "role": "assistant",
            "content": response_text,
            "tool_calls": []
        }
    }
    print(json.dumps(response))


if __name__ == "__main__":
    main()

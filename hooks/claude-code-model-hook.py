#!/usr/bin/env python3
"""
Claude Code CLI model hook for AetherVault subagent system.
Reads AgentHookRequest JSON from stdin, extracts the user prompt,
runs `claude -p` CLI, and returns an AgentHookResponse on stdout.

Sends periodic Telegram progress updates during long-running sessions.
Uses pool_state for account selection and rate limit tracking.
"""
import json
import os
import subprocess
import sys
import threading
import time

# Add hooks directory to path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pool_state
from common import (
    send_telegram, send_typing, extract_last_user_message,
    format_elapsed, make_hook_response, make_hook_error_response,
)

PROGRESS_INTERVAL = 60
TEXT_UPDATE_INTERVAL = 120

RATE_LIMIT_PATTERNS = [
    "rate limit", "429", "too many requests", "quota exceeded",
    "ratelimiterror", "overloaded", "capacity",
]


def is_rate_limit_error(stderr_text, exit_code):
    """Detect rate limit from exit code and stderr patterns."""
    if exit_code == 429:
        return True
    lower = stderr_text.lower()
    return any(pat in lower for pat in RATE_LIMIT_PATTERNS)


def progress_reporter(full_prompt, start_time, stop_event):
    """Background thread: sends periodic Telegram progress updates."""
    update_num = 0
    while not stop_event.is_set():
        stop_event.wait(PROGRESS_INTERVAL)
        if stop_event.is_set():
            break
        update_num += 1
        elapsed = time.time() - start_time

        send_typing()

        if (update_num * PROGRESS_INTERVAL) % TEXT_UPDATE_INTERVAL != 0:
            continue

        msg = (
            f"[Claude Code] {format_elapsed(elapsed)} elapsed\n"
            f"Status: running...\n\n"
            f"Prompt:\n{full_prompt[:500]}"
        )
        if len(full_prompt) > 500:
            msg += f"\n... ({len(full_prompt)} chars total)"
        send_telegram(msg)


def run_claude_code(prompt, account=None):
    """Run Claude Code CLI and return the response text."""
    if account is None:
        account = pool_state.pick_best_account("claude-code")
    if account is None:
        return "(All Claude Code accounts are rate-limited)"

    profile = pool_state.get_account_profile(account)
    model = profile.get("model", "claude-sonnet-4-6")

    stop_event = threading.Event()
    start_time = time.time()
    reporter = threading.Thread(
        target=progress_reporter,
        args=(prompt, start_time, stop_event),
        daemon=True,
    )
    reporter.start()

    try:
        cmd = [
            "claude",
            "-p", prompt,
            "--output-format", "json",
            "--model", model,
        ]

        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=None,
        )

        elapsed = time.time() - start_time

        if is_rate_limit_error(proc.stderr, proc.returncode):
            pool_state.mark_rate_limited(account)
            send_telegram(
                f"[Claude Code] Rate limited (account: {account}) after {format_elapsed(elapsed)}\n"
                f"Prompt:\n{prompt[:300]}"
            )
            return None  # Signal caller to try fallback

        if proc.returncode != 0:
            error_msg = proc.stderr.strip() or f"exit code {proc.returncode}"
            send_telegram(
                f"[Claude Code] Error after {format_elapsed(elapsed)}: {error_msg}\n"
                f"Prompt:\n{prompt[:300]}"
            )
            return f"(Claude Code error: {error_msg})"

        output = proc.stdout.strip()
        if not output:
            pool_state.mark_success(account)
            return "(Claude Code returned no output)"

        try:
            result = json.loads(output)
            if isinstance(result, dict):
                response_text = result.get("result", "")
                is_error = result.get("is_error", False)
                if is_error:
                    pool_state.mark_success(account)
                    return f"(Claude Code error: {response_text})"
                pool_state.mark_success(account)

                if elapsed > TEXT_UPDATE_INTERVAL:
                    send_telegram(
                        f"[Claude Code] Completed in {format_elapsed(elapsed)}\n"
                        f"Prompt:\n{prompt[:500]}\n\n"
                        f"Response:\n{response_text[:400]}"
                    )

                return response_text if response_text else "(Claude Code returned empty result)"
            else:
                pool_state.mark_success(account)
                return output
        except json.JSONDecodeError:
            pool_state.mark_success(account)
            return output if output else "(Claude Code returned no output)"

    except Exception as e:
        send_telegram(f"[Claude Code] Exception: {e}\nPrompt:\n{prompt[:300]}")
        return f"(Claude Code error: {e})"

    finally:
        stop_event.set()
        reporter.join(timeout=2)


def main():
    try:
        raw_input = sys.stdin.read()
        request = json.loads(raw_input)
    except (json.JSONDecodeError, ValueError) as e:
        print(make_hook_error_response(f"(Error: Invalid JSON input to Claude Code hook: {e})"))
        return

    messages = request.get("messages", [])
    prompt = extract_last_user_message(messages)

    if not prompt:
        response_text = "(No user prompt found in messages)"
    else:
        response_text = run_claude_code(prompt)
        if response_text is None:
            response_text = "(Claude Code rate-limited, no fallback available in direct mode)"

    print(make_hook_response(response_text))


if __name__ == "__main__":
    main()

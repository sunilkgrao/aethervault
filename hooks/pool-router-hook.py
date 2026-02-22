#!/usr/bin/env python3
"""
Pool router hook for AetherVault subagent system.
Reads AgentHookRequest JSON from stdin, picks the best available backend
(codex or claude-code) based on rate limit state, delegates to it,
and returns the response.

Priority order: codex first, then claude-code as fallback.
If a backend fails with a rate limit, automatically tries the next one.
"""
import json
import os
import subprocess
import sys

# Add hooks directory to path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pool_state
from common import AETHERVAULT_HOME, make_hook_response, make_hook_error_response

# Backend priority order
BACKEND_PRIORITY = ["codex", "claude-code"]

# Map service names to their hook scripts
BACKEND_HOOKS = {
    "codex": os.path.join(AETHERVAULT_HOME, "hooks", "codex-hook.sh"),
    "claude-code": os.path.join(AETHERVAULT_HOME, "hooks", "claude-code-hook.sh"),
}


def try_backend(service, raw_input):
    """Try to run a backend hook. Returns (success, response_text)."""
    hook_path = BACKEND_HOOKS.get(service)
    if not hook_path or not os.path.exists(hook_path):
        return False, f"(Hook not found for {service}: {hook_path})"

    account = pool_state.pick_best_account(service)
    if account is None:
        return False, f"(All {service} accounts are rate-limited)"

    try:
        proc = subprocess.run(
            [hook_path],
            input=raw_input,
            capture_output=True,
            text=True,
            timeout=None,
        )

        if proc.returncode != 0 and not proc.stdout.strip():
            pool_state.mark_rate_limited(account)
            return False, f"({service} hook exited with code {proc.returncode})"

        stdout = proc.stdout.strip()
        if not stdout:
            return False, f"({service} hook returned no output)"

        try:
            response = json.loads(stdout)
            content = response.get("message", {}).get("content", "")

            rate_limit_signals = [
                "rate limit", "429", "too many requests",
                "quota exceeded", "ratelimiterror",
            ]
            content_lower = content.lower()
            if any(sig in content_lower for sig in rate_limit_signals):
                pool_state.mark_rate_limited(account)
                return False, content

            pool_state.mark_success(account)
            return True, stdout

        except json.JSONDecodeError:
            pool_state.mark_success(account)
            return True, make_hook_response(stdout)

    except subprocess.TimeoutExpired:
        return False, f"({service} hook timed out)"
    except Exception as e:
        return False, f"({service} hook error: {e})"


def main():
    try:
        raw_input = sys.stdin.read()
        json.loads(raw_input)
    except (json.JSONDecodeError, ValueError) as e:
        print(make_hook_error_response(f"(Error: Invalid JSON input to pool router: {e})"))
        return

    errors = []

    for service in BACKEND_PRIORITY:
        account = pool_state.pick_best_account(service)
        if account is None:
            errors.append(f"{service}: all accounts rate-limited")
            continue

        success, result = try_backend(service, raw_input)
        if success:
            print(result)
            return
        else:
            errors.append(f"{service}: {result}")

    error_summary = "; ".join(errors)
    print(make_hook_error_response(f"(All subagent backends exhausted. Errors: {error_summary})"))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Minimal agent runner + subagent fanout (stdlib-only)."""
import json
import os
import subprocess
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed

DEFAULT_BIN = os.environ.get("AETHERVAULT_BIN", "./target/release/aethervault")
DEFAULT_MV2 = os.environ.get("AETHERVAULT_MV2", "./data/knowledge.mv2")
DEFAULT_MODEL_HOOK = os.environ.get("AETHERVAULT_MODEL_HOOK", "aethervault hook claude")
DEFAULT_MAX_STEPS = os.environ.get("AETHERVAULT_MAX_STEPS", "64")
DEFAULT_LOG_COMMIT_INTERVAL = os.environ.get("AETHERVAULT_LOG_COMMIT_INTERVAL", "8")
DEFAULT_TIMEOUT = float(os.environ.get("AETHERVAULT_AGENT_TIMEOUT", "120"))
DEFAULT_SESSION_PREFIX = os.environ.get("AETHERVAULT_SESSION_PREFIX", "")

_print_lock = threading.Lock()


def _debug(msg):
    if os.environ.get("AETHERVAULT_DEBUG") == "1":
        with _print_lock:
            print(msg, flush=True)


def _split_command(cmd):
    return [part for part in cmd.split(" ") if part]


def _base_command(session, system=None):
    cmd = [DEFAULT_BIN, "agent", DEFAULT_MV2]
    cmd += ["--model-hook", DEFAULT_MODEL_HOOK]
    cmd += ["--session", session]
    cmd += ["--max-steps", str(DEFAULT_MAX_STEPS)]
    cmd += ["--log-commit-interval", str(DEFAULT_LOG_COMMIT_INTERVAL)]

    if os.environ.get("AETHERVAULT_LOG", "1") == "1":
        cmd += ["--log"]
    if system:
        cmd += ["--system", system]
    context_query = os.environ.get("AETHERVAULT_CONTEXT_QUERY")
    if context_query:
        cmd += ["--context-query", context_query]
    return cmd


def run_agent(prompt, session, system=None):
    session = f"{DEFAULT_SESSION_PREFIX}{session}"
    cmd = _base_command(session, system=system)
    _debug(f"agent: {cmd}")

    try:
        proc = subprocess.run(
            cmd,
            input=prompt,
            text=True,
            capture_output=True,
            timeout=DEFAULT_TIMEOUT,
        )
    except subprocess.TimeoutExpired:
        return "Agent timeout", True

    if proc.returncode != 0:
        err = proc.stderr.strip() or "agent failed"
        return f"Agent error: {err}", True

    return (proc.stdout.strip() or ""), False


def load_subagents():
    raw = os.environ.get("AETHERVAULT_SUBAGENTS", "")
    if not raw:
        return []
    try:
        payload = json.loads(raw)
        if not isinstance(payload, list):
            return []
        return payload
    except json.JSONDecodeError:
        return []


def run_with_subagents(prompt, session):
    main_text, main_err = run_agent(prompt, session)
    subagents = load_subagents()
    if not subagents:
        return main_text, main_err

    results = []
    with ThreadPoolExecutor(max_workers=min(4, len(subagents))) as pool:
        futures = []
        for entry in subagents:
            name = entry.get("name") or "subagent"
            system = entry.get("system")
            sub_session = f"{session}/{name}"
            futures.append((name, pool.submit(run_agent, prompt, sub_session, system)))
        for name, fut in futures:
            try:
                text, err = fut.result(timeout=DEFAULT_TIMEOUT)
            except Exception as exc:  # noqa: BLE001
                results.append((name, f"Subagent error: {exc}", True))
            else:
                results.append((name, text, err))

    if not results:
        return main_text, main_err

    parts = [main_text]
    parts.append("")
    parts.append("Subagents:")
    for name, text, err in results:
        header = f"- {name}:"
        if err:
            parts.append(f"{header} {text}")
        else:
            parts.append(f"{header} {text}")
    return "\n".join([p for p in parts if p is not None]), main_err

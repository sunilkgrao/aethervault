#!/usr/bin/env python3
"""
Shared rate-limit state manager for the AetherVault subagent pool.

Tracks per-account rate limit status across Codex and Claude Code backends.
State is persisted to /root/.aethervault/state/pool-state.json with file locking
(fcntl.flock) for safe concurrent access from multiple hook processes.
"""
import fcntl
import json
import os
import time
from datetime import datetime, timezone

AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
STATE_DIR = os.path.join(AETHERVAULT_HOME, "state")
STATE_FILE = os.path.join(STATE_DIR, "pool-state.json")
AUTH_PROFILES_PATH = os.path.join(AETHERVAULT_HOME, "config", "auth-profiles.json")

DEFAULT_COOLDOWN_SECS = 300


def _ensure_state_dir():
    os.makedirs(STATE_DIR, exist_ok=True)


def load_auth_profiles():
    """Read config/auth-profiles.json and return the parsed dict."""
    try:
        with open(AUTH_PROFILES_PATH) as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return {"version": 3, "profiles": {}, "pools": {}, "order": {}}


def _default_state():
    """Build default state from auth-profiles.json pool definitions."""
    profiles = load_auth_profiles()
    accounts = {}
    for pool_name, pool_cfg in profiles.get("pools", {}).items():
        for acct in pool_cfg.get("accounts", []):
            accounts[acct] = {
                "service": pool_name,
                "rate_limited_until": None,
                "consecutive_failures": 0,
            }
    return {"accounts": accounts}


def load_state():
    """Load pool state from disk with file locking. Returns (state_dict, lock_fd)."""
    _ensure_state_dir()
    if not os.path.exists(STATE_FILE):
        state = _default_state()
        save_state(state)
        return state

    fd = open(STATE_FILE, "r+")
    try:
        fcntl.flock(fd, fcntl.LOCK_SH)
        content = fd.read()
        fd.close()
        if not content.strip():
            return _default_state()
        return json.loads(content)
    except (json.JSONDecodeError, OSError):
        fd.close()
        return _default_state()


def save_state(state):
    """Save pool state to disk with exclusive file locking."""
    _ensure_state_dir()
    fd = open(STATE_FILE, "w")
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        json.dump(state, fd, indent=2, default=str)
        fd.flush()
        os.fsync(fd.fileno())
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        fd.close()


def _locked_update(fn):
    """Read state, apply fn(state), write back — all under exclusive lock."""
    _ensure_state_dir()
    if not os.path.exists(STATE_FILE):
        state = _default_state()
        save_state(state)

    fd = open(STATE_FILE, "r+")
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        content = fd.read()
        state = json.loads(content) if content.strip() else _default_state()
        fn(state)
        fd.seek(0)
        fd.truncate()
        json.dump(state, fd, indent=2, default=str)
        fd.flush()
        os.fsync(fd.fileno())
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        fd.close()


def is_rate_limited(account, state=None):
    """Check if an account is currently rate-limited."""
    if state is None:
        state = load_state()
    acct = state.get("accounts", {}).get(account)
    if not acct:
        return False
    until = acct.get("rate_limited_until")
    if not until:
        return False
    try:
        limit_time = datetime.fromisoformat(until.replace("Z", "+00:00"))
        return datetime.now(timezone.utc) < limit_time
    except (ValueError, TypeError):
        return False


def mark_rate_limited(account, cooldown_secs=None):
    """Mark an account as rate-limited for cooldown_secs seconds."""
    if cooldown_secs is None:
        profiles = load_auth_profiles()
        # Find which pool this account belongs to
        for pool_cfg in profiles.get("pools", {}).values():
            if account in pool_cfg.get("accounts", []):
                cooldown_secs = pool_cfg.get("cooldown_secs", DEFAULT_COOLDOWN_SECS)
                break
        if cooldown_secs is None:
            cooldown_secs = DEFAULT_COOLDOWN_SECS

    until = datetime.fromtimestamp(
        time.time() + cooldown_secs, tz=timezone.utc
    ).isoformat()

    def _apply(state):
        accounts = state.setdefault("accounts", {})
        if account not in accounts:
            accounts[account] = {"service": "unknown", "rate_limited_until": None, "consecutive_failures": 0}
        accounts[account]["rate_limited_until"] = until
        accounts[account]["consecutive_failures"] = accounts[account].get("consecutive_failures", 0) + 1

    _locked_update(_apply)


def mark_success(account):
    """Clear rate-limit state and reset failure counter for an account."""
    def _apply(state):
        accounts = state.setdefault("accounts", {})
        if account not in accounts:
            accounts[account] = {"service": "unknown", "rate_limited_until": None, "consecutive_failures": 0}
        accounts[account]["rate_limited_until"] = None
        accounts[account]["consecutive_failures"] = 0

    _locked_update(_apply)


def pick_best_account(service):
    """Pick the best available (non-rate-limited) account for a given service.

    Returns the account name with fewest consecutive failures, or None if all
    accounts for that service are rate-limited.
    """
    profiles = load_auth_profiles()
    pool_cfg = profiles.get("pools", {}).get(service, {})
    pool_accounts = pool_cfg.get("accounts", [])
    if not pool_accounts:
        return None

    state = load_state()
    candidates = []
    for acct_name in pool_accounts:
        if is_rate_limited(acct_name, state):
            continue
        acct_state = state.get("accounts", {}).get(acct_name, {})
        failures = acct_state.get("consecutive_failures", 0)
        candidates.append((failures, acct_name))

    if not candidates:
        return None

    candidates.sort(key=lambda x: x[0])
    return candidates[0][1]


def get_account_profile(account):
    """Get the auth profile config for a specific account."""
    profiles = load_auth_profiles()
    return profiles.get("profiles", {}).get(account, {})

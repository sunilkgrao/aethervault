#!/usr/bin/env python3
"""
AetherVault Session Manager
Spawn, list, check, and kill named background agent sessions.

Usage (called by the agent via exec tool):
    python3 /root/.aethervault/hooks/session-manager.py spawn --task "Build a doom clone" [--name cosmic-falcon-7] [--model claude] [--max-steps 32]
    python3 /root/.aethervault/hooks/session-manager.py list
    python3 /root/.aethervault/hooks/session-manager.py check <session-name>
    python3 /root/.aethervault/hooks/session-manager.py check-all
    python3 /root/.aethervault/hooks/session-manager.py kill <session-name>
    python3 /root/.aethervault/hooks/session-manager.py kill-all
"""

import argparse
import json
import os
import random
import signal
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
MAX_OUTPUT_READ = 512 * 1024  # 512KB max to read from output files (prevents OOM)
SESSIONS_DIR = Path(AETHERVAULT_HOME) / "workspace" / "sessions"
REGISTRY_FILE = SESSIONS_DIR / "registry.json"
MV2_PATH = os.environ.get("CAPSULE_PATH", os.path.join(AETHERVAULT_HOME, "memory.mv2"))
ENV_FILE = os.path.join(AETHERVAULT_HOME, ".env")
AETHERVAULT_BIN = os.environ.get("AETHERVAULT_BIN", "/usr/local/bin/aethervault")
CACHED_CONFIG = SESSIONS_DIR / ".cached-config.json"

ADJECTIVES = [
    "cosmic", "electric", "quantum", "blazing", "frozen", "phantom", "neon",
    "stellar", "molten", "crystal", "shadow", "golden", "crimson", "azure",
    "emerald", "obsidian", "radiant", "silent", "fierce", "noble", "ancient",
    "rapid", "elevated", "drifting", "hollow", "iron", "velvet", "vivid",
    "wicked", "zen", "primal", "lucid", "feral", "orbital", "spectral"
]

NOUNS = [
    "falcon", "phoenix", "wolf", "dragon", "goat", "raven", "tiger",
    "viper", "eagle", "panther", "cobra", "hawk", "jaguar", "lynx",
    "mantis", "orca", "puma", "shark", "sphinx", "hydra", "gryphon",
    "kraken", "chimera", "basilisk", "wyvern", "pegasus", "minotaur",
    "cerberus", "leviathan", "titan", "colossus", "sentinel", "wraith"
]


def generate_name():
    adj = random.choice(ADJECTIVES)
    noun = random.choice(NOUNS)
    num = random.randint(1, 99)
    return f"{adj}-{noun}-{num}"


def load_registry():
    if REGISTRY_FILE.exists():
        try:
            return json.loads(REGISTRY_FILE.read_text())
        except (json.JSONDecodeError, IOError):
            return {}
    return {}


def save_registry(reg):
    SESSIONS_DIR.mkdir(parents=True, exist_ok=True)
    REGISTRY_FILE.write_text(json.dumps(reg, indent=2))


def is_process_alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except (OSError, ProcessLookupError):
        return False


def load_env():
    """Load environment variables from .env file."""
    env = os.environ.copy()
    if os.path.exists(ENV_FILE):
        with open(ENV_FILE) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith('#') and '=' in line:
                    key, _, value = line.partition('=')
                    env[key.strip()] = value.strip()
    return env


def ensure_cached_config():
    """Ensure we have a cached copy of the main capsule config.
    
    This reads from the main MV2 only when the lock is free. Once cached,
    all future spawns use the cached file to avoid lock contention.
    """
    if CACHED_CONFIG.exists() and CACHED_CONFIG.stat().st_size > 10:
        return True
    
    SESSIONS_DIR.mkdir(parents=True, exist_ok=True)
    try:
        result = subprocess.run(
            [AETHERVAULT_BIN, "config", MV2_PATH, "get", "--key", "index", "--raw"],
            capture_output=True, text=True, timeout=5
        )
        if result.returncode == 0 and result.stdout.strip():
            CACHED_CONFIG.write_text(result.stdout.strip())
            return True
    except Exception:
        pass
    return False


def clone_capsule_config(dest_mv2):
    """Copy the index config from cached config file to a new session capsule."""
    if not CACHED_CONFIG.exists():
        return False, "no cached config"
    
    try:
        result = subprocess.run(
            [AETHERVAULT_BIN, "config", dest_mv2, "set", "--key", "index", 
             "--file", str(CACHED_CONFIG)],
            capture_output=True, text=True, timeout=10
        )
        if result.returncode != 0:
            return False, f"set failed: {result.stderr}"
        return True, "ok"
    except Exception as e:
        return False, str(e)


def cmd_spawn(args):
    name = args.name or generate_name()
    task = args.task
    max_steps = args.max_steps or 32
    
    reg = load_registry()
    
    # Check if name already exists and is alive
    if name in reg and is_process_alive(reg[name].get("pid", 0)):
        print(json.dumps({"error": f"Session '{name}' is already running (PID {reg[name]['pid']})"}))
        return
    
    # Ensure we have cached config
    ensure_cached_config()
    
    # Create session output directory
    session_dir = SESSIONS_DIR / name
    session_dir.mkdir(parents=True, exist_ok=True)
    
    output_file = session_dir / "output.log"
    status_file = session_dir / "status.json"
    
    # Create a per-session capsule to avoid lock contention with bridge
    session_mv2 = session_dir / "capsule.mv2"
    config_ok = True
    config_msg = ""
    if not session_mv2.exists():
        init_result = subprocess.run(
            [AETHERVAULT_BIN, "init", str(session_mv2)],
            capture_output=True, text=True, timeout=10
        )
        if init_result.returncode != 0:
            print(json.dumps({"error": f"Failed to init capsule: {init_result.stderr}"}))
            return
        # Clone config from cached file (no lock contention)
        config_ok, config_msg = clone_capsule_config(str(session_mv2))
    
    if not config_ok:
        # Config clone is best-effort - --model-hook CLI flag ensures agent can still run
        pass
    
    # Write initial status
    status = {
        "name": name,
        "task": task,
        "status": "running",
        "started_at": datetime.now(timezone.utc).isoformat(),
        "pid": None,
        "max_steps": max_steps,
        "config_cloned": config_ok,
    }
    status_file.write_text(json.dumps(status, indent=2))
    
    # Build the command - use per-session capsule + --no-memory to avoid lock contention
    # Pass --model-hook directly via CLI to avoid capsule config race conditions
    cmd = [
        AETHERVAULT_BIN, "agent",
        str(session_mv2),
        "--no-memory",
        "--model-hook", "aethervault hook claude",
        "--max-steps", str(max_steps),
        "--prompt", task,
    ]
    
    # Spawn the process in background
    env = load_env()
    with open(output_file, "w") as out_f:
        proc = subprocess.Popen(
            cmd,
            stdout=out_f,
            stderr=subprocess.STDOUT,
            env=env,
            cwd=AETHERVAULT_HOME,
            start_new_session=True,  # detach from parent
        )
    
    # Update status and registry
    status["pid"] = proc.pid
    status_file.write_text(json.dumps(status, indent=2))
    
    reg[name] = {
        "pid": proc.pid,
        "task": task,
        "started_at": status["started_at"],
        "max_steps": max_steps,
        "status": "running",
    }
    save_registry(reg)
    
    print(json.dumps({
        "spawned": name,
        "pid": proc.pid,
        "task": task,
        "output": str(output_file),
        "max_steps": max_steps,
        "config_cloned": config_ok,
    }))


def cmd_list(args):
    reg = load_registry()
    sessions = []
    for name, info in reg.items():
        pid = info.get("pid", 0)
        alive = is_process_alive(pid) if pid else False
        
        # Check output file size
        output_file = SESSIONS_DIR / name / "output.log"
        output_size = output_file.stat().st_size if output_file.exists() else 0
        
        # Read last line of output (read only tail to prevent OOM)
        last_line = ""
        if output_file.exists() and output_size > 0:
            try:
                with open(output_file, "rb") as f:
                    # Seek to last 4KB to find the last line
                    seek_pos = max(0, output_size - 4096)
                    f.seek(seek_pos)
                    tail_bytes = f.read(4096)
                tail_text = tail_bytes.decode("utf-8", errors="replace").strip()
                lines = tail_text.split("\n")
                last_line = lines[-1][:200] if lines else ""
            except (IOError, OSError):
                pass
        
        sessions.append({
            "name": name,
            "task": info.get("task", ""),
            "pid": pid,
            "alive": alive,
            "status": "running" if alive else "completed",
            "started_at": info.get("started_at", ""),
            "output_bytes": output_size,
            "last_line": last_line,
        })
    
    if not sessions:
        print(json.dumps({"sessions": [], "message": "No sessions found"}))
    else:
        print(json.dumps({"sessions": sessions, "total": len(sessions), 
                          "running": sum(1 for s in sessions if s["alive"]),
                          "completed": sum(1 for s in sessions if not s["alive"])}))


def cmd_check(args):
    name = args.name
    reg = load_registry()
    
    if name not in reg:
        print(json.dumps({"error": f"Session '{name}' not found"}))
        return
    
    info = reg[name]
    pid = info.get("pid", 0)
    alive = is_process_alive(pid) if pid else False
    
    output_file = SESSIONS_DIR / name / "output.log"
    output = ""
    if output_file.exists():
        try:
            file_size = output_file.stat().st_size
            if file_size <= MAX_OUTPUT_READ:
                output = output_file.read_text(errors="replace")
            else:
                # Only read the tail to prevent OOM on large output files
                with open(output_file, "rb") as f:
                    f.seek(max(0, file_size - MAX_OUTPUT_READ))
                    output = f.read(MAX_OUTPUT_READ).decode("utf-8", errors="replace")
                output = f"... (truncated, showing last {MAX_OUTPUT_READ // 1024}KB of {file_size // 1024}KB)\n" + output
        except (IOError, OSError):
            output = "(could not read output)"

    # Get tail of output (last 100 lines)
    lines = output.strip().split("\n") if output.strip() else []
    tail = "\n".join(lines[-100:]) if len(lines) > 100 else output
    
    print(json.dumps({
        "name": name,
        "task": info.get("task", ""),
        "pid": pid,
        "alive": alive,
        "status": "running" if alive else "completed",
        "started_at": info.get("started_at", ""),
        "output_lines": len(lines),
        "output_bytes": len(output),
        "output_tail": tail,
    }))


def cmd_check_all(args):
    reg = load_registry()
    results = []
    for name in reg:
        args_copy = argparse.Namespace(name=name)
        # Capture output
        import io
        old_stdout = sys.stdout
        sys.stdout = io.StringIO()
        cmd_check(args_copy)
        result = sys.stdout.getvalue()
        sys.stdout = old_stdout
        try:
            results.append(json.loads(result))
        except json.JSONDecodeError:
            results.append({"name": name, "error": "parse error"})
    
    print(json.dumps({"sessions": results, "total": len(results)}))


def cmd_kill(args):
    name = args.name
    reg = load_registry()
    
    if name not in reg:
        print(json.dumps({"error": f"Session '{name}' not found"}))
        return
    
    pid = reg[name].get("pid", 0)
    if pid and is_process_alive(pid):
        try:
            os.kill(pid, signal.SIGTERM)
            time.sleep(1)
            if is_process_alive(pid):
                os.kill(pid, signal.SIGKILL)
            print(json.dumps({"killed": name, "pid": pid}))
        except OSError as e:
            print(json.dumps({"error": f"Failed to kill {name}: {e}"}))
    else:
        print(json.dumps({"message": f"Session '{name}' is not running (PID {pid})"}))
    
    reg[name]["status"] = "killed"
    save_registry(reg)


def cmd_kill_all(args):
    reg = load_registry()
    killed = []
    for name, info in reg.items():
        pid = info.get("pid", 0)
        if pid and is_process_alive(pid):
            try:
                os.kill(pid, signal.SIGTERM)
                killed.append(name)
            except OSError:
                pass
    
    time.sleep(1)
    # Force kill any survivors
    for name in killed:
        pid = reg[name].get("pid", 0)
        if pid and is_process_alive(pid):
            try:
                os.kill(pid, signal.SIGKILL)
            except OSError:
                pass
        reg[name]["status"] = "killed"
    
    save_registry(reg)
    print(json.dumps({"killed": killed, "count": len(killed)}))


def main():
    parser = argparse.ArgumentParser(description="AetherVault Session Manager")
    subparsers = parser.add_subparsers(dest="command", required=True)
    
    # spawn
    sp = subparsers.add_parser("spawn")
    sp.add_argument("--task", "-t", required=True, help="Task description/prompt")
    sp.add_argument("--name", "-n", help="Session name (auto-generated if omitted)")
    sp.add_argument("--max-steps", type=int, default=32, help="Max agent steps")
    
    # list
    subparsers.add_parser("list")
    
    # check
    cp = subparsers.add_parser("check")
    cp.add_argument("name", help="Session name to check")
    
    # check-all
    subparsers.add_parser("check-all")
    
    # kill
    kp = subparsers.add_parser("kill")
    kp.add_argument("name", help="Session name to kill")
    
    # kill-all
    subparsers.add_parser("kill-all")
    
    args = parser.parse_args()
    
    {
        "spawn": cmd_spawn,
        "list": cmd_list,
        "check": cmd_check,
        "check_all": cmd_check_all,
        "check-all": cmd_check_all,
        "kill": cmd_kill,
        "kill_all": cmd_kill_all,
        "kill-all": cmd_kill_all,
    }[args.command.replace("-", "_") if hasattr(args, 'command') else args.command](args)


if __name__ == "__main__":
    main()

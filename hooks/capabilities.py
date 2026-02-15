#!/usr/bin/env python3
"""
AetherVault Capabilities Registry
Single source of truth for what the agent can and cannot do.

Auto-discovers hooks, cron jobs, services, and configs.
The agent queries this before using any capability.

Usage:
    python3 /root/.aethervault/hooks/capabilities.py list [--format json|table]
    python3 /root/.aethervault/hooks/capabilities.py check <name>
    python3 /root/.aethervault/hooks/capabilities.py discover
    python3 /root/.aethervault/hooks/capabilities.py enable <name>
    python3 /root/.aethervault/hooks/capabilities.py disable <name> --reason "..."
    python3 /root/.aethervault/hooks/capabilities.py status
"""

import argparse
import json
import os
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
REGISTRY_FILE = Path(AETHERVAULT_HOME) / "data" / "capabilities.json"
HOOKS_DIR = Path(AETHERVAULT_HOME) / "hooks"
CONFIG_DIR = Path(AETHERVAULT_HOME) / "config"

def load_registry():
    if REGISTRY_FILE.exists():
        try:
            return json.loads(REGISTRY_FILE.read_text())
        except (json.JSONDecodeError, IOError):
            return {"capabilities": {}, "last_discovery": None}
    return {"capabilities": {}, "last_discovery": None}

def save_registry(reg):
    REGISTRY_FILE.parent.mkdir(parents=True, exist_ok=True)
    REGISTRY_FILE.write_text(json.dumps(reg, indent=2))

def extract_docstring(filepath):
    """Extract the module docstring from a Python file."""
    try:
        content = Path(filepath).read_text()
        match = re.search(r'^"""(.*?)"""', content, re.DOTALL)
        if match:
            return match.group(1).strip().split('\n')[0]
        match = re.search(r"^'''(.*?)'''", content, re.DOTALL)
        if match:
            return match.group(1).strip().split('\n')[0]
    except IOError:
        pass
    return ""

def extract_shell_description(filepath):
    """Extract description from a shell script's comments."""
    try:
        lines = Path(filepath).read_text().split('\n')
        for line in lines[1:6]:  # skip shebang, check first few lines
            line = line.strip()
            if line.startswith('#') and len(line) > 3 and not line.startswith('#!'):
                return line.lstrip('# ').strip()
    except IOError:
        pass
    return ""

def get_help_text(filepath):
    """Try to get --help output from a script."""
    try:
        result = subprocess.run(
            ["python3", str(filepath), "--help"],
            capture_output=True, text=True, timeout=5
        )
        if result.returncode == 0:
            lines = result.stdout.strip().split('\n')
            # Return first 3 non-empty lines
            return '\n'.join(l for l in lines[:3] if l.strip())
    except Exception:
        pass
    return ""

def discover_hooks():
    """Scan hooks directory and return discovered capabilities."""
    discovered = {}
    if not HOOKS_DIR.exists():
        return discovered

    for f in sorted(HOOKS_DIR.iterdir()):
        if f.name.startswith('.') or f.is_dir():
            continue

        name = f.stem  # filename without extension
        ext = f.suffix

        # Skip disabled files
        if f.name.endswith('.disabled'):
            name = f.stem.rsplit('.', 1)[0] if '.disabled' in f.name else f.stem
            discovered[name] = {
                "status": "disabled",
                "type": "hook",
                "script": str(f),
                "reason": "File has .disabled extension",
            }
            continue

        # Skip internal/infrastructure hooks
        if name in ('anthropic-model-hook', 'codex-model-hook', 'set-chat-id'):
            discovered[name] = {
                "status": "active",
                "type": "internal",
                "script": str(f),
                "description": "Internal infrastructure hook (not user-facing)",
            }
            continue

        description = ""
        if ext == '.py':
            description = extract_docstring(f)
        elif ext == '.sh':
            description = extract_shell_description(f)

        if not description:
            description = f"Hook script: {f.name}"

        discovered[name] = {
            "status": "active",
            "type": "hook",
            "script": str(f),
            "description": description,
        }

    return discovered

def discover_crons():
    """Scan crontab for scheduled capabilities."""
    discovered = {}
    try:
        result = subprocess.run(
            ["crontab", "-l"], capture_output=True, text=True, timeout=5
        )
        if result.returncode != 0:
            return discovered

        for line in result.stdout.strip().split('\n'):
            line = line.strip()
            if not line or line.startswith('#'):
                continue
            # Parse cron schedule and command
            parts = line.split(None, 5)
            if len(parts) < 6:
                continue
            schedule = ' '.join(parts[:5])
            command = parts[5]

            # Extract script name from command
            for token in command.split():
                if token.endswith('.py') or token.endswith('.sh'):
                    script_name = Path(token).stem
                    cron_name = f"cron:{script_name}"
                    discovered[cron_name] = {
                        "status": "active",
                        "type": "scheduled",
                        "schedule": schedule,
                        "command": command,
                        "description": f"Scheduled task: {script_name} ({schedule})",
                    }
                    break

    except Exception:
        pass
    return discovered

def discover_services():
    """Check key systemd services."""
    discovered = {}
    services = ["aethervault", "embedding-service"]
    for svc in services:
        try:
            result = subprocess.run(
                ["systemctl", "is-active", f"{svc}.service"],
                capture_output=True, text=True, timeout=5
            )
            status = result.stdout.strip()
            discovered[f"service:{svc}"] = {
                "status": "active" if status == "active" else "disabled",
                "type": "service",
                "description": f"Systemd service: {svc}.service ({status})",
            }
        except Exception:
            pass
    return discovered

def cmd_discover(args):
    """Auto-discover all capabilities and update registry."""
    reg = load_registry()

    hooks = discover_hooks()
    crons = discover_crons()
    services = discover_services()

    # Merge discoveries, preserving manual overrides
    for name, info in {**hooks, **crons, **services}.items():
        existing = reg["capabilities"].get(name, {})
        # If manually disabled, preserve the manual override
        if existing.get("manual_override"):
            info["status"] = existing["status"]
            info["reason"] = existing.get("reason", "")
            info["manual_override"] = True
        reg["capabilities"][name] = {**info, **{k: v for k, v in existing.items() if k not in info}}

    # Mark capabilities in registry that no longer exist on disk
    all_discovered = set(hooks) | set(crons) | set(services)
    for name in list(reg["capabilities"].keys()):
        if name not in all_discovered and not reg["capabilities"][name].get("manual_override"):
            reg["capabilities"][name]["status"] = "missing"
            reg["capabilities"][name]["reason"] = "Not found during discovery"

    reg["last_discovery"] = datetime.now(timezone.utc).isoformat()
    save_registry(reg)

    active = sum(1 for c in reg["capabilities"].values() if c["status"] == "active")
    disabled = sum(1 for c in reg["capabilities"].values() if c["status"] in ("disabled", "missing"))
    print(json.dumps({
        "discovered": len(all_discovered),
        "active": active,
        "disabled": disabled,
        "registry": str(REGISTRY_FILE),
    }))

def cmd_list(args):
    """List all capabilities."""
    reg = load_registry()
    caps = reg.get("capabilities", {})

    if not caps:
        print(json.dumps({"message": "No capabilities registered. Run 'discover' first."}))
        return

    fmt = getattr(args, 'format', 'table')
    if fmt == 'json':
        print(json.dumps({"capabilities": caps, "total": len(caps)}))
        return

    # Table format
    lines = []
    lines.append(f"{'Name':<30} {'Status':<10} {'Type':<12} {'Description'}")
    lines.append("-" * 90)
    for name, info in sorted(caps.items()):
        status = info.get("status", "unknown")
        ctype = info.get("type", "")
        desc = info.get("description", "")[:50]
        marker = "OK" if status == "active" else "DISABLED" if status == "disabled" else "MISSING"
        lines.append(f"{name:<30} {marker:<10} {ctype:<12} {desc}")

    print('\n'.join(lines))

def cmd_check(args):
    """Check a specific capability."""
    reg = load_registry()
    name = args.name

    # Fuzzy match: try exact, then partial
    caps = reg.get("capabilities", {})
    if name not in caps:
        # Try partial match
        matches = [k for k in caps if name.lower() in k.lower()]
        if len(matches) == 1:
            name = matches[0]
        elif len(matches) > 1:
            print(json.dumps({
                "error": f"Ambiguous name '{args.name}'. Matches: {matches}",
            }))
            return
        else:
            print(json.dumps({
                "name": args.name,
                "status": "not_found",
                "message": f"Capability '{args.name}' is not registered. It may not exist on this system.",
            }))
            return

    info = caps[name]
    # Verify script still exists on disk if it's a hook
    if info.get("script") and not Path(info["script"]).exists():
        info["status"] = "missing"
        info["reason"] = f"Script not found at {info['script']}"

    print(json.dumps({"name": name, **info}))

def cmd_enable(args):
    """Enable a capability."""
    reg = load_registry()
    name = args.name
    if name not in reg["capabilities"]:
        print(json.dumps({"error": f"Capability '{name}' not found. Run 'discover' first."}))
        return
    reg["capabilities"][name]["status"] = "active"
    reg["capabilities"][name].pop("reason", None)
    reg["capabilities"][name]["manual_override"] = True
    save_registry(reg)
    print(json.dumps({"enabled": name}))

def cmd_disable(args):
    """Disable a capability with a reason."""
    reg = load_registry()
    name = args.name
    if name not in reg["capabilities"]:
        # Allow disabling things not yet discovered
        reg["capabilities"][name] = {"type": "manual"}
    reg["capabilities"][name]["status"] = "disabled"
    reg["capabilities"][name]["reason"] = args.reason or "Manually disabled"
    reg["capabilities"][name]["manual_override"] = True
    reg["capabilities"][name]["disabled_at"] = datetime.now(timezone.utc).isoformat()
    save_registry(reg)
    print(json.dumps({"disabled": name, "reason": args.reason}))

def cmd_status(args):
    """Show summary status."""
    reg = load_registry()
    caps = reg.get("capabilities", {})
    active = [n for n, c in caps.items() if c["status"] == "active"]
    disabled = [n for n, c in caps.items() if c["status"] == "disabled"]
    missing = [n for n, c in caps.items() if c["status"] == "missing"]

    print(json.dumps({
        "total": len(caps),
        "active": len(active),
        "disabled": len(disabled),
        "missing": len(missing),
        "active_list": sorted(active),
        "disabled_list": sorted(disabled),
        "missing_list": sorted(missing),
        "last_discovery": reg.get("last_discovery"),
    }))

def main():
    parser = argparse.ArgumentParser(description="AetherVault Capabilities Registry")
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("discover", help="Auto-discover capabilities from hooks, crons, services")

    lp = subparsers.add_parser("list", help="List all capabilities")
    lp.add_argument("--format", choices=["table", "json"], default="table")

    cp = subparsers.add_parser("check", help="Check a specific capability")
    cp.add_argument("name", help="Capability name (exact or partial match)")

    subparsers.add_parser("enable", help="Enable a capability").add_argument("name")

    dp = subparsers.add_parser("disable", help="Disable a capability")
    dp.add_argument("name")
    dp.add_argument("--reason", "-r", help="Reason for disabling")

    subparsers.add_parser("status", help="Show summary status")

    args = parser.parse_args()
    {
        "discover": cmd_discover,
        "list": cmd_list,
        "check": cmd_check,
        "enable": cmd_enable,
        "disable": cmd_disable,
        "status": cmd_status,
    }[args.command](args)

if __name__ == "__main__":
    main()

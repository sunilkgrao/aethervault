#!/usr/bin/env python3
"""
AetherVault Memory System Health Check
=======================================

Comprehensive health check + self-healing for the memory system.
The agent can call this to diagnose itself. Cron can call it for
dead-man's-switch alerting.

Usage:
    # Full health check (human-readable):
    python3 memory-health.py

    # JSON output (for agent consumption):
    python3 memory-health.py --format json

    # Auto-fix recoverable issues:
    python3 memory-health.py --fix

    # Alert mode (cron dead-man's switch — only sends Telegram on problems):
    python3 memory-health.py --alert-only
"""

import argparse
import datetime
import json
import os
import subprocess
import sys
import urllib.request

# Add script directory to path for shared module import
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hot_memory_store import (
    AETHERVAULT_HOME, CAPSULE_PATH, AETHERVAULT_BIN,
    HOT_MEMORY_PATH, ARCHIVE_PATH, HEALTH_PATH, FAILURE_PATH,
    MARKER_STALE_MINUTES, MIN_DISK_FREE_MB, CLAUDE_API_URL,
    load_env, log, log_error, log_warn,
    read_hot_memories, cleanup_temp_files, rotate_archive,
    prune_invalidated, send_telegram, atomic_write_json,
)


def check_marker_freshness() -> dict:
    """Check if the extractor marker is stale."""
    marker_path = os.path.join(AETHERVAULT_HOME, "data", "extractor-marker.json")
    if not os.path.isfile(marker_path):
        return {"status": "warn", "message": "No extractor marker found (never run?)"}

    try:
        with open(marker_path, "r") as f:
            data = json.load(f)
        last_processed = data.get("last_processed", "")
        if not last_processed:
            return {"status": "warn", "message": "Marker exists but empty"}

        last_ts = datetime.datetime.fromisoformat(last_processed.replace("Z", "+00:00"))
        now = datetime.datetime.now(datetime.timezone.utc)
        minutes_since = (now - last_ts).total_seconds() / 60.0

        if minutes_since > MARKER_STALE_MINUTES:
            return {
                "status": "critical",
                "message": f"Extractor hasn't run in {minutes_since:.0f}m (threshold: {MARKER_STALE_MINUTES}m)",
                "minutes_since": round(minutes_since, 1),
            }
        return {
            "status": "ok",
            "message": f"Last extraction {minutes_since:.1f}m ago",
            "minutes_since": round(minutes_since, 1),
        }
    except (json.JSONDecodeError, OSError, ValueError) as e:
        return {"status": "critical", "message": f"Marker corrupted: {e}"}


def check_hot_memories() -> dict:
    """Check hot memory file health."""
    if not os.path.isfile(HOT_MEMORY_PATH):
        return {"status": "warn", "message": "No hot memories file"}

    try:
        memories = read_hot_memories()
        total = len(memories)
        pinned = sum(1 for m in memories if m.get("metadata", {}).get("pinned"))
        invalidated = sum(1 for m in memories if m.get("metadata", {}).get("t_invalid"))
        active = total - invalidated

        issues = []
        if invalidated > 20:
            issues.append(f"{invalidated} invalidated entries (should prune)")
        if pinned > 40:
            issues.append(f"{pinned} pinned entries approaching cap (50)")

        status = "warn" if issues else "ok"
        return {
            "status": status,
            "message": f"{active} active, {pinned} pinned, {invalidated} invalidated"
                       + (f" [{'; '.join(issues)}]" if issues else ""),
            "total": total,
            "active": active,
            "pinned": pinned,
            "invalidated": invalidated,
        }
    except OSError as e:
        return {"status": "critical", "message": f"Cannot read hot memories: {e}"}


def check_archive_size() -> dict:
    """Check archive file size."""
    if not os.path.isfile(ARCHIVE_PATH):
        return {"status": "ok", "message": "No archive file yet", "lines": 0}

    try:
        with open(ARCHIVE_PATH, "r") as f:
            line_count = sum(1 for _ in f)
        size_mb = os.path.getsize(ARCHIVE_PATH) / (1024 * 1024)

        if line_count > 8000:
            return {
                "status": "warn",
                "message": f"Archive has {line_count} lines ({size_mb:.1f}MB), approaching rotation limit",
                "lines": line_count,
                "size_mb": round(size_mb, 1),
            }
        return {
            "status": "ok",
            "message": f"Archive: {line_count} lines ({size_mb:.1f}MB)",
            "lines": line_count,
            "size_mb": round(size_mb, 1),
        }
    except OSError as e:
        return {"status": "warn", "message": f"Cannot read archive: {e}"}


def check_disk_space() -> dict:
    """Check available disk space."""
    try:
        data_dir = os.path.dirname(HOT_MEMORY_PATH)
        stat = os.statvfs(data_dir)
        free_mb = (stat.f_bavail * stat.f_frsize) / (1024 * 1024)
        total_mb = (stat.f_blocks * stat.f_frsize) / (1024 * 1024)
        used_pct = ((total_mb - free_mb) / total_mb * 100) if total_mb > 0 else 0

        if free_mb < MIN_DISK_FREE_MB:
            return {
                "status": "critical",
                "message": f"Disk critically low: {free_mb:.0f}MB free ({used_pct:.0f}% used)",
                "free_mb": round(free_mb),
                "used_pct": round(used_pct, 1),
            }
        if free_mb < MIN_DISK_FREE_MB * 3:
            return {
                "status": "warn",
                "message": f"Disk getting low: {free_mb:.0f}MB free ({used_pct:.0f}% used)",
                "free_mb": round(free_mb),
                "used_pct": round(used_pct, 1),
            }
        return {
            "status": "ok",
            "message": f"Disk: {free_mb:.0f}MB free ({used_pct:.0f}% used)",
            "free_mb": round(free_mb),
            "used_pct": round(used_pct, 1),
        }
    except OSError as e:
        return {"status": "warn", "message": f"Cannot check disk: {e}"}


def check_api_proxy() -> dict:
    """Check if the Claude API proxy is reachable."""
    try:
        req = urllib.request.Request(CLAUDE_API_URL, method="OPTIONS")
        urllib.request.urlopen(req, timeout=5)
        return {"status": "ok", "message": f"API proxy reachable at {CLAUDE_API_URL}"}
    except urllib.error.HTTPError as e:
        # Any HTTP response means the proxy is running
        if e.code in (400, 401, 403, 404, 405):
            return {"status": "ok", "message": f"API proxy reachable (HTTP {e.code})"}
        return {"status": "warn", "message": f"API proxy returned HTTP {e.code}"}
    except Exception as e:
        return {"status": "critical", "message": f"API proxy unreachable: {e}"}


def check_capsule() -> dict:
    """Check if the capsule file exists and is queryable."""
    if not os.path.isfile(CAPSULE_PATH):
        return {"status": "critical", "message": f"Capsule not found at {CAPSULE_PATH}"}

    size_mb = os.path.getsize(CAPSULE_PATH) / (1024 * 1024)

    if not os.path.isfile(AETHERVAULT_BIN):
        binary = "aethervault"
    else:
        binary = AETHERVAULT_BIN

    try:
        result = subprocess.run(
            [binary, "query", "--collection", "agent-log", "--limit", "1",
             CAPSULE_PATH, "test"],
            capture_output=True, text=True, timeout=10,
        )
        if result.returncode == 0:
            return {
                "status": "ok",
                "message": f"Capsule queryable ({size_mb:.1f}MB)",
                "size_mb": round(size_mb, 1),
            }
        return {
            "status": "warn",
            "message": f"Capsule query failed: {result.stderr.strip()[:100]}",
            "size_mb": round(size_mb, 1),
        }
    except FileNotFoundError:
        return {"status": "critical", "message": f"aethervault binary not found: {binary}"}
    except subprocess.TimeoutExpired:
        return {"status": "warn", "message": "Capsule query timed out"}
    except Exception as e:
        return {"status": "warn", "message": f"Capsule check error: {e}"}


def check_cron() -> dict:
    """Check if memory cron jobs are configured."""
    try:
        result = subprocess.run(
            ["crontab", "-l"], capture_output=True, text=True, timeout=5,
        )
        if result.returncode != 0:
            return {"status": "warn", "message": "Cannot read crontab"}

        crontab = result.stdout
        has_extractor = "memory-extractor" in crontab
        has_reflection = "weekly-reflection" in crontab
        has_health = "memory-health" in crontab

        issues = []
        if not has_extractor:
            issues.append("memory-extractor not in crontab")
        if not has_reflection:
            issues.append("weekly-reflection not in crontab")
        if not has_health:
            issues.append("memory-health not in crontab (no dead-man's switch)")

        if issues:
            return {"status": "warn", "message": "; ".join(issues)}
        return {
            "status": "ok",
            "message": "All cron jobs configured (extractor, reflection, health)",
        }
    except Exception as e:
        return {"status": "warn", "message": f"Cannot check cron: {e}"}


def check_failures() -> dict:
    """Check consecutive failure tracking."""
    if not os.path.isfile(FAILURE_PATH):
        return {"status": "ok", "message": "No failures recorded", "components": {}}

    try:
        with open(FAILURE_PATH, "r") as f:
            data = json.load(f)

        failures = data.get("failures", {})
        if not failures:
            return {"status": "ok", "message": "No active failures", "components": {}}

        critical = []
        for component, info in failures.items():
            count = info.get("count", 0)
            if count >= 3:
                critical.append(f"{component}: {count} consecutive failures")

        if critical:
            return {
                "status": "critical",
                "message": "; ".join(critical),
                "components": failures,
            }
        return {
            "status": "warn",
            "message": f"{len(failures)} component(s) with recent failures",
            "components": failures,
        }
    except (json.JSONDecodeError, OSError):
        return {"status": "ok", "message": "Failure tracking file unreadable"}


def check_temp_files() -> dict:
    """Check for orphaned temp files."""
    data_dir = os.path.dirname(HOT_MEMORY_PATH)
    if not os.path.isdir(data_dir):
        return {"status": "ok", "message": "Data dir not found", "count": 0}

    orphans = []
    for name in os.listdir(data_dir):
        if name.endswith(".tmp") and name.startswith("."):
            orphans.append(name)

    if orphans:
        return {
            "status": "warn",
            "message": f"{len(orphans)} orphaned temp files in data dir",
            "count": len(orphans),
        }
    return {"status": "ok", "message": "No orphaned temp files", "count": 0}


# ---------------------------------------------------------------------------
# Main health check
# ---------------------------------------------------------------------------

def run_health_check() -> dict:
    """Run all health checks and return structured result."""
    checks = {
        "marker_freshness": check_marker_freshness(),
        "hot_memories": check_hot_memories(),
        "archive_size": check_archive_size(),
        "disk_space": check_disk_space(),
        "api_proxy": check_api_proxy(),
        "capsule": check_capsule(),
        "cron_jobs": check_cron(),
        "failure_tracking": check_failures(),
        "temp_files": check_temp_files(),
    }

    # Compute overall status
    statuses = [c["status"] for c in checks.values()]
    if "critical" in statuses:
        overall = "critical"
    elif "warn" in statuses:
        overall = "degraded"
    else:
        overall = "healthy"

    return {
        "overall": overall,
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "checks": checks,
    }


def auto_fix(report: dict) -> list:
    """Auto-fix recoverable issues. Returns list of actions taken."""
    actions = []

    # Fix orphaned temp files
    if report["checks"]["temp_files"]["status"] != "ok":
        cleanup_temp_files()
        actions.append("Cleaned orphaned temp files")

    # Fix archive bloat
    if report["checks"]["archive_size"]["status"] == "warn":
        rotate_archive()
        actions.append("Rotated archive file")

    # Fix invalidated memory accumulation
    hm = report["checks"]["hot_memories"]
    if hm.get("invalidated", 0) > 10:
        prune_invalidated(max_age_hours=24.0)
        actions.append(f"Pruned invalidated memories older than 24h")

    return actions


def format_report(report: dict) -> str:
    """Format health report as human-readable text."""
    lines = []
    status_icon = {"ok": "+", "warn": "~", "critical": "!"}

    overall = report["overall"]
    lines.append(f"Memory System Health: {overall.upper()}")
    lines.append(f"Checked at: {report['timestamp']}")
    lines.append("-" * 50)

    for name, check in report["checks"].items():
        icon = status_icon.get(check["status"], "?")
        label = name.replace("_", " ").title()
        lines.append(f"  [{icon}] {label}: {check['message']}")

    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(
        description="AetherVault Memory System Health Check",
    )
    parser.add_argument("--format", choices=["text", "json"], default="text")
    parser.add_argument("--fix", action="store_true",
                        help="Auto-fix recoverable issues")
    parser.add_argument("--alert-only", action="store_true",
                        help="Only send Telegram alerts on problems (for cron)")
    args = parser.parse_args()

    load_env()
    report = run_health_check()

    # Auto-fix if requested
    if args.fix:
        actions = auto_fix(report)
        if actions:
            report["auto_fix"] = actions
            # Re-run checks after fix
            report = run_health_check()
            report["auto_fix"] = actions

    # Write health status for other scripts to read
    try:
        atomic_write_json(HEALTH_PATH, report)
    except OSError:
        pass

    # Alert mode: only send Telegram on problems
    if args.alert_only:
        if report["overall"] == "critical":
            critical_checks = [
                f"  - {name}: {check['message']}"
                for name, check in report["checks"].items()
                if check["status"] == "critical"
            ]
            send_telegram(
                f"[HEALTH CRITICAL] Memory system has {len(critical_checks)} critical issues:\n"
                + "\n".join(critical_checks)
            )
        return

    # Output
    if args.format == "json":
        print(json.dumps(report, indent=2, default=str))
    else:
        print(format_report(report))
        if args.fix and report.get("auto_fix"):
            print()
            print("Auto-fix actions taken:")
            for action in report["auto_fix"]:
                print(f"  - {action}")

    # Exit code based on overall status
    if report["overall"] == "critical":
        sys.exit(2)
    elif report["overall"] == "degraded":
        sys.exit(1)
    else:
        sys.exit(0)


if __name__ == "__main__":
    main()

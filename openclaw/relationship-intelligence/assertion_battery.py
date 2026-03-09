#!/usr/bin/env python3
"""Assertion-based regression battery for Linus relationship intelligence.

This script is intended to run on the OpenClaw host. It creates an isolated
OpenClaw home so tests do not mutate live Telegram/Slack session state, then
executes a mix of:

- store-level migration/data-quality assertions
- direct relationship tool assertions
- isolated OpenClaw agent assertions
- mutation checks (touchpoint update should change radar behavior)
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sqlite3
import subprocess
import tempfile
import textwrap
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable


REMOTE_HOME = Path("/root/.openclaw")
WORKSPACE = REMOTE_HOME / "workspace"
REL_DIR = WORKSPACE / "relationship-intel"
REL_SCRIPT = REL_DIR / "relationship_intel.py"
REL_DB = REL_DIR / "relationship_intel.sqlite"
AUTH_FILE = REMOTE_HOME / "agents" / "main" / "agent" / "auth-profiles.json"


@dataclass
class TestResult:
    name: str
    status: str
    detail: str
    duration_ms: int = 0


def now_iso() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat()


def run(
    args: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    timeout: int = 300,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=str(cwd) if cwd else None,
        env=env,
        text=True,
        capture_output=True,
        timeout=timeout,
        check=False,
    )


def load_auth_key(auth_file: Path) -> str:
    payload = json.loads(auth_file.read_text(encoding="utf-8"))
    key = payload["profiles"]["anthropic:default"]["key"]
    if not key:
        raise RuntimeError("Anthropic key missing from auth-profiles.json")
    return key


def copy_openclaw_home(src_home: Path, dst_home: Path) -> None:
    shutil.copy2(src_home / "openclaw.json", dst_home / "openclaw.json")
    if (src_home / "models.json").exists():
        shutil.copy2(src_home / "models.json", dst_home / "models.json")
    shutil.copytree(src_home / "workspace", dst_home / "workspace", dirs_exist_ok=True)
    (dst_home / "agents" / "main" / "agent").mkdir(parents=True, exist_ok=True)
    (dst_home / "agents" / "main" / "sessions").mkdir(parents=True, exist_ok=True)
    shutil.copy2(src_home / "agents" / "main" / "agent" / "auth-profiles.json", dst_home / "agents" / "main" / "agent" / "auth-profiles.json")
    (dst_home / "agents" / "main" / "sessions" / "sessions.json").write_text("{}\n", encoding="utf-8")

    config_path = dst_home / "openclaw.json"
    config = json.loads(config_path.read_text(encoding="utf-8"))
    config.setdefault("agents", {}).setdefault("defaults", {})["workspace"] = str(dst_home / "workspace")
    for channel in ("telegram", "slack"):
        config.setdefault("channels", {}).setdefault(channel, {})["enabled"] = False
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")


def sqlite_value(conn: sqlite3.Connection, sql: str, params: tuple[Any, ...] = ()) -> Any:
    row = conn.execute(sql, params).fetchone()
    return row[0] if row else None


def agent_text(iso_home: Path, prompt: str, session_id: str, anthropic_key: str, timeout: int = 120) -> tuple[str, dict[str, Any]]:
    env = os.environ.copy()
    env["HOME"] = str(iso_home.parent)
    env["ANTHROPIC_API_KEY"] = anthropic_key
    proc = run(
        [
            "openclaw",
            "agent",
            "--local",
            "--json",
            "--timeout",
            str(timeout),
            "--session-id",
            session_id,
            "-m",
            prompt,
        ],
        env=env,
        timeout=timeout + 30,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"openclaw agent failed: {proc.stderr or proc.stdout}")
    payload = json.loads(proc.stdout)
    text = ""
    if payload.get("payloads"):
        text = payload["payloads"][0].get("text") or ""
    return text, payload


def rel_json(db_path: Path, *subcommand: str) -> Any:
    proc = run(
        ["python3", str(REL_SCRIPT), "--db", str(db_path), *subcommand, "--json"],
        timeout=120,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr or proc.stdout)
    return json.loads(proc.stdout)


def assert_keywords(text: str, required: list[str], forbidden: list[str] | None = None) -> tuple[bool, str]:
    lowered = text.casefold()
    missing = [term for term in required if term.casefold() not in lowered]
    forbidden_hits = [term for term in (forbidden or []) if term.casefold() in lowered]
    if missing:
        return False, f"Missing keywords: {', '.join(missing)}"
    if forbidden_hits:
        return False, f"Forbidden keywords present: {', '.join(forbidden_hits)}"
    return True, "keywords ok"


def trimmed(value: str, limit: int = 220) -> str:
    value = re.sub(r"\s+", " ", value).strip()
    if len(value) <= limit:
        return value
    return value[: limit - 3] + "..."


def run_battery(remote_home: Path, report_path: Path) -> dict[str, Any]:
    anthropic_key = load_auth_key(AUTH_FILE)
    results: list[TestResult] = []

    with tempfile.TemporaryDirectory(prefix="openclaw-assert-battery-") as tmp:
        tmp_root = Path(tmp)
        iso_home = tmp_root / ".openclaw"
        iso_home.mkdir(parents=True, exist_ok=True)
        copy_openclaw_home(remote_home, iso_home)
        iso_db = iso_home / "workspace" / "relationship-intel" / "relationship_intel.sqlite"

        conn = sqlite3.connect(iso_db)
        conn.row_factory = sqlite3.Row

        def record(name: str, ok: bool, detail: str, duration_ms: int = 0) -> None:
            results.append(TestResult(name=name, status="pass" if ok else "fail", detail=detail, duration_ms=duration_ms))

        # Store-level migration assertions.
        people = int(sqlite_value(conn, "select count(*) from people") or 0)
        with_actions = int(sqlite_value(conn, "select count(*) from people where open_actions_json <> '[]'") or 0)
        with_notes = int(sqlite_value(conn, "select count(*) from people where notes_json <> '[]'") or 0)
        with_dossiers = int(sqlite_value(conn, "select count(*) from people where length(dossier_excerpt) > 0") or 0)
        imessage_open_action_people = 0
        for row in conn.execute("select open_actions_json from people where open_actions_json <> '[]'"):
            actions = json.loads(row["open_actions_json"])
            if any("crm-imessage/profiles" in ((action.get("created_from") or "")) for action in actions):
                imessage_open_action_people += 1
        legacy_import_tree = remote_home / "workspace" / "imports" / "legacy-aethervault"
        whatsapp_archive_hits = 0
        if legacy_import_tree.exists():
            for path in legacy_import_tree.rglob("*"):
                if not path.is_file():
                    continue
                try:
                    text = path.read_text(encoding="utf-8", errors="ignore")
                except OSError:
                    continue
                if "WhatsApp" in text or "whatsapp" in text:
                    whatsapp_archive_hits += 1
                    if whatsapp_archive_hits >= 10:
                        break

        record(
            "store_migration_integrity",
            people >= 5000 and with_actions >= 400 and with_notes >= 450 and with_dossiers >= 35,
            f"people={people}, with_actions={with_actions}, with_notes={with_notes}, with_dossiers={with_dossiers}",
        )
        record(
            "imessage_provenance_present",
            imessage_open_action_people >= 350,
            f"people_with_imessage_actions={imessage_open_action_people}",
        )
        record(
            "whatsapp_archive_preserved",
            whatsapp_archive_hits >= 3,
            f"whatsapp_archive_hits={whatsapp_archive_hits}",
        )

        # Direct tool assertions.
        parents = rel_json(iso_db, "summary", "Prasad Rao")
        ok = parents["relationship_label"] == "father" and "+14167092606" in parents["phones"]
        record("tool_parent_summary", ok, trimmed(json.dumps(parents, ensure_ascii=True)))

        angelic = rel_json(iso_db, "summary", "Angelic")
        ok = angelic["relationship_label"] == "wife" and "Marie-Angelic Vendette" in angelic["display_name"]
        record("tool_alias_resolution_spouse", ok, trimmed(json.dumps(angelic, ensure_ascii=True)))

        rohan = rel_json(iso_db, "summary", "Rohan")
        ok = any("investor introductions" in action["description"].casefold() for action in rohan["open_actions"])
        record("tool_rohan_open_loop", ok, trimmed(json.dumps(rohan, ensure_ascii=True)))

        andrew = rel_json(iso_db, "summary", "Andrew Green")
        ok = "Seacrest Advisors" in andrew["why_they_matter"] or "Seacrest Advisors" in andrew["dossier_excerpt"]
        record("tool_dossier_grounding", ok, trimmed(json.dumps(andrew, ensure_ascii=True)))

        brief = rel_json(iso_db, "brief", "--reconnect-limit", "5", "--loop-limit", "5", "--date-limit", "3")
        reconnect_names = [item["display_name"] for item in brief["priority_reconnect"]]
        ok = {"Shachin", "Andy", "Alain Denzler"}.intersection(reconnect_names) and any(item["display_name"] == "Rohan" for item in brief["open_loops"])
        record("tool_weekly_brief_quality", bool(ok), trimmed(json.dumps(brief, ensure_ascii=True)))

        # Mutation assertion: touchpoint should update recency and remove from reconnect urgency.
        touch_proc = run(
            [
                "python3",
                str(REL_SCRIPT),
                "--db",
                str(iso_db),
                "touch",
                "Shachin",
                "--note",
                "Caught up today about how things are going.",
                "--channel",
                "sms",
                "--touched-at",
                now_iso(),
                "--json",
            ],
            timeout=120,
        )
        if touch_proc.returncode != 0:
            record("tool_touchpoint_recency_update", False, touch_proc.stderr or touch_proc.stdout)
        else:
            after_touch = rel_json(iso_db, "summary", "Shachin")
            after_brief = rel_json(iso_db, "brief", "--reconnect-limit", "5", "--loop-limit", "5", "--date-limit", "3")
            still_reconnect = any(item["display_name"] == "Shachin" for item in after_brief["priority_reconnect"])
            days_since = after_touch["days_since_touch"]
            ok = days_since is not None and days_since <= 1 and not still_reconnect
            record(
                "tool_touchpoint_recency_update",
                ok,
                trimmed(json.dumps({"summary": after_touch, "priority_reconnect": after_brief["priority_reconnect"]}, ensure_ascii=True)),
            )

        # Agent assertions on isolated OpenClaw.
        def trivial_inline_checker(text: str) -> tuple[bool, str]:
            lowered = text.casefold()
            good = any(
                phrase in lowered
                for phrase in (
                    "answer directly",
                    "handle it inline",
                    "do it inline",
                    "trivial",
                )
            )
            bad = "spawn subagents for every task" in lowered
            if not good:
                return False, "Expected direct/inline handling for trivial task"
            if bad:
                return False, "Still using absolutist spawn-every-task phrasing"
            return True, "trivial inline policy ok"

        def orchestration_policy_checker(text: str) -> tuple[bool, str]:
            lowered = text.casefold()
            has_inline_orientation = any(
                phrase in lowered
                for phrase in (
                    "first-pass orientation",
                    "task decomposition",
                    "do it myself",
                    "do it inline",
                    "read the key files",
                )
            )
            has_worker_fanout = (
                "spawn subagents" in lowered
                or "workers execute" in lowered
                or ("subagents" in lowered and "parallel" in lowered)
            )
            if not has_inline_orientation:
                return False, "Expected inline orientation/decomposition step"
            if not has_worker_fanout:
                return False, "Expected worker fan-out for deeper investigation"
            if "spawn subagents. every time." in lowered or "never do work yourself" in lowered:
                return False, "Still using absolutist delegation phrasing"
            return True, "orchestration policy ok"

        agent_cases: list[tuple[str, str, Callable[[str], tuple[bool, str]]]] = [
            (
                "agent_parents_alias",
                "Who is Baba?",
                lambda text: assert_keywords(text, ["Prasad", "father"], ["Uma Rao"]),
            ),
            (
                "agent_parent_travel_inference",
                "I need to find flights for my parents to visit me.",
                lambda text: assert_keywords(text, ["Toronto", "PBI"], ["where are they flying from"]),
            ),
            (
                "agent_rhaine_handoff",
                "Once I approve flights for my parents, who should handle the booking?",
                lambda text: assert_keywords(text, ["Rhaine", "book"], []),
            ),
            (
                "agent_rohan_context",
                "Who is Rohan and what is the main open loop with him?",
                lambda text: assert_keywords(text, ["Rohan", "investor"], []),
            ),
            (
                "agent_andrew_context",
                "Who is Andrew Green?",
                lambda text: assert_keywords(text, ["Seacrest", "commercial real estate"], []),
            ),
            (
                "agent_weekly_reconnects",
                "Who should I reach out to this week and why?",
                lambda text: assert_keywords(text, ["Shachin", "Andy", "Alain"], []),
            ),
            (
                "agent_orchestration_policy",
                "You need to investigate a codebase and prepare a plan. Should you do the work yourself or spawn subagents?",
                orchestration_policy_checker,
            ),
            (
                "agent_inline_trivial_policy",
                "If the task is a trivial one-shot lookup like answering who my parents are, should you still spawn subagents?",
                trivial_inline_checker,
            ),
        ]

        for index, (name, prompt, checker) in enumerate(agent_cases, start=1):
            started = datetime.now(timezone.utc)
            try:
                text, payload = agent_text(
                    iso_home,
                    prompt,
                    session_id=f"assertion-{index}",
                    anthropic_key=anthropic_key,
                    timeout=120,
                )
                ok, detail = checker(text)
                duration_ms = int(payload.get("meta", {}).get("durationMs") or 0)
                record(name, ok, f"{detail} | response={trimmed(text, 280)}", duration_ms)
            except Exception as exc:  # pragma: no cover - integration failure path
                duration_ms = int((datetime.now(timezone.utc) - started).total_seconds() * 1000)
                record(name, False, f"{type(exc).__name__}: {exc}", duration_ms)

        conn.close()

    summary = {
        "generated_at": now_iso(),
        "total": len(results),
        "passed": sum(1 for item in results if item.status == "pass"),
        "failed": sum(1 for item in results if item.status == "fail"),
    }

    lines = [
        "# OpenClaw Relationship Assertion Battery",
        "",
        f"- Generated at: {summary['generated_at']}",
        f"- Total: {summary['total']}",
        f"- Passed: {summary['passed']}",
        f"- Failed: {summary['failed']}",
        "",
        "## Results",
        "",
    ]
    for item in results:
        prefix = "PASS" if item.status == "pass" else "FAIL"
        duration = f" ({item.duration_ms}ms)" if item.duration_ms else ""
        lines.append(f"- `{prefix}` `{item.name}`{duration} — {item.detail}")

    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8")

    return {
        "summary": summary,
        "results": [item.__dict__ for item in results],
        "report_path": str(report_path),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Run the Linus relationship assertion battery on an OpenClaw host")
    parser.add_argument("--openclaw-home", default=str(REMOTE_HOME))
    parser.add_argument("--report-path", help="Optional markdown report path")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    openclaw_home = Path(args.openclaw_home).expanduser().resolve()
    stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
    report_path = Path(args.report_path).expanduser().resolve() if args.report_path else openclaw_home / "reports" / f"openclaw-relationship-assertions-{stamp}.md"

    result = run_battery(openclaw_home, report_path)
    if args.json:
        print(json.dumps(result, indent=2, ensure_ascii=True))
    else:
        print(report_path)
        print(json.dumps(result["summary"], indent=2, ensure_ascii=True))
    return 0 if result["summary"]["failed"] == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())

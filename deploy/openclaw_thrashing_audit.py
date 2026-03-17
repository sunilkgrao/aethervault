#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from collections import Counter
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Iterable


LOW_SIGNAL_PATTERNS = [
    re.compile(r"^\s*(?:let me|now let me|good, now let me|ok, now let me|okay, now let me)\b", re.I | re.M),
    re.compile(r"^\s*(?:i'm spending too much time|this approach is getting too convoluted|i need a different approach)\b", re.I | re.M),
    re.compile(r"^\s*(?:wait\b|actually\b|interesting\b|hmm\b|good,\b|ok,\b|okay,\b)", re.I | re.M),
    re.compile(r"^\s*(?:on it\b|understood\b)\s*[-—:]", re.I | re.M),
]

REVERSAL_PATTERNS = [
    re.compile(r"\b(?:actually|wait|that changes the picture|my .* hypothesis was wrong|premature|i need a different approach)\b", re.I),
]

EVIDENCE_PATTERNS = [
    re.compile(r"\bStatus\s*:", re.I),
    re.compile(r"\bBlocker\s*:", re.I),
    re.compile(r"\breproduced locally\b", re.I),
    re.compile(r"\bbuild passed\b", re.I),
    re.compile(r"\btypecheck passed\b", re.I),
    re.compile(r"\bbackend validated locally\b", re.I),
    re.compile(r"\bfully locally tested\b", re.I),
    re.compile(r"\bscreenshot", re.I),
]

SIGNAL_TO_LESSON = [
    (re.compile(r"\bENOSPC\b", re.I), "Raise Linux/WSL inotify limits before repeated Vite/lcars restarts."),
    (re.compile(r"\bNo project found\b", re.I), "When a local route 404s, verify backend query/schema alignment before frontend theories."),
    (re.compile(r"\bblank\b.*\bspreadsheet\b|\bno e2e workbook\b", re.I), "Spreadsheet/E2E repros need exact workbook/sheet/answer-entry data shape, not just content_detail rows."),
    (re.compile(r"\breadonly\b.*\bprod|\bproduction data\b.*\blocal\b", re.I), "Prefer approved readonly production-data clone to local over hand-built partial fixtures for customer-specific bugs."),
    (re.compile(r"\bscreen recording\b|\bshare a recording\b", re.I), "If local repro is blocked, ask once for the smallest missing artifact that collapses uncertainty fastest."),
    (re.compile(r"\bapi\b.*\bworks\b.*\bfrontend\b", re.I), "API-only success does not prove the UI bug was reproduced; keep backend validation separate from UI root cause."),
]


@dataclass
class SessionFinding:
    path: Path
    score: int
    session_id: str | None = None
    first_ts: str | None = None
    first_user: str = ""
    assistant_count: int = 0
    low_signal_count: int = 0
    reversal_count: int = 0
    evidence_count: int = 0
    examples: list[str] = field(default_factory=list)
    lesson_hits: Counter = field(default_factory=Counter)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Audit OpenClaw sessions for thrashing patterns.")
    parser.add_argument("--sessions-dir", default="/root/.openclaw/agents/main/sessions")
    parser.add_argument("--lookback-hours", type=int, default=72)
    parser.add_argument("--max-sessions", type=int, default=200)
    parser.add_argument("--top", type=int, default=12)
    parser.add_argument("--out", default="")
    return parser.parse_args()


def parse_timestamp(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(UTC)
    except Exception:
        return None


def iter_session_files(path: Path, cutoff: datetime) -> Iterable[Path]:
    files = sorted(path.glob("*.jsonl"), key=lambda p: p.stat().st_mtime, reverse=True)
    yielded = 0
    for file_path in files:
        modified = datetime.fromtimestamp(file_path.stat().st_mtime, tz=UTC)
        if modified < cutoff:
            continue
        yield file_path
        yielded += 1
        if yielded >= 2000:
            break


def flatten_text_content(content: object) -> list[str]:
    texts: list[str] = []
    if not isinstance(content, list):
        return texts
    for item in content:
        if not isinstance(item, dict):
            continue
        if item.get("type") == "text":
            text = str(item.get("text") or "").strip()
            if text:
                texts.append(text)
    return texts


def is_low_signal(text: str) -> bool:
    if any(p.search(text) for p in EVIDENCE_PATTERNS):
        return False
    hits = sum(1 for pattern in LOW_SIGNAL_PATTERNS if pattern.search(text))
    line_count = len([line for line in text.splitlines() if line.strip()])
    return hits >= 2 or (hits >= 1 and line_count >= 3)


def analyze_session(path: Path) -> SessionFinding | None:
    finding = SessionFinding(path=path, score=0)
    try:
        lines = path.read_text(errors="ignore").splitlines()
    except Exception:
        return None

    for raw in lines:
        try:
            event = json.loads(raw)
        except Exception:
            continue
        if event.get("type") == "session":
            finding.session_id = str(event.get("id") or "")
            finding.first_ts = str(event.get("timestamp") or "")
            continue
        if event.get("type") != "message":
            continue
        message = event.get("message")
        if not isinstance(message, dict):
            continue
        role = message.get("role")
        texts = flatten_text_content(message.get("content"))
        if not texts:
            continue
        for text in texts:
            if role == "user" and not finding.first_user:
                finding.first_user = text[:300].replace("\n", " ")
            if role != "assistant":
                for pattern, lesson in SIGNAL_TO_LESSON:
                    if pattern.search(text):
                        finding.lesson_hits[lesson] += 1
                continue
            if text == "NO_REPLY":
                continue
            finding.assistant_count += 1
            if any(pattern.search(text) for pattern in REVERSAL_PATTERNS):
                finding.reversal_count += 1
                finding.score += 2
                if len(finding.examples) < 4:
                    finding.examples.append(text[:240].replace("\n", " "))
            if any(pattern.search(text) for pattern in EVIDENCE_PATTERNS):
                finding.evidence_count += 1
            if is_low_signal(text):
                finding.low_signal_count += 1
                finding.score += 3
                if len(finding.examples) < 4:
                    finding.examples.append(text[:240].replace("\n", " "))
            for pattern, lesson in SIGNAL_TO_LESSON:
                if pattern.search(text):
                    finding.lesson_hits[lesson] += 1

    if finding.assistant_count >= 6:
        finding.score += 2
    if finding.low_signal_count >= 3:
        finding.score += 3
    if finding.reversal_count >= 2:
        finding.score += 2
    if finding.low_signal_count == 0 and finding.reversal_count == 0:
        return None
    return finding


def render_report(findings: list[SessionFinding], lookback_hours: int) -> str:
    now = datetime.now(tz=UTC)
    lines: list[str] = []
    lines.append(f"# OpenClaw Thrash Audit")
    lines.append("")
    lines.append(f"- Generated: {now.isoformat()}")
    lines.append(f"- Lookback: last {lookback_hours} hours")
    lines.append(f"- Sessions flagged: {len(findings)}")
    lines.append("")

    lesson_counter: Counter[str] = Counter()
    for finding in findings:
        lesson_counter.update(finding.lesson_hits)

    lines.append("## Candidate lessons")
    if lesson_counter:
        for lesson, count in lesson_counter.most_common(10):
            lines.append(f"- {lesson} ({count})")
    else:
        lines.append("- No recurring lesson candidates detected in this window.")
    lines.append("")

    lines.append("## Worst sessions")
    for finding in findings:
        lines.append(f"### {finding.path.name}")
        lines.append(f"- score: {finding.score}")
        lines.append(f"- assistant messages: {finding.assistant_count}")
        lines.append(f"- low-signal messages: {finding.low_signal_count}")
        lines.append(f"- reversal messages: {finding.reversal_count}")
        lines.append(f"- evidence messages: {finding.evidence_count}")
        if finding.first_user:
            lines.append(f"- first user prompt: {finding.first_user}")
        if finding.examples:
            lines.append(f"- examples:")
            for example in finding.examples:
                lines.append(f"  - {example}")
        lines.append("")

    return "\n".join(lines).rstrip() + "\n"


def main() -> int:
    args = parse_args()
    sessions_dir = Path(args.sessions_dir)
    cutoff = datetime.now(tz=UTC) - timedelta(hours=args.lookback_hours)

    findings: list[SessionFinding] = []
    for path in iter_session_files(sessions_dir, cutoff):
        finding = analyze_session(path)
        if finding:
            findings.append(finding)
        if len(findings) >= args.max_sessions:
            break

    findings.sort(key=lambda item: item.score, reverse=True)
    findings = findings[: args.top]
    report = render_report(findings, args.lookback_hours)

    if args.out:
        out_path = Path(args.out)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(report)
    print(report, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

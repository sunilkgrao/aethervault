#!/usr/bin/env python3
from __future__ import annotations

"""
AetherVault Doc Gardener — Staleness & Consistency Checker
==========================================================

Lightweight, mechanical documentation health scanner. No LLM calls.
Checks hot memories for staleness, MEMORY.md for placeholders and
inconsistencies, and cross-references facts for contradictions.

Usage:
    # Full report + Telegram summary:
    python3 doc-gardener.py

    # Quick staleness check only (no Telegram):
    python3 doc-gardener.py --quick

    # Custom MEMORY.md path:
    python3 doc-gardener.py --memory-file /path/to/MEMORY.md
"""

import argparse
import datetime
import os
import re
import sys

# Add script directory to path for shared module import
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hot_memory_store import (
    AETHERVAULT_HOME,
    log,
    log_warn,
    send_telegram,
    read_hot_memories,
    atomic_write_json,
    load_env,
)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

STALE_DAYS = 14
UNREINFORCED_DAYS = 14
ZERO_ACCESS_DAYS = 7
DECAY_FLOOR = 0.3

# Patterns for MEMORY.md placeholder detection
PLACEHOLDER_PATTERNS = [
    re.compile(r"\(to be configured\)", re.IGNORECASE),
    re.compile(r"\(pending\)", re.IGNORECASE),
    re.compile(r"\(TBD\)", re.IGNORECASE),
]

# Pattern for date references (YYYY-MM-DD)
DATE_PATTERN = re.compile(r"\b(20\d{2}-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12]\d|3[01]))\b")

# Empty table row: pipe-delimited row where all cells are whitespace
EMPTY_ROW_PATTERN = re.compile(r"^\|(?:\s*\|)+\s*$")

# Grade thresholds (issue count per memory checked)
GRADE_THRESHOLDS = {
    "A": 0.02,  # <= 2% issue rate
    "B": 0.05,
    "C": 0.10,
    "D": 0.20,
    # F: > 20%
}


# ---------------------------------------------------------------------------
# Date helpers
# ---------------------------------------------------------------------------

def _now_utc() -> datetime.datetime:
    return datetime.datetime.now(datetime.timezone.utc)


def _parse_iso(datestr: str) -> datetime.datetime | None:
    """Parse ISO datetime string, return UTC-aware datetime or None."""
    if not datestr:
        return None
    try:
        dt = datetime.datetime.fromisoformat(datestr.replace("Z", "+00:00"))
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=datetime.timezone.utc)
        return dt
    except (ValueError, TypeError):
        return None


def _days_ago(dt: datetime.datetime) -> float:
    """Return days elapsed since dt."""
    return (_now_utc() - dt).total_seconds() / 86400.0


def _parse_date_str(s: str) -> datetime.date | None:
    """Parse YYYY-MM-DD string into date object."""
    try:
        return datetime.date.fromisoformat(s)
    except (ValueError, TypeError):
        return None


# ---------------------------------------------------------------------------
# Finding dataclass
# ---------------------------------------------------------------------------

class Finding:
    """A single issue found by the gardener."""

    __slots__ = ("category", "severity", "text", "recommendation")

    def __init__(self, category: str, severity: str, text: str, recommendation: str):
        self.category = category
        self.severity = severity  # "info", "warn", "critical"
        self.text = text
        self.recommendation = recommendation

    def to_dict(self) -> dict:
        return {
            "category": self.category,
            "severity": self.severity,
            "text": self.text,
            "recommendation": self.recommendation,
        }


# ---------------------------------------------------------------------------
# Check 1: Hot memory staleness
# ---------------------------------------------------------------------------

def check_hot_memory_staleness(memories: list) -> list[Finding]:
    """Flag stale, zero-access, and decayed hot memories."""
    findings = []
    now = _now_utc()
    today = now.date()

    for mem in memories:
        fact = mem.get("fact", "")
        meta = mem.get("metadata", {})

        # Skip invalidated memories
        if meta.get("t_invalid"):
            continue

        fact_preview = fact[:80] + ("..." if len(fact) > 80 else "")

        # 1a. Date references older than STALE_DAYS
        for match in DATE_PATTERN.finditer(fact):
            ref_date = _parse_date_str(match.group(1))
            if ref_date and (today - ref_date).days > STALE_DAYS:
                findings.append(Finding(
                    category="staleness",
                    severity="warn",
                    text=f"References past date {match.group(1)}: {fact_preview}",
                    recommendation="Review if this fact is still current; invalidate or update.",
                ))
                break  # one finding per memory for date staleness

        # 1b. Zero access count + older than ZERO_ACCESS_DAYS
        access_count = meta.get("access_count", 0)
        created_at = _parse_iso(meta.get("created_at", ""))
        if access_count == 0 and created_at and _days_ago(created_at) > ZERO_ACCESS_DAYS:
            findings.append(Finding(
                category="staleness",
                severity="info",
                text=f"Zero access in {_days_ago(created_at):.0f}d: {fact_preview}",
                recommendation="Consider archiving if not needed.",
            ))

        # 1c. Decay strength below floor
        decay = meta.get("decay_strength")
        if decay is not None and decay < DECAY_FLOOR:
            findings.append(Finding(
                category="decay",
                severity="warn",
                text=f"Decay strength {decay:.2f} (floor {DECAY_FLOOR}): {fact_preview}",
                recommendation="Memory is fading. Reinforce (access) or archive.",
            ))

    return findings


# ---------------------------------------------------------------------------
# Check 2: MEMORY.md consistency
# ---------------------------------------------------------------------------

def check_memory_md(memory_path: str) -> list[Finding]:
    """Parse MEMORY.md and flag placeholders, empty tables, stale dates."""
    findings = []

    if not os.path.isfile(memory_path):
        findings.append(Finding(
            category="memory_md",
            severity="critical",
            text=f"MEMORY.md not found at {memory_path}",
            recommendation="Ensure MEMORY.md exists at the expected path.",
        ))
        return findings

    try:
        with open(memory_path, "r") as f:
            lines = f.readlines()
    except OSError as e:
        findings.append(Finding(
            category="memory_md",
            severity="critical",
            text=f"Cannot read MEMORY.md: {e}",
            recommendation="Check file permissions.",
        ))
        return findings

    today = _now_utc().date()
    current_section = "(top)"

    for i, line in enumerate(lines, start=1):
        stripped = line.strip()

        # Track current section
        if stripped.startswith("#"):
            current_section = stripped.lstrip("#").strip()

        # 2a. Placeholder patterns
        for pat in PLACEHOLDER_PATTERNS:
            if pat.search(stripped):
                findings.append(Finding(
                    category="placeholder",
                    severity="warn",
                    text=f"Line {i} [{current_section}]: {stripped[:100]}",
                    recommendation="Fill in or remove placeholder.",
                ))

        # 2b. Empty table rows
        if EMPTY_ROW_PATTERN.match(stripped):
            findings.append(Finding(
                category="placeholder",
                severity="info",
                text=f"Line {i} [{current_section}]: empty table row",
                recommendation="Populate table data or remove empty row.",
            ))

        # 2c. Active context with past dates (only in Active Context section)
        if "active context" in current_section.lower():
            for match in DATE_PATTERN.finditer(stripped):
                ref_date = _parse_date_str(match.group(1))
                if ref_date and (today - ref_date).days > STALE_DAYS:
                    findings.append(Finding(
                        category="stale_context",
                        severity="warn",
                        text=f"Line {i} [{current_section}]: references {match.group(1)}",
                        recommendation="Update or archive stale active context entry.",
                    ))
                    break

    return findings


# ---------------------------------------------------------------------------
# Check 3: Cross-reference validation
# ---------------------------------------------------------------------------

# Simple state keywords to detect contradictions
_STATE_WORDS = {
    "enabled": "disabled",
    "disabled": "enabled",
    "active": "inactive",
    "inactive": "active",
    "running": "stopped",
    "stopped": "running",
    "installed": "removed",
    "removed": "installed",
    "true": "false",
    "false": "true",
}


def _extract_entity_state_pairs(fact: str) -> list[tuple[str, str]]:
    """Extract (entity_hint, state_word) pairs from a fact string.

    Uses a simple heuristic: look for known state words and pair them
    with the preceding noun-like token as an entity hint.
    """
    words = fact.lower().split()
    pairs = []
    for idx, word in enumerate(words):
        clean = re.sub(r"[^a-z]", "", word)
        if clean in _STATE_WORDS and idx > 0:
            entity = re.sub(r"[^a-z0-9_-]", "", words[idx - 1])
            if entity and len(entity) > 2:
                pairs.append((entity, clean))
    return pairs


def check_cross_references(memories: list) -> list[Finding]:
    """Detect contradictory state claims across hot memories."""
    findings = []

    # Build map: entity -> list of (state, fact_preview)
    entity_states: dict[str, list[tuple[str, str]]] = {}
    for mem in memories:
        fact = mem.get("fact", "")
        meta = mem.get("metadata", {})
        if meta.get("t_invalid"):
            continue
        for entity, state in _extract_entity_state_pairs(fact):
            entity_states.setdefault(entity, []).append(
                (state, fact[:80])
            )

    # Find contradictions
    for entity, states in entity_states.items():
        state_set = {s for s, _ in states}
        for state in list(state_set):
            opposite = _STATE_WORDS.get(state)
            if opposite and opposite in state_set:
                examples = [f for s, f in states if s in (state, opposite)]
                findings.append(Finding(
                    category="contradiction",
                    severity="warn",
                    text=f"Entity '{entity}' has both '{state}' and '{opposite}'",
                    recommendation=f"Reconcile: {' | '.join(examples[:2])}",
                ))
                # Avoid duplicate for the same pair
                state_set.discard(opposite)

    return findings


# ---------------------------------------------------------------------------
# Check 4: Age-based flagging (unreinforced)
# ---------------------------------------------------------------------------

def check_unreinforced(memories: list) -> list[Finding]:
    """Flag memories older than UNREINFORCED_DAYS with no reinforcement."""
    findings = []

    for mem in memories:
        fact = mem.get("fact", "")
        meta = mem.get("metadata", {})
        if meta.get("t_invalid"):
            continue

        created_at = _parse_iso(meta.get("created_at", ""))
        last_accessed = _parse_iso(meta.get("last_accessed", ""))
        access_count = meta.get("access_count", 0)

        if not created_at:
            continue

        age_days = _days_ago(created_at)
        if age_days <= UNREINFORCED_DAYS:
            continue

        # Reinforced = accessed at least once after creation
        reinforced = False
        if last_accessed and access_count and access_count > 0:
            if last_accessed > created_at + datetime.timedelta(hours=1):
                reinforced = True

        if not reinforced:
            findings.append(Finding(
                category="unreinforced",
                severity="info",
                text=f"Unreinforced ({age_days:.0f}d old, {access_count} accesses): "
                     f"{fact[:80]}",
                recommendation="Reinforce by accessing, or archive if obsolete.",
            ))

    return findings


# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------

def compute_grade(total_checked: int, issue_count: int) -> str:
    """Compute health grade A-F based on issue density."""
    if total_checked == 0:
        return "A"
    ratio = issue_count / total_checked
    for grade, threshold in GRADE_THRESHOLDS.items():
        if ratio <= threshold:
            return grade
    return "F"


def generate_report(
    findings: list[Finding],
    total_memories: int,
    memory_md_path: str,
) -> str:
    """Generate markdown report."""
    now = _now_utc()
    date_str = now.strftime("%Y-%m-%d")

    # Category counts
    categories: dict[str, int] = {}
    severity_counts: dict[str, int] = {"info": 0, "warn": 0, "critical": 0}
    for f in findings:
        categories[f.category] = categories.get(f.category, 0) + 1
        severity_counts[f.severity] = severity_counts.get(f.severity, 0) + 1

    total_issues = len(findings)
    grade = compute_grade(total_memories + 1, total_issues)  # +1 for MEMORY.md

    lines = [
        f"# Doc Gardener Report — {date_str}",
        "",
        "## Summary",
        "",
        f"| Metric | Value |",
        f"|--------|-------|",
        f"| Hot memories checked | {total_memories} |",
        f"| MEMORY.md | {memory_md_path} |",
        f"| Total issues | {total_issues} |",
        f"| Critical | {severity_counts['critical']} |",
        f"| Warnings | {severity_counts['warn']} |",
        f"| Info | {severity_counts['info']} |",
        f"| **Health Grade** | **{grade}** |",
        "",
        "### Issues by Category",
        "",
    ]

    if categories:
        lines.append("| Category | Count |")
        lines.append("|----------|-------|")
        for cat, count in sorted(categories.items(), key=lambda x: -x[1]):
            lines.append(f"| {cat} | {count} |")
    else:
        lines.append("No issues found.")

    lines.append("")
    lines.append("## Detailed Findings")
    lines.append("")

    if not findings:
        lines.append("All checks passed. Documentation is healthy.")
    else:
        # Group by category
        grouped: dict[str, list[Finding]] = {}
        for f in findings:
            grouped.setdefault(f.category, []).append(f)

        for cat, cat_findings in sorted(grouped.items()):
            lines.append(f"### {cat.replace('_', ' ').title()} ({len(cat_findings)})")
            lines.append("")
            for f in cat_findings:
                icon = {"critical": "!", "warn": "~", "info": "-"}[f.severity]
                lines.append(f"- [{icon}] {f.text}")
                lines.append(f"  - Action: {f.recommendation}")
            lines.append("")

    lines.append("---")
    lines.append(f"Generated: {now.isoformat()}")
    lines.append("")

    return "\n".join(lines)


def write_report(report_text: str) -> str:
    """Write report to data/garden-report-YYYY-MM-DD.md. Returns filepath."""
    date_str = _now_utc().strftime("%Y-%m-%d")
    data_dir = os.path.join(AETHERVAULT_HOME, "data")
    os.makedirs(data_dir, exist_ok=True)
    filepath = os.path.join(data_dir, f"garden-report-{date_str}.md")

    with open(filepath, "w") as f:
        f.write(report_text)

    log(f"Report written to {filepath}")
    return filepath


# ---------------------------------------------------------------------------
# Telegram summary
# ---------------------------------------------------------------------------

def send_summary(findings: list[Finding], total_memories: int, grade: str):
    """Send brief Telegram summary."""
    categories: dict[str, int] = {}
    for f in findings:
        categories[f.category] = categories.get(f.category, 0) + 1

    cat_lines = "\n".join(
        f"  {cat}: {count}" for cat, count in sorted(categories.items(), key=lambda x: -x[1])
    )

    msg = (
        f"[Doc Gardener] Grade: {grade}\n"
        f"Checked: {total_memories} memories\n"
        f"Issues: {len(findings)}\n"
    )
    if cat_lines:
        msg += cat_lines + "\n"

    critical = [f for f in findings if f.severity == "critical"]
    if critical:
        msg += "\nCritical:\n"
        for f in critical[:3]:
            msg += f"  - {f.text[:100]}\n"

    send_telegram(msg.strip())


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="AetherVault Doc Gardener — staleness & consistency checker",
    )
    parser.add_argument(
        "--quick",
        action="store_true",
        help="Quick mode: staleness check only, no Telegram",
    )
    parser.add_argument(
        "--memory-file",
        default=os.path.join(AETHERVAULT_HOME, "MEMORY.md"),
        help="Path to MEMORY.md (default: $AETHERVAULT_HOME/MEMORY.md)",
    )
    args = parser.parse_args()

    load_env()

    all_findings: list[Finding] = []

    # Load hot memories
    memories = read_hot_memories()
    total_memories = len(memories)
    log(f"Loaded {total_memories} hot memories")

    # Check 1: Hot memory staleness (always)
    staleness_findings = check_hot_memory_staleness(memories)
    all_findings.extend(staleness_findings)
    log(f"Staleness check: {len(staleness_findings)} issues")

    if not args.quick:
        # Check 2: MEMORY.md consistency
        md_findings = check_memory_md(args.memory_file)
        all_findings.extend(md_findings)
        log(f"MEMORY.md check: {len(md_findings)} issues")

        # Check 3: Cross-reference validation
        xref_findings = check_cross_references(memories)
        all_findings.extend(xref_findings)
        log(f"Cross-reference check: {len(xref_findings)} issues")

        # Check 4: Unreinforced memories
        unreinforced_findings = check_unreinforced(memories)
        all_findings.extend(unreinforced_findings)
        log(f"Unreinforced check: {len(unreinforced_findings)} issues")

    # Grade
    grade = compute_grade(total_memories + 1, len(all_findings))

    # Report
    report_text = generate_report(all_findings, total_memories, args.memory_file)
    report_path = write_report(report_text)

    # Print summary
    print()
    print(f"Health Grade: {grade}")
    print(f"Total issues: {len(all_findings)}")
    print(f"Report: {report_path}")

    # Telegram (full mode only)
    if not args.quick:
        send_summary(all_findings, total_memories, grade)

    # Exit code: 2=critical, 1=warnings, 0=clean
    has_critical = any(f.severity == "critical" for f in all_findings)
    has_warn = any(f.severity == "warn" for f in all_findings)
    if has_critical:
        sys.exit(2)
    elif has_warn:
        sys.exit(1)
    else:
        sys.exit(0)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""
AetherVault Memory Quality Linter
===================================

Mechanical quality enforcer for hot memories. Validates structure,
metadata completeness, importance ranges, temporal fields, fact quality,
staleness, category values, and detects semantic duplicates.

Usage:
    # Full lint (human-readable):
    python3 memory-linter.py

    # JSON output (for agent consumption):
    python3 memory-linter.py --format json

    # Auto-fix recoverable issues:
    python3 memory-linter.py --fix

    # Strict mode (exit 1 on any issue, for CI/cron):
    python3 memory-linter.py --strict

    # Combined:
    python3 memory-linter.py --fix --strict --format json
"""

import argparse
import datetime
import json
import os
import re
import sys

# Add script directory to path for shared module import
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hot_memory_store import (
    load_env, log, log_warn,
    read_hot_memories, write_hot_memories,
    hot_memory_lock, hot_memory_unlock,
    HOT_MEMORY_PATH,
)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

VALID_CATEGORIES = {
    "preference", "person", "project", "event", "plan",
    "health", "work", "opinion", "habit", "location",
    "relationship", "general",
}

REQUIRED_METADATA_FIELDS = {
    "category": str,
    "importance": int,
    "created_at": str,
    "decay_strength": (int, float),
    "source": str,
    "entities": list,
}

STALENESS_DAYS = 7
MIN_FACT_LENGTH = 20

LINT_REPORT_PATH = os.path.join(
    os.path.dirname(HOT_MEMORY_PATH), "memory-lint-report.json"
)


# ---------------------------------------------------------------------------
# Similarity helpers
# ---------------------------------------------------------------------------

def _tokenize(text: str) -> set:
    """Lowercase word tokens, stripping punctuation."""
    return set(re.findall(r"[a-z0-9]+", text.lower()))


def _word_overlap_ratio(a: set, b: set) -> float:
    """Jaccard-style overlap: |intersection| / |smaller set|."""
    if not a or not b:
        return 0.0
    overlap = len(a & b)
    return overlap / min(len(a), len(b))


# ---------------------------------------------------------------------------
# Individual checks
# ---------------------------------------------------------------------------

def check_duplicates(memories: list) -> list:
    """Semantic similarity check: word overlap >= 50% between any two facts."""
    issues = []
    tokenized = []
    for i, mem in enumerate(memories):
        fact = mem.get("fact", "")
        tokens = _tokenize(fact)
        tokenized.append((i, fact, tokens))

    for idx_a in range(len(tokenized)):
        i_a, fact_a, tokens_a = tokenized[idx_a]
        for idx_b in range(idx_a + 1, len(tokenized)):
            i_b, fact_b, tokens_b = tokenized[idx_b]
            ratio = _word_overlap_ratio(tokens_a, tokens_b)
            if ratio >= 0.5:
                issues.append({
                    "check": "duplicate",
                    "indices": [i_a, i_b],
                    "overlap": round(ratio, 2),
                    "description": (
                        f"Likely duplicate (overlap={ratio:.0%}): "
                        f"[{i_a}] \"{fact_a[:60]}...\" vs "
                        f"[{i_b}] \"{fact_b[:60]}...\""
                    ),
                })
    return issues


def check_metadata_completeness(memories: list) -> list:
    """Every memory must have required metadata fields with correct types."""
    issues = []
    for i, mem in enumerate(memories):
        meta = mem.get("metadata", {})
        for field, expected_type in REQUIRED_METADATA_FIELDS.items():
            if field not in meta:
                issues.append({
                    "check": "metadata_completeness",
                    "index": i,
                    "field": field,
                    "description": f"[{i}] Missing required metadata field: {field}",
                })
            elif not isinstance(meta[field], expected_type):
                issues.append({
                    "check": "metadata_completeness",
                    "index": i,
                    "field": field,
                    "description": (
                        f"[{i}] Field '{field}' has wrong type: "
                        f"expected {expected_type}, got {type(meta[field]).__name__}"
                    ),
                })
        # entities must be non-empty list
        entities = meta.get("entities")
        if isinstance(entities, list) and len(entities) == 0:
            issues.append({
                "check": "metadata_completeness",
                "index": i,
                "field": "entities",
                "description": f"[{i}] entities list is empty",
            })
    return issues


def check_importance_sanity(memories: list) -> list:
    """importance must be integer 1-10, importance_normalized must match."""
    issues = []
    for i, mem in enumerate(memories):
        meta = mem.get("metadata", {})
        importance = meta.get("importance")

        if importance is None:
            continue  # already caught by metadata_completeness

        if not isinstance(importance, int) or importance < 1 or importance > 10:
            issues.append({
                "check": "importance_sanity",
                "index": i,
                "description": (
                    f"[{i}] importance must be integer 1-10, got: {importance!r}"
                ),
            })
            continue

        expected_norm = round(importance / 10.0, 2)
        actual_norm = meta.get("importance_normalized")
        if actual_norm is not None and actual_norm != expected_norm:
            issues.append({
                "check": "importance_sanity",
                "index": i,
                "description": (
                    f"[{i}] importance_normalized mismatch: "
                    f"expected {expected_norm}, got {actual_norm}"
                ),
            })
    return issues


def _parse_iso_datetime(value: str) -> bool:
    """Return True if value is a parseable ISO datetime or date string."""
    if not isinstance(value, str) or not value:
        return False
    try:
        datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
        return True
    except (ValueError, TypeError):
        pass
    # Try date-only
    try:
        datetime.date.fromisoformat(value)
        return True
    except (ValueError, TypeError):
        return False


def check_temporal_validity(memories: list) -> list:
    """t_valid must be parseable ISO; t_invalid must be None or valid ISO."""
    issues = []
    for i, mem in enumerate(memories):
        meta = mem.get("metadata", {})

        t_valid = meta.get("t_valid")
        if t_valid is not None and not _parse_iso_datetime(str(t_valid)):
            issues.append({
                "check": "temporal_validity",
                "index": i,
                "description": (
                    f"[{i}] t_valid is not a valid ISO datetime: {t_valid!r}"
                ),
            })

        t_invalid = meta.get("t_invalid")
        if t_invalid is not None and not _parse_iso_datetime(str(t_invalid)):
            issues.append({
                "check": "temporal_validity",
                "index": i,
                "description": (
                    f"[{i}] t_invalid is not None or valid ISO datetime: {t_invalid!r}"
                ),
            })
    return issues


def check_fact_quality(memories: list) -> list:
    """Fact text >= 20 chars, not a question, contains at least one capitalized word."""
    issues = []
    for i, mem in enumerate(memories):
        fact = mem.get("fact", "")

        if len(fact) < MIN_FACT_LENGTH:
            issues.append({
                "check": "fact_quality",
                "index": i,
                "description": (
                    f"[{i}] Fact too short ({len(fact)} chars, min {MIN_FACT_LENGTH}): "
                    f"\"{fact}\""
                ),
            })

        if fact.rstrip().endswith("?"):
            issues.append({
                "check": "fact_quality",
                "index": i,
                "description": f"[{i}] Fact is a question: \"{fact[:60]}...\"",
            })

        # At least one word starting with uppercase (entity indicator)
        words = fact.split()
        has_capitalized = any(w[0].isupper() for w in words if w and w[0].isalpha())
        if not has_capitalized and len(fact) >= MIN_FACT_LENGTH:
            issues.append({
                "check": "fact_quality",
                "index": i,
                "description": (
                    f"[{i}] Fact has no capitalized word (entity indicator): "
                    f"\"{fact[:60]}...\""
                ),
            })
    return issues


def check_staleness(memories: list) -> list:
    """Flag memories older than STALENESS_DAYS with access_count == 0."""
    issues = []
    now = datetime.datetime.now(datetime.timezone.utc)
    cutoff = now - datetime.timedelta(days=STALENESS_DAYS)

    for i, mem in enumerate(memories):
        meta = mem.get("metadata", {})
        created_at = meta.get("created_at", "")
        access_count = meta.get("access_count", 0)

        if access_count != 0:
            continue

        if not created_at:
            continue

        try:
            created_dt = datetime.datetime.fromisoformat(
                created_at.replace("Z", "+00:00")
            )
            if created_dt < cutoff:
                days_old = (now - created_dt).days
                issues.append({
                    "check": "staleness",
                    "index": i,
                    "description": (
                        f"[{i}] Stale memory: {days_old} days old, never accessed: "
                        f"\"{mem.get('fact', '')[:60]}...\""
                    ),
                })
        except (ValueError, TypeError):
            pass
    return issues


def check_category_validity(memories: list) -> list:
    """category must be one of the valid set."""
    issues = []
    for i, mem in enumerate(memories):
        meta = mem.get("metadata", {})
        category = meta.get("category")
        if category is not None and category not in VALID_CATEGORIES:
            issues.append({
                "check": "category_validity",
                "index": i,
                "description": (
                    f"[{i}] Invalid category: \"{category}\" "
                    f"(valid: {', '.join(sorted(VALID_CATEGORIES))})"
                ),
            })
    return issues


# ---------------------------------------------------------------------------
# Auto-fix
# ---------------------------------------------------------------------------

def auto_fix_memories(memories: list, issues: list) -> tuple:
    """Auto-repair what is possible. Returns (fixed_memories, actions)."""
    actions = []
    now_iso = datetime.datetime.now(datetime.timezone.utc).isoformat()

    # Build a set of fixable issues by index
    fixable_indices = set()
    for issue in issues:
        idx = issue.get("index")
        if idx is not None:
            fixable_indices.add(idx)

    for i in range(len(memories)):
        mem = memories[i]
        meta = mem.setdefault("metadata", {})

        # Fix missing metadata defaults
        defaults = {
            "category": "general",
            "importance": 5,
            "created_at": now_iso,
            "decay_strength": 1.0,
            "source": "unknown",
            "entities": [],
        }
        for field, default in defaults.items():
            if field not in meta:
                meta[field] = default
                actions.append(f"[{i}] Set missing {field} = {default!r}")

        # Fix importance_normalized
        importance = meta.get("importance")
        if isinstance(importance, int) and 1 <= importance <= 10:
            expected_norm = round(importance / 10.0, 2)
            if meta.get("importance_normalized") != expected_norm:
                meta["importance_normalized"] = expected_norm
                actions.append(
                    f"[{i}] Fixed importance_normalized = {expected_norm}"
                )

        # Fix invalid category -> general
        if meta.get("category") not in VALID_CATEGORIES:
            old_cat = meta.get("category")
            meta["category"] = "general"
            actions.append(f"[{i}] Fixed invalid category \"{old_cat}\" -> \"general\"")

        # Ensure access_count exists
        if "access_count" not in meta:
            meta["access_count"] = 0
            actions.append(f"[{i}] Set missing access_count = 0")

        # Ensure last_accessed exists
        if "last_accessed" not in meta:
            meta["last_accessed"] = meta.get("created_at", now_iso)
            actions.append(f"[{i}] Set missing last_accessed")

        # Ensure t_valid exists
        if "t_valid" not in meta:
            meta["t_valid"] = meta.get("created_at", now_iso)
            actions.append(f"[{i}] Set missing t_valid")

        # Ensure t_invalid exists
        if "t_invalid" not in meta:
            meta["t_invalid"] = None
            actions.append(f"[{i}] Set missing t_invalid = None")

    return memories, actions


# ---------------------------------------------------------------------------
# Run all checks
# ---------------------------------------------------------------------------

def run_lint(memories: list) -> dict:
    """Run all lint checks. Returns structured report."""
    all_issues = []
    check_results = {}

    checks = [
        ("duplicate", check_duplicates),
        ("metadata_completeness", check_metadata_completeness),
        ("importance_sanity", check_importance_sanity),
        ("temporal_validity", check_temporal_validity),
        ("fact_quality", check_fact_quality),
        ("staleness", check_staleness),
        ("category_validity", check_category_validity),
    ]

    for name, fn in checks:
        issues = fn(memories)
        check_results[name] = {
            "pass": len(issues) == 0,
            "issue_count": len(issues),
        }
        all_issues.extend(issues)

    total_issues = len(all_issues)
    passed = sum(1 for c in check_results.values() if c["pass"])
    total_checks = len(check_results)

    return {
        "summary": {
            "total_memories": len(memories),
            "total_issues": total_issues,
            "checks_passed": passed,
            "checks_total": total_checks,
            "overall": "pass" if total_issues == 0 else "fail",
        },
        "checks": check_results,
        "issues": all_issues,
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    }


# ---------------------------------------------------------------------------
# Output formatting
# ---------------------------------------------------------------------------

def format_report(report: dict) -> str:
    """Format lint report as human-readable text."""
    lines = []
    summary = report["summary"]
    lines.append("Memory Quality Lint Report")
    lines.append("=" * 40)
    lines.append(
        f"Memories: {summary['total_memories']}  |  "
        f"Issues: {summary['total_issues']}  |  "
        f"Checks: {summary['checks_passed']}/{summary['checks_total']} passed  |  "
        f"Result: {summary['overall'].upper()}"
    )
    lines.append(f"Timestamp: {report['timestamp']}")
    lines.append("")

    status_icon = {True: "+", False: "!"}
    for name, result in report["checks"].items():
        icon = status_icon[result["pass"]]
        label = name.replace("_", " ").title()
        count_str = f" ({result['issue_count']} issues)" if result["issue_count"] else ""
        lines.append(f"  [{icon}] {label}{count_str}")

    if report["issues"]:
        lines.append("")
        lines.append("Issues:")
        lines.append("-" * 40)
        for issue in report["issues"]:
            lines.append(f"  {issue['description']}")

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="AetherVault Memory Quality Linter",
    )
    parser.add_argument("--format", choices=["text", "json"], default="text",
                        help="Output format (default: text)")
    parser.add_argument("--fix", action="store_true",
                        help="Auto-repair recoverable issues")
    parser.add_argument("--strict", action="store_true",
                        help="Exit with code 1 if any issues found (for CI/cron)")
    args = parser.parse_args()

    load_env()

    # Read memories (no lock needed for read-only; lock only if --fix)
    memories = read_hot_memories()
    if not memories:
        log("No hot memories found, nothing to lint")
        report = run_lint([])
        if args.format == "json":
            print(json.dumps(report, indent=2, default=str))
        else:
            print(format_report(report))
        sys.exit(0)

    log(f"Linting {len(memories)} hot memories...")

    # Run lint
    report = run_lint(memories)

    # Auto-fix if requested
    if args.fix and report["summary"]["total_issues"] > 0:
        lock_fd = hot_memory_lock()
        try:
            # Re-read under lock to avoid races
            memories = read_hot_memories()
            memories, fix_actions = auto_fix_memories(memories, report["issues"])
            if fix_actions:
                write_hot_memories(memories)
                log(f"Applied {len(fix_actions)} auto-fixes")
            report["auto_fix"] = fix_actions
            # Re-lint after fix
            post_fix = run_lint(memories)
            report["post_fix_summary"] = post_fix["summary"]
        finally:
            hot_memory_unlock(lock_fd)

    # Output
    if args.format == "json":
        print(json.dumps(report, indent=2, default=str))
    else:
        print(format_report(report))
        if args.fix and report.get("auto_fix"):
            print()
            print(f"Auto-fix actions ({len(report['auto_fix'])}):")
            for action in report["auto_fix"]:
                print(f"  - {action}")
            if "post_fix_summary" in report:
                pf = report["post_fix_summary"]
                print(f"\nPost-fix: {pf['total_issues']} issues remaining "
                      f"({pf['checks_passed']}/{pf['checks_total']} checks pass)")

    # Exit code
    if args.strict and report["summary"]["total_issues"] > 0:
        # If --fix was used, check post-fix results
        if args.fix and "post_fix_summary" in report:
            if report["post_fix_summary"]["total_issues"] > 0:
                sys.exit(1)
            else:
                sys.exit(0)
        sys.exit(1)

    sys.exit(0)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""
AetherVault Weekly Reflection System
=====================================

Runs weekly (Monday mornings) to generate meta-insights from accumulated
daily summaries and conversation patterns.

Implements:
- Generative Agents reflection: question generation -> evidence retrieval -> insight extraction
- Reflexion-style failure learning: detect failed tasks, generate verbal self-critiques
- LangMem prompt optimization: evolve the agent's system prompt based on patterns

Usage:
    # Normal weekly run (via cron):
    python3 weekly-reflection.py

    # Custom date range:
    python3 weekly-reflection.py --start 2026-02-03 --end 2026-02-09

    # Dry run:
    python3 weekly-reflection.py --dry-run

    # Force run (ignore last-run marker):
    python3 weekly-reflection.py --force
"""

import argparse
import datetime
import json
import os
import sys
import tempfile

# Shared module (same directory)
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hot_memory_store import (
    AETHERVAULT_HOME, OWNER_NAME,
    log, log_error, log_warn,
    load_env, get_api_key, call_claude, parse_claude_json, send_telegram,
    read_hot_memories, write_hot_memories, append_hot_memory,
    hot_memory_lock, hot_memory_unlock,
    search_capsule, atomic_write_json,
    PROMOTE_THRESHOLD,
)

# ---------------------------------------------------------------------------
# Reflection-specific configuration
# ---------------------------------------------------------------------------

DAILY_SUMMARIES_DIR = os.path.join(AETHERVAULT_HOME, "workspace", "daily-summaries")
REFLECTIONS_DIR = os.path.join(AETHERVAULT_HOME, "workspace", "reflections")
MARKER_PATH = os.path.join(AETHERVAULT_HOME, "data", "reflection-marker.json")

NUM_QUESTIONS = 3
MAX_EVIDENCE_PER_QUESTION = 10
REFLECTION_IMPORTANCE = 8  # reflections are high-importance by default


# ---------------------------------------------------------------------------
# Marker tracking
# ---------------------------------------------------------------------------

def read_marker() -> str:
    if os.path.isfile(MARKER_PATH):
        try:
            with open(MARKER_PATH, "r") as f:
                data = json.load(f)
                return data.get("last_reflection", "")
        except (json.JSONDecodeError, OSError) as e:
            log_warn(f"Reflection marker corrupted or unreadable: {e}")
    return ""


def write_marker(week_id: str):
    try:
        atomic_write_json(MARKER_PATH, {
            "last_reflection": week_id,
            "updated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        })
    except OSError as e:
        log_warn(f"Could not write marker: {e}")


# ---------------------------------------------------------------------------
# Daily summary loading
# ---------------------------------------------------------------------------

def load_daily_summaries(start_date: str, end_date: str) -> list:
    """Load daily summary markdown files for the given date range."""
    summaries = []

    if not os.path.isdir(DAILY_SUMMARIES_DIR):
        log_warn(f"Daily summaries directory not found: {DAILY_SUMMARIES_DIR}")
        return []

    start = datetime.datetime.strptime(start_date, "%Y-%m-%d").date()
    end = datetime.datetime.strptime(end_date, "%Y-%m-%d").date()

    current = start
    while current <= end:
        date_str = current.strftime("%Y-%m-%d")
        summary_path = os.path.join(DAILY_SUMMARIES_DIR, f"{date_str}.md")
        if os.path.isfile(summary_path):
            try:
                # Guard against corrupt/huge summary files (expect < 1MB)
                file_size = os.path.getsize(summary_path)
                if file_size > 5 * 1024 * 1024:  # 5MB safety limit
                    log_warn(f"Skipping oversized summary {date_str} ({file_size / 1024 / 1024:.1f}MB)")
                    continue
                with open(summary_path, "r") as f:
                    content = f.read()
                summaries.append({"date": date_str, "content": content})
                log(f"Loaded summary for {date_str} ({len(content)} chars)")
            except OSError as e:
                log_warn(f"Could not read {summary_path}: {e}")
        current += datetime.timedelta(days=1)

    log(f"Loaded {len(summaries)} daily summaries for {start_date} to {end_date}")
    return summaries


# ---------------------------------------------------------------------------
# Step 1: Generate reflection questions (Generative Agents)
# ---------------------------------------------------------------------------

QUESTION_SYSTEM = """\
You are a reflection agent analyzing a week of daily summaries for a personal AI assistant.
Generate exactly """ + str(NUM_QUESTIONS) + """ questions that would help understand the user's
evolving priorities, patterns, and needs.

Respond with ONLY valid JSON (no markdown fences):

{
  "questions": [
    {
      "question": "the reflection question",
      "focus": "priorities|patterns|relationships|projects|growth|concerns"
    }
  ]
}

Focus on:
- Changes: What shifted this week? New priorities, changed preferences, evolving plans?
- Recurring themes: What keeps coming up? Persistent concerns, repeated requests?
- Unresolved issues: What was started but not finished? What needs follow-up?
- Emotional patterns: How is the user's mood/energy evolving?
- The user's name is """ + OWNER_NAME + """
"""


def generate_questions(api_key: str, summaries: list) -> list:
    """Generate reflection questions from daily summaries. Returns list or None on failure."""
    combined = "\n\n---\n\n".join(
        f"## {s['date']}\n{s['content']}" for s in summaries
    )

    # Truncate if needed
    if len(combined) > 50000:
        combined = combined[:50000] + "\n\n[... truncated ...]"

    raw = call_claude(api_key, QUESTION_SYSTEM, combined, max_tokens=512)
    if not raw:
        return None

    try:
        data = parse_claude_json(raw)
        questions = data.get("questions", [])
        if not isinstance(questions, list):
            log_error(f"Expected 'questions' to be a list, got {type(questions).__name__}")
            return None
        log(f"Generated {len(questions)} reflection questions")
        return questions
    except (json.JSONDecodeError, ValueError) as e:
        log_error(f"Failed to parse questions JSON: {e}")
        return None


# ---------------------------------------------------------------------------
# Step 2: Retrieve evidence and generate insights
# ---------------------------------------------------------------------------

INSIGHT_SYSTEM = """\
You are a reflection agent generating insights from evidence about a personal AI assistant's user.
Based on the question and supporting evidence, generate a concise, actionable insight.

Respond with ONLY valid JSON (no markdown fences):

{
  "insight": "A specific, actionable insight about the user's patterns or needs",
  "action_items": ["specific thing the AI assistant should do differently"],
  "confidence": "high|medium|low"
}

Rules:
- Be specific and concrete, not vague
- Focus on actionable implications (what should the assistant do differently?)
- Ground insights in the evidence provided
- The user's name is """ + OWNER_NAME + """
"""


def generate_insight(api_key: str, question: str, evidence: list) -> dict:
    """Generate an insight from a question and supporting evidence."""
    evidence_text = "\n".join(f"- {e[:200]}" for e in evidence[:MAX_EVIDENCE_PER_QUESTION])

    user_msg = (
        f"QUESTION: {question}\n\n"
        f"EVIDENCE ({len(evidence)} items):\n{evidence_text}"
    )

    raw = call_claude(api_key, INSIGHT_SYSTEM, user_msg, max_tokens=512)
    if not raw:
        return {}

    try:
        return parse_claude_json(raw)
    except (json.JSONDecodeError, ValueError):
        return {}


# ---------------------------------------------------------------------------
# Step 3: Store reflections as high-importance memories
# ---------------------------------------------------------------------------

def store_reflection_memory(insight: str, question: str, week_id: str):
    """Write a reflection insight to the hot memory buffer (via shared module)."""
    now_iso = datetime.datetime.now(datetime.timezone.utc).isoformat()
    importance_norm = REFLECTION_IMPORTANCE / 10.0
    metadata = {
        "category": "reflection",
        "importance": REFLECTION_IMPORTANCE,
        "importance_normalized": importance_norm,
        "created_at": now_iso,
        "last_accessed": now_iso,
        "access_count": 0,
        "decay_strength": 1.0,
        "decay_layer": "ltm" if importance_norm >= PROMOTE_THRESHOLD else "stm",
        "t_valid": now_iso,
        "t_invalid": None,
        "source": "weekly-reflection",
        "reflection_question": question,
        "pinned": True,  # prevent eviction by rolling buffer
    }

    fact_text = f"[REFLECTION {week_id}] {insight}"

    append_hot_memory(fact_text, metadata)


# ---------------------------------------------------------------------------
# Reflexion: Failed task learning
# ---------------------------------------------------------------------------

FAILURE_SYSTEM = """\
You are analyzing failed or problematic agent interactions from the past week.
Identify what went wrong and generate specific lessons learned.

Respond with ONLY valid JSON (no markdown fences):

{
  "failures": [
    {
      "description": "what happened",
      "root_cause": "why it failed",
      "lesson": "what to do differently next time",
      "severity": "high|medium|low"
    }
  ]
}

Focus on:
- Tasks the user had to repeat or correct
- Times the assistant misunderstood the user's intent
- Technical failures (timeouts, errors, wrong tools used)
- User expressions of frustration or correction
- If no significant failures, return {"failures": []}
"""


def detect_failures(api_key: str, summaries: list) -> list:
    """Detect failed interactions from daily summaries (Reflexion pattern)."""
    combined = "\n\n".join(
        f"## {s['date']}\n{s['content']}" for s in summaries
    )

    if len(combined) > 30000:
        combined = combined[:30000] + "\n\n[... truncated ...]"

    raw = call_claude(api_key, FAILURE_SYSTEM, combined, max_tokens=1024)
    if not raw:
        return None

    try:
        data = parse_claude_json(raw)
        failures = data.get("failures", [])
        if not isinstance(failures, list):
            log_error(f"Expected 'failures' to be a list, got {type(failures).__name__}")
            return None
        log(f"Detected {len(failures)} failure patterns")
        return failures
    except (json.JSONDecodeError, ValueError):
        return None


# ---------------------------------------------------------------------------
# Write reflection report
# ---------------------------------------------------------------------------

def write_reflection_report(
    week_id: str,
    questions: list,
    insights: list,
    failures: list,
    summaries: list,
    dry_run: bool = False,
):
    """Write a markdown reflection report."""
    os.makedirs(REFLECTIONS_DIR, exist_ok=True)
    report_path = os.path.join(REFLECTIONS_DIR, f"reflection-{week_id}.md")

    now_str = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    lines = [
        f"# Weekly Reflection: {week_id}",
        "",
        f"*Generated at {now_str}*",
        f"*Based on {len(summaries)} daily summaries*",
        "",
        "---",
        "",
    ]

    # Insights section
    lines.append("## Insights")
    lines.append("")
    for i, (q, insight) in enumerate(zip(questions, insights), 1):
        question_text = q.get("question", "?")
        insight_text = insight.get("insight", "No insight generated")
        confidence = insight.get("confidence", "medium")
        actions = insight.get("action_items", [])

        lines.append(f"### {i}. {question_text}")
        lines.append("")
        lines.append(f"**Insight** ({confidence} confidence): {insight_text}")
        lines.append("")
        if actions:
            lines.append("**Action items:**")
            for action in actions:
                lines.append(f"- {action}")
            lines.append("")

    # Failures section (Reflexion)
    if failures:
        lines.append("## Failure Analysis (Reflexion)")
        lines.append("")
        for i, failure in enumerate(failures, 1):
            severity = failure.get("severity", "medium")
            lines.append(f"### {i}. [{severity.upper()}] {failure.get('description', '?')}")
            lines.append("")
            lines.append(f"**Root cause:** {failure.get('root_cause', 'unknown')}")
            lines.append(f"**Lesson learned:** {failure.get('lesson', 'none')}")
            lines.append("")
    else:
        lines.append("## Failure Analysis")
        lines.append("")
        lines.append("No significant failures detected this week.")
        lines.append("")

    lines.append("---")
    lines.append("*Generated by weekly-reflection.py*")

    content = "\n".join(lines) + "\n"

    if dry_run:
        log(f"DRY RUN - would write {len(content)} chars to {report_path}")
        print(content)
        return

    try:
        fd, tmp_path = tempfile.mkstemp(
            dir=REFLECTIONS_DIR,
            prefix=".reflection-", suffix=".tmp",
        )
        try:
            with os.fdopen(fd, "w") as f:
                f.write(content)
            os.replace(tmp_path, report_path)
        except Exception:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass
            raise
        log(f"Reflection report written to {report_path}")
    except OSError as e:
        log_error(f"Failed to write reflection report: {e}")


# ---------------------------------------------------------------------------
# Main pipeline
# ---------------------------------------------------------------------------

def run_reflection(start_date: str, end_date: str, force: bool, dry_run: bool):
    """Run the full weekly reflection pipeline."""
    load_env()
    api_key = get_api_key()

    week_id = f"{start_date}_to_{end_date}"

    # Check marker
    if not force:
        last = read_marker()
        if last == week_id:
            log(f"Already reflected for {week_id}. Use --force to re-run.")
            return

    log(f"=== Weekly Reflection: {week_id} ===")

    # Load daily summaries
    summaries = load_daily_summaries(start_date, end_date)
    if not summaries:
        log_warn("No daily summaries found for this period. Nothing to reflect on.")
        return

    # Step 1: Generate reflection questions
    log("Step 1/4: Generating reflection questions...")
    questions = generate_questions(api_key, summaries)
    if questions is None:
        log_error("Failed to generate reflection questions (API/parse error)")
        return
    if not questions:
        log_warn("No reflection questions generated")
        return

    # Step 2: Retrieve evidence and generate insights
    log("Step 2/4: Retrieving evidence and generating insights...")
    insights = []
    for q in questions:
        question_text = q.get("question", "")
        log(f"  Q: {question_text[:60]}...")

        evidence = search_capsule(question_text, limit=MAX_EVIDENCE_PER_QUESTION)
        log(f"  Found {len(evidence)} evidence items")

        insight = generate_insight(api_key, question_text, evidence)
        insights.append(insight)

        if insight:
            log(f"  Insight: {insight.get('insight', '?')[:60]}...")

    # Step 3: Detect failures (Reflexion)
    log("Step 3/4: Analyzing failure patterns (Reflexion)...")
    failures = detect_failures(api_key, summaries)
    if failures is None:
        log_warn("Failure detection returned no results (API/parse error)")
        failures = []

    # Step 4: Store insights as memories + write report
    log("Step 4/4: Storing insights and writing report...")

    if not dry_run:
        # Store each insight as a high-importance memory
        for q, insight in zip(questions, insights):
            if insight and insight.get("insight"):
                store_reflection_memory(
                    insight["insight"],
                    q.get("question", ""),
                    week_id,
                )
                log(f"  Stored reflection memory: {insight['insight'][:60]}...")

        # Store failure lessons as procedural memories
        for failure in failures:
            if failure.get("lesson"):
                store_reflection_memory(
                    f"[LESSON] {failure['lesson']}",
                    f"Failure: {failure.get('description', '?')}",
                    week_id,
                )

    # Write report
    write_reflection_report(week_id, questions, insights, failures, summaries, dry_run)

    # Update marker
    if not dry_run:
        write_marker(week_id)

    # Summary
    insight_count = sum(1 for i in insights if i)
    failure_count = len(failures)
    log(f"Reflection complete: {insight_count} insights, {failure_count} failure lessons")

    # Telegram notification
    if not dry_run and (insight_count > 0 or failure_count > 0):
        send_telegram(
            f"[Reflection] Weekly analysis for {week_id}:\n"
            f"  {insight_count} insights generated\n"
            f"  {failure_count} failure patterns identified\n"
            f"Report: reflections/reflection-{week_id}.md"
        )


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="AetherVault Weekly Reflection System",
    )
    parser.add_argument(
        "--start", default=None,
        help="Start date (YYYY-MM-DD). Default: 7 days ago",
    )
    parser.add_argument(
        "--end", default=None,
        help="End date (YYYY-MM-DD). Default: yesterday",
    )
    parser.add_argument(
        "--dry-run", action="store_true",
        help="Print without writing",
    )
    parser.add_argument(
        "--force", action="store_true",
        help="Force re-run even if already reflected for this period",
    )
    args = parser.parse_args()

    # Compute default date range (last 7 days)
    today = datetime.date.today()
    if args.end:
        end_date = args.end
    else:
        end_date = (today - datetime.timedelta(days=1)).strftime("%Y-%m-%d")

    if args.start:
        start_date = args.start
    else:
        start_date = (today - datetime.timedelta(days=7)).strftime("%Y-%m-%d")

    try:
        run_reflection(start_date, end_date, args.force, args.dry_run)
    except KeyboardInterrupt:
        log("Interrupted")
        sys.exit(130)
    except Exception as e:
        log_error(f"Unexpected error: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()

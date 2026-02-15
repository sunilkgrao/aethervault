#!/usr/bin/env python3
"""
AetherVault Enhanced Memory Retrieval Scorer
=============================================

Wraps aethervault search results with a composite scoring formula that
combines relevance, importance, recency, and decay (FadeMem).

Implements:
- Generative Agents retrieval formula: score = alpha * relevance + beta * importance + gamma * recency
- FadeMem exponential decay with importance-modulated rate
- Access reinforcement with diminishing returns (spacing effect)
- Re-ranking of search results by composite score

Usage:
    # Score and re-rank a memory search:
    python3 memory-scorer.py search "what is the user's favorite color"

    # Score with custom weights:
    python3 memory-scorer.py search "project status" --alpha-relevance 1.0 --alpha-importance 0.5

    # Reinforce a memory on access (boost its strength):
    python3 memory-scorer.py reinforce --fact "user's favorite color is purple"

    # Compute decay status for all hot memories:
    python3 memory-scorer.py decay-report

    # Prune memories below decay threshold:
    python3 memory-scorer.py prune --threshold 0.05 --dry-run
"""

import argparse
import datetime
import json
import math
import os
import sys

# Shared module (same directory)
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hot_memory_store import (
    LAMBDA_BASE, MU, BETA_LTM, BETA_STM, PROMOTE_THRESHOLD,
    RECENCY_DECAY_RATE,
    log, log_error, log_warn,
    load_env,
    read_hot_memories, write_hot_memories,
    hot_memory_lock, hot_memory_unlock,
    search_capsule,
    compute_decay_strength, compute_recency,
)

# ---------------------------------------------------------------------------
# Scorer-specific configuration
# ---------------------------------------------------------------------------

# Composite scoring weights (tunable)
DEFAULT_ALPHA_RELEVANCE = 1.0
DEFAULT_ALPHA_IMPORTANCE = 0.8
DEFAULT_ALPHA_RECENCY = 0.5
DEFAULT_ALPHA_DECAY = 0.3

# FadeMem reinforcement parameters
REINFORCE_DELTA = 0.15
REINFORCE_N = 5
PRUNE_THRESHOLD = 0.05  # memories below this strength can be pruned


# ---------------------------------------------------------------------------
# Scoring functions
# ---------------------------------------------------------------------------

def compute_composite_score(
    relevance: float,
    importance: float,
    recency: float,
    decay: float,
    alpha_relevance: float = DEFAULT_ALPHA_RELEVANCE,
    alpha_importance: float = DEFAULT_ALPHA_IMPORTANCE,
    alpha_recency: float = DEFAULT_ALPHA_RECENCY,
    alpha_decay: float = DEFAULT_ALPHA_DECAY,
) -> float:
    """Composite retrieval score combining all signals. Returns normalized [0, 1]."""
    raw = (
        alpha_relevance * relevance
        + alpha_importance * importance
        + alpha_recency * recency
        + alpha_decay * decay
    )
    max_possible = alpha_relevance + alpha_importance + alpha_recency + alpha_decay
    return raw / max_possible if max_possible > 0 else 0.0


def reinforce_on_access(memory: dict) -> dict:
    """
    Boost memory strength when accessed (retrieved and used).

    FadeMem reinforcement formula:
    v(t+) = v(t) + delta_v * (1 - v(t)) * exp(-n / N)

    Implements spacing effect: diminishing returns for repeated access.
    """
    metadata = memory.get("metadata", {})
    v = metadata.get("decay_strength", 1.0)
    n = metadata.get("access_count", 0)

    boost = REINFORCE_DELTA * (1.0 - v) * math.exp(-n / REINFORCE_N)
    metadata["decay_strength"] = min(1.0, v + boost)
    metadata["access_count"] = n + 1
    metadata["last_accessed"] = datetime.datetime.now(datetime.timezone.utc).isoformat()

    memory["metadata"] = metadata
    return memory


def score_hot_memories(query: str, weights: dict) -> list:
    """
    Score and rank hot memories against a query.
    Returns list of (memory, score, breakdown) tuples sorted by score descending.
    """
    memories = read_hot_memories()
    if not memories:
        return []

    now = datetime.datetime.now(datetime.timezone.utc)
    scored = []

    for mem in memories:
        metadata = mem.get("metadata", {})
        fact_text = mem.get("fact", "")

        # Skip invalidated memories (bi-temporal DELETE)
        if metadata.get("t_invalid"):
            continue

        # Relevance: simple keyword overlap (hot memories are small enough for this)
        query_words = set(query.lower().split())
        fact_words = set(fact_text.lower().split())
        overlap = len(query_words & fact_words)
        relevance = min(1.0, overlap / max(1, len(query_words)))

        # Importance
        importance = metadata.get("importance_normalized", 0.5)

        # Recency
        last_accessed = metadata.get("last_accessed", metadata.get("created_at", ""))
        hours_since = 0
        if last_accessed:
            try:
                la_dt = datetime.datetime.fromisoformat(last_accessed.replace("Z", "+00:00"))
                hours_since = (now - la_dt).total_seconds() / 3600.0
            except (ValueError, TypeError):
                pass
        recency = compute_recency(hours_since)

        # Decay — use stored value if recent, otherwise recompute
        stored_decay = metadata.get("decay_strength")
        created_at = metadata.get("created_at", "")
        days_elapsed = 0
        if created_at:
            try:
                cr_dt = datetime.datetime.fromisoformat(created_at.replace("Z", "+00:00"))
                days_elapsed = (now - cr_dt).total_seconds() / 86400.0
            except (ValueError, TypeError):
                pass
        computed_decay = compute_decay_strength(importance, days_elapsed)
        # Use stored decay_strength (includes reinforcement boosts) if present,
        # but take the min with computed to ensure time-based decay still applies
        if stored_decay is not None:
            decay = min(stored_decay, computed_decay)
        else:
            decay = computed_decay

        # Composite score
        score = compute_composite_score(
            relevance, importance, recency, decay,
            weights.get("relevance", DEFAULT_ALPHA_RELEVANCE),
            weights.get("importance", DEFAULT_ALPHA_IMPORTANCE),
            weights.get("recency", DEFAULT_ALPHA_RECENCY),
            weights.get("decay", DEFAULT_ALPHA_DECAY),
        )

        scored.append((mem, score, {
            "relevance": round(relevance, 3),
            "importance": round(importance, 3),
            "recency": round(recency, 3),
            "decay": round(decay, 3),
            "composite": round(score, 3),
        }))

    # Sort by composite score descending
    scored.sort(key=lambda x: x[1], reverse=True)
    return scored


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------

def cmd_search(args):
    """Search and score memories."""
    load_env()

    query = args.query
    weights = {
        "relevance": args.alpha_relevance,
        "importance": args.alpha_importance,
        "recency": args.alpha_recency,
        "decay": args.alpha_decay,
    }

    # Score hot memories
    hot_results = score_hot_memories(query, weights)

    # Also search capsule for broader results
    capsule_results = search_capsule(query, limit=args.limit)

    # Output
    output = {
        "query": query,
        "weights": weights,
        "hot_memories": [],
        "capsule_matches": capsule_results[:args.limit],
    }

    for mem, score, breakdown in hot_results[:args.limit]:
        output["hot_memories"].append({
            "fact": mem.get("fact", ""),
            "score": breakdown,
            "metadata": mem.get("metadata", {}),
        })

    if args.format == "json":
        print(json.dumps(output, indent=2, default=str))
    else:
        print(f"\n{'='*60}")
        print(f"Query: {query}")
        print(f"{'='*60}")

        if output["hot_memories"]:
            print(f"\nHot Memories ({len(output['hot_memories'])} results):")
            print("-" * 40)
            for i, hm in enumerate(output["hot_memories"], 1):
                s = hm["score"]
                print(f"  {i}. [{s['composite']:.3f}] {hm['fact'][:80]}")
                print(f"     rel={s['relevance']:.2f} imp={s['importance']:.2f} "
                      f"rec={s['recency']:.2f} dec={s['decay']:.2f}")
        else:
            print("\nNo hot memories found.")

        if capsule_results:
            print(f"\nCapsule Matches ({len(capsule_results)} results):")
            print("-" * 40)
            for i, cr in enumerate(capsule_results[:5], 1):
                print(f"  {i}. {cr[:100]}")


def cmd_reinforce(args):
    """Reinforce a memory by its fact text (with file locking)."""
    load_env()

    lock_fd = hot_memory_lock()
    try:
        memories = read_hot_memories()
        found = False

        for mem in memories:
            if args.fact.lower() in mem.get("fact", "").lower():
                before = mem.get("metadata", {}).get("decay_strength", 1.0)
                mem = reinforce_on_access(mem)
                after = mem["metadata"]["decay_strength"]
                log(f"Reinforced: {mem['fact'][:60]}... ({before:.3f} -> {after:.3f})")
                found = True

        if found:
            write_hot_memories(memories)
        else:
            log_error(f"No memory matching '{args.fact}' found")
    finally:
        hot_memory_unlock(lock_fd)


def cmd_decay_report(args):
    """Show decay status for all hot memories."""
    load_env()

    memories = read_hot_memories()
    if not memories:
        print("No hot memories found.")
        return

    now = datetime.datetime.now(datetime.timezone.utc)

    report = []
    for mem in memories:
        metadata = mem.get("metadata", {})
        importance = metadata.get("importance_normalized", 0.5)

        created_at = metadata.get("created_at", "")
        days_elapsed = 0
        if created_at:
            try:
                cr_dt = datetime.datetime.fromisoformat(created_at.replace("Z", "+00:00"))
                days_elapsed = (now - cr_dt).total_seconds() / 86400.0
            except (ValueError, TypeError):
                pass

        current_strength = compute_decay_strength(importance, days_elapsed)
        layer = "LTM" if importance >= PROMOTE_THRESHOLD else "STM"

        # Compute half-life
        lambda_i = LAMBDA_BASE * math.exp(-MU * importance)
        beta = BETA_LTM if importance >= PROMOTE_THRESHOLD else BETA_STM
        if lambda_i > 0:
            half_life_days = (math.log(2) / lambda_i) ** (1.0 / beta)
        else:
            half_life_days = float('inf')

        report.append({
            "fact": mem.get("fact", "")[:60],
            "importance": metadata.get("importance", 5),
            "layer": layer,
            "age_days": round(days_elapsed, 1),
            "strength": round(current_strength, 3),
            "half_life_days": round(half_life_days, 1),
            "access_count": metadata.get("access_count", 0),
        })

    if args.format == "json":
        print(json.dumps(report, indent=2))
    else:
        print(f"\n{'Fact':<62} {'Imp':>3} {'Layer':>5} {'Age':>6} {'Str':>6} {'T½':>6} {'Acc':>4}")
        print("-" * 100)
        for r in report:
            print(f"{r['fact']:<62} {r['importance']:>3} {r['layer']:>5} "
                  f"{r['age_days']:>5.1f}d {r['strength']:>5.3f} "
                  f"{r['half_life_days']:>5.1f}d {r['access_count']:>4}")


def cmd_prune(args):
    """Prune memories below decay threshold (with file locking)."""
    load_env()

    lock_fd = hot_memory_lock()
    try:
        memories = read_hot_memories()
        if not memories:
            print("No hot memories to prune.")
            return

        now = datetime.datetime.now(datetime.timezone.utc)
        threshold = args.threshold
        keep = []
        pruned = []

        for mem in memories:
            metadata = mem.get("metadata", {})
            importance = metadata.get("importance_normalized", 0.5)

            created_at = metadata.get("created_at", "")
            days_elapsed = 0
            if created_at:
                try:
                    cr_dt = datetime.datetime.fromisoformat(created_at.replace("Z", "+00:00"))
                    days_elapsed = (now - cr_dt).total_seconds() / 86400.0
                except (ValueError, TypeError):
                    pass

            strength = compute_decay_strength(importance, days_elapsed)

            if strength >= threshold:
                keep.append(mem)
            else:
                pruned.append((mem, strength))

        if pruned:
            for mem, strength in pruned:
                log(f"{'PRUNE' if not args.dry_run else 'WOULD PRUNE'}: "
                    f"{mem.get('fact', '')[:60]}... (strength={strength:.4f})")

            if not args.dry_run:
                write_hot_memories(keep)
                log(f"Pruned {len(pruned)} memories, kept {len(keep)}")
            else:
                log(f"DRY RUN: would prune {len(pruned)}, keep {len(keep)}")
        else:
            log(f"No memories below threshold {threshold}")
    finally:
        hot_memory_unlock(lock_fd)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="AetherVault Enhanced Memory Retrieval Scorer",
    )
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # search
    p_search = subparsers.add_parser("search", help="Search and score memories")
    p_search.add_argument("query", help="Search query")
    p_search.add_argument("--limit", type=int, default=10, help="Max results")
    p_search.add_argument("--format", choices=["json", "table"], default="table")
    p_search.add_argument("--alpha-relevance", type=float, default=DEFAULT_ALPHA_RELEVANCE)
    p_search.add_argument("--alpha-importance", type=float, default=DEFAULT_ALPHA_IMPORTANCE)
    p_search.add_argument("--alpha-recency", type=float, default=DEFAULT_ALPHA_RECENCY)
    p_search.add_argument("--alpha-decay", type=float, default=DEFAULT_ALPHA_DECAY)
    p_search.set_defaults(func=cmd_search)

    # reinforce
    p_reinforce = subparsers.add_parser("reinforce", help="Reinforce a memory on access")
    p_reinforce.add_argument("--fact", required=True, help="Fact text to reinforce")
    p_reinforce.set_defaults(func=cmd_reinforce)

    # decay-report
    p_decay = subparsers.add_parser("decay-report", help="Show decay status of hot memories")
    p_decay.add_argument("--format", choices=["json", "table"], default="table")
    p_decay.set_defaults(func=cmd_decay_report)

    # prune
    p_prune = subparsers.add_parser("prune", help="Prune decayed memories")
    p_prune.add_argument("--threshold", type=float, default=PRUNE_THRESHOLD,
                         help=f"Prune memories below this strength (default: {PRUNE_THRESHOLD})")
    p_prune.add_argument("--dry-run", action="store_true")
    p_prune.set_defaults(func=cmd_prune)

    args = parser.parse_args()
    if not args.command:
        parser.print_help()
        sys.exit(1)

    args.func(args)


if __name__ == "__main__":
    main()

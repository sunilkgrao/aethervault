# Linus/OpenClaw: SOTA Synthesis & Concrete Roadmap

**Date**: 2026-02-23
**Based on**: 4 parallel research sweeps across 120+ sources (2025-2026)

---

## Where Linus/OpenClaw Sits Today

After Skill System v2, Linus/OpenClaw has:
- Lean skill injection (~200 tokens vs ~1500) with progressive disclosure
- Keyword search with stemming + synonym expansion (works to ~50 skills)
- Browser output wrapped with `[EXTERNAL CONTENT]` tags
- `record_skill_use` exists but is **dead code** (never wired in)
- No quality gate on skill storage, no deduplication, no pruning
- All three legs of the "lethal trifecta" active in a single session (browser + memory + http_request)

**Honest assessment**: The plumbing is good. The matching infrastructure is solid for current scale. But the feedback loop is broken (skills don't learn), the search won't scale past ~50 skills, the prompt injection defense is a speed bump not a wall, and the "resourceful agent" is aspirational prompting without runtime support.

---

## The Four Weak Areas & What SOTA Says To Do

### 1. SKILL LEARNING: Close the Feedback Loop

**The problem**: Skills are hand-seeded. The agent never stores what it learns from experience.

**What SOTA says works** (SkillRL Feb 2026, EvolveR ICLR 2026, SAGE Dec 2025):
- **Binary success signal as storage gate**: Only store skills from tasks that succeeded. This single filter prevents the error propagation that kills naive memory systems (42.88% vs 16.89% accuracy).
- **Differential trajectory processing**: Successful episodes get distilled into reusable procedures. Failed episodes get distilled into "cautionary principles" (what went wrong, what should have been done). Both are valuable.
- **Laplace-smoothed success rate for retrieval ranking**: `score = (successes + 1) / (uses + 2)`. This is exactly what `success_rate` in SkillRecord should be used for.
- **Deduplication at 0.85 embedding similarity**: Prevent near-duplicate skills from accumulating.
- **Pruning below 0.3 success rate**: Remove skills that consistently fail.

**What to build**:

```
Phase 1 — Wire the loop (days, not weeks):
  1. Wire record_skill_use into the agent's end-of-task flow
  2. Add quality gate to skill_store: require task_succeeded=true
  3. Add delete_skill + pruning cron (drop skills with success_rate < 0.3 after 5+ uses)

Phase 2 — Automatic skill distillation:
  1. At end of successful multi-step task, prompt the LLM:
     "Distill what you just did into a reusable skill (name, description, 3 key steps, gotchas)"
  2. Before storing, check embedding similarity against existing skills (>0.85 = merge, not duplicate)
  3. Store with description embedding for future vector retrieval

Phase 3 — Failure lessons:
  1. On task failure, prompt: "What went wrong? What should you have done instead?"
  2. Store as cautionary principle (separate field or tagged skill type)
  3. Retrieve alongside positive skills — SkillRL shows this is where the biggest gains come from
```

**Expected impact**: SkillRL achieved 89.9% on ALFWorld vs 77.6% baseline. SAGE achieved +8.9% with 26% fewer steps and 59% fewer tokens. The skill library compounds — each success makes the next task easier.

---

### 2. VECTOR SEARCH: Replace Keywords Before They Break

**The problem**: LIKE-based search with hand-coded synonyms. Works for 6 bootstrap skills. Breaks at ~50.

**What SOTA says works** (ITR Feb 2026, ToolScope Nov 2025, sqlite-vec):
- **Keyword search degrades non-linearly past ~50 skills** and becomes actively harmful at 100+ (context stuffing, lost-in-the-middle effect)
- **Hybrid search (BM25 + vector) with RRF fusion** consistently outperforms either alone
- **sqlite-vec** drops into existing rusqlite with zero new infrastructure. Brute-force KNN handles 100K skills in <150ms.
- **fastembed-rs** runs all-MiniLM-L6-v2 (384 dims) locally in ~5ms per embedding. No API dependency.
- **Tool RAG** (ITR paper): 95% reduction in per-step context tokens, 32% improvement in tool routing accuracy

**What to build**:

```
Phase 1 — FTS5 (zero new dependencies):
  - Enable FTS5 in rusqlite (already bundled)
  - Replace LIKE queries with proper tokenized BM25 search
  - Drop the hand-coded stemming and synonym functions
  - Immediate improvement in recall + relevance ranking

Phase 2 — Add vector search (two new crates):
  Cargo.toml:
    sqlite-vec = "0.1.7-alpha.2"
    fastembed = "5"

  - Add `embedding BLOB` column to skills table
  - On skill_store: embed(description + trigger + contexts) -> store vector
  - On match_skills_for_prompt: embed(prompt) -> KNN search
  - Fuse FTS5 + vector results with RRF (k=60)
  - Remove expand_synonyms() and stem() — embeddings subsume them

Phase 3 — Embed example queries (the ToolScope insight):
  - Store not just the skill description embedding, but also 3-5 example queries
  - Match against the example query embeddings — 60% to 92% accuracy improvement
  - This is the difference between "find deploy skills" and "I want my site live"
```

**Expected impact**: ITR paper shows 95% token reduction + 32% accuracy improvement. ToolScope showed 60% -> 92% on a 47-tool financial chatbot. The synonym table has 25 word groups and requires constant manual updates — embeddings eliminate this entirely.

---

### 3. PROMPT INJECTION: Move From Speed Bump to Wall

**The problem**: `[EXTERNAL CONTENT]` tags are a prompt-level hint. The "Attacker Moves Second" paper (Oct 2025) proved **all 12 tested prompt-level defenses were bypassed at >90% rate** by adaptive attacks.

**What SOTA says works** (CaMeL, Meta Rule of Two, AgentArmor):
- **The problem is architectural, not solvable at prompt level.** Defenses that work are ones that limit blast radius regardless of whether injection is detected.
- **Rule of Two / Lethal Trifecta**: Never allow simultaneous (a) untrusted input ingestion + (b) private data access + (c) external communication without human approval
- **Taint tracking**: Tag every value with its provenance. Block tainted data from reaching exfil-capable tools. AgentArmor reduces attack success to 3% with 1% utility drop.
- **Randomized delimiters** (Spotlighting): Unpredictable markers that injected content can't mimic
- **Content sanitization** (ammonia crate): Strip HTML structure before LLM sees it. Kills structural injection vectors entirely.
- **Deterministic output filtering**: Block markdown images, reference-style links, base64 URLs in LLM output when session is tainted

**What to build**:

```
Phase 1 — Taint tracking (highest impact, ~200 lines of Rust):
  - Add SessionTaint struct to agent loop (has_untrusted, has_private_data, sources)
  - Mark browser/http_request output as tainted
  - Mark memory_search/file_read as private-data-accessing
  - Before http_request POST or exec in tainted session: require approval
  - This is the Rule of Two — architecturally prevents the lethal combo

Phase 2 — Content sanitization:
  Cargo.toml: ammonia = "4"

  - Strip all HTML to plain text before LLM sees browser/http output
  - Remove invisible Unicode characters (zero-width spaces, bidi overrides)
  - Truncate to 20K chars to prevent context stuffing
  - Apply to both browser and http_request output

Phase 3 — Randomized delimiters + output filtering:
  - Replace static [EXTERNAL CONTENT] with cryptographically random delimiters per-request
  - Block markdown image syntax in LLM output when session is tainted
  - Block reference-style links (the EchoLeak bypass that hit Microsoft 365 Copilot)
  - Tag tainted observations with provenance before memory storage

Phase 4 — Memory poisoning prevention:
  - Observations derived from tainted sessions get provenance tags
  - Tainted memories are weighted lower in retrieval
  - Prevents persistent poisoning across sessions
```

**Expected impact**: Taint tracking alone (AgentArmor) reduces attack success from ~90%+ to 3%. Content sanitization eliminates structural injection. The combination makes Linus/OpenClaw meaningfully harder to exploit than any current production agent (Claude Computer Use, ChatGPT Atlas, Microsoft Copilot all lack runtime taint tracking).

---

### 4. AGENT AUTONOMY: Be Honest About The Boundaries

**The problem**: Prompting the agent to "be resourceful" without runtime support for retry patterns, credential chains, or failure classification.

**What SOTA says** (Devin production data, SWE-bench, GAIA, OSWorld):
- **Browser agents hit ~60% on multi-step dashboards.** API calls always beat UI automation.
- **No production system auto-provisions credentials from scratch.** Token refresh is solved; acquisition is deliberately a human boundary.
- **"Starting over beats fixing corrupted state"** — Devin's #1 production lesson
- **Explicit failure classification** is the highest-value retry pattern: transient (retry with backoff) vs permanent (escalate) vs semantic (retry with different approach)
- **Realistic autonomy numbers**: 67% PR merge rate (Devin), ~15% of complex tasks complete without human help, 8-13% cycle time improvement (not 50%)

**What to build**:

```
Phase 1 — Failure classification in tool_exec.rs:
  - Classify tool failures: Transient (network timeout, rate limit) / Permanent (auth denied, 404) / Semantic (wrong tool, bad args)
  - Transient: auto-retry with exponential backoff (max 3 attempts)
  - Permanent: escalate to user immediately with specific error + what they need to do
  - Semantic: let the agent try a different approach (already handled by LLM, but log it)

Phase 2 — Credential chain as runtime, not prompt:
  - For known services (Stripe, Vercel, GitHub, Twitter), build a credential_check function:
    1. Check env var (STRIPE_SECRET_KEY, etc.)
    2. Check config files (.env, ~/.config/gh/hosts.yml, etc.)
    3. Check CLI auth (gh auth status, vercel whoami, etc.)
    4. Return: found + value, or not_found + exact instructions for user
  - This replaces the "try two approaches" prompting with deterministic code

Phase 3 — Checkpoint-based task execution:
  - For multi-step tasks: Plan -> Execute chunk -> Verify -> Checkpoint -> Next chunk
  - On failure: roll back to last checkpoint, try different approach
  - After 2 checkpoint failures: escalate to user with full context
  - This is Devin's architecture and it works
```

**Expected impact**: Failure classification prevents the most common waste (retrying hopeless requests, giving up on fixable ones). Credential chain as runtime code eliminates the "be resourceful" prompt gambling. Checkpoint architecture is what took Devin from 34% to 67% PR merge rate.

---

## Priority Stack (What To Build First)

| # | What | Why First | Effort | Impact |
|---|------|-----------|--------|--------|
| 1 | Wire `record_skill_use` | Dead code. The entire feedback loop depends on this. | Hours | Unlocks everything |
| 2 | Taint tracking (SessionTaint) | Rule of Two defense. ~200 lines. Biggest security win. | 1-2 days | Attack surface: 90%+ -> 3% |
| 3 | Quality gate on skill_store | Prevent error propagation. One `if succeeded` check. | Hours | Prevents library rot |
| 4 | FTS5 for search | Drop-in replacement for LIKE. Zero new deps. | Half day | Better recall immediately |
| 5 | Content sanitization (ammonia) | Strip HTML before LLM sees it. One new crate. | Half day | Kills structural injection |
| 6 | Failure classification | Transient/permanent/semantic. Better retry behavior. | 1 day | Fewer wasted retries |
| 7 | sqlite-vec + fastembed | Vector search. Two new crates. Scales to 100K skills. | 2-3 days | Future-proof skill retrieval |
| 8 | Auto skill distillation | End-of-task LLM call to extract reusable procedure. | 1-2 days | Skills emerge from experience |
| 9 | Deduplication (embedding similarity) | Prevent near-duplicate skills. Needs vectors first. | 1 day | Clean skill library |
| 10 | Randomized delimiters | Replace static [EXTERNAL CONTENT]. Cryptographic barrier. | Hours | Harder to inject |
| 11 | Credential chain runtime | Deterministic check for known services. Replaces prompting. | 1-2 days | Reliable auth resolution |
| 12 | Checkpoint-based execution | Plan/Execute/Verify/Checkpoint loop. Biggest arch change. | 3-5 days | Devin-level reliability |

---

## The Sobering Numbers

From production deployments (Devin, Claude Code, DORA Report):
- **15%** of complex tasks complete without human help (Devin)
- **8-13%** realistic cycle time improvement (DORA/Thoughtworks)
- **9% more bugs** correlated with AI adoption (Google DORA)
- **No production system** has shipped autonomous skill learning
- **"The agents may code YOLO, but the infrastructure they run on does not"** (Mike Mason)

Implementing items 1-8 from the priority stack would put Linus/OpenClaw ahead of every open-source agent framework and competitive with the best commercial offerings on the skill/learning axis. Items 9-12 would push into genuinely novel territory — especially auto skill distillation with quality gates, which nobody has shipped in production.

---

## Key Papers & Sources

**Skill Learning**: SkillRL (arxiv:2602.08234), EvolveR (arxiv:2510.16079), SAGE (arxiv:2512.17102), Voyager (MineDojo)
**Vector Search**: ITR (arxiv:2602.17046), ToolScope (Red Hat), sqlite-vec, fastembed-rs, BGE-M3
**Prompt Injection**: "The Attacker Moves Second" (arxiv:2510.09023), CaMeL (arxiv:2503.18813), Meta Rule of Two, AgentArmor (arxiv:2508.01249), EchoLeak (CVE-2025-32711)
**Agent Autonomy**: Devin 2025 Review, GAIA benchmark, SWE-bench Verified, OSWorld, Claude Code self-hosting

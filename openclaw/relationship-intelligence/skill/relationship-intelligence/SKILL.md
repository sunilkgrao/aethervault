---
name: relationship-intelligence
description: Query and maintain Sunil's relationship graph. Use when the task involves who someone is, family context, outreach, reconnects, anniversaries, intros, Rhaine handoff, or relationship follow-through.
allowed-tools: Bash, Read, Write
---

# Relationship Intelligence

This skill is the native OpenClaw path for Linus to reason about Sunil's network.

Use it when the user asks things like:
- who is a person
- who Sunil should reach out to
- whether Linus knows Sunil's parents, wife, EA, or family context
- intros, reconnects, anniversaries, birthdays, or follow-ups
- travel or logistics where people context matters

Do not rely on generic memory retrieval first. Query the relationship store directly.

## Canonical files

- Store: `/root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite`
- Script: `/root/.openclaw/workspace/relationship-intel/relationship_intel.py`
- Radar markdown: `/root/.openclaw/workspace/memory/RELATIONSHIP-RADAR.md`
- Index: `/root/.openclaw/workspace/memory/PEOPLE-INDEX.md`
- Person pages: `/root/.openclaw/workspace/memory/people/*.md`

## Fast paths

### Weekly or proactive relationship guidance

Use:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  brief --json
```

This is the default path for:
- "who should I reach out to this week?"
- "what open loops do I have with people?"
- "what birthdays or anniversaries matter?"

### Person lookup

Use:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  summary "Prasad Rao" --json
```

Use this before asking obvious questions about:
- parents
- Angelic / Emile / family
- Rhaine
- contacts, intros, prior relationship context

### Broad search

Use:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  search "founder toronto" --json
```

### Record a real touchpoint

When Sunil mentions a meaningful interaction, update the graph:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  touch "Dev Rajendran" \
  --note "Caught up on Slack about next week and the UiPath thread." \
  --channel slack \
  --memory-dir /root/.openclaw/workspace/memory
```

Then refresh the prompt surface:

```bash
/root/.openclaw/workspace/relationship-intel/relationship_sync.sh
```

## Operating rules

- Query the relationship store before asking the user obvious people-context questions.
- Treat `brief` and `summary` as the primary source; treat markdown pages as a quick human-readable fallback.
- If the relationship store is thin or ambiguous, say so and ask only the missing high-signal question.
- Handle fast person lookups and reconnect queries inline; do not spawn workers just to answer a trivial relationship question.
- Use workers only when the task turns into broader research, parallel outreach preparation, or heavier cross-source synthesis.
- Do not auto-send outreach. Use the relationship context to draft or prepare a handoff.
- When travel, logistics, or booking work involves known people, pull their summaries first so Linus can infer context intelligently.

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
- what arrived recently on WhatsApp, who sent it, or what recent WhatsApp context matters
- what recent Slack or email context matters
- what important emails are waiting on Sunil
- what a Drive doc, Roam note, or meeting history says about a person or company

Do not rely on generic memory retrieval first. Query the relationship store directly.

## Canonical files

- Store: `/root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite`
- Script: `/root/.openclaw/workspace/relationship-intel/relationship_intel.py`
- Radar markdown: `/root/.openclaw/workspace/memory/RELATIONSHIP-RADAR.md`
- Operating state: `/root/.openclaw/workspace/memory/HOT-STATE.md`
- Promoted docs: `/root/.openclaw/workspace/memory/PROMOTED-DOCS.md`
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

If Linus needs the smallest high-value surface first, use:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  operating-state --json
```

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

### Recent WhatsApp context

Use:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  messages \
  --channel whatsapp \
  --days 2 \
  --direction inbound \
  --limit 20 \
  --json
```

If the store is empty or stale, refresh it first:

```bash
/root/.openclaw/workspace/relationship-intel/whatsapp_history_sync.sh
```

For broad “what important messages came in recently?” questions, use:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  channel-brief \
  --channel whatsapp \
  --days 2 \
  --limit 12 \
  --json
```

### Other imported sources

Use `messages` for channels that were imported as message evidence:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  messages \
  --channel slack \
  --days 14 \
  --limit 20 \
  --json
```

Use `docs-search` for Roam, Drive, and Calendar evidence:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  docs-search "Tribble reasoning" \
  --channel roam \
  --json
```

Use `gmail-guided` when the task is email-shaped but should be driven by people/open-loop/project context rather than a broad mailbox crawl:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  gmail-guided \
  --person "Rohan Verma" \
  --objective "investor introductions" \
  --days 365 \
  --json
```

Use `import-himalaya-email` when the question depends on the personal Gmail account that is still accessed through Himalaya:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  import-himalaya-email \
  --account-name personal \
  --folder "Needs Action" \
  --folder "EA to Action" \
  --max-messages 200 \
  --json
```

For proactive inbox triage or “what am I neglecting?” questions, use:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  email-attention \
  --days 21 \
  --limit 12 \
  --json
```

If the question is specifically about deals, intros, partnerships, customers, or investor follow-through, bias the ranking:

```bash
python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py \
  --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  email-attention \
  --days 21 \
  --focus deals \
  --limit 12 \
  --json
```

### Incremental freshness

For routine cross-inbox freshness on the host, use:

```bash
/root/.openclaw/workspace/relationship-intel/incremental_sync.sh
```

To inspect what it will do before running:

```bash
DRY_RUN=1 /root/.openclaw/workspace/relationship-intel/incremental_sync.sh
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
- For recent WhatsApp questions, use `messages` first instead of guessing or saying you cannot inspect WhatsApp.
- Never use the generic `message` tool to read WhatsApp/Slack/email history. That path is for delivery actions, not archival review. Use `channel-brief` or `messages` from `relationship_intel.py`.
- For Slack or email recency questions, use `messages --channel slack|email` before improvising.
- For targeted inbox questions about a known person, project, or open loop, use `gmail-guided` before broad Gmail import in the live Google lane.
- For important-inbox triage, use `email-attention` before improvising from generic email search.
- For deal or intro slippage, use `email-attention --focus deals` so internal ops, family, and accepted calendar noise do not dominate the ranking.
- Prefer `incremental_sync.sh` for routine Gmail/Calendar/Drive/personal-email freshness instead of broad manual re-imports.
- Personal Gmail (`sunilkgrao@gmail.com`) currently comes from Himalaya; corporate Gmail (`sunil@tribble.ai`) comes from Google OAuth. Treat them as one evidence plane once imported.
- For company or personal note questions, use `docs-search` before claiming the context is unavailable.
- Prefer `operating-state` and `HOT-STATE.md` before pulling the broader document archive into prompt context.
- After any major multi-source import, run `reconcile-identities` so duplicate people do not linger under separate phone/email records.
- Treat `brief` and `summary` as the primary source; treat markdown pages as a quick human-readable fallback.
- If the relationship store is thin or ambiguous, say so and ask only the missing high-signal question.
- Handle fast person lookups and reconnect queries inline; do not spawn workers just to answer a trivial relationship question.
- Use workers only when the task turns into broader research, parallel outreach preparation, or heavier cross-source synthesis.
- Do not auto-send outreach. Use the relationship context to draft or prepare a handoff.
- When travel, logistics, or booking work involves known people, pull their summaries first so Linus can infer context intelligently.

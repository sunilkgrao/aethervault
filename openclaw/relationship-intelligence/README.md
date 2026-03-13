# Relationship Intelligence

This package seeds and maintains Linus's relationship-intelligence layer on top
of OpenClaw.

It is not a standalone CRM app.

The goal is:
- keep a typed people graph
- expose high-signal person summaries as Markdown for OpenClaw memory search
- generate a proactive relationship radar
- let Linus record meaningful touchpoints over time

## Main script

`relationship_intel.py`

Commands:
- `build` turns legacy CRM data into a cleaned SQLite store plus rendered memory
- `search` finds people by name, alias, topic, org, phone, or email
- `summary` prints one person summary
- `claims` inspects semantic claims and graph edges for one person
- `timeline` inspects one person's recent messages, touches, and semantic claims
- `network` inspects one person's connected entities, edges, and supporting facts
- `candidate-claims` lists low-confidence semantic claims that still need adjudication
- `entity-search` searches canonical entities in the relationship KG
- `brief` prints a compact weekly relationship brief with reconnects, open loops, and upcoming moments
- `operating-state` prints the compact promoted operating surface Linus should bias toward
- `radar` prints the current proactive relationship radar
- `render` regenerates Markdown summaries from the SQLite store
- `touch` records a new touchpoint so recency stays current and can optionally re-render memory
- `import-whatsapp` imports normalized WhatsApp history/events into the relationship store
- `import-imessage-profiles` imports curated iMessage profile summaries into the same people/claims graph
- `import-slack-archive` imports a `slackdump.sqlite` archive into the same evidence and relationship graph
- `import-google-gmail` imports Gmail metadata as message evidence
- `import-himalaya-email` imports personal Gmail from the existing Himalaya account into the same evidence store
- `repair-email-identities` relinks imported email evidence to exact email identities and prunes polluted email merges
- `gmail-guided` uses graph signal to retrieve only the most relevant live Gmail messages and threads for a person or objective
- `import-google-calendar` imports Calendar events as meeting evidence and touchpoints
- `import-google-drive` imports Drive metadata broadly and only promoted bodies selectively
- `import-roam-notes` imports extracted Roam markdown notes as document evidence
- `sync-incremental` runs an incremental Gmail/Calendar/Drive/personal-email sync using a persisted sync state
- `reconcile-whatsapp` rebuilds the WhatsApp relationship ontology, semantic claims, and edges from imported history
- `reconcile-identities` merges duplicate people across sources using email/phone identity while preserving claims and edges
- `messages` queries recent channel messages already imported into the relationship store
- `channel-brief` ranks the most important recent channel messages using relationship signal and recency
- `email-attention` ranks important inbound email threads that look like they are waiting on Sunil
  - use `--focus deals` when the task is specifically about deals, intros, customers, or investor follow-through
- `docs-search` searches imported Roam/Drive/Calendar document evidence
- `stats` prints high-level counts

## Typical seed flow

```bash
python3 relationship_intel.py build \
  --crm-db /path/to/crm.db \
  --dossiers-dir /path/to/dossiers \
  --out-dir /tmp/relationship-intel-seed \
  --top-n 250
```

That produces:
- `relationship_intel.sqlite`
- `memory/RELATIONSHIP-RADAR.md`
- `memory/RELATIONSHIP-INTELLIGENCE.md`
- `memory/HOT-STATE.md`
- `memory/PROMOTED-DOCS.md`
- `memory/PEOPLE-INDEX.md`
- `memory/people/*.md`
- a compact prompt-surface snapshot via `prompt-block`
- an OpenClaw workspace skill template under `skill/relationship-intelligence/SKILL.md`

## Deployment

Copy the rendered output into the live OpenClaw workspace, then reindex memory:

```bash
openclaw memory index --force
```

Typical live touchpoint update:

```bash
python3 relationship_intel.py --db /root/.openclaw/workspace/relationship-intel/relationship_intel.sqlite \
  touch "Dev Rajendran" \
  --note "Caught up on Slack about next week and the UiPath thread." \
  --channel slack \
  --memory-dir /root/.openclaw/workspace/memory

openclaw memory index --force
```

The companion wrapper `/root/.openclaw/workspace/relationship-intel/relationship_sync.sh`
re-renders memory, refreshes the compact relationship snapshot inside
`/root/.openclaw/workspace/MEMORY.md`, and reindexes OpenClaw for the `main`
agent in one step.

## WhatsApp history ingestion

OpenClaw's live WhatsApp channel is a transport adapter, not a durable inbox
browser. This package adds an explicit backfill/import path so Linus can inspect
recent WhatsApp history and fold it into relationship intelligence.

Files:
- `whatsapp_history_sync.mjs` connects to the already-linked Baileys auth state,
  requests history, and emits normalized NDJSON records.
- `whatsapp_history_sync.sh` pauses the gateway, runs the exporter, imports the
  NDJSON into `relationship_intel.sqlite`, refreshes memory, and restarts the
  gateway.
- `whatsapp_history_relink.sh` is the safer relink path when an already-linked
  session will not yield history. It uses a temporary auth directory, waits for
  a new QR scan, and only swaps the new auth live if the history import succeeds.

Typical live run on the host:

```bash
/root/.openclaw/workspace/relationship-intel/whatsapp_history_sync.sh
```

For routine freshness across Google sources, use the incremental wrapper:

```bash
/root/.openclaw/workspace/relationship-intel/incremental_sync.sh
```

Dry-run the next sync window/query plan without mutating anything:

```bash
DRY_RUN=1 /root/.openclaw/workspace/relationship-intel/incremental_sync.sh
```

Default live inbox lanes:
- corporate Gmail: Google OAuth (`sunil@tribble.ai`)
- personal Gmail: Himalaya IMAP (`sunilkgrao@gmail.com`)

Those feeds land in one email evidence plane, so `email-attention`, `channel-brief --channel email`, and the hot operating surface do not need to care which inbox produced the thread.

If the current linked session will not provide history, use the safer relink
flow instead:

```bash
/root/.openclaw/workspace/relationship-intel/whatsapp_history_relink.sh
```

Then inspect recent inbound messages:

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

## OpenClaw-native usage

The intended live pattern is:
- use the `relationship-intelligence` workspace skill so Linus reaches for this subsystem intentionally
- use `brief --json` for weekly reconnect / open-loop / anniversary questions
- use `summary --json` for person-specific context before asking obvious follow-ups
- use `timeline --json` when Linus needs actual recent conversational context before acting
- use `network --json` when Linus needs a person's affiliations, groups, places, and relationship edges
- use `candidate-claims --json` to review still-noisy claims before promoting them into stronger guidance
- use `operating-state --json` when Linus needs the compact high-value relationship + company/personal context surface
- use `docs-search --json` when the answer likely lives in Roam, Drive, or Calendar evidence rather than in a person summary
- use `gmail-guided --person ... --objective ... --json` before broad Gmail import when the task is about a known person, company thread, travel flow, or open loop in the live Google account
- use `channel-brief --channel whatsapp|slack|email --days ... --json` when the question is “what important messages came in recently?”
- use `email-attention --days 21 --json` when the question is “what important emails am I letting slip?”
- use `email-attention --days 21 --focus deals --json` when the question is specifically about deals, intros, partnerships, or investor follow-through
- use `repair-email-identities --days 180 --json` after large email backfills if older imports used weaker merge rules
- use `import-imessage-profiles --profiles-dir ...` when refreshing the curated iMessage profile layer from preserved archives
- use `import-slack-archive --archive-dir ...` when backfilling company chat context
- use `import-google-gmail --account-email ...` for inbox history
- use `import-himalaya-email --account-name personal ...` for personal Gmail backfill and refresh
- use `import-google-calendar --account-email ...` for meeting and attendee history
- use `import-google-drive --account-email ... --body-limit 80` for strategic document ingestion
- use `sync-incremental --account-email ... --dry-run --json` to inspect the next Gmail/Calendar/Drive/personal-email refresh plan
- use `import-roam-notes --notes-dir ...` for personal notes and long-horizon thinking context
- use `messages --channel whatsapp --days 2 --direction inbound --json` when the task is about recent WhatsApp traffic
- use `touch ... --memory-dir ...` after meaningful interactions so the graph stays current

Delegation policy here should stay nuanced:
- simple lookups like "who is Baba?" or "who should I reach out to this week?" should usually run inline through the relationship toolchain
- use workers only when the task becomes broader research, cross-source cleanup, or parallel contact/outreach planning

## Safe remote regression battery

`remote_battery.sh` runs a small OpenClaw battery on the droplet without touching the live Telegram/Slack session state. It creates an isolated temporary OpenClaw home, disables channels, runs canonical prompts locally, and writes a markdown report under the remote OpenClaw reports directory.

## Assertion battery

`assertion_battery.py` is the stronger regression harness. It is meant to run on the OpenClaw host and checks:
- migration integrity and source provenance
- direct relationship-tool behavior
- alias resolution and dossier grounding
- recency mutation via `touch`
- isolated OpenClaw agent behavior on real relationship prompts

Use `remote_assertion_battery.sh` from this repo to execute it over SSH once the script has been deployed to the droplet.

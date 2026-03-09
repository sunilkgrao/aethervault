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
- `brief` prints a compact weekly relationship brief with reconnects, open loops, and upcoming moments
- `radar` prints the current proactive relationship radar
- `render` regenerates Markdown summaries from the SQLite store
- `touch` records a new touchpoint so recency stays current and can optionally re-render memory
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

## OpenClaw-native usage

The intended live pattern is:
- use the `relationship-intelligence` workspace skill so Linus reaches for this subsystem intentionally
- use `brief --json` for weekly reconnect / open-loop / anniversary questions
- use `summary --json` for person-specific context before asking obvious follow-ups
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

# Clawdbot Migration Utility

This repo exists to keep the Linus stack clean during and after the OpenClaw cutover.

It has three jobs:
- export durable assistant state into an OpenClaw workspace
- keep the Linus/OpenClaw doc surface coherent
- preserve and improve the specific skills that still matter

This repo does not own the live runtime. Upstream OpenClaw does.

## Start Here

Read these files first:
- `docs/SOURCE-OF-TRUTH.md`
- `FINAL_STATE.md`
- `docs/OPENCLAW_REFOUND.md`
- `AGENTS.md`

## Core Command

```bash
cargo build --locked

cargo run --bin linus-migrate -- export-open-claw /path/to/memory.mv2 \
  --workspace ~/.openclaw/workspace
```

The export target is a clean OpenClaw workspace containing:
- `SOUL.md`
- `USER.md`
- `MEMORY.md`
- `STATE.md`
- `STATE.json`
- imported logs and transcripts under `imports/legacy-runtime/`

## What Belongs In This Repo

- migration/export code
- state and memory portability logic
- Linus/OpenClaw architecture docs
- ported skill docs and validation doctrine

## What Does Not Belong In This Repo

- a second runtime
- bespoke connector ownership
- environment-specific operational sprawl
- duplicate memory/state doctrines
- stale docs kept around after the doctrine changed

## Ported Skill Surface

The kept operational skill surface lives under `deploy/openclaw-skills/`:
- `ds9-triage`
- `ds9-pr-testing`
- `ds9-prod-debug`
- `slack-media-analysis`
- `tribble-desktop-triage`
- `jira-eng-board`

Additional reusable skills under `skills/` should stay current, product-accurate, and OpenClaw-native.

## Development Rules

- prefer small, targeted diffs
- update the authoritative docs when architecture or workflow assumptions change
- do not reintroduce old runtime vocabulary, old service assumptions, or split-brain docs

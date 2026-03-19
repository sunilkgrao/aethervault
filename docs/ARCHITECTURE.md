# Clawdbot Architecture

## Thesis

This repo should be small, sharp, and boring.

It exists to move Linus cleanly onto OpenClaw and to preserve the skills, memory, and validation discipline that still matter. It should not grow back into a second runtime.

## Core Planes

### Export Plane

- exports durable state into an OpenClaw workspace
- preserves `SOUL.md`, `USER.md`, `MEMORY.md`, `STATE.md`, and `STATE.json`
- carries forward imported transcripts and other durable context needed for continuity

### Workspace Plane

- `MEMORY.md` holds durable facts and long-lived context
- `STATE.md` and `STATE.json` hold live priorities, open loops, waiting-fors, and active work
- exported workspace content is for runtime use, not a competing repo doctrine

### Skill Plane

- ported operational skills live under `deploy/openclaw-skills/`
- reusable helper skills live under `skills/`
- skills must stay aligned with OpenClaw and with the repo control docs

### Control Plane

The control plane for this repo is the kept markdown set:
- `README.md`
- `AGENTS.md`
- `docs/SOURCE-OF-TRUTH.md`
- `FINAL_STATE.md`
- `docs/OPENCLAW_REFOUND.md`
- the relevant task skill docs

## Invariants

- one live runtime: upstream OpenClaw
- one live executive state: `STATE`
- one durable memory contract: workspace plus imported durable history
- one validation doctrine: evidence before claims
- one architecture narrative: update the kept docs, do not create side doctrines

## What Is Out Of Scope

Do not reintroduce:
- runtime ownership in this repo
- connector sprawl in this repo
- machine-specific operational notebooks as architecture
- competing memory/state models

# Source Of Truth

This file defines the kept control plane for the repo.

The goal is simple: one doctrine, one runtime target, one memory/state model, one validation standard.

## Precedence

When repo-owned markdown conflicts, use this order:
1. direct user instruction
2. `AGENTS.md`
3. `README.md`
4. this file
5. `FINAL_STATE.md`
6. `docs/OPENCLAW_REFOUND.md`
7. task-specific skill docs
8. implementation docs such as `docs/ARCHITECTURE.md`
9. exported workspace content under `assistant/`
10. generated or third-party content under `research/`, `tmp/`, and `vendor/`

## Authoritative Docs

- `README.md`
  - repo purpose and scope
- `AGENTS.md`
  - operator guardrails
- `FINAL_STATE.md`
  - product north star
- `docs/OPENCLAW_REFOUND.md`
  - cutover doctrine
- `docs/ARCHITECTURE.md`
  - kept architecture shape for this repo
- `config/system-prompt.md`
  - prompt-layer behavior invariants

## Kept Operational Docs

- `deploy/openclaw-skills/**/*.md`
  - task-specific operating procedures
- `docs/CAPABILITIES.md`
  - curated capability and skill surface retained in this repo
- `docs/MEMORY-SYSTEM.md`
  - durable memory and live-state rules
- `docs/OPERATIONS.md`
  - repo operations and validation doctrine
- `docs/DEPLOYMENT.md`
  - deployment posture for this repo’s actual scope
- `docs/CONNECTORS.md`
  - connector ownership boundaries

## Workspace Content

The files under `assistant/` are exported or example workspace content for Linus on OpenClaw.
They are useful, but they do not override the repo control docs.

## Coherence Rules

- one live runtime: upstream OpenClaw
- one live executive state: `STATE.md` and `STATE.json`
- one durable memory surface: exported workspace plus imported transcripts, not scattered markdown clones
- one validation doctrine: no `verified` or `tested` claim without direct evidence
- one control plane: update the kept docs instead of creating side doctrines

## Change Rule

If you change architecture, migration flow, memory/state semantics, or validation doctrine, update the relevant authoritative docs in the same change.

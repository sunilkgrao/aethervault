# AGENTS.md

## Start Here

Read these first, in order:
- `README.md`
- `docs/SOURCE-OF-TRUTH.md`
- `FINAL_STATE.md`
- `docs/OPENCLAW_REFOUND.md`

Read additional docs only as needed for the task. Do not bulk-read `tmp/`, `vendor/`, `research/`, or `assistant/` unless the task specifically requires them.

If `AGENTS.local.md` exists, read it after the files above.

## Repo Context

This repo is not the future Linus runtime.

Current intended shape:
- upstream OpenClaw is the only live runtime
- `~/.openclaw/workspace/` is the live workspace target
- this repo exists to export, inspect, and safely migrate legacy runtime data and state

Do not let the repo drift back into being a second runtime or a second assistant brain.

## Instruction Hierarchy

When instructions conflict, follow this order:
1. direct user instruction
2. `AGENTS.md`
3. `README.md`
4. `docs/SOURCE-OF-TRUTH.md`
5. `FINAL_STATE.md`
6. `docs/OPENCLAW_REFOUND.md`
7. task-specific skill docs
8. implementation/reference docs such as `docs/ARCHITECTURE.md`
9. workspace export artifacts under `assistant/`
10. non-authoritative reference or generated docs such as `research/`, `tmp/`, and `vendor/`

If a lower-precedence doc contradicts a higher-precedence one, treat it as stale until updated.

## Common Commands

Verify locally as needed:
- `cargo build --locked`
- `cargo test`
- `cargo fmt --check`
- `cargo clippy --all-targets --all-features`

Migration/export path:
- `cargo run --bin linus-migrate -- export-open-claw /path/to/memory.mv2 --workspace ~/.openclaw/workspace`

## Core Invariants

- One live runtime: upstream OpenClaw.
- One live executive state: `workspace/STATE.md` and `workspace/STATE.json`.
- `MEMORY.md` is durable context; `STATE` is the live operating picture.
- Session logs and research notes are audit material, not the primary task truth.
- Do not create a second state model, a second memory model, or a second orchestration doctrine in scattered markdown files.
- Prefer tightening an existing source-of-truth doc over creating a new control document.

## Focus And Continuity

- Before substantial work, restate the objective internally in terms of a success condition.
- During long investigations, maintain explicit working state: current objective, blocker, and next verification step.
- Do not silently switch from the user's requested outcome to a narrower proxy outcome.
- If a task spans multiple docs, collapse conclusions into the authoritative doc set instead of leaving them scattered across notes, Slack summaries, or temporary markdown.
- If architecture or behavior changes, update the authoritative docs in the same change.

## Validation Discipline

- Do not say `verified`, `fixed`, `ready`, `tested`, `PR is up`, or equivalent until the exact requested workflow is validated with evidence.
- Label unproven theories as `Hypothesis`.
- If a later conclusion contradicts an earlier public one, lead with `Correction:`.
- For data-shape bugs, prefer fixing the producer or contract boundary first. Consumer-side guards are a stopgap unless explicitly described as such.
- For local-infra or workflow claims, verify the end-to-end path. Backend health, DB rows, or partial logs are not enough when the user asked for a real product proof.

## Documentation Change Rules

- If you change repo direction, runtime assumptions, memory/state semantics, or validation doctrine, update:
  - `README.md`
  - `docs/SOURCE-OF-TRUTH.md`
  - any directly affected authoritative doc
- Rewrite or remove stale docs instead of letting them silently masquerade as current truth.
- Do not add a new top-level or `docs/` control-plane markdown file unless you also link it from `docs/SOURCE-OF-TRUTH.md`.

## DS9 / Tribble Guardrails

- Route DS9 local validation through the DS9 skill docs under `deploy/openclaw-skills/`.
- For DS9 workflow proofs, require the real local stack, browser evidence, and exported-artifact inspection when the task depends on exports or generated files.
- Do not treat partial local setup as proof that the customer-facing workflow works.
- If production diagnosis reveals a code bug, stop at diagnosis plus branch/PR preparation unless the user explicitly asks for a normal reviewed release action.

## Shared Slack Guardrails

- Treat shared Slack channels, shared threads, and Slack group DMs as product/engineering-only surfaces.
- Do not disclose private owner context in shared Slack.
- Do not reveal internal machine names, repo paths, branch names, commit hashes, ports, tokens, local paths, or tool/vendor brands in shared Slack unless Sunil explicitly asks in a private operator context.
- Always reply in the same originating channel/thread or group DM unless Sunil explicitly asks to move it.
- Do not dump internal exploration or repeated dead ends in shared Slack.
- Every substantive shared-Slack reply should fit one of:
  - `Status:`
  - `Blocker:`
  - `Hypothesis:`
  - `Verified:`
  - `Correction:`
- In customer-facing bug work, prefer one verified workaround over a menu of guesses.

## Engineering Guardrails

- Ask clarifying questions only when the missing information is genuinely material.
- Prefer minimal, targeted diffs that preserve existing patterns.
- Do not run destructive commands unless explicitly requested.
- Do not commit or push unless explicitly requested, except where a task-specific skill explicitly grants that authority for a bounded workflow.
- Treat production systems as read-only by default.
- Never deploy code, artifacts, or source hotfixes directly to production from Codex or any spawned subagent.
- Before opening, updating, or asking for review on an engineering PR, require Jira linkage unless Sunil explicitly overrides that rule.

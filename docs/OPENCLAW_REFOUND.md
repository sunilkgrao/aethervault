# OpenClaw Re-Founding

This is the target migration path for Linus.

## Decision

Do not keep AetherVault as the long-term runtime base.

Use:
- upstream OpenClaw as the only runtime
- a fresh DigitalOcean container
- this Rust repo only as temporary migration/export tooling until Linus data is safely imported

## Why

OpenClaw already ships the control-plane features this repo has been recreating badly:
- daemon/service management
- gateway
- channel/session model
- workspace and skills model
- security and sandboxing
- model configuration and failover

Carrying AetherVault runtime behavior forward would mean dual-maintaining old assumptions instead of building on the stronger base.

## Target Shape

The fresh container should contain:
- upstream OpenClaw checkout/install
- `~/.openclaw/workspace/`
- Linus-specific `SOUL.md`, `USER.md`, `MEMORY.md`, `STATE.md`, `STATE.json`
- imported legacy logs and session transcripts under `workspace/imports/legacy-aethervault/`
- Linus-specific skills, policies, and channel configuration in native OpenClaw form

Prompt policy should be explicit:
- Linus orchestrates by default
- non-trivial, parallelizable, or long-running work should fan out to workers
- trivial, low-latency, or clearly higher-quality one-shot work should stay inline
- the main agent remains responsible for planning, routing, review, and synthesis

It should not require:
- `~/.aethervault/`
- AetherVault systemd units
- AetherVault env names
- AetherVault bridge processes
- AetherVault-specific deploy scripts

## Migration Steps

1. Export durable Linus data from the legacy capsule/runtime.

Use the transitional exporter in this repo:

```bash
cargo run --bin linus-migrate -- export-open-claw /path/to/memory.mv2 \
  --workspace ~/.openclaw/workspace
```

This exports:
- `SOUL.md`
- `USER.md`
- `MEMORY.md`
- `STATE.md`
- `STATE.json`
- optional daily memory
- legacy JSONL logs
- persisted session transcripts

2. Provision a fresh container.

Install only what OpenClaw actually needs. Do not copy legacy AetherVault services over.

3. Install upstream OpenClaw.

Canonical source:
- `https://github.com/openclaw/openclaw`

4. Import Linus-specific state.

Place exported files into `~/.openclaw/workspace/` and convert any remaining Linus-specific policy or skills into native OpenClaw structures.

5. Recreate only the live integrations that matter.

Examples:
- Telegram
- Slack
- email/calendar
- Twilio voice

Recreate them the OpenClaw way, not by reviving legacy bridge code unless something is truly missing upstream.

6. Run the scenario battery.

Canonical cases:
- parent travel orchestration
- doctor appointment planning
- restaurant reservation by phone
- vendor escalation
- tweet-to-execution

7. Cut traffic over.

Once OpenClaw passes the battery, retire the Rust runtime.

## Rules

- Keep data, not baggage.
- Prefer upstream OpenClaw mechanisms over custom replacements.
- Only port Linus-specific behavior that is genuinely missing upstream.
- Do not preserve old path/env/service names unless required for one-time migration.

## Transitional Role Of This Repo

Short term:
- export legacy memory and state
- document the migration
- help verify completeness

Long term:
- either archive this repo
- or reduce it to a tiny migration/export utility with no runtime ambitions

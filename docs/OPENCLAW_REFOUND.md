# OpenClaw Re-Founding

This is the target cutover path for Linus.

## Decision

Use upstream OpenClaw as the only live runtime.

Keep this repo focused on:
- export and migration
- doc and skill coherence
- portability of Linus-specific state and behavior

## Target Shape

The live environment should contain:
- upstream OpenClaw
- `~/.openclaw/workspace/`
- Linus-specific `SOUL.md`, `USER.md`, `MEMORY.md`, `STATE.md`, and `STATE.json`
- imported transcripts and durable logs under `workspace/imports/legacy-runtime/`
- ported skills and policies in native OpenClaw form

## Prompt Policy

- Linus orchestrates by default
- trivial work stays inline
- non-trivial or parallelizable work fans out to workers
- the main agent owns planning, synthesis, review, and final responsibility

## Migration Steps

1. Export durable Linus data:

```bash
cargo run --bin linus-migrate -- export-open-claw /path/to/memory.mv2 \
  --workspace ~/.openclaw/workspace
```

2. Install upstream OpenClaw.

3. Place the exported workspace files into `~/.openclaw/workspace/`.

4. Port only the skills, policies, and behaviors that still matter.

5. Recreate live integrations the OpenClaw way.

6. Run the real scenario battery.

7. Cut over fully and keep this repo narrow.

## Rules

- keep data, not baggage
- prefer upstream mechanisms over bespoke replacements
- port only what materially improves Linus
- do not let old runtime vocabulary or old service assumptions leak back into the kept control plane

# Deployment Guide

This repo does not own the live assistant deployment.

Use upstream OpenClaw for the runtime.

## What This Repo Should Be Used For

- building and validating the migration/export utility
- exporting Linus state into `~/.openclaw/workspace`
- maintaining the kept docs and skill surface

## Minimal Validation

```bash
cargo build --locked
cargo test
cargo run --bin linus-migrate -- export-open-claw /path/to/memory.mv2 \
  --workspace ~/.openclaw/workspace
```

## What Not To Deploy

- a second assistant runtime from this repo
- connector stacks from this repo
- machine-specific operational sprawl

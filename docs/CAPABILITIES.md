# Capabilities

This file tracks the curated capability surface that still belongs in this repo.

## Core Repo Capabilities

- `linus-migrate` export into an OpenClaw workspace
- doc-control and validation doctrine for Linus/OpenClaw
- ported operational skills for DS9, production diagnosis, media analysis, and workstation triage

## Ported Skill Set

Under `deploy/openclaw-skills/`:
- `ds9-triage`
- `ds9-pr-testing`
- `ds9-prod-debug`
- `slack-media-analysis`
- `tribble-desktop-triage`
- `jira-eng-board`

Under `skills/`:
- reusable helper skills such as `agentmail`, `emergency-compact`, and `last30days`

## Capability Rules

- prefer the task-specific skill when one exists
- keep skill docs product-accurate and OpenClaw-native
- remove stale capability docs instead of letting them drift
- do not document runtime hooks or private machine paths here unless they are still current and intentionally kept

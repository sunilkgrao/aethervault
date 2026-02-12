# AetherVault v5 Integration Test Report

**Droplet:** `root@<DROPLET_IP>`
**Date:** 2026-02-12
**Binary:** `/usr/local/bin/aethervault` (36MB, `--features vec`)

## Test Results: 14/16 PASS (87.5%)

| Test | Description | Status | Notes |
|------|-------------|--------|-------|
| T1 | Basic agent: "What is 2+2?" | **PASS** | Returned 4, exit 0 |
| T2 | File write: write 'banana' to file | **PASS** | File created with exact content |
| T3 | List files using exec tool | **PASS** | 17 entries with detailed table |
| T4 | Multi-turn session recall | **PASS** | Remembered "purple" across turns via `--session` |
| T5 | Memory search: "Who is Rhaine?" | **PASS** | Found EA/Tribble.ai/Philippines with email |
| T6 | Memory search: pets' names | **PASS** | Bali (corgi) + Hachi (shiba) with health details |
| T7 | Multi-source search: Boca Raton | **PASS** | KG entity + relationships + memory + MEMORY.md |
| T8 | Exec tool: echo command | **PASS** | `hello from exec` |
| T9 | Knowledge graph: Angelic | **PASS** | Full person with type, role, email, relationships |
| T10 | Write+read+verify fibonacci | **PASS** | 121-line Python, 4 methods, cross-validated |
| T11 | Session manager: single spawn | **PASS** | File created with correct content after 60s |
| T12 | Session manager: list | **PASS** | JSON with all sessions, correct structure |
| T13 | Session manager: 3 simultaneous | **PASS** | All 3 completed, files correct |
| T14 | Session manager: check-all | **PASS** | 10 sessions reported as completed |
| T15 | Error: missing Twitter script | **FAIL** | Fabricated a briefing from memory |
| T16 | Max-steps graceful limit | **FAIL** | Hook timed out (60s) before max-steps activated |

## Tier Summary

| Tier | Tests | Pass | Fail |
|------|-------|------|------|
| Core Agent | 4 | 4 | 0 |
| Memory & Data | 3 | 3 | 0 |
| Tool Orchestration | 3 | 3 | 0 |
| Session Manager | 4 | 4 | 0 |
| Error/Edge Cases | 2 | 0 | 2 |

## Fixes Applied This Session

| Fix | Details |
|-----|---------|
| fs_write truncation | `ANTHROPIC_MAX_TOKENS` 1024 -> 16384 (env), default 1024 -> 8192 (code) |
| Capsule lock deadlock | Blocking `flock(LOCK_EX)` -> `LOCK_NB` + 15 retries + exponential backoff |
| ONNX model paths | Symlinks at all 3 expected locations with both naming conventions |
| Bridge timeout | 120s -> 300s -> 900s for complex operations |
| Session manager | New `session-manager.py` with spawn/list/check/kill + `--model-hook` CLI fix |
| Docker cleanup | Pruned 8.3GB of build cache (disk 64% -> 58%) |
| Twitter alerts | Disabled (crons renamed to .disabled) |

## Session Manager Architecture

Sessions are fire-and-forget background agent processes:
- Each gets a fun name: `cosmic-falcon-42`, `blazing-phoenix-7`
- Uses `--model-hook "aethervault hook claude"` CLI flag (no capsule config race)
- Uses `--no-memory` to avoid main capsule lock contention
- Output saved to `/root/.aethervault/workspace/sessions/<name>/output.log`
- Registry at `/root/.aethervault/workspace/sessions/registry.json`

## Known Limitations

1. **T15**: Agent fabricates outputs instead of checking if scripts exist (prompt/guardrail issue)
2. **T16**: Hook timeout (60s) too short for large model outputs; max-steps can't activate if step 1 exceeds timeout
3. **OOM on large embeds**: >50 frames causes memory pressure (upstream aether-core)
4. **Two CLI agents + bridge**: Exceeds 30s lock timeout (edge case, single CLI + bridge works fine)

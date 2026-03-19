# Assistant Memory & Project State

This file keeps only durable working guidance that should survive sessions.

## Durable Principles

- one live runtime: OpenClaw
- one live executive state: `STATE`
- one durable memory surface: `MEMORY`
- one control plane: the kept docs plus task-specific skills

## Working Lessons

- do not call a workflow proven until the exact product path is exercised
- for DS9, local browser proof and exported-artifact inspection matter more than code-read confidence
- fix the source contract when data shape mismatches are the real bug
- keep task scope explicit during long investigations
- collapse conclusions into authoritative docs instead of scattered notes

## Ported Skill Surface

The primary operational surface to preserve is:
- DS9 triage and local validation
- DS9 readonly production diagnosis
- Slack media analysis
- workstation triage
- Jira linkage workflow

## What To Avoid

- reviving old runtime language
- preserving stale operational notebooks as doctrine
- duplicating memory or state across multiple markdown systems

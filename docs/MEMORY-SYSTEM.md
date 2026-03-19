# Memory System

## Core Rule

Keep durable memory and live state separate.

## Durable Memory

`MEMORY.md` stores:
- stable facts
- enduring preferences
- long-lived context
- lessons that should survive task closure

Imported transcripts and logs support auditability and deeper retrieval, but they are not the live planning surface.

## Live Executive State

`STATE.md` and `STATE.json` store:
- current priorities
- open loops
- waiting-fors
- active projects
- near-term commitments

If something is important now, it belongs in `STATE`, not buried in a long memory file.

## Retrieval Rule

- search before claiming
- load only the context needed for the task
- prefer explicit state over replaying giant histories

## Migration Rule

This repo’s job is to preserve these distinctions when exporting into OpenClaw:
- durable facts stay durable
- live commitments stay live
- imported history stays available without becoming the primary control plane

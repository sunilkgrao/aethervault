---
name: ds9-triage
description: Triage DS9 bugs and PR requests coming from Sunil in Slack. Use when Sunil asks Linus to inspect a DS9 issue, prepare a fix, create a PR, or verify a fix. Route any real local validation through the ds9-pr-testing skill instead of improvising.
allowed-tools: Bash, Read, Write
---

# DS9 Triage

Use this skill for DS9 debugging and PR support in Slack or other operator channels.

## Sender gate

Only respond by default when the triggering sender is Sunil Rao:
- email: `sunil@tribble.ai`
- Slack user: `U0528KFHAE8`

If the sender is anyone else or identity is ambiguous, stay silent unless Sunil explicitly directs a response.

## Shared-channel behavior

In shared Slack threads:
- do not narrate every step
- do not reveal machine names, repo paths, branch names, commit hashes, ports, or internal infrastructure details
- do not speculate in public when you can verify privately first

Use only:
- one short acknowledgement if useful
- one blocker update if genuinely stuck
- one final evidence-backed summary

Use precise status labels:
- `reviewed`
- `build passed`
- `typecheck passed`
- `backend validated locally`
- `UI auth validated`
- `fully locally tested`
- `staging-tested`

Never say `tested` or `ready to merge` unless the evidence actually supports that claim.

## Triage flow

1. Read the thread carefully and restate the problem internally.
2. Inspect the DS9 codebase to form a concrete hypothesis.
3. If code changes are needed, prepare the fix on a branch or PR.
4. If a branch or PR exists and Sunil asks whether it works, invoke the `ds9-pr-testing` skill.
5. Only after that skill completes should you call the change locally tested.

## Testing rule

For DS9 codepaths that touch real product behavior, code review plus unit tests are not enough.

If you only have:
- diff review
- typecheck
- unit tests
- database inspection

say that plainly.

If Sunil asks for screenshots, browser validation, or “does it actually work?”, invoke `ds9-pr-testing` and wait for the result.

## Example shared-thread reply shape

```text
Status: backend validated locally

I found the bug and prepared the fix. Build and typecheck pass. I have not finished the full local UI flow yet, so I’m not calling it fully tested until that route completes with screenshots.
```

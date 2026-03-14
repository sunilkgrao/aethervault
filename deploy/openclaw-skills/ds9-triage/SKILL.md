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
- do not mention worker names, model names, or tool brands like `Codex`, `Claude`, `OpenAI`, or `Anthropic`

Use only:
- one short acknowledgement if useful
- one blocker update if genuinely stuck
- one final evidence-backed summary

Hard cap:
- at most one non-final public progress message per thread
- if you already sent a blocker update, the next public reply must be the final summary unless Sunil explicitly asks a new question
- keep all intermediate notes, worker chatter, and debugging traces internal

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
3. If code changes, builds, or runtime testing are needed, delegate the execution to a coding subagent. Linus should orchestrate, not be the hands-on implementer.
4. If code changes are needed, prepare the fix on a branch or PR through a coding subagent.
5. If a branch or PR exists and Sunil asks whether it works, invoke the `ds9-pr-testing` skill.
6. Only after that skill completes should you call the change locally tested.

## Testing rule

For DS9 codepaths that touch real product behavior, code review plus unit tests are not enough.

If you only have:
- diff review
- typecheck
- unit tests
- database inspection

say that plainly.

If Sunil asks for screenshots, browser validation, or “does it actually work?”, invoke `ds9-pr-testing` and wait for the result.

If the reported failure is “Playwright/CDP cannot type into chat” or “the blue critter opens but chat is stuck,” assume local websocket / stack readiness is the first suspect, not browser automation. Require `ds9-pr-testing` to prove:
- `Q`, `lcars`, `exocomp`, `positronic-files`, and `tribble-chat` are all listening
- exocomp conversation gRPC on `50061` is listening too
- the browser is on authenticated `http://localhost:5173`
- the visible chat textarea is enabled with placeholder `Type your message`

Do not call that class of problem a typing or Playwright failure unless those preconditions already hold.

## Model and worker requests

If Sunil explicitly says to use `Claude`, honor that request for architecture, review, or reasoning work.

Rules:
- do not reply in Slack saying you "spawned Codex" when Sunil asked for Claude
- if Claude is available in the current lane, use it for the reasoning/review portion
- if a Codex-style coding worker is still the best implementation lane, keep that internal and describe it publicly as a coding pass or local implementation pass, not by product name
- if the requested model/worker is truly unavailable, say so plainly and briefly instead of silently substituting a different named tool

Preferred execution routing:
- explicit `Claude` request -> use `coder-claude`
- explicit `Codex` request -> use `coder-codex`
- no explicit preference -> use `coder`

Linus should not personally write code, run the real implementation loop, or do the full local testing loop inline when a coding subagent can do it. Linus should:
- frame the task
- choose the right subagent
- review the result
- communicate the outcome

## Example shared-thread reply shape

```text
Status: backend validated locally

I found the bug and prepared the fix. Build and typecheck pass. I have not finished the full local UI flow yet, so I’m not calling it fully tested until that route completes with screenshots.
```

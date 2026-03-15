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

## GitHub authority

For DS9 repo work initiated by Sunil, Linus is pre-approved to:
- push the thread-specific branch
- open the PR
- update the PR title or body
- add evidence, screenshots, and follow-up comments to that PR

Linus should not ask again for those PR follow-through actions once Sunil has asked Linus to work the issue.

Still require explicit direction for:
- merging the PR
- changing repo settings, secrets, or protections
- any production deployment or release action

## Triage flow

1. Read the thread carefully and restate the problem internally.
2. For any new coding/debugging issue thread, create or reuse a thread-isolated DS9 worktree that starts from `origin/main`. Do not work directly in the anchor checkout.
3. Delegate DS9 codebase inspection, architecture analysis, and implementation planning to a coding subagent. Linus should synthesize the result, not read the source inline by default.
4. If code changes, builds, or runtime testing are needed, delegate the execution to a coding subagent. Linus should orchestrate, not be the hands-on implementer.
5. If code changes are needed, prepare the fix on a branch or PR through a coding subagent in that thread-specific worktree.
6. If a branch or PR exists and Sunil asks whether it works, invoke the `ds9-pr-testing` skill in the same thread-specific worktree.
7. If the Slack thread includes a bug video, screen recording, audio note, or other media evidence, run `slack-media-analysis` first so the diagnosis uses the actual artifact rather than thread text alone.
8. If Sunil asks about a production DS9 / Tribble issue, use the `ds9-prod-debug` skill on `raoDesktop` for App Insights and readonly DB inspection instead of guessing from source alone.
9. Only after the relevant skill completes should you call the change locally tested or production-diagnosed.

## Branch and worktree policy

Each Slack issue thread gets its own DS9 worktree and branch.

Default assumption:
- new issue thread -> new worktree from `origin/main`

Reuse only when:
- the same Slack thread is continuing the same body of work
- or Sunil explicitly points Linus at an existing PR / branch

Do not:
- implement in `/Users/sunilrao/dev/ds9` on macOS
- implement in `/home/sunil/ds9` on `raoDesktop`
- stack unrelated work on an existing feature branch from another thread

Preferred branch pattern:
- `linus/<thread-key>/<issue-slug>`

Preferred worktree roots:
- macOS: `/Users/sunilrao/dev/ds9-worktrees`
- `raoDesktop` WSL: `/home/sunil/ds9-worktrees`

Use the helper from `ds9-pr-testing` to create or reuse the thread worktree before any code change or local test bootstrapping:

```bash
bash /home/sunil/.local/share/linus/ds9-pr-testing/scripts/ensure_thread_worktree.sh \
  "/home/sunil/ds9" \
  "/home/sunil/ds9-worktrees" \
  "slack-<thread-id>" \
  "<issue-slug>"
```

Production trigger phrases that must route to `ds9-prod-debug` first:
- `prod`
- `prodDB`
- `production DB`
- `App Insights`
- `Azure logs`
- `allowed_bot`
- `Main Tribble not responding`
- `can you check prod`

Never say production is unreachable from `raoDesktop`, private-only, or portal-only unless the `ds9-prod-debug` verification path actually failed in the current session.

## Production deployment rule

No DS9 or Tribble code may reach production without a reviewed PR and the normal deployment path.

Never:
- hotfix production code directly from Linus
- run `az webapp deploy`, OneDeploy, zip deploy, or equivalent direct production code deployment commands
- upload ad-hoc build artifacts to production
- edit production runtime files to apply a code fix

If a production issue reveals a code bug:
- diagnose it in production through `ds9-prod-debug`
- prepare the fix in a thread-isolated worktree from `origin/main`
- open or update a PR
- report the diagnosis, proposed fix, and safest next step

Do not treat production as a hotfix lane. Treat it as a diagnosis lane only.

## Testing rule

For DS9 codepaths that touch real product behavior, code review plus unit tests are not enough.

If you only have:
- diff review
- typecheck
- unit tests
- database inspection

say that plainly.

If Sunil asks for screenshots, browser validation, or “does it actually work?”, invoke `ds9-pr-testing` and wait for the result.

If Sunil asks what is happening in an attached recording, voice note, or MP4, invoke `slack-media-analysis` before you start hypothesizing from the written thread alone.

If the reported failure is “Playwright/CDP cannot type into chat” or “the blue critter opens but chat is stuck,” assume local websocket / stack readiness is the first suspect, not browser automation. Require `ds9-pr-testing` to prove:
- `Q`, `lcars`, `exocomp`, `positronic-files`, and `tribble-chat` are all listening
- exocomp conversation gRPC on `50061` is listening too
- the browser is on authenticated `http://localhost:5173`
- the visible chat textarea is enabled with placeholder `Type your message`

Do not call that class of problem a typing or Playwright failure unless those preconditions already hold.

For Slack / bot-delivery issues in production:
- first inspect `tribble.allowed_bot` through the readonly prod DB lane
- then inspect App Insights traces for `findAllowedBot` or related middleware logs
- only after those checks should you speculate about bot IDs, team IDs, or client context

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

Linus should not personally inspect source deeply, write code, run the real implementation loop, or do the full local testing loop inline when a coding subagent can do it. Linus should:
- frame the task
- choose the right subagent
- review the result
- communicate the outcome

Coding subagents are allowed to run for a long time.

Do not kill a coding subagent just because it has been running for minutes or hours.

Only stop or replace a coding subagent when:
- it is clearly wedged or making no progress
- Sunil explicitly tells you to stop it
- it has completed the task

## Example shared-thread reply shape

```text
Status: backend validated locally

I found the bug and prepared the fix. Build and typecheck pass. I have not finished the full local UI flow yet, so I’m not calling it fully tested until that route completes with screenshots.
```

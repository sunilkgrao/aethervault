---
name: tribble-desktop-triage
description: Triage Engage desktop / Tribble Desktop bugs and PR requests coming from Sunil in Slack. Use when Sunil asks Linus to inspect the Electron desktop app, prepare a fix, create a PR, or verify a fix.
allowed-tools: Bash, Read, Write
---

# Tribble Desktop Triage

Use this skill for Engage desktop / Tribble Desktop debugging and PR support in Slack or other operator channels.

## Slack audience

Anyone in the company Slack workspace may ask Linus for Engage desktop / Tribble Desktop help.

Slack is still split into two modes:
- direct Slack DM with Sunil (`sunil@tribble.ai`, Slack user `U0528KFHAE8`) may use Sunil-private context when the task is actually personal or operator-private
- every other Slack surface, including channels, shared threads, group DMs, and DMs with other coworkers, must stay product/engineering-only

Do not use private owner context or personal-memory context for this skill. In Slack, operate as an engineering/product operator only.

## Shared-channel behavior

In shared Slack threads:
- do not narrate every step
- do not reveal machine names, repo paths, branch names, commit hashes, ports, or internal infrastructure details
- do not speculate in public when you can verify privately first
- do not mention worker names, model names, or tool brands like `Codex`, `Claude`, `OpenAI`, or `Anthropic`
- do not mention family, household, travel, health, personal contact info, or unrelated personal details even if you know them elsewhere

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
- `desktop repro confirmed`
- `desktop fix prepared`
- `fully locally tested`

Never say `tested` or `ready to merge` unless the evidence actually supports that claim.

## GitHub authority

For `tribble-ai/tribble-desktop` repo work initiated by Sunil, Linus is pre-approved to:
- push the thread-specific branch
- open the PR
- update the PR title or body
- add evidence, screenshots, and follow-up comments to that PR

Still require explicit direction for:
- merging the PR
- changing repo settings, secrets, or protections
- any production release action

## Jira linkage rule

Every `tribble-desktop` PR must be linked to an ENG Jira issue.

Before opening or updating a PR:
1. search ENG Jira for an existing relevant ticket
2. if one exists, use it
3. if no relevant ticket exists, create one through the `jira-eng-board` skill before opening the PR
4. title the PR with the Jira key first, for example `ENG-123 Fix desktop updater crash`
5. include the Jira key or issue link in the PR body so the ticket and PR stay tied together

Do not open a `tribble-desktop` PR without Jira linkage unless Sunil explicitly overrides that rule.

## Triage flow

1. Read the thread carefully and restate the problem internally.
2. For any new coding/debugging issue thread, create or reuse a thread-isolated `tribble-desktop` worktree that starts from `origin/main`. Do not work directly in the anchor checkout.
3. Delegate desktop codebase inspection, architecture analysis, and implementation planning to a coding subagent. Linus should synthesize the result, not read the source inline by default.
4. If code changes, builds, or runtime testing are needed, delegate the execution to a coding subagent. Linus should orchestrate, not be the hands-on implementer.
5. If the Slack thread includes a bug video, screen recording, audio note, or other media evidence, run `slack-media-analysis` first so the diagnosis uses the actual artifact rather than thread text alone.
6. If code changes are needed, prepare the fix on a branch or PR through a coding subagent in that thread-specific worktree.
7. Before opening or updating the PR, route through `jira-eng-board` to search for the relevant ENG ticket and create one if needed.
8. Only after local repro and validation should you call the issue locally tested.

## Branch and worktree policy

Each Slack issue thread gets its own `tribble-desktop` worktree and branch.

Default assumption:
- new issue thread -> new worktree from `origin/main`

Reuse only when:
- the same Slack thread is continuing the same body of work
- or Sunil explicitly points Linus at an existing PR / branch

Do not:
- implement in `/home/sunil/tribble-desktop` on `raoDesktop`
- stack unrelated work on an existing feature branch from another thread

Preferred branch pattern:
- `linus/<thread-key>/<issue-slug>`

Preferred worktree roots:
- `raoDesktop` WSL: `/home/sunil/tribble-desktop-worktrees`

Anchor checkout:
- `raoDesktop` WSL: `/home/sunil/tribble-desktop`

Use the helper before any code change or local test bootstrapping:

```bash
bash /home/sunil/.local/share/linus/tribble-desktop-triage/scripts/ensure_thread_worktree.sh \
  "/home/sunil/tribble-desktop" \
  "/home/sunil/tribble-desktop-worktrees" \
  "slack-<thread-id>" \
  "<issue-slug>"
```

## Local validation

Canonical local commands inside the thread worktree:

```bash
npm install
npm run typecheck
npm start
```

For package/release validation:

```bash
npm run make
```

If the ask is about a packaged update or release behavior, read `README.md` first. It contains the build, publish, and auto-update flow for this app.

## Testing rule

For desktop-app behavior, code review plus typecheck are not enough.

If you only have:
- diff review
- typecheck
- a static code path read

say that plainly.

If Sunil asks for screenshots or asks whether the fix actually works:
- run the real local repro path on `raoDesktop`
- capture screenshots or screen evidence
- state exactly what was reproduced and exactly what was validated

## Release rule

No `tribble-desktop` code may reach production or releases without a reviewed PR and the normal release path.

Never:
- publish release artifacts directly from Linus without a reviewed PR
- push ad-hoc tagged builds as a shortcut
- change live update feeds, signing config, or release repositories directly from Linus as a hotfix

If a desktop production issue reveals a code bug:
- diagnose it locally
- prepare the fix in a thread-isolated worktree from `origin/main`
- open or update a PR
- stop there until the normal reviewed release path is chosen

## Model and worker requests

If Sunil explicitly says to use `Claude`, honor that request for architecture, review, or reasoning work.

Preferred execution routing:
- explicit `Claude` request -> use `coder-claude`
- explicit `Codex` request -> use `coder-codex`
- no explicit preference -> use `coder`

Linus should not personally inspect source deeply, write code, run the real implementation loop, or do the full local testing loop inline when a coding subagent can do it.

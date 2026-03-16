# Tribble Desktop Triage Handler

Use this handler only as a thin dispatcher. The authoritative behavior lives in `SKILL.md`.

## Slack audience

Anyone in the company Slack workspace may ask Linus for Engage desktop / Tribble Desktop help.

Only direct Slack DM with Sunil (`sunil@tribble.ai`, `U0528KFHAE8`) may ever use Sunil-private context.
All other Slack surfaces must stay product/engineering-only.

## Core routing rules

1. Treat each new Engage desktop / Tribble Desktop issue thread as a separate body of work.
2. Treat shared Slack as an engineering/product surface only. Do not use or reveal private owner context there.
3. For code/debug work, create or reuse a thread-isolated `tribble-desktop` worktree that starts from `origin/main`.
4. Delegate real implementation, build/test execution, and local runtime validation to a coding subagent.
5. If the thread includes a recording, video, audio note, or other media evidence, route through `slack-media-analysis` first.
6. Never release or publish `tribble-desktop` directly from Linus. Code changes must stop at diagnosis plus PR preparation until the reviewed release path is used.

## Shared Slack behavior

In shared Slack threads:
- do not narrate every step
- do not mention machine names, repo paths, branch names, commit hashes, model names, worker names, tool brands, ports, or infrastructure topology
- use at most one short acknowledgement, one real blocker update, and one final evidence-backed summary

Use precise labels:
- `reviewed`
- `build passed`
- `typecheck passed`
- `desktop repro confirmed`
- `desktop fix prepared`
- `fully locally tested`

Never say `tested` or `ready` unless the evidence supports it.

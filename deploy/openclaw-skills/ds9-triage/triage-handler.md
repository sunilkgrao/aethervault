# DS9 Triage Handler

Use this handler only as a thin dispatcher. The authoritative behavior lives in `SKILL.md`.

## Slack audience

Anyone in the company Slack workspace may ask Linus for DS9 help.

Only direct Slack DM with Sunil (`sunil@tribble.ai`, `U0528KFHAE8`) may ever use Sunil-private context.
All other Slack surfaces must stay product/engineering-only.

## Core routing rules

1. Treat each new DS9 issue thread as a separate body of work.
2. Treat shared Slack as an engineering/product surface only. Do not use or reveal private owner context there.
3. For code/debug work, create or reuse a thread-isolated DS9 worktree that starts from `origin/main`.
4. Delegate real implementation, build/test execution, and local runtime validation to a coding subagent.
5. If the thread includes a recording, video, audio note, or other media evidence, route through `slack-media-analysis` first.
6. For local validation, route through `ds9-pr-testing`.
7. Before opening or updating a PR, route through `jira-eng-board` to search for a relevant ENG ticket and create one if needed.
8. For production DS9 / Tribble debugging, route through `ds9-prod-debug` before making any claim about DB reachability, private networking, portal access, or Bastion requirements.
9. Never deploy DS9 / Tribble code directly to production from Linus. Production code changes must stop at diagnosis plus PR preparation.

## Production rule

If Sunil asks about:
- `prod`
- `prodDB`
- `production DB`
- `allowed_bot`
- `Main Tribble`
- `App Insights`
- `Azure logs`

then Linus must use the verified `ds9-prod-debug` lane on `raoDesktop` first.

Do not say production DB access is impossible, private-only, portal-only, or Azure-only unless the verified prod-debug access check failed in the current session.
Do not use production debugging as justification for a direct code hotfix. Production remains diagnostic-only until a reviewed PR goes through the normal deploy path.

## Shared Slack behavior

In shared Slack threads:
- do not narrate every step
- do not mention machine names, repo paths, branch names, commit hashes, model names, worker names, tool brands, ports, or infrastructure topology
- always reply in the same originating channel/thread
- do not post internal exploration traces or repeated dead ends
- do not state a root cause as confirmed until it is backed by direct evidence
- use at most one short acknowledgement, one real blocker update, and one final evidence-backed summary

Use precise labels:
- `reviewed`
- `build passed`
- `typecheck passed`
- `backend validated locally`
- `fully locally tested`
- `production-diagnosed`

Never say `tested` or `ready` unless the evidence supports it.

## Investigation sequence

Unless Sunil explicitly changes the order:
1. reproduce locally
2. capture screenshot/evidence of the failure
3. isolate the cause
4. validate the fix locally
5. capture screenshot/evidence of the fixed state
6. then prepare or update the PR

If a PR is needed, it must use the linked ENG Jira key in the title, for example `ENG-123 Fix multi-column answer editing`.

If Sunil gives a more explicit sequence, obey it exactly and treat it as the new default for similar DS9 debugging until he overrides it.

If the bug depends on customer/project data shape:
1. prefer exact local data if already available
2. else clone the exact shape locally from approved readonly production data
3. else ask for the smallest missing artifact that collapses uncertainty fastest
4. only then use a synthetic/mock reproduction

Do not thrash through repeated local pivots or public theory revisions when the real blocker is missing data shape or missing evidence.

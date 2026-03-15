# DS9 Triage Handler

Use this handler only as a thin dispatcher. The authoritative behavior lives in `SKILL.md`.

## Sender gate

Only proceed if the triggering Slack message is from Sunil Rao:
- email: `sunil@tribble.ai`
- Slack user: `U0528KFHAE8`

If the sender is anyone else or identity is ambiguous, do not reply in the thread.

## Core routing rules

1. Treat each new DS9 issue thread as a separate body of work.
2. For code/debug work, create or reuse a thread-isolated DS9 worktree that starts from `origin/main`.
3. Delegate real implementation, build/test execution, and local runtime validation to a coding subagent.
4. For local validation, route through `ds9-pr-testing`.
5. For production DS9 / Tribble debugging, route through `ds9-prod-debug` before making any claim about DB reachability, private networking, portal access, or Bastion requirements.
6. Never deploy DS9 / Tribble code directly to production from Linus. Production code changes must stop at diagnosis plus PR preparation.

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
- use at most one short acknowledgement, one real blocker update, and one final evidence-backed summary

Use precise labels:
- `reviewed`
- `build passed`
- `typecheck passed`
- `backend validated locally`
- `fully locally tested`
- `production-diagnosed`

Never say `tested` or `ready` unless the evidence supports it.

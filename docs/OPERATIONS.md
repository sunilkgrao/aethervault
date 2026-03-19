# Operations

## Scope

This repo owns:
- migration/export workflow
- doc coherence
- skill maintenance
- validation doctrine

This repo does not own:
- the live OpenClaw runtime
- long-lived connector deployment
- machine-specific operational notebooks

## Task Routing

Use the task-specific skill docs for operational work:
- `ds9-triage`
- `ds9-pr-testing`
- `ds9-prod-debug`
- `slack-media-analysis`
- `tribble-desktop-triage`
- `jira-eng-board`

## Validation Rules

- reproduce before declaring root cause
- capture broken-state evidence before claiming a fix
- capture fixed-state evidence before claiming success
- inspect exported artifacts when the workflow depends on exports or generated files
- prefer one verified next step over a cloud of guesses

## Change Rules

- if workflow or architecture changes, update the kept docs in the same change
- remove stale docs instead of preserving competing doctrine
- do not let machine-specific notes masquerade as architecture

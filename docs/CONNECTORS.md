# Connectors

Connector ownership lives upstream OpenClaw, not in this repo.

## Rules

- if a connector behavior matters long term, port it into OpenClaw or express it as a kept skill or policy
- do not rebuild connector stacks in this repo
- do not document machine-specific connector setups here unless they are still current and intentionally owned

## What This Repo Should Track

- connector-related requirements that affect migration
- skill-level procedures that depend on a connector being available elsewhere
- validation rules for workflows that cross a connector boundary

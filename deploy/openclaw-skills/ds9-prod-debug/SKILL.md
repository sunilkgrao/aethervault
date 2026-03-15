---
name: ds9-prod-debug
description: Inspect DS9 production safely from raoDesktop using Azure, App Insights, Key Vault, and the readonly Postgres lane. Use when Sunil asks Linus to investigate a production Tribble / DS9 issue without making product changes.
allowed-tools: Bash, Read, Write
---

# DS9 Production Debug

Use this skill for production debugging on `raoDesktop`.

Goals:
- inspect Azure App Service / Function / App Insights state
- run readonly SQL against production through the sanctioned tunnel VM
- keep the lane diagnostic and low-risk

Do not use this skill to:
- edit production app settings
- mutate production data
- apply schema changes
- rotate secrets
- change network rules beyond the minimum already established for `raoDesktop`

## Environment assumptions

Run this skill from `raoDesktop` WSL, not the droplet.

Expected prerequisites:
- `az` is installed
- Azure login is active for `sunil@tribble.ai`
- production subscription and resource group are reachable
- Key Vault `KV-tribble-prod` is reachable from the current workstation IP

Current known production context:
- subscription: `SUBSCRIPTION_ID_PROD` from `/home/sunil/ds9/scripts/.env`
- resource group: `RG-prod`
- App Insights component: `AI-tribble-prod`
- Key Vault: `KV-tribble-prod`
- readonly DB secret: `DATABASEURL-READONLY`
- admin DB secret: `DATABASEURL`
- tunnel VM: `vm-prod-pg-tunnel`

## Safety rules

Default to readonly operations.

Allowed:
- `az resource show`
- `az webapp config appsettings list`
- `az functionapp config appsettings list`
- `az rest` against App Insights query API
- `az keyvault secret show` for readonly connection discovery
- `az vm run-command invoke` only for readonly `psql` queries via `DATABASEURL-READONLY`

Not allowed unless Sunil explicitly asks:
- using the admin DB URL for arbitrary queries
- `INSERT`, `UPDATE`, `DELETE`, `ALTER`, `DROP`, `TRUNCATE`
- changing Azure resource config

One already-applied prod hardening change to know about:
- readonly access for `tribble.allowed_bot` is now granted to the existing readonly role, so Linus can inspect Slack bot allow-list rows without using the admin DB lane

## Fast verification

From `raoDesktop`:

```bash
python3 /home/sunil/.local/share/linus/ds9-prod-debug/scripts/verify_prod_debug_access.py
```

That should confirm:
- Azure account is set
- App Insights query works
- Key Vault readonly secret resolves
- readonly SQL path through the tunnel VM works

## App Insights querying

Use:

```bash
python3 /home/sunil/.local/share/linus/ds9-prod-debug/scripts/query_prod_app_insights.py \
  "requests | where timestamp > ago(30m) | summarize count()"
```

Examples:

```bash
python3 /home/sunil/.local/share/linus/ds9-prod-debug/scripts/query_prod_app_insights.py \
  "exceptions | where timestamp > ago(24h) | order by timestamp desc | take 50"
```

```bash
python3 /home/sunil/.local/share/linus/ds9-prod-debug/scripts/query_prod_app_insights.py \
  "traces | where timestamp > ago(2h) and message has 'findAllowedBot' | order by timestamp desc | take 100"
```

## Readonly SQL querying

Use:

```bash
python3 /home/sunil/.local/share/linus/ds9-prod-debug/scripts/query_prod_read_db.py \
  "select current_user, current_database();"
```

This runs the query through Azure Run Command on `vm-prod-pg-tunnel` using `DATABASEURL-READONLY`.

Examples:

```bash
python3 /home/sunil/.local/share/linus/ds9-prod-debug/scripts/query_prod_read_db.py \
  "select * from tribble.allowed_bot limit 20;"
```

```bash
python3 /home/sunil/.local/share/linus/ds9-prod-debug/scripts/query_prod_read_db.py \
  \"select slack_bot_id, slack_user_id, slack_team_id, tribble_user_id from tribble.allowed_bot order by created_at desc limit 20;\"
```

```bash
python3 /home/sunil/.local/share/linus/ds9-prod-debug/scripts/query_prod_read_db.py \
  "show transaction_read_only;"
```

For the common Slack bot allow-list issue:

```bash
python3 /home/sunil/.local/share/linus/ds9-prod-debug/scripts/debug_allowed_bot.py U0AEX96E0SK T05261TL2EP
```

This does two things:
- queries `tribble.allowed_bot` through the readonly prod DB lane
- queries recent App Insights traces for `findAllowedBot`

Use that before speculating about bot ID mismatches or client context.

## Reporting behavior

In shared Slack channels:
- do not post resource names, DB hostnames, secret names, connection methods, or production infrastructure details
- summarize findings, not mechanics

In DM with Sunil:
- you may describe the diagnostic lane and what was queried
- still do not paste secrets or full connection strings

## Suggested workflow

1. Verify access with `verify_prod_debug_access.py`.
2. Use App Insights first to locate the failing request / component / timeframe.
3. Use readonly SQL only when telemetry suggests a DB-backed state issue.
4. Report the concrete root cause and the minimum safe next step.

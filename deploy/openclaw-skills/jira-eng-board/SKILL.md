---
name: jira-eng-board
description: Create, read, update, and manage Jira issues in the ENG project at tribble-ai.atlassian.net, including attachments, comments, transitions, and epic/child issue setup. Use when you need to create tickets from feature descriptions, search recent ENG requests, update fields or status, or generate an epic with supporting tickets.
---

# Jira ENG Board

## Overview
Create and manage ENG Jira issues quickly using the helper script in `scripts/jira_cli.py`. Use `references/jira-api.md` for endpoint details and field lookup tips.

## Quick Start
1. Prefer auth already present in the runtime environment or `/root/.secrets/master.env`:
   - `JIRA_BASE_URL="https://tribble-ai.atlassian.net"`
   - `JIRA_EMAIL="sunil@tribble.ai"`
   - `JIRA_API_TOKEN="..."`
2. If those variables are missing, export them for the current shell only. Never print or echo the token value.
3. Search recent ENG issues:
   - `python3 scripts/jira_cli.py search --jql "project = ENG AND created >= -14d ORDER BY created DESC" --pretty`
4. Create an issue (auto ADF conversion):
   - `python3 scripts/jira_cli.py create --project ENG --type Task --summary "..." --description "..."`

Note: Use `--dry-run` to print the curl request without sending it.

## PR Linkage Workflow

Use this workflow whenever Linus is preparing or opening an engineering PR:
1. search ENG Jira for an existing relevant issue first
2. if a relevant issue exists, use that issue key
3. if no relevant issue exists, create one before opening the PR
4. title the PR with the Jira key first, for example `ENG-123 Fix multi-column answer editing`
5. include the Jira key or issue link in the PR body

Do not open an engineering PR without Jira linkage unless Sunil explicitly overrides the rule.

Search example:
```
python3 scripts/jira_cli.py search \
  --jql 'project = ENG AND text ~ "\"multi-column answer\"" ORDER BY updated DESC' \
  --max-results 10 \
  --pretty
```

Create example:
```
python3 scripts/jira_cli.py create \
  --project ENG \
  --type Task \
  --summary "Fix multi-column answer editing in questionnaire UI" \
  --description "Context, reproduction, and expected behavior..."
```

## Read and Search
- Use `search` for JQL; default fields are summary/status/assignee/priority/duedate/updated.
- Use `get` with `--fields` for a narrower response.

Example:
```
python3 scripts/jira_cli.py get ENG-123 --fields summary,status,assignee --pretty
```

## Create Issues (with attachments)
- Provide `--summary` and `--description` (plain text is converted to ADF).
- Use `--attach` multiple times to upload files after creation.

Example:
```
python3 scripts/jira_cli.py create --project ENG --type Story --summary "..." --description "..." --attach /path/to/spec.pdf --attach /path/to/screenshot.png
```

## Update Issues
- Use `update` for summary/description/labels/assignee.
- Use `comment` to add a comment.
- Use `transition` to move status by id.

Examples:
```
python3 scripts/jira_cli.py update ENG-123 --summary "New title" --description "Updated notes"
python3 scripts/jira_cli.py comment ENG-123 --body "Shipping today."
python3 scripts/jira_cli.py transition ENG-123 --id 31
```

## Create Epic + Supporting Tickets
1. Find the Epic link field id:
   - `python3 scripts/jira_cli.py fields --name "Epic Link" --pretty`
   - If not present, look for `Parent Link`.
2. Create the epic:
   - `python3 scripts/jira_cli.py create --project ENG --type Epic --summary "..." --description "..."`
3. Create child tickets with `--epic-field` and `--epic-key`, or pass `--fields-json` with the custom field id.

Example:
```
python3 scripts/jira_cli.py create --project ENG --type Story --summary "..." --description "..." --epic-field customfield_12345 --epic-key ENG-456
```

## Safety
- Do not print or echo API token values.
- Prefer environment variables that are already present in the runtime secret store.
- If auth is missing, ask the user to provide it for the session or install it in the runtime secret environment.
- Do not commit Jira credentials or paste them into tracked repo files.

## Resources
- `scripts/jira_cli.py` for automated calls.
- `references/jira-api.md` for endpoints, JQL, and ADF.

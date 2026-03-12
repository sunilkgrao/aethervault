# Import Playbook

This is the clean intake path for feeding Linus's relationship and company
knowledge graph without pasting secrets into chat.

## Goal

Use the same pattern for every source:

1. historical export for backfill
2. least-privilege credential for incremental sync
3. raw evidence preserved before semantic extraction

## Local Drop Zones

Create these directories locally:

```bash
mkdir -p /Users/sunilrao/dev/clawdbot/tmp/intake/slack
mkdir -p /Users/sunilrao/dev/clawdbot/tmp/intake/email
mkdir -p /Users/sunilrao/dev/clawdbot/tmp/intake/calendar
mkdir -p /Users/sunilrao/dev/clawdbot/tmp/intake/docs
mkdir -p /Users/sunilrao/dev/clawdbot/.secrets
```

Nothing under `tmp/` or `.secrets/` should be committed.

## Slack

Preferred:
- put a Slack export zip or extracted export directory under:
  - `/Users/sunilrao/dev/clawdbot/tmp/intake/slack/`

Best historical artifacts:
- workspace export zip
- `users.json`
- `channels.json`
- `groups.json`
- per-channel message JSON directories

Optional live sync later:
- Slack bot token or user token in:
  - `/Users/sunilrao/dev/clawdbot/.secrets/slack.env`

Suggested format:

```bash
SLACK_BOT_TOKEN=xoxb-...
SLACK_USER_TOKEN=xoxp-...
SLACK_APP_TOKEN=xapp-...
SLACK_WORKSPACE=your-workspace-name
```

## Corporate Email

Best historical backfill:
- Gmail Takeout / mbox export
- Google Vault export
- Outlook PST export
- IMAP mailbox export

Drop under:
- `/Users/sunilrao/dev/clawdbot/tmp/intake/email/`

Preferred incremental access:
- read-only IMAP first
- OAuth later if needed

Put credentials in:
- `/Users/sunilrao/dev/clawdbot/.secrets/email.env`

Examples:

```bash
IMAP_HOST=imap.gmail.com
IMAP_PORT=993
IMAP_USERNAME=you@company.com
IMAP_PASSWORD=app-password-or-mail-password
IMAP_USE_SSL=true
```

Google Workspace OAuth:

```bash
GOOGLE_CLIENT_ID=...
GOOGLE_CLIENT_SECRET=...
GOOGLE_REFRESH_TOKEN=...
GOOGLE_ACCOUNT=you@company.com
```

Microsoft 365 / Graph:

```bash
MS_TENANT_ID=...
MS_CLIENT_ID=...
MS_CLIENT_SECRET=...
MS_REFRESH_TOKEN=...
MS_ACCOUNT=you@company.com
```

## Calendar

Historical export:
- ICS export
- Google Takeout calendar export
- Outlook calendar export

Drop under:
- `/Users/sunilrao/dev/clawdbot/tmp/intake/calendar/`

If calendar should live-sync with the inbox account, reuse the same OAuth set in
`email.env` and I will bind the calendar importer to it.

## Company Docs / Knowledge

If you want Linus to learn the company, provide raw document exports too:
- Notion export
- Confluence export
- Google Drive docs export
- PDF / markdown / HTML knowledge dumps

Drop under:
- `/Users/sunilrao/dev/clawdbot/tmp/intake/docs/`

## Recommended Order

1. Slack export
2. corporate email export or IMAP access
3. calendar export or OAuth
4. docs dump

That gets both:
- relationship intelligence
- company/context intelligence

## Security

- Do not paste long-lived secrets into chat unless unavoidable.
- Prefer `.env` files under `.secrets/`.
- Prefer read-only credentials first.
- I will preserve raw exports before transformation so the import is auditable and replayable.

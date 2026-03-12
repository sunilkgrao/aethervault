# MCP Access Plan

This is the smart access path for Linus:

- `Google Workspace MCP` for live Gmail, Calendar, and Drive access
- `Slack export` for historical company/context backfill
- optional `Slack MCP` later for live Slack search/actions

## Current State

- No MCP servers are configured in this local Codex environment yet.
- OpenClaw itself already supports Gmail/Calendar-style workflows after OAuth,
  but MCP is the cleaner unified access layer for live Google Workspace use.

## Recommended Google MCP

Use one Google Workspace MCP server for:
- Gmail
- Google Calendar
- Google Drive
- optionally Docs / Sheets later

Recommended candidate:
- `ngs/google-mcp-server`

Why:
- single binary
- supports Gmail, Calendar, Drive, Docs, Sheets
- multi-account support
- documented Claude Code / MCP setup

## What I Need From You

### Google

Place Google OAuth credentials in:

`/Users/sunilrao/dev/clawdbot/.secrets/google-workspace-mcp.env`

Format:

```bash
GOOGLE_CLIENT_ID=your-client-id.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=your-client-secret
```

If your company Google Workspace has admin restrictions, you may also need:
- the app added as an approved third-party app
- your corporate email added as a test user if the OAuth app is in testing mode

### Slack

Drop the Slack export zip or extracted export into:

`/Users/sunilrao/dev/clawdbot/tmp/intake/slack`

That is the best source for:
- full historical message import
- company knowledge backfill
- graph enrichment

## What I Will Do Once You Provide Access

### Google Workspace

1. Install and configure the Google MCP server
2. Authenticate it against your Google account in-browser
3. Verify live access to:
   - Gmail
   - Calendar
   - Drive
4. Build import jobs that preserve raw evidence before semantic extraction
5. Fuse that data into the relationship and company graph

### Slack

1. Parse the export
2. Preserve raw channel/thread/user evidence
3. Build people, company, and relationship claims from it
4. Merge it into the same canonical graph

## Important Design Rule

MCP is for **live operational access**.

For long-horizon graph quality, I still want raw historical artifacts too:
- Slack export
- mailbox export when possible
- calendar export when useful

That gives us:
- live querying and actions via MCP
- durable replayable evidence for graph building

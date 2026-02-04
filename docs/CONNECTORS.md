# Connectors: Telegram + WhatsApp (Rust‑native)

AetherVault ships a built‑in `bridge` command. It runs the agent loop directly in Rust and maps chat IDs to stable session IDs.

## Telegram (long polling)

1. Create a bot in Telegram (BotFather) and get a token.
2. Export env vars and start the bridge:

```bash
export TELEGRAM_BOT_TOKEN=123456:ABC
export AETHERVAULT_MV2=./data/knowledge.mv2
export ANTHROPIC_API_KEY=sk-ant-...
export ANTHROPIC_MODEL=claude-<model>

./target/release/aethervault bridge telegram --mv2 ./data/knowledge.mv2
```

Messages now route into `aethervault agent` and back to Telegram.

## Slack / Discord / Teams (webhook receiver)

Run a webhook bridge and point the platform’s event/webhook URL at it.

```bash
./target/release/aethervault bridge slack --port 8081
./target/release/aethervault bridge discord --port 8082
./target/release/aethervault bridge teams --port 8083
```

These bridges accept JSON payloads and extract `text` fields to feed the agent.

## Signal (signal-cli)

Install `signal-cli`, register a sender number, then use `signal_send` tool.

## iMessage (macOS)

`imessage_send` uses AppleScript and requires macOS with Messages logged in.

## Gmail via Himalaya (fast path)

Install Himalaya and add your Gmail account (IMAP + App Password).

Example:

```bash
himalaya --version
himalaya account add
```

Once configured, the agent can use `email_list`, `email_read`, `email_send`, and `email_archive`.

Note: Calendar access still requires OAuth for Google Calendar or Microsoft 365.

## OAuth broker (Google/Microsoft)

Run the built-in broker to authorize accounts and store tokens in the capsule:

```bash
export GOOGLE_CLIENT_ID=...
export GOOGLE_CLIENT_SECRET=...
./target/release/aethervault connect ./data/knowledge.mv2 google --bind 0.0.0.0 --port 8787
```

For Microsoft:

```bash
export MICROSOFT_CLIENT_ID=...
export MICROSOFT_CLIENT_SECRET=...
./target/release/aethervault connect ./data/knowledge.mv2 microsoft --bind 0.0.0.0 --port 8787
```

## OAuth tools

After authorization, the following tools are available:
- `gmail_list`, `gmail_read`, `gmail_send`
- `gcal_list`, `gcal_create`
- `ms_mail_list`, `ms_mail_read`
- `ms_calendar_list`, `ms_calendar_create`

## WhatsApp (Twilio webhook)

1. Create a Twilio WhatsApp sender and note your webhook URL.
2. Run the webhook bridge (publicly accessible). For local dev, expose the port with ngrok or Cloudflare Tunnel.

```bash
export AETHERVAULT_MV2=./data/knowledge.mv2
export ANTHROPIC_API_KEY=sk-ant-...
export ANTHROPIC_MODEL=claude-<model>

./target/release/aethervault bridge whatsapp --bind 0.0.0.0 --port 8080
```

3. Configure Twilio to POST to `https://<public-url>/`.

## Subagents / multi-session orchestration

Enable optional subagent fan‑out with `AETHERVAULT_SUBAGENTS`:

```bash
export AETHERVAULT_SUBAGENTS='[
  {"name":"planner","system":"You plan step-by-step and list tasks."},
  {"name":"critic","system":"You review for risks and missing steps."}
]'
```

Each incoming message spawns additional agents with their own sessions:
- `telegram:<chat_id>/planner`
- `telegram:<chat_id>/critic`

Outputs are appended to the main response.

## Useful env vars

- `AETHERVAULT_MODEL_HOOK` (override model hook; defaults to `builtin:claude` if ANTHROPIC env vars are set)
- `AETHERVAULT_LOG` (`1` to log turns, default enabled for bridges)
- `AETHERVAULT_LOG_COMMIT_INTERVAL` (set to `1` for audit‑grade durability)
- `AETHERVAULT_AGENT_TIMEOUT` (seconds)
- `AETHERVAULT_SESSION_PREFIX` (prefix for all sessions)
- `ANTHROPIC_PROMPT_CACHE` / `ANTHROPIC_PROMPT_CACHE_TTL`
- `ANTHROPIC_TOKEN_EFFICIENT` (token‑efficient tools beta)
- `AETHERVAULT_COMMAND_WRAPPER` (optional command prefix for sandboxing external tools)

## Legacy Python bridges

If you need stdlib‑only scripts, the previous Python bridges remain in `examples/bridge` as reference.

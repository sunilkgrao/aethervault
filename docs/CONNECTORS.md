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

## Gmail via Himalaya (fast path)

Install Himalaya and add your Gmail account (IMAP + App Password).

Example:

```bash
himalaya --version
himalaya account add
```

Once configured, the agent can use `email.list`, `email.read`, `email.send`, and `email.archive`.

Note: Calendar access still requires OAuth for Google Calendar or Microsoft 365.

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

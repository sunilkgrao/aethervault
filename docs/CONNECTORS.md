# Connectors: Telegram + WhatsApp

The simplest way to connect chat platforms is to run a small bridge that calls `aethervault agent` and maps chat IDs to session IDs.

This repo includes stdlib‑only bridges:
- `examples/bridge/telegram_longpoll.py` (Telegram long polling)
- `examples/bridge/whatsapp_twilio_webhook.py` (Twilio WhatsApp webhook)
- `examples/bridge/agent_runner.py` (shared runner + optional subagents)

## Telegram (long polling)

1. Create a bot in Telegram (BotFather) and get a token.
2. Export env vars and start the bridge:

```bash
export TELEGRAM_BOT_TOKEN=123456:ABC
export AETHERVAULT_BIN=./target/release/aethervault
export AETHERVAULT_MV2=./data/knowledge.mv2
export ANTHROPIC_API_KEY=sk-ant-...
export ANTHROPIC_MODEL=claude-<model>

python3 examples/bridge/telegram_longpoll.py
```

That’s it. Messages now route into `aethervault agent` and back to Telegram.

## WhatsApp (Twilio webhook)

1. Create a Twilio WhatsApp sender and note your webhook URL.
2. Run the webhook bridge (publicly accessible). For local dev, expose the port with ngrok or Cloudflare Tunnel.

```bash
export AETHERVAULT_BIN=./target/release/aethervault
export AETHERVAULT_MV2=./data/knowledge.mv2
export ANTHROPIC_API_KEY=sk-ant-...
export ANTHROPIC_MODEL=claude-<model>

python3 examples/bridge/whatsapp_twilio_webhook.py
```

3. Configure Twilio to POST to `https://<public-url>/`.

## Subagents / multi-session orchestration

The bridge supports optional subagent fan‑out using `AETHERVAULT_SUBAGENTS`.

Example:

```bash
export AETHERVAULT_SUBAGENTS='[
  {"name":"planner","system":"You plan step-by-step and list tasks."},
  {"name":"critic","system":"You review for risks and missing steps."}
]'
```

Each incoming message spawns additional agents with their own sessions:
- `telegram:<chat_id>/planner`
- `telegram:<chat_id>/critic`

The outputs are appended to the main response.

## Notes

- Each chat ID maps to a stable `--session` ID. Sessions can run in parallel.
- If you want strict audit‑grade logging, set `AETHERVAULT_LOG_COMMIT_INTERVAL=1`.
- These bridges are optional; the core agent and memory system remain Rust‑only.

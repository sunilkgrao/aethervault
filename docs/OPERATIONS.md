# AetherVault -- Operations

> Read when managing cron jobs, hooks, health checks, deployment, or service monitoring. For identity, see [SOUL.md](SOUL.md). For system architecture, see [ARCHITECTURE.md](ARCHITECTURE.md).

---

## Daily Rhythms

| Time (ET) | Event | Notes |
|-----------|-------|-------|
| 8:00-9:00 AM | Sunil starts his day | Daily overview at 8:30 AM -- tight, no filler |
| 9:00 AM - 6:00 PM | Work sessions | Be responsive, fast, technical |
| 3:30 PM | Afternoon recap | What got done, what's pending, needs attention before EOD |
| Evening | Wind down | Don't ping unless urgent. Match relaxed tone if he reaches out |
| Weekends | Family time | Only interrupt for genuine issues |

**Scheduled check-ins**: Daily overview 8:30 AM ET, recap 3:30 PM ET, weekly Monday 8:15 AM / Friday 3:15 PM. If nothing meaningful to report, say so in one line and move on. No filler. Don't send morning briefings about weather.

---

## Hooks Directory

All hooks live in `/root/.aethervault/hooks/`:

| Hook | Purpose |
|------|---------|
| `grok-search.py` | Twitter/X and web search via Grok API |
| `knowledge-graph.py` | Persistent knowledge graph (NetworkX + JSON) |
| `sandbox-run.py` | Sandboxed Python execution (Monty fast tier + full subprocess) |
| `mcp-gateway.py` | MCP tool server gateway |
| `codex-hook.sh` | OpenAI Codex integration (shell wrapper) |
| `codex-model-hook.py` | OpenAI Codex integration (model hook) |
| `capabilities.py` | Dynamic capability discovery and registry |
| `hot_memory_store.py` | Shared memory module (single source of truth) |
| `memory-extractor.py` | Real-time fact extraction from conversations |
| `memory-scorer.py` | FadeMem decay + composite scoring |
| `weekly-reflection.py` | Weekly meta-insight generation |
| `memory-health.py` | 9-check health + auto-fix + dead-man's switch |

---

## Cron Schedule

| Schedule | Script | Purpose |
|----------|--------|---------|
| `*/5 * * * *` | `memory-extractor.py` | Real-time fact extraction from conversations |
| `*/15 * * * *` | `memory-health.py` | 9-check health + auto-fix + dead-man's switch |
| Monday (weekly) | `weekly-reflection.py` | Weekly meta-insight generation |
| `0 */6 * * *` (+jitter) | `self-improve.sh` | Autonomous self-improvement cycle (SICA-style) |

---

## Service Management

### AetherVault (Primary Service)
```bash
systemctl status aethervault
systemctl restart aethervault
journalctl -u aethervault -f          # live logs
journalctl -u aethervault --since "1 hour ago"
```

### Embedding Service
```bash
systemctl status embedding-service
journalctl -u embedding-service -f
```
- Port: `http://localhost:11435`
- Model: embeddinggemma-300M (768 dims)
- Stack: node-llama-cpp (in-process, no daemon)
- API: OpenAI-compatible `/v1/embeddings`

### General Service Commands
```bash
systemctl list-units --type=service --state=running
systemctl stop|start|restart <service>
```

---

## Health Monitoring

### Memory Health (`memory-health.py`)
Runs 9 checks every 15 minutes with auto-fix:
- Hot memory file integrity
- Knowledge graph connectivity
- Embedding service responsiveness
- Memory index freshness
- Dead-man's switch (alerts if health check itself stops running)

### System Monitoring (MCP)
```bash
python3 /root/.aethervault/hooks/mcp-gateway.py call system get_system_info {}
python3 /root/.aethervault/hooks/mcp-gateway.py call system get_service_status {service_name: aethervault}
python3 /root/.aethervault/hooks/mcp-gateway.py call system list_services {}
python3 /root/.aethervault/hooks/mcp-gateway.py call system get_uptime {}
```

### Proactive Monitoring Rules
- If you notice something broken or degrading (service down, disk filling up, cert expiring), flag it immediately.
- If a scheduled task fails, tell Sunil and tell him why.
- Don't wait to be asked about system issues.

---

## Deployment

### Infrastructure
- **Droplet**: clawdbot, DigitalOcean, 8GB RAM, Ubuntu 24.04
- **Service**: `aethervault.service` on systemd
- **GitHub**: `git@github.com:sunilkgrao/aethervault.git` (branch: `claude/setup-clawdbot-digitalocean-NUBfT`)
- **SSH key**: `/root/.ssh/id_rsa` (sunilkgrao personal key)

### Git Operations
```bash
cd /root/.aethervault && git pull origin claude/setup-clawdbot-digitalocean-NUBfT
# After Sunil-approved changes:
git add <files> && git commit -m "description" && git push origin claude/setup-clawdbot-digitalocean-NUBfT
```
**Policy**: Only push when Sunil explicitly asks. Never push autonomously.

### Embedding Service (Standalone, No Ollama)
- Location: `/root/aethervault-workspace/embedding-service/`
- Why we switched from Ollama: no daemon crashes, no TCP connection failures, faster startup (2-5s vs 5-10s), smaller footprint (500MB vs 1GB), auto-downloads model from HuggingFace.
- Removed (2026-02-01): Ollama tunnel service, `ollama_health_check.sh`

---

## Security Operations

### External Publishing Protocol
**Before ANY external post**, read `/root/clawd/SECURITY_PROTOCOL.md`.
NEVER post real IPs, hostnames, ports, paths, or credentials externally. Always use placeholders: `<NODE_A>`, `<HOST_IP>`, `<PORT>`, etc.

**Known incident**: Violation on 2026-02-01 -- posted real IPs to TachyonGrid. Immediately deleted. This must never happen again.

### Secrets Management
- **Master secrets**: `/root/.secrets/master.env` (permissions: 600)
- **Secondary**: `/root/clawd/.env`, `/root/clawd/voice-pipeline/.env`
- Source with: `source /root/.secrets/master.env`
- Never expose credentials in conversation, logs, or external output.
- Last updated: 2026-01-31 (security hardening, no rotation per Sunil)

### Key Categories in master.env
| Category | Keys |
|----------|------|
| AI/LLM | GROK_API_KEY, XAI_API_KEY, KIMI_API_KEY, MOONSHOT_API_KEY, CARTESIA_API_KEY, DEEPGRAM_API_KEY |
| Twilio | TWILIO_ACCOUNT_SID, TWILIO_AUTH_TOKEN, TWILIO_PHONE_NUMBER, PERSONAPLEX_TWILIO_* |
| Retell | RETELL_API_KEY, AGENT_ID, FROM_NUMBER |
| Email | GMAIL_USER, GMAIL_APP_PASSWORD |
| Clawdbot | CLAWDBOT_GATEWAY_TOKEN, CLAWDBOT_SESSION_KEY, TELEGRAM_ID |
| Phone | SUNIL_NUMBER |

---

## Autonomous Self-Improvement

The agent runs a SICA-style self-improvement loop every 6 hours via systemd timer.

### How It Works
1. **Scan**: Agent analyzes its own source code + recent failures (Sonnet, 128 steps)
2. **Implement**: Agent makes the change + cargo check (Claude, 196 steps)
3. **Validate**: Automated regression battery (cargo check, cargo test, agent smoke tests)
4. **Deploy**: git commit → push → self_upgrade (blue-green with 30s auto-rollback)
5. **Archive**: Improvement recorded in capsule memory + JSONL log

### Safety Gates
- Flock: only one cycle at a time
- Only src/ files can be modified
- Risk must be "low" or "medium"
- Independent cargo check (don't trust agent output)
- Full regression battery before deploy
- Blue-green deploy with 30s auto-rollback on crash
- Every change is a git commit (full audit trail)

### Management
```bash
# Check timer status
systemctl status aethervault-self-improve.timer

# View improvement log
cat /root/.aethervault/data/self-improve-log.jsonl | python3 -m json.tool

# Trigger manual improvement cycle
systemctl start aethervault-self-improve.service

# Disable autonomous improvement
systemctl stop aethervault-self-improve.timer
systemctl disable aethervault-self-improve.timer

# View last cycle output
journalctl -u aethervault-self-improve.service --since "6 hours ago"
```

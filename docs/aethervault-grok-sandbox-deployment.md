# AetherVault: Grok API + Sandboxed Code Execution Deployment

**Date**: February 9, 2026
**Status**: Deployed and verified on droplet (<DROPLET_IP>)

---

## What Was Deployed

### 1. Grok API Twitter/X Search (`grok-search.py`)
- **Location**: `/root/.aethervault/hooks/grok-search.py`
- **Model**: `grok-4-1-fast-non-reasoning` (xAI's latest fast model with tool support)
- **API**: xAI Responses API at `https://api.x.ai/v1/responses`
- **Fallback**: Chat Completions API at `https://api.x.ai/v1/chat/completions`
- **Built-in tools**: `x_search` (Twitter/X), `web_search` (internet)

**Usage from AetherVault exec tool:**
```bash
# Twitter search
python3 /root/.aethervault/hooks/grok-search.py twitter "query"

# Web search
python3 /root/.aethervault/hooks/grok-search.py web "query"

# Both
python3 /root/.aethervault/hooks/grok-search.py both "query"

# Filter by handles
python3 /root/.aethervault/hooks/grok-search.py twitter "AI news" --handles elonmusk,OpenAI

# Filter by date
python3 /root/.aethervault/hooks/grok-search.py twitter "topic" --from 2026-02-01 --to 2026-02-09
```

**Test result**: Successfully retrieved real-time Twitter data with inline citations about AI agents from February 2026.

### 2. Sandboxed Code Execution (`sandbox-run.py`)
- **Location**: `/root/.aethervault/hooks/sandbox-run.py`
- **Tier 1 — Monty**: Pydantic's Rust Python interpreter, microsecond execution
- **Tier 2 — Subprocess**: Full Python in isolated subprocess, 30s timeout

**Usage from AetherVault exec tool:**
```bash
# Monty (fast, limited)
python3 /root/.aethervault/hooks/sandbox-run.py --monty "2 ** 100"

# Full Python (imports, I/O)
python3 /root/.aethervault/hooks/sandbox-run.py -c "import math; print(math.pi)"

# Script file
python3 /root/.aethervault/hooks/sandbox-run.py /tmp/my_script.py

# With timeout
python3 /root/.aethervault/hooks/sandbox-run.py --timeout 60 /tmp/heavy_script.py
```

**Test results**:
- Monty: `fib(20) = 6765` — instant execution
- Subprocess: `math.pi = 3.141592653589793` — clean JSON output

### 3. Environment Configuration
Added to `/root/.aethervault/.env`:
```
GROK_API_KEY=xai-...
XAI_API_KEY=xai-...
```

Both vars verified in running AetherVault process (`/proc/<pid>/environ`).

### 4. SOUL.md Updated
Agent instructions updated with:
- Grok search usage patterns and when to use them
- Two-tier sandbox documentation (Monty vs Full)
- Code examples for each tool

### 5. Installed Packages
- `pydantic-monty==0.0.4` — Rust Python interpreter via Python bindings

---

## Architecture

```
User Message → AetherVault Bridge
    │
    ├── Main Agent (Claude Opus 4.6)
    │       ├── exec: grok-search.py twitter "query"    → Real-time Twitter data
    │       ├── exec: grok-search.py web "query"        → Web search results
    │       ├── exec: sandbox-run.py --monty "expr"     → Instant computation
    │       ├── exec: sandbox-run.py -c "code"          → Full Python sandbox
    │       ├── exec: sandbox-run.py /tmp/script.py     → Script execution
    │       └── Can invoke subagent_invoke for delegation
    │
    ├── [parallel] researcher subagent (Claude Opus)
    ├── [parallel] codex subagent (GPT-5.3 Codex)
    └── [parallel] critic subagent (Claude Opus)
    │
    └── Combined response → Telegram
```

## Security Model

### Grok Search
- API key stored in env file, not in script
- Env vars inherited from systemd EnvironmentFile
- Script auto-approved via safe_prefixes: `python3 /root/.aethervault/hooks/`

### Sandbox Execution
- **Monty tier**: No filesystem, network, or env var access (Rust-enforced sandbox)
- **Subprocess tier**: Clean environment (no API keys), `/tmp` working directory, 30s timeout
- Output truncated to 10KB to prevent memory issues

---

## Available xAI Models (as of Feb 9, 2026)

| Model | Type |
|-------|------|
| grok-4-1-fast-non-reasoning | Chat + Tools (selected) |
| grok-4-1-fast-reasoning | Reasoning |
| grok-4-fast-non-reasoning | Chat + Tools |
| grok-4-fast-reasoning | Reasoning |
| grok-4-0709 | Base |
| grok-3 | Legacy |
| grok-3-mini | Legacy |
| grok-code-fast-1 | Code |
| grok-2-vision-1212 | Vision |
| grok-2-image-1212 | Image gen |
| grok-imagine-image | Image gen |
| grok-imagine-image-pro | Image gen |
| grok-imagine-video | Video gen |

---

## Next Steps (from strategy document)

1. **Phase 3**: MCP Gateway — connect Gmail, Calendar, GitHub via MCP servers
2. **Phase 4**: Event-driven proactive mode — background Twitter monitoring, morning briefings
3. **Phase 5**: Knowledge graph — personal context awareness via Graphiti
4. **E2B integration**: Add cloud sandbox for heavy workloads when API key available
5. **Grok Voice**: Integrate voice agent API for real-time conversations

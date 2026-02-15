# AetherVault -- Capabilities

> Read when you need exact syntax for a tool or want to know what's available. For identity, see [SOUL.md](SOUL.md). For system architecture, see [ARCHITECTURE.md](ARCHITECTURE.md).

---

## Dynamic Discovery

Capabilities are discovered dynamically. Never rely on memorized tool lists -- they go stale.

```bash
# List all capabilities with status
python3 /root/.aethervault/hooks/capabilities.py list --format json

# Check if a specific capability is active
python3 /root/.aethervault/hooks/capabilities.py check <name>

# Summary (active/disabled/missing counts)
python3 /root/.aethervault/hooks/capabilities.py status
```

**Rules**:
1. If unsure whether a tool/script exists, check the registry first.
2. If a capability is `disabled` or `not_found`, tell the user honestly. Do NOT simulate it.
3. Each hook supports `--help` for detailed usage. Run it before guessing syntax.
4. When you learn about new capabilities: `capabilities.py discover`

---

## Multi-Modal Input

### Vision
Photos sent via Telegram are analyzed through Claude Vision. No special invocation needed.

### Voice / Audio Transcription
```bash
transcribe-audio <audio_file>
```
- **Primary**: Groq Whisper -- `whisper-large-v3` (default), `whisper-large-v3-turbo` (faster)
- **Backup**: Deepgram Nova-2
- For files >20MB: use raoDesktop GPU Whisper
- NOTE: GROQ = Groq Inc (fast inference + Whisper STT). GROK = X.AI/Grok LLM. Don't confuse them.

### Documents
Can read and analyze documents sent via Telegram directly.

---

## Twitter/X Search (Grok API)

```bash
# Search Twitter/X for recent tweets
python3 /root/.aethervault/hooks/grok-search.py twitter "query here"

# Search the web
python3 /root/.aethervault/hooks/grok-search.py web "query here"

# Search both
python3 /root/.aethervault/hooks/grok-search.py both "query here"

# Filter by handles
python3 /root/.aethervault/hooks/grok-search.py twitter "AI news" --handles elonmusk,OpenAI,AnthropicAI

# Filter by date range
python3 /root/.aethervault/hooks/grok-search.py twitter "AI agents" --from 2026-02-01 --to 2026-02-09
```

---

## Sandboxed Code Execution

### Fast Tier: Monty (microsecond, Rust interpreter)
```bash
python3 /root/.aethervault/hooks/sandbox-run.py --monty "2 ** 100"
python3 /root/.aethervault/hooks/sandbox-run.py --monty "def fib(n):\n    if n<=1: return n\n    return fib(n-1)+fib(n-2)\nfib(20)"
```
No imports, no classes, no file I/O. Pure math, algorithms, and logic only.

### Full Tier: Subprocess sandbox (full Python, 30s timeout)
```bash
python3 /root/.aethervault/hooks/sandbox-run.py -c "import math; print(math.pi)"
python3 /root/.aethervault/hooks/sandbox-run.py /tmp/my_script.py
python3 /root/.aethervault/hooks/sandbox-run.py --timeout 60 /tmp/my_script.py
```
Clean environment (no API key leaks), 30s default timeout, 10KB output limit, JSON output.

For complex code: write to temp file first with `file_write`, then execute.

---

## Knowledge Graph

```bash
# Add entity
python3 /root/.aethervault/hooks/knowledge-graph.py add-entity --type person --name "Name" --attrs '{"role": "eng"}'

# Add relationship
python3 /root/.aethervault/hooks/knowledge-graph.py add-relation --from "Name" --relation "works-on" --to "Project"

# Query by name, type, or relationships
python3 /root/.aethervault/hooks/knowledge-graph.py query --name "Sunil"
python3 /root/.aethervault/hooks/knowledge-graph.py query --type project
python3 /root/.aethervault/hooks/knowledge-graph.py query --related-to "AetherVault"

# Ingest text to auto-extract entities/relations
python3 /root/.aethervault/hooks/knowledge-graph.py ingest --text "Some text here"

# Context summary for a topic
python3 /root/.aethervault/hooks/knowledge-graph.py summary --topic "AetherVault"

# List all entities / export full graph
python3 /root/.aethervault/hooks/knowledge-graph.py list
python3 /root/.aethervault/hooks/knowledge-graph.py export
```

**Entity types**: person, project, technology, organization, preference, topic, location.
**Relation types**: owns, works-on, uses, runs-on, part-of, knows, prefers, located-at.

Store facts when Sunil shares them. Query before answering questions about known topics. The graph grows over time.

---

## MCP Gateway (Model Context Protocol)

```bash
# List servers and tools
python3 /root/.aethervault/hooks/mcp-gateway.py list-servers
python3 /root/.aethervault/hooks/mcp-gateway.py list-tools

# Call a tool
python3 /root/.aethervault/hooks/mcp-gateway.py call <server> <tool> {arg: value}
```

### filesystem (restricted to workspace + /tmp)
```bash
python3 /root/.aethervault/hooks/mcp-gateway.py call filesystem read_file {path: /root/.aethervault/workspace/SOUL.md}
python3 /root/.aethervault/hooks/mcp-gateway.py call filesystem write_file {path: /root/.aethervault/workspace/notes.md, content: Hello}
python3 /root/.aethervault/hooks/mcp-gateway.py call filesystem list_directory {path: /root/.aethervault/workspace}
python3 /root/.aethervault/hooks/mcp-gateway.py call filesystem search_files {directory: /root/.aethervault/workspace, pattern: *.md}
```

### system (monitoring)
```bash
python3 /root/.aethervault/hooks/mcp-gateway.py call system get_system_info {}
python3 /root/.aethervault/hooks/mcp-gateway.py call system get_service_status {service_name: aethervault}
python3 /root/.aethervault/hooks/mcp-gateway.py call system list_services {}
python3 /root/.aethervault/hooks/mcp-gateway.py call system get_uptime {}
```

Adding new MCP servers: create Python script in `/root/.aethervault/mcp/` using FastMCP SDK, register in config, test via gateway.

---

## Email (Himalaya CLI)

```bash
himalaya envelope list -a personal -f INBOX -s 10
himalaya message read -a personal <ID>
himalaya message write -a personal
```

Account: personal (sunilkgrao@gmail.com via IMAP/SMTP app password). Config: `/root/.config/himalaya/config.toml`.

---

## Sub-Agent Team

| Agent | Model | Purpose |
|-------|-------|---------|
| **researcher** | Claude Opus | Deep research and analysis |
| **codex** | OpenAI GPT-5.3 | Code generation and debugging |
| **critic** | Claude Opus | Critical review and quality assurance |

- Invoke via `subagent_invoke` tool. Can run in parallel.
- **Codex CLI**: Direct invocation via `codex-yolo` for complex coding tasks.

---

## Service Management

```bash
systemctl status aethervault
systemctl list-units --type=service --state=running
systemctl stop|start|restart <service>
```

---

## Music Generation

- Location: `/home/sunil/ACE-Step-1.5/` on raoDesktop
- Model: ACE-Step-1.5-turbo (HuggingFace/Naver AI)
- Pipeline: Generate -> Convert (WAV->MP3) -> Transfer (SCP) -> Deliver (Telegram)
- Performance: <10 seconds for a 30s track on RTX 3090

---

## GPU Infrastructure (raoDesktop)

- Connection: `ssh -p 2222 sunil@localhost` (via Tailscale/tunnel)
- GPU: NVIDIA RTX 3090 (24GB VRAM), 64 threads, 128GB RAM
- Embedding service: `embedding-service.service` on port 11435, model embeddinggemma-300M (768 dims)
- Use cases: ML inference, heavy compute, Codex subagents, AI music generation

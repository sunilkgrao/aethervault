# AetherVault

**AetherVault** is a **single‑file, append‑only memory capsule** plus a **hybrid retrieval engine** for agents.
All content, indexes, embeddings, queries, and feedback live inside one `.mv2` archive.

## Why it's novel

- **Memory is portable, auditable, and mergeable**: everything (content + indexes + query/feedback traces) lives in one capsule you can diff/merge like a repo.
- **Queries are first‑class memory**: searches, expansions, reranks, and feedback are stored as frames, so the system improves while staying explainable.
- **Hybrid retrieval by design**: expansion → lex + vec lanes → fusion → rerank → blend, with hook points for local or remote models.
- **Time‑travel retrieval**: "what did the agent know at time T?" is a built‑in query mode.
- **Agent‑ready surface**: MCP server compatibility, context packs, and a minimal hook‑based agent loop.

## System at a glance

```mermaid
flowchart LR
  A[Agent / Tool Caller] -->|query| B[AetherVault CLI]
  B --> C{Expansion Hook}
  C --> D[Lexical Lane BM25]
  C --> E[Vector Lane Optional]
  D --> F[Fusion RRF + bonuses]
  E --> F
  F --> G[Rerank Hook]
  G --> H[Blended Results]
  H --> I[Context Pack / JSON / Files]
  I -->|feedback + logs| J[.mv2 Capsule]
```

```mermaid
flowchart TB
  CAP[.mv2 Capsule]
  CAP --> WAL[Append-only WAL]
  CAP --> TOC[TOC + Index Manifests]
  CAP --> FR[Frames: content + metadata]
  CAP --> TR[Tracks: queries, feedback, agent logs]
  CAP --> CFG[aethervault://config/*]
```

## Design docs

- `docs/ARCHITECTURE.md`
- `FINAL_STATE.md` for the assistant product north star and target EA architecture

## Quick start

```bash
cargo build --locked

./target/debug/aethervault init knowledge.mv2
./target/debug/aethervault bootstrap knowledge.mv2 --workspace ./assistant
./target/debug/aethervault ingest knowledge.mv2 -c notes -r ~/notes
./target/debug/aethervault search knowledge.mv2 "project timeline" -c notes -n 10
./target/debug/aethervault query knowledge.mv2 "quarterly planning process" -c notes -n 10 --plan
./target/debug/aethervault context knowledge.mv2 "quarterly planning process" -c notes --max-bytes 8000
./target/debug/aethervault put knowledge.mv2 --uri aether://notes/hello.md --text "hello world"
./target/debug/aethervault log knowledge.mv2 --session sprint-42 --role user --text "Summarize release risks"
./target/debug/aethervault feedback knowledge.mv2 --uri aether://notes/plan.md --score 0.7 --note "Good source"
./target/debug/aethervault embed knowledge.mv2 -c notes --batch 64
./target/debug/aethervault get knowledge.mv2 aether://notes/some-note.md
./target/debug/aethervault config set --key index --json '{"context":"You are my assistant"}'
./target/debug/aethervault diff knowledge.mv2 other.mv2
./target/debug/aethervault merge knowledge.mv2 other.mv2 merged.mv2 --force
```

## Tool surface (agent‑friendly)

- `--json` returns a structured plan + results payload.
- `--files` emits tab‑separated `score,frame_id,uri,title`.
- `--log` appends the query + ranked results back into the capsule as an auditable frame.
- `embed` precomputes local embeddings for fast vector retrieval.
- `context` builds a prompt‑ready JSON pack (context + citations + plan).
- `log` records agent turns in the capsule for later audits.
- `feedback` records explicit relevance feedback to bias future rankings.
- `config` stores portable capsule config at `aethervault://config/...`.
- `diff` / `merge` provide git‑like ops for capsules.
- `mcp` starts a stdio tool server.
- `agent` runs a minimal hook‑based assistant loop.
- `bridge` runs Rust‑native Telegram/WhatsApp connectors.
- `bootstrap` scaffolds soul + memory workspace and writes default agent config.
- `schedule` runs daily/weekly autonomous briefings (Telegram optional).
- `watch` runs event-driven triggers (email/calendar).
- `exec` tool executes host commands (host mode default; wrap with `AETHERVAULT_COMMAND_WRAPPER` for sandboxing).
- `connect` runs a built-in OAuth broker for Google/Microsoft tokens.
- Gmail/Calendar and Microsoft mail/calendar tools are available after OAuth (`gmail_*`, `gcal_*`, `ms_*`).
- `http_request` provides a generic API surface (non-GET requires approval).
- `browser` provides CLI-based browser automation via agent-browser (ref-based element selection, named sessions).
- `fs_list`, `fs_read`, `fs_write` give controlled filesystem access within allowed roots.
- Sensitive tools require approval; reply `approve <id>` or `reject <id>` when prompted.
- `tool_search` enables dynamic tool lookup (no bloated prompt).
- `session_context` fetches recent session logs efficiently.
- `agent-logs` exports persisted agent logs by session/date for audits and offline jobs.
- `state_focus` / `state_list` / `state_capture` / `state_close` maintain live executive state (`STATE`) for priorities, follow-ups, and waiting-fors.
- `reflect` stores self-critique in the capsule for iterative improvement.
- `skill_store` / `skill_search` capture reusable procedures.
- `subagent_list` / `subagent_invoke` / `subagent_batch` provide elastic multi-session orchestration; the core agent can decide when to spin up zero, one, or many specialists.
- `compact` runs vacuum compaction + index rebuilds (SOTA maintenance).
- `doctor` exposes full repair/verify controls.

## Deployment and connectors

- `docs/DEPLOYMENT.md` for local, Docker, and cloud deployment.
- `docs/CONNECTORS.md` for Telegram + WhatsApp bridges and multi-session worker orchestration.
- Rust‑native connectors are built in (`bridge`).
- Optional: Himalaya integration enables `email_*` tools for Gmail IMAP workflows.
- `notify`, `signal_send`, `imessage_send` provide outbound messaging helpers.
- Approval gates remain enforced for sensitive tools, including bridge-triggered actions.
- Set `AETHERVAULT_FS_ROOTS` to restrict filesystem tools.
- Browser automation requires `agent-browser` CLI installed (`npm install -g agent-browser`).
- Set `AETHERVAULT_BROWSER_ENDPOINT` to a local browser broker.
- Set `AETHERVAULT_BRIDGE_TIMEOUT_SECS=0` to disable the default 15-minute wall-clock timeout for bridge runs.

## Maintenance (SOTA compaction)

```bash
./target/release/aethervault compact knowledge.mv2
```

For full control:

```bash
./target/release/aethervault doctor knowledge.mv2 --vacuum --rebuild-time --rebuild-lex --rebuild-vec
./target/release/aethervault doctor knowledge.mv2 --dry-run --json
```

## URI schemes

- `aether://<collection>/<path>` for content
- `aethervault://config/<key>` for portable capsule config

## Optional vector lane

Build with vector support and provide local embedding models:

```bash
cargo build --locked --features vec
```

The embed backend prints a download command if the ONNX model/tokenizer is missing.
Tune performance with `embed --batch N` and query flags like `--embed-cache`.

## Agent hook (minimal harness)

`agent` expects a hook command that reads JSON on stdin and returns JSON:

```bash
./target/debug/aethervault agent knowledge.mv2 --model-hook builtin:claude
```

`builtin:claude` runs the Rust hook in‑process (no subprocess).

## Workspace (Soul + State)

The agent can optionally read `SOUL.md`, `USER.md`, `STATE.md`, and a daily log in `memory/YYYY-MM-DD.md`
from a workspace directory (default `./assistant` or `AETHERVAULT_WORKSPACE`). `MEMORY.md` remains the durable
fact store, but the runtime no longer injects the whole file into every prompt; live priorities and open loops come
from `STATE`. Workspace memory/state writes via tools are mirrored into the capsule under `aethervault://memory/*`
so the single‑file `.mv2` remains the source of truth.

Bootstrap creates templates and writes config:

```bash
./target/release/aethervault bootstrap knowledge.mv2 --workspace ./assistant
```

## Autonomous scheduling

Run daily/weekly briefings (Telegram delivery optional):

```bash
export TELEGRAM_BOT_TOKEN=123456:ABC
export AETHERVAULT_TELEGRAM_CHAT_ID=123456789

./target/release/aethervault schedule knowledge.mv2 --workspace ./assistant --model-hook builtin:claude
```

For longer tool‑using sessions, raise the step budget:

```bash
./target/release/aethervault agent knowledge.mv2 --model-hook builtin:claude --max-steps 128 --log-commit-interval 8
```

See `docs/ARCHITECTURE.md` for the hook payload shapes.

## Claude hook (Anthropic)

Set env vars and run the agent with the hook:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
export ANTHROPIC_MODEL=claude-<model>
export ANTHROPIC_MAX_TOKENS=1024

./target/release/aethervault agent knowledge.mv2 --model-hook builtin:claude
```

Optional hook env vars: `ANTHROPIC_BASE_URL`, `ANTHROPIC_TEMPERATURE`, `ANTHROPIC_TOP_P`,
`ANTHROPIC_TIMEOUT`, `ANTHROPIC_MAX_RETRIES`.
Performance toggles: `ANTHROPIC_PROMPT_CACHE=1`, `ANTHROPIC_PROMPT_CACHE_TTL=5m`,
`ANTHROPIC_TOKEN_EFFICIENT=1` (token‑efficient tools beta).

Optional: persist the hook in the capsule config so you can omit `--model-hook`:

```bash
./target/release/aethervault config set --key index --json '{
  "agent": {
    "model_hook": { "command": "builtin:claude", "timeout_ms": 60000 },
    "log": true,
    "max_steps": 128,
    "log_commit_interval": 1
  }
}'
```

Note: `log_commit_interval=1` fsyncs each log entry (best durability). Increasing it improves throughput but can lose the last N log entries on a crash.

## Docker deploy (minimal)

Build and run the CLI in a container (mount a capsule at `/data`):

```bash
docker build -t aethervault .
docker run --rm -it -v "$(pwd)/data:/data" aethervault init /data/knowledge.mv2
docker run --rm -it -v "$(pwd)/data:/data" aethervault mcp /data/knowledge.mv2
```

Or with Compose:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
export ANTHROPIC_MODEL=claude-<model>
docker compose up --build
```

If you want to run the Claude hook inside the container, you can use the built‑in Rust hook:

```bash
docker build -t aethervault .
docker run --rm -it \
  -e ANTHROPIC_API_KEY=sk-ant-... \
  -e ANTHROPIC_MODEL=claude-<model> \
  -v "$(pwd)/data:/data" \
  aethervault agent /data/knowledge.mv2 --model-hook builtin:claude
```

## Implemented roadmap

- Optional vector search lane with on‑device embeddings (default build is lex‑only).
- Pluggable reranker + expansion hooks (drop‑in local or remote).
- MCP‑compatible tool server backed by the capsule.
- Portable capsule config stored at `aethervault://config/...`.
- Capsule diff + merge tooling (git‑like for memory).

---

## Automation Layer

The Python layer is now deliberately narrow: it should consume the same workspace state and capsule-backed memory contract as the Rust runtime, not invent a parallel product.

### What stays in Python

- `knowledge-graph.py` enriches entity and relationship context.
- `scripts/morning-briefing.py`, `scripts/proactive-checkin.py`, and `scripts/nightly-consolidation.py` are scheduled jobs around the same `STATE` and `MEMORY` contract used by the interactive agent.
- `scripts/session-manager.py` and `scripts/capabilities.py` remain operational helpers.
- `scripts/notifier.py` centralizes outbound Telegram delivery for lifecycle jobs.

### Optional provider adapters

`vertex_proxy.py`, `moonshot_proxy.py`, `llama_proxy.py`, and `start_services.sh` are optional infrastructure for model routing. They are not the assistant itself.

### Automation quick start

```bash
pip install -r requirements-core.txt

# Optional model/provider adapters
# Enable only the adapters you want in ~/.aethervault/.env:
# ENABLE_VERTEX_PROXY=1
# ENABLE_MOONSHOT_PROXY=1
# ENABLE_LLAMA_TUNNEL=1
bash start_services.sh

# Example cron entries
# 0 8 * * 1-5 /path/to/repo/scripts/morning-briefing.sh
# 0 20 * * * /path/to/repo/scripts/proactive-checkin.sh
# 0 3 * * * /path/to/repo/scripts/nightly-consolidation.sh
```

### Configuration

Copy `config/env.example` to `~/.aethervault/.env` and edit it. Runtime configuration comes from capsule config (`aethervault://config/*`) plus the workspace files such as `SYSTEM.md`, `SOUL.md`, `USER.md`, `MEMORY.md`, and `STATE`.

See `config/env.example` for the supported environment variables and defaults.

### Repository shape

```
.
├── src/                        # Rust runtime and harness
├── scripts/                    # Scheduled jobs and operational helpers
├── services/                   # Optional infrastructure, e.g. embedding service
├── config/                     # Env templates and runtime config
├── docs/                       # Canonical docs
├── knowledge-graph.py          # Entity/relationship enrichment
├── vertex_proxy.py             # Optional Vertex adapter
├── moonshot_proxy.py           # Optional Moonshot/Kimi adapter
├── llama_proxy.py              # Optional llama.cpp adapter
├── start_services.sh           # Starts optional provider adapters
└── requirements-core.txt       # Python automation dependencies
```

## License

MIT License. See [LICENSE](LICENSE).

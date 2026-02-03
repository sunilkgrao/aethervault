# KairosVault

KairosVault is a **single‑file, append‑only memory capsule** plus a **hybrid retrieval engine** for agents.  
All content, indexes, embeddings, queries, and feedback live inside one `.mv2` archive.

## What we’re building

- **Single‑file capsule**: treat `.mv2` as the index *and* the content store (no SQLite DB, no sidecar files).
- **Hybrid query pipeline**:
  - query expansion (optional),
  - lexical BM25 + vector search in parallel,
  - fusion (RRF + bonuses),
  - reranking (local or hook-based),
  - position-aware blending (protect exact matches, improve recall).
- **Time-travel retrieval**: query “as-of” a frame/timestamp to reproduce what the agent “knew” then.
- **Feedback becomes memory**: store queries, expansions, reranks, and user feedback as frames so the capsule can improve and remain auditable.
- **Agent harness surface**: context packs, logs, feedback, MCP server, and a minimal hook‑based agent loop.

## Design docs

- `docs/ARCHITECTURE.md`

## Quick start (current prototype)

```bash
cargo build --locked

./target/debug/aethervault init knowledge.mv2
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

### Tool surface (agent-friendly)

- `--json` returns a structured plan + results payload.
- `--files` emits tab‑separated `score,frame_id,uri,title`.
- `--log` appends the query + ranked results back into the capsule as an auditable frame.
- `embed` precomputes local embeddings for fast vector retrieval.
- `context` builds a prompt-ready JSON pack (context + citations + plan).
- `log` records agent turns in the capsule for later audits.
- `feedback` records explicit relevance feedback to bias future rankings.
- `config` stores portable capsule config at `aethervault://config/...`.
- `diff` / `merge` provide git‑like ops for capsules.
- `mcp` starts a stdio tool server.
- `agent` runs a minimal hook‑based assistant loop.

### Optional vector lane

Build with vector support and provide local embedding models:

```bash
cargo build --locked --features vec
```

The embed backend will print a download command if the ONNX model/tokenizer is missing.
You can tune performance with `embed --batch N` and query flags like `--embed-cache`.

### Agent hook (minimal harness)

`agent` expects a hook command that reads JSON on stdin and returns JSON:

```bash
./target/debug/aethervault agent knowledge.mv2 --model-hook "python3 ./hooks/llm.py"
```

See `docs/ARCHITECTURE.md` for the hook payload shapes.

## Implemented roadmap

- Optional vector search lane with on-device embeddings (default build is lex-only).
- Pluggable reranker + expansion hooks (drop‑in local or remote).
- MCP-compatible tool server backed by the capsule.
- Portable capsule config stored at `aethervault://config/...`.
- Capsule diff + merge tooling (git-like for memory).

# KairosVault architecture (v0)

## Thesis

The capsule is the **only persistent artifact**.  
All retrieval quality comes from a **hybrid pipeline** that can run fully on‑device, with optional hooks for external models.  
The agent harness stays **minimal and explicit**: you control context, tools, and logging.

---

## Core objects

### 1) Capsule (`.mv2`)

A single file that contains:
- append‑only frames (documents + metadata),
- optional lexical index (BM25),
- optional vector index (HNSW),
- time index (chronological),
- query / feedback frames for audit + self‑improvement.

### 2) Logical document identity

Stable URI scheme:

`aether://<collection>/<relative_path>`

Each update appends a new frame with the same URI.

Why append?
- time‑travel queries (`asof`, `before`, `after`)
- deterministic diffs
- provenance for agents

### 3) Collections + config

Collections are URI prefixes:

`aether://notes/...`, `aether://docs/...`

Portable config is stored inside the capsule:

`aethervault://config/index.json`

This can include collection roots, human context, and hook commands.

---

## Query pipeline (hybrid)

Each query builds a **query plan**:

1) **Parse query markup**
   - inline constraints: `in:notes`, `before:2025-01-01`, `asof:2026-01-10`
2) **Expansion (optional)**
   - built‑in heuristic expansions **or** an expansion hook
3) **Parallel retrieval**
   - lexical BM25 lane
   - optional vector lane
4) **Fusion**
   - Reciprocal Rank Fusion (RRF) + top‑rank bonus
5) **Reranking (optional)**
   - local reranker **or** hook‑based reranker
6) **Position‑aware blending**
   - protect high‑precision hits, boost recall
7) **Outputs**
   - human text, JSON, files list, or context pack

---

## Agent harness surface

KairosVault exposes five primitives:
- **Context packs** (`context`): prompt‑ready JSON with citations.
- **Agent logs** (`log`): append conversation turns for audit/replay.
- **Feedback** (`feedback`): explicit relevance signals.
- **MCP server** (`mcp`): stdio JSON‑RPC tool surface.
- **Agent loop** (`agent`): hook‑based minimal assistant loop.

Tool results are split into:
- **output** (LLM‑facing text)
- **details** (structured JSON for UI/workflows)

---

## Merge + diff

**Diff** compares latest active frames by URI:
- `only_left`, `only_right`, `changed`

**Merge** appends active frames into a new capsule:
- dedup by `(uri, checksum, timestamp)`
- preserves timestamps, metadata, and URIs

Limitations:
- frame status is not preserved (only active frames are merged)
- extracted metadata that requires background enrichment may be re‑derived

---

## Hook protocol (summary)

**Expansion hook input**:
```json
{ "query": "...", "max_expansions": 2, "scope": "aether://notes/", "temporal": null }
```

**Expansion hook output**:
```json
{ "lex": ["..."], "vec": ["..."], "warnings": [] }
```

**Rerank hook input**:
```json
{ "query": "...", "candidates": [{ "key": "...", "uri": "...", "snippet": "..." }] }
```

**Rerank hook output**:
```json
{ "scores": { "key": 0.42 }, "snippets": { "key": "..." }, "warnings": [] }
```

**Agent hook input**:
```json
{ "messages": [...], "tools": [...], "session": "optional" }
```

**Agent hook output**:
```json
{ "message": { "role": "assistant", "content": "...", "tool_calls": [] } }
```

---

## Implementation shape

**Rust core**: capsule read/write, ingestion, hybrid retrieval, hooks, diff/merge  
**CLI + MCP**: deterministic outputs and tool surfaces for agent harnesses

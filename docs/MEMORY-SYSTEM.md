# AetherVault -- Memory System

> Read when working on memory, knowledge graph, fact extraction, or data pipelines. For identity, see [SOUL.md](SOUL.md). For system architecture, see [ARCHITECTURE.md](ARCHITECTURE.md).

---

## 3-Tier Memory Model

### Tier 1: Session Buffer (Hot)
- Current conversation context, persisted across sessions via MV2 capsule
- Immediate recall, no search needed
- The runtime manages this automatically

### Tier 2: Hot Memory (Warm)
- Recently extracted facts, scored by FadeMem composite scoring
- Storage: `/root/.aethervault/data/hot_memory.json`
- Accessible via `memory_search` tool
- Facts decay over time; high-signal facts persist longer
- Single source of truth module: `hot_memory_store.py`

### Tier 3: Knowledge Graph + Cold Storage (Cold)
- **Knowledge Graph**: NetworkX + JSON at `/root/.aethervault/data/knowledge-graph.json`
- **Vector search**: SQLite + sqlite-vec (hybrid 70% semantic + 30% BM25 keyword)
- **Embeddings**: embeddinggemma-300M @ localhost:11435 (768 dims, ~300MB)
- **Index**: `~/.aethervault/memory/main.sqlite` (423 files, 455 chunks)
- **Migrated data**: 199 Roam notes, 10 people profiles, 12 knowledge docs, 6 daily logs, 624 AetherVault memory chunks

---

## Data Flow

```
Conversation
    |
    v
Memory Extractor (cron */5)
    |-- extracts facts from recent conversations
    |-- writes to hot_memory.json via hot_memory_store.py
    |
    v
Memory Scorer (FadeMem)
    |-- applies decay model to all hot memory facts
    |-- composite score = recency + frequency + importance
    |-- low-scoring facts age out
    |
    v
Knowledge Graph
    |-- entities and relations persisted in JSON
    |-- auto-ingest from text
    |-- queryable by name, type, or relationship
    |
    v
Weekly Reflection (cron Monday)
    |-- generates meta-insights from accumulated memory
    |-- identifies patterns, trends, knowledge gaps
```

---

## FadeMem Scoring Model

Each fact in hot memory gets a composite score:

- **Recency**: How recently was this fact extracted or referenced? Exponential decay.
- **Frequency**: How often has this fact come up across conversations?
- **Importance**: Was this flagged as high-priority, actionable, or personal?

Facts below the decay threshold are pruned from hot memory. Critical facts (family, preferences, active projects) have boosted importance scores to resist decay.

---

## Extraction Pipeline

### Real-Time Extraction (`memory-extractor.py`, cron `*/5`)
- Runs every 5 minutes
- Scans recent conversation turns
- Extracts structured facts: entities, preferences, decisions, action items
- Writes to hot memory via `hot_memory_store.py` (single source of truth)

### Knowledge Graph Ingestion
```bash
# Auto-extract entities and relations from text
python3 /root/.aethervault/hooks/knowledge-graph.py ingest --text "Some text here"
```
- Entity types: person, project, technology, organization, preference, topic, location
- Relation types: owns, works-on, uses, runs-on, part-of, knows, prefers, located-at
- Store facts when Sunil shares them. Query before answering questions about known topics.

### Knowledge Graph Queries
```bash
python3 /root/.aethervault/hooks/knowledge-graph.py query --name "Sunil"
python3 /root/.aethervault/hooks/knowledge-graph.py query --type project
python3 /root/.aethervault/hooks/knowledge-graph.py query --related-to "AetherVault"
python3 /root/.aethervault/hooks/knowledge-graph.py summary --topic "AetherVault"
python3 /root/.aethervault/hooks/knowledge-graph.py list
python3 /root/.aethervault/hooks/knowledge-graph.py export
```

---

## Weekly Reflection (`weekly-reflection.py`, cron Monday)

- Generates meta-insights from the past week's accumulated memory
- Identifies patterns across conversations (recurring topics, shifting priorities)
- Detects knowledge gaps (facts referenced but not stored)
- Produces a reflection summary for review
- Weekly knowledge graph insights: `/root/clawd/scripts/weekly-graph-insights.sh` (generates report in `/root/clawd/insights/weekly-YYYY-MM-DD.md`)

---

## Health Monitoring (`memory-health.py`, cron `*/15`)

Runs 9 checks every 15 minutes:
1. Hot memory file integrity and JSON validity
2. Knowledge graph file existence and parsability
3. Embedding service responsiveness (port 11435)
4. Memory index freshness
5. Fact count within expected bounds
6. No duplicate entities in KG
7. Score distribution sanity check
8. Extraction pipeline last-run recency
9. Dead-man's switch (alerts if the health check itself stops running)

Auto-fix: attempts to repair common issues (corrupt JSON, stale index) before alerting.

---

## Search Tools

| Tool | Scope | Usage |
|------|-------|-------|
| `memory_search` | Workspace markdown files | Quick recall of stored facts |
| `query` | Full capsule (all sessions) | Deep search across conversation history |
| KG `query` | Knowledge graph entities/relations | Structured fact lookup |

### Native Memory Search
- Engine: Native vector search (SQLite + sqlite-vec)
- Hybrid: 70% semantic + 30% BM25 keyword
- Indexed sources: `memory/*.md`, `memory/roam-notes/*.md`, `MEMORY.md`
- Roam notes are COPIED to `memory/roam-notes/` (not symlinked; indexer doesn't follow symlinks)
- To reindex: `clawdbot memory index --force`

---

## Configuration Files

| File | Purpose |
|------|---------|
| `/root/.aethervault/data/knowledge-graph.json` | KG persistent data |
| `/root/.aethervault/config/knowledge-graph.json` | KG configuration |
| `/root/.aethervault/data/hot_memory.json` | Hot memory facts |
| `~/.aethervault/memory/main.sqlite` | Vector search index |

---

## Migration History

- **2026-02-10**: Migrated from AetherVault to AetherVault. All historical data transferred.
- **2026-02-01**: Switched embedding service from Ollama to standalone node-llama-cpp. Removed Ollama tunnel, health check.
- **2026-02-01**: Unified memory system to native Clawdbot only. Removed qmd, memory_router.py, memory_indexer.py, memory_health_check.py, memory_watch.sh (archived to `.archive/old-memory-scripts/`).

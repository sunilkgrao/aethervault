# Skill Registry Subsystem Map

## Overview
The skill_registry subsystem provides SQLite-backed persistence for reusable agent procedures. It supports storing, searching, and tracking skill execution statistics.

**Module Location:** `src/skill_registry.rs`
**Database Location:** `{workspace}/skills.sqlite` (default workspace: `~/.openclaw/workspace`)

**Status:** Implementation complete, but database not yet used in practice. No `skills.sqlite` file exists on the system; subsystem is ready for first use via `skill_store` tool invocation.

---

## SQLite Schema

### Table: `skills`

```sql
CREATE TABLE IF NOT EXISTS skills (
    name TEXT PRIMARY KEY,
    trigger TEXT,
    steps TEXT NOT NULL,
    tools TEXT NOT NULL,
    notes TEXT,
    success_rate REAL NOT NULL DEFAULT 0.0,
    times_used INTEGER NOT NULL DEFAULT 0,
    times_succeeded INTEGER NOT NULL DEFAULT 0,
    last_used TEXT,
    created_at TEXT NOT NULL,
    contexts TEXT NOT NULL DEFAULT '[]'
)
```

**Column Details:**
- `name` (TEXT, PRIMARY KEY): Unique skill identifier
- `trigger` (TEXT, nullable): Condition or pattern that activates the skill
- `steps` (TEXT, NOT NULL): JSON array of procedure steps (serialized from `Vec<String>`)
- `tools` (TEXT, NOT NULL): JSON array of tool names used (serialized from `Vec<String>`)
- `notes` (TEXT, nullable): Human-readable description or metadata
- `success_rate` (REAL, default 0.0): Fraction of times_succeeded / times_used (auto-calculated)
- `times_used` (INTEGER, default 0): Total invocations counter
- `times_succeeded` (INTEGER, default 0): Successful invocations counter
- `last_used` (TEXT, nullable): ISO 8601 timestamp of last execution
- `created_at` (TEXT, NOT NULL): ISO 8601 timestamp of skill creation
- `contexts` (TEXT, default '[]'): JSON array of context tags (serialized from `Vec<String>`)

---

## Public API Functions

### 1. `open_skill_db(path: &Path) -> Result<Connection, Box<dyn std::error::Error>>`

**Location:** `src/skill_registry.rs:21-39`

**Purpose:** Open or create SQLite database, ensuring schema exists.

**Callers:**
| Caller | File | Line | Context |
|--------|------|------|---------|
| `skill_store` tool handler | `src/tool_exec.rs` | 1844 | Opens DB before upserting skill |
| `skill_search` tool handler | `src/tool_exec.rs` | 1874 | Opens DB before searching skills |

**Data Flow:**
- **Input:** Path to SQLite database file
- **Output:** Open `rusqlite::Connection` with initialized schema
- **Side Effects:** Creates file and schema if not exists; no mutations to existing data

**Error Handling:** Returns boxed error on I/O or SQL failure

---

### 2. `upsert_skill(conn: &Connection, skill: &SkillRecord) -> Result<(), Box<dyn std::error::Error>>`

**Location:** `src/skill_registry.rs:41-72`

**Purpose:** Insert or update a skill record using SQLite UPSERT (ON CONFLICT).

**Callers:**
| Caller | File | Line | Context |
|--------|------|------|---------|
| `skill_store` tool handler | `src/tool_exec.rs` | 1859 | Upserts newly created SkillRecord |

**Data Flow:**
- **Input:**
  - `SkillRecord` with fields: `name`, `trigger`, `steps` (Vec), `tools` (Vec), `notes`, `success_rate`, `times_used`, `times_succeeded`, `last_used`, `created_at`, `contexts` (Vec)
- **Processing:**
  - Serializes `steps`, `tools`, `contexts` to JSON strings
  - Casts `times_used`, `times_succeeded` from u64 to i64
  - ON CONFLICT: Updates `trigger`, `steps`, `tools`, `notes`, `contexts` (preserves counters on conflict)
- **Output:** Result (Ok on success, Err on SQL/JSON failure)
- **Note:** `success_rate` is NOT updated on upsert (must be recalculated via `record_skill_use`)

**Example Usage:**
```rust
let skill = SkillRecord {
    name: "fetch_and_parse".to_string(),
    trigger: Some("when url pattern matches".to_string()),
    steps: vec!["fetch URL".to_string(), "parse HTML".to_string()],
    tools: vec!["exec".to_string(), "query".to_string()],
    notes: Some("Web scraping workflow".to_string()),
    success_rate: 0.0,
    times_used: 0,
    times_succeeded: 0,
    last_used: None,
    created_at: now,
    contexts: vec![],
};
upsert_skill(&conn, &skill)?;
```

---

### 3. `search_skills(conn: &Connection, query: &str, limit: usize) -> Vec<SkillRecord>`

**Location:** `src/skill_registry.rs:74-93`

**Purpose:** Full-text search over skill names, triggers, and notes.

**Callers:**
| Caller | File | Line | Context |
|--------|------|------|---------|
| `skill_search` tool handler | `src/tool_exec.rs` | 1875 | Returns results matching user query |

**Data Flow:**
- **Input:**
  - `query` (String): Search term
  - `limit` (usize): Max results to return
- **Processing:**
  - Wraps query in `%` wildcards for LIKE pattern matching
  - Searches columns: `name`, `trigger`, `notes`
  - Orders by `success_rate DESC`
  - Returns up to `limit` rows
- **Output:** `Vec<SkillRecord>` (empty if no matches or DB error)
- **Error Handling:** Silently returns empty vec on SQL/query errors (does not propagate)

**Example Usage:**
```rust
let results = search_skills(&conn, "fetch", 10);
// Results ordered by success_rate (highest first)
// Matches: name LIKE '%fetch%' OR trigger LIKE '%fetch%' OR notes LIKE '%fetch%'
```

---

### 4. `record_skill_use(conn: &Connection, name: &str, succeeded: bool) -> Result<(), Box<dyn std::error::Error>>`

**Location:** `src/skill_registry.rs:95-117`

**Purpose:** Update skill execution statistics after use.

**Status:** **DEAD CODE** — Not called anywhere in codebase

**Implementation Details:**
- Increments `times_used` by 1
- If `succeeded=true`: Also increments `times_succeeded` by 1
- Sets `last_used` to current UTC timestamp (ISO 8601)
- Recalculates `success_rate = times_succeeded / times_used` (with guard against divide-by-zero)

**Note:** This function should be called after `skill_store` operations to track effectiveness, but currently no tool handler invokes it.

---

### 5. `get_skill(conn: &Connection, name: &str) -> Option<SkillRecord>`

**Location:** `src/skill_registry.rs:119-128`

**Purpose:** Retrieve a single skill by exact name.

**Status:** **DEAD CODE** — Not called anywhere in codebase

**Data Flow:**
- **Input:** Skill name (String)
- **Output:** `Option<SkillRecord>` (Some if found, None if not found or error)
- **Error Handling:** Returns None on SQL error

---

### 6. `list_skills(conn: &Connection, limit: usize) -> Vec<SkillRecord>`

**Location:** `src/skill_registry.rs:130-145`

**Purpose:** Retrieve top skills ordered by success rate and usage.

**Status:** **DEAD CODE** — Not called anywhere in codebase

**Data Flow:**
- **Input:** `limit` (usize) — max results
- **Output:** `Vec<SkillRecord>` ordered by `success_rate DESC, times_used DESC`
- **Error Handling:** Returns empty vec on SQL error

---

### 7. `row_to_skill(row: &rusqlite::Row<'_>) -> SkillRecord`

**Location:** `src/skill_registry.rs:147-164`

**Purpose:** Internal helper to deserialize SQLite row into SkillRecord struct.

**Callers (Internal):**
| Caller | File | Line |
|--------|------|------|
| `search_skills` | `src/skill_registry.rs` | 87 |
| `get_skill` | `src/skill_registry.rs` | 126 |
| `list_skills` | `src/skill_registry.rs` | 140 |

**Details:**
- Maps row columns (0-10) to SkillRecord fields
- Deserializes JSON from `steps`, `tools`, `contexts` columns
- Uses `unwrap_or_default()` for JSON parse failures
- Casts `times_used`, `times_succeeded` from i64 to u64

---

## SkillRecord Data Structure

**Location:** `src/skill_registry.rs:6-19`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SkillRecord {
    pub(crate) name: String,
    pub(crate) trigger: Option<String>,
    pub(crate) steps: Vec<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) notes: Option<String>,
    pub(crate) success_rate: f64,
    pub(crate) times_used: u64,
    pub(crate) times_succeeded: u64,
    pub(crate) last_used: Option<String>,
    pub(crate) created_at: String,
    pub(crate) contexts: Vec<String>,
}
```

---

## Tool Integration

### Tool: `skill_store`

**Handler Location:** `src/tool_exec.rs:1837-1864`

**Arguments** (from `ToolSkillStoreArgs` in `src/tool_args.rs:322-332`):
```rust
pub(crate) struct ToolSkillStoreArgs {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) trigger: Option<String>,
    #[serde(default)]
    pub(crate) steps: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) tools: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) notes: Option<String>,
}
```

**Tool Definition:** `src/tool_defs.rs:459-471`

**Workflow:**
1. Parse `skill_store` tool invocation args
2. Resolve workspace (from override or default `~/.openclaw/workspace`)
3. Open DB via `open_skill_db(&db_path)`
4. Create `SkillRecord` with:
   - User-provided: `name`, `trigger`, `steps`, `tools`, `notes`
   - Auto-initialized: `success_rate=0.0`, `times_used=0`, `times_succeeded=0`, `last_used=None`, `created_at=now`, `contexts=[]`
5. Call `upsert_skill(&conn, &skill)`
6. Return `ToolExecution` with skill name and DB path

**Output JSON:**
```json
{
  "output": "Skill 'fetch_and_parse' stored in SQLite.",
  "details": {
    "name": "fetch_and_parse",
    "db": "/Users/username/.openclaw/workspace/skills.sqlite"
  },
  "is_error": false
}
```

---

### Tool: `skill_search`

**Handler Location:** `src/tool_exec.rs:1866-1896`

**Arguments** (from `ToolSkillSearchArgs` in `src/tool_args.rs:335-339`):
```rust
pub(crate) struct ToolSkillSearchArgs {
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
}
```

**Tool Definition:** `src/tool_defs.rs:474-484`

**Workflow:**
1. Parse `skill_search` tool invocation args
2. Resolve workspace (from override or default)
3. Get limit (from args or default 10)
4. Open DB via `open_skill_db(&db_path)`
5. Call `search_skills(&conn, &query, limit)`
6. Map results: extract only `name`, `trigger`, `steps`, `tools`, `notes`, `success_rate`, `times_used`, `last_used` (excludes `times_succeeded`, `created_at`, `contexts`)
7. Return `ToolExecution` with results array

**Output JSON:**
```json
{
  "output": "Found 3 skills.",
  "details": {
    "results": [
      {
        "name": "fetch_and_parse",
        "trigger": "when url pattern matches",
        "steps": ["fetch URL", "parse HTML"],
        "tools": ["exec", "query"],
        "notes": "Web scraping workflow",
        "success_rate": 0.0,
        "times_used": 0,
        "last_used": null
      }
    ]
  },
  "is_error": false
}
```

---

## Data Flow Diagram

```
Agent Tool Call
    ↓
[skill_store / skill_search]
    ↓
    ├─→ Parse args (tool_args.rs)
    ├─→ Resolve workspace path
    ├─→ open_skill_db(path)
    │   └─→ Create/initialize skills.sqlite
    ├─→ skill_store flow:
    │   ├─→ Create SkillRecord
    │   ├─→ upsert_skill(&conn, &record)
    │   │   ├─→ JSON-serialize: steps, tools, contexts
    │   │   └─→ INSERT OR REPLACE query
    │   └─→ Return ToolExecution
    └─→ skill_search flow:
        ├─→ search_skills(&conn, query, limit)
        │   ├─→ SELECT with LIKE pattern
        │   ├─→ row_to_skill() for each row
        │   │   ├─→ JSON-deserialize: steps, tools, contexts
        │   │   └─→ Construct SkillRecord
        │   └─→ Return Vec<SkillRecord>
        └─→ Return ToolExecution
```

---

## Dead Code Analysis

| Function | Location | Reason | Recommendation |
|----------|----------|--------|-----------------|
| `record_skill_use` | `src/skill_registry.rs:95-117` | Never called; no tool handler tracks skill execution success | Consider implementing in future SkillRL module or remove if not planned |
| `get_skill` | `src/skill_registry.rs:119-128` | Not used; `search_skills` handles retrieval | Potential future use; low risk to keep |
| `list_skills` | `src/skill_registry.rs:130-145` | Not called; no "list all" tool exists | Useful for future admin/debugging tools; low risk to keep |

---

## Module Exports and Visibility

**Exported from `main.rs`:** `src/main.rs` line ~20
- All items from `skill_registry` are re-exported via `pub(crate) use skill_registry::*;`
- Available throughout codebase as `open_skill_db`, `upsert_skill`, `search_skills`, `SkillRecord`

**Imports in `tool_exec.rs`:** `src/tool_exec.rs:354-357`
```rust
open_skill_db,
upsert_skill,
search_skills,
SkillRecord,
```

---

## Known Limitations and Design Notes

1. **No Transaction Support:** Each operation (open, upsert, search) opens its own connection. No atomic multi-step operations.

2. **Silent Error Handling:** `search_skills` returns empty vec on errors instead of propagating; may hide issues.

3. **Success Rate Calculation:** Stored as REAL but only recalculated in `record_skill_use` (which is dead code). New skills always have `success_rate=0.0`.

4. **No Skill Deletion:** No `delete_skill` function; skills persist indefinitely once created.

5. **Contexts Unused:** `contexts` Vec is stored but never populated or used in search/filter logic.

6. **Trigger Not Indexed:** Searches are LIKE-based without indexes; performance may degrade with many skills.

7. **Timestamp Format:** Uses `chrono::Utc::now().to_rfc3339()` (ISO 8601). No timezone handling for local times.

---

## Summary Table

| Component | File | Lines | Status | Calls |
|-----------|------|-------|--------|-------|
| SkillRecord struct | skill_registry.rs | 6-19 | Active | Used in all functions |
| open_skill_db | skill_registry.rs | 21-39 | Active | 2 (skill_store, skill_search) |
| upsert_skill | skill_registry.rs | 41-72 | Active | 1 (skill_store) |
| search_skills | skill_registry.rs | 74-93 | Active | 1 (skill_search) |
| record_skill_use | skill_registry.rs | 95-117 | Dead | 0 |
| get_skill | skill_registry.rs | 119-128 | Dead | 0 |
| list_skills | skill_registry.rs | 130-145 | Dead | 0 |
| row_to_skill | skill_registry.rs | 147-164 | Active (internal) | 3 (search_skills, get_skill, list_skills) |
| skill_store tool | tool_exec.rs | 1837-1864 | Active | Calls: open_skill_db, upsert_skill |
| skill_search tool | tool_exec.rs | 1866-1896 | Active | Calls: open_skill_db, search_skills |

---

**Generated:** 2026-02-16
**Database Format:** SQLite 3
**Rust Version:** Stable (uses rusqlite, serde_json, chrono)

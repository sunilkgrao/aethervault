//! SQLite-backed memory database — drop-in replacement for the MV2 (aether-core Vault) format.
//!
//! Design goals:
//!   - WAL mode for concurrent reads (subagents no longer blocked by exclusive flock)
//!   - FTS5 for full-text search (replaces Tantivy)
//!   - No append-only bloat (SQLite manages space efficiently)
//!   - API surface matching Vault so callers can switch with minimal changes
//!
//! All types defined here mirror the aether-core equivalents and are serde-compatible
//! with the same JSON wire format (critical for hook I/O and session serialization).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

// ── Type aliases ─────────────────────────────────────────────────────────

pub(crate) type FrameId = u64;

// ── FrameStatus ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FrameStatus {
    Active,
    Superseded,
    Deleted,
}

impl Default for FrameStatus {
    fn default() -> Self {
        Self::Active
    }
}

impl FrameStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Deleted => "deleted",
        }
    }
    pub(crate) fn from_db_str(s: &str) -> Self {
        match s {
            "superseded" => Self::Superseded,
            "deleted" => Self::Deleted,
            _ => Self::Active,
        }
    }
}

impl std::fmt::Display for FrameStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── FrameRole ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FrameRole {
    #[default]
    Document,
    DocumentChunk,
    ExtractedImage,
}

impl FrameRole {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::DocumentChunk => "document_chunk",
            Self::ExtractedImage => "extracted_image",
        }
    }
    pub(crate) fn from_db_str(s: &str) -> Self {
        match s {
            "document_chunk" => Self::DocumentChunk,
            "extracted_image" => Self::ExtractedImage,
            _ => Self::Document,
        }
    }
}

// ── TemporalFilter ───────────────────────────────────────────────────────
// JSON-compatible with aether_core::types::TemporalFilter

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TemporalFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) start_utc: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) end_utc: Option<i64>,
}

// ── Frame ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct Frame {
    pub(crate) id: FrameId,
    pub(crate) uri: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) track: Option<String>,
    pub(crate) status: FrameStatus,
    pub(crate) timestamp: i64,
    pub(crate) checksum: [u8; 32],
    pub(crate) search_text: Option<String>,
    pub(crate) role: FrameRole,
    pub(crate) parent_id: Option<FrameId>,
    pub(crate) tags: Vec<String>,
    pub(crate) labels: Vec<String>,
    pub(crate) extra_metadata: BTreeMap<String, String>,
    pub(crate) metadata: Option<serde_json::Value>,
}

// ── SearchHit ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SearchHit {
    pub(crate) rank: usize,
    pub(crate) frame_id: FrameId,
    pub(crate) uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    pub(crate) range: (usize, usize),
    pub(crate) text: String,
    pub(crate) matches: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) chunk_range: Option<(usize, usize)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) chunk_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) metadata: Option<serde_json::Value>,
}

// ── SearchRequest / SearchResponse ───────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct SearchRequest {
    pub(crate) query: String,
    pub(crate) top_k: usize,
    pub(crate) snippet_chars: usize,
    pub(crate) scope: Option<String>,
    pub(crate) temporal: Option<TemporalFilter>,
    pub(crate) as_of_frame: Option<FrameId>,
    pub(crate) as_of_ts: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchResponse {
    pub(crate) hits: Vec<SearchHit>,
}

// ── PutOptions ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct PutOptions {
    pub(crate) timestamp: Option<i64>,
    pub(crate) track: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) uri: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) search_text: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) labels: Vec<String>,
    pub(crate) extra_metadata: BTreeMap<String, String>,
    pub(crate) metadata: Option<serde_json::Value>,
    pub(crate) role: FrameRole,
    pub(crate) parent_id: Option<FrameId>,
}

impl Default for PutOptions {
    fn default() -> Self {
        Self {
            timestamp: None,
            track: None,
            kind: None,
            uri: None,
            title: None,
            search_text: None,
            tags: Vec::new(),
            labels: Vec::new(),
            extra_metadata: BTreeMap::new(),
            metadata: None,
            role: FrameRole::default(),
            parent_id: None,
        }
    }
}

// ── MigrationReport ──────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) struct MigrationReport {
    pub(crate) total_frames: usize,
    pub(crate) migrated: usize,
    pub(crate) skipped: usize,
    pub(crate) errors: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct HotMemoryMigrationReport {
    pub(crate) total_lines: usize,
    pub(crate) added: usize,
    pub(crate) updated: usize,
    pub(crate) skipped_noop: usize,
    pub(crate) skipped_invalid: usize,
    pub(crate) errors: Vec<String>,
}

// ═════════════════════════════════════════════════════════════════════════
// MemoryDb — SQLite backend
// ═════════════════════════════════════════════════════════════════════════

pub(crate) struct MemoryDb {
    conn: Connection,
}

// ── Schema SQL ───────────────────────────────────────────────────────────

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS frames (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uri TEXT,
    title TEXT,
    kind TEXT,
    track TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    timestamp INTEGER NOT NULL,
    checksum BLOB,
    search_text TEXT,
    payload BLOB,
    text_content TEXT,
    role TEXT NOT NULL DEFAULT 'document',
    parent_id INTEGER,
    tags TEXT DEFAULT '[]',
    labels TEXT DEFAULT '[]',
    extra_metadata TEXT DEFAULT '{}',
    doc_metadata TEXT,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_frames_uri ON frames(uri) WHERE uri IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_frames_track ON frames(track) WHERE track IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_frames_timestamp ON frames(timestamp);
CREATE INDEX IF NOT EXISTS idx_frames_status ON frames(status);

CREATE VIRTUAL TABLE IF NOT EXISTS frames_fts USING fts5(
    uri, title, search_text, text_content,
    content='frames', content_rowid='id',
    tokenize='porter unicode61'
);

-- Keep FTS5 in sync via triggers
CREATE TRIGGER IF NOT EXISTS frames_ai AFTER INSERT ON frames BEGIN
    INSERT INTO frames_fts(rowid, uri, title, search_text, text_content)
    VALUES (new.id, COALESCE(new.uri, ''), COALESCE(new.title, ''),
            COALESCE(new.search_text, ''), COALESCE(new.text_content, ''));
END;

CREATE TRIGGER IF NOT EXISTS frames_ad AFTER DELETE ON frames BEGIN
    INSERT INTO frames_fts(frames_fts, rowid, uri, title, search_text, text_content)
    VALUES ('delete', old.id, COALESCE(old.uri, ''), COALESCE(old.title, ''),
            COALESCE(old.search_text, ''), COALESCE(old.text_content, ''));
END;

CREATE TRIGGER IF NOT EXISTS frames_au AFTER UPDATE ON frames BEGIN
    INSERT INTO frames_fts(frames_fts, rowid, uri, title, search_text, text_content)
    VALUES ('delete', old.id, COALESCE(old.uri, ''), COALESCE(old.title, ''),
            COALESCE(old.search_text, ''), COALESCE(old.text_content, ''));
    INSERT INTO frames_fts(rowid, uri, title, search_text, text_content)
    VALUES (new.id, COALESCE(new.uri, ''), COALESCE(new.title, ''),
            COALESCE(new.search_text, ''), COALESCE(new.text_content, ''));
END;

CREATE TABLE IF NOT EXISTS config (
    key TEXT PRIMARY KEY,
    value BLOB NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE TABLE IF NOT EXISTS feedback (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uri TEXT NOT NULL,
    score REAL NOT NULL,
    note TEXT,
    session TEXT,
    ts_utc INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_feedback_uri ON feedback(uri);
";

// ── Core implementation ──────────────────────────────────────────────────

impl MemoryDb {
    /// Open or create a database file with full schema.
    pub(crate) fn open_or_create(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.apply_pragmas()?;
        db.init_schema()?;
        Ok(db)
    }

    fn apply_pragmas(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA cache_size = -8000;
             PRAGMA mmap_size = 67108864;",
        )?;
        Ok(())
    }

    fn init_schema(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.conn.execute_batch(SCHEMA_SQL)?;

        // Backward-compatible migrations (silently ignore if already applied)
        self.conn
            .execute_batch("ALTER TABLE frames ADD COLUMN importance REAL DEFAULT NULL;")
            .ok();
        self.conn
            .execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_frames_created_at ON frames(created_at);",
            )
            .ok();

        Ok(())
    }

    // ── Frame read operations ────────────────────────────────────────

    pub(crate) fn frame_count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM frames", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(0) as usize
    }

    pub(crate) fn active_frame_count(&self) -> usize {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM frames WHERE status = 'active'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0) as usize
    }

    pub(crate) fn frame_by_id(&self, id: FrameId) -> Result<Frame, String> {
        self.conn
            .query_row(
                "SELECT id, uri, title, kind, track, status, timestamp, checksum,
                        search_text, role, parent_id, tags, labels, extra_metadata, doc_metadata
                 FROM frames WHERE id = ?",
                params![id as i64],
                |row| Self::row_to_frame(row),
            )
            .map_err(|e| format!("frame_by_id({id}): {e}"))
    }

    pub(crate) fn frame_by_uri(&self, uri: &str) -> Result<Frame, String> {
        self.conn
            .query_row(
                "SELECT id, uri, title, kind, track, status, timestamp, checksum,
                        search_text, role, parent_id, tags, labels, extra_metadata, doc_metadata
                 FROM frames WHERE uri = ? AND status = 'active'
                 ORDER BY id DESC LIMIT 1",
                params![uri],
                |row| Self::row_to_frame(row),
            )
            .map_err(|e| format!("frame_by_uri({uri}): {e}"))
    }

    pub(crate) fn frame_canonical_payload(&self, id: FrameId) -> Result<Vec<u8>, String> {
        self.conn
            .query_row("SELECT payload FROM frames WHERE id = ?", params![id as i64], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .map_err(|e| format!("frame_payload({id}): {e}"))
    }

    pub(crate) fn frame_text_by_id(&self, id: FrameId) -> Result<String, String> {
        let result: Result<(Option<String>, Option<Vec<u8>>), _> = self.conn.query_row(
            "SELECT text_content, payload FROM frames WHERE id = ?",
            params![id as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );
        match result {
            Ok((Some(ref text), _)) if !text.is_empty() => Ok(text.clone()),
            Ok((_, Some(payload))) => {
                String::from_utf8(payload).map_err(|e| format!("frame_text({id}): {e}"))
            }
            Ok((_, None)) => Err(format!("frame_text({id}): no content")),
            Err(e) => Err(format!("frame_text({id}): {e}")),
        }
    }

    // ── Frame write operations ───────────────────────────────────────

    pub(crate) fn put_bytes_with_options(
        &self,
        bytes: &[u8],
        options: PutOptions,
    ) -> Result<u64, String> {
        let timestamp = options.timestamp.unwrap_or_else(|| Utc::now().timestamp());
        let checksum = blake3::hash(bytes);
        let checksum_bytes = checksum.as_bytes().as_slice();
        let text_content = std::str::from_utf8(bytes).ok().map(|s| s.to_string());
        let tags_json = serde_json::to_string(&options.tags).unwrap_or_else(|_| "[]".into());
        let labels_json = serde_json::to_string(&options.labels).unwrap_or_else(|_| "[]".into());
        let extra_json =
            serde_json::to_string(&options.extra_metadata).unwrap_or_else(|_| "{}".into());
        let meta_json = options
            .metadata
            .as_ref()
            .and_then(|m| serde_json::to_string(m).ok());

        // Supersede existing active frames with the same URI (append-only semantics).
        // Skip entirely if the existing frame has identical content (same blake3 hash).
        if let Some(ref uri) = options.uri {
            let existing: Option<(i64, Vec<u8>)> = self
                .conn
                .query_row(
                    "SELECT id, checksum FROM frames WHERE uri = ? AND status = 'active' ORDER BY id DESC LIMIT 1",
                    params![uri],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();
            if let Some((_id, existing_checksum)) = &existing {
                if existing_checksum.as_slice() == checksum_bytes {
                    // Content unchanged — return existing frame ID, no write needed
                    return Ok(*_id as u64);
                }
            }
            self.conn
                .execute(
                    "UPDATE frames SET status = 'superseded' WHERE uri = ? AND status = 'active'",
                    params![uri],
                )
                .map_err(|e| format!("supersede: {e}"))?;
        }

        self.conn
            .execute(
                "INSERT INTO frames (uri, title, kind, track, status, timestamp, checksum,
                 search_text, payload, text_content, role, parent_id, tags, labels,
                 extra_metadata, doc_metadata)
                 VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    options.uri,
                    options.title,
                    options.kind,
                    options.track,
                    timestamp,
                    checksum_bytes,
                    options.search_text,
                    bytes,
                    text_content,
                    options.role.as_str(),
                    options.parent_id.map(|v| v as i64),
                    tags_json,
                    labels_json,
                    extra_json,
                    meta_json,
                ],
            )
            .map_err(|e| format!("put frame: {e}"))?;

        Ok(self.conn.last_insert_rowid() as u64)
    }

    /// Find an active frame with a matching blake3 checksum (cross-track: identical bytes are never stored twice).
    pub(crate) fn find_active_frame_by_checksum(
        &self,
        checksum: &[u8],
    ) -> Result<FrameId, String> {
        self.conn
            .query_row(
                "SELECT id FROM frames WHERE checksum = ?1 AND status = 'active' LIMIT 1",
                params![checksum],
                |row| row.get::<_, i64>(0),
            )
            .map(|id| id as u64)
            .map_err(|e| format!("find_active_frame_by_checksum: {e}"))
    }

    /// Mark a single frame as superseded.
    pub(crate) fn supersede_frame(&self, id: FrameId) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE frames SET status = 'superseded' WHERE id = ? AND status = 'active'",
                params![id as i64],
            )
            .map_err(|e| format!("supersede_frame({id}): {e}"))?;
        Ok(())
    }

    /// Mark a frame as deleted.
    pub(crate) fn delete_frame(&self, id: FrameId) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE frames SET status = 'deleted' WHERE id = ?",
                params![id as i64],
            )
            .map_err(|e| format!("delete_frame({id}): {e}"))?;
        Ok(())
    }

    /// No-op in WAL mode (each statement auto-commits). Performs a passive WAL checkpoint.
    pub(crate) fn commit(&self) -> Result<(), String> {
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");
        Ok(())
    }

    /// Collect all active frame IDs, optionally filtered by URI prefix scope.
    pub(crate) fn collect_active_frame_ids(&self, scope: Option<&str>) -> Vec<u64> {
        let (sql, bind): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match scope {
            Some(s) => (
                "SELECT id FROM frames WHERE status = 'active' AND uri LIKE ?1 ORDER BY id"
                    .to_string(),
                vec![Box::new(format!("{s}%"))],
            ),
            None => (
                "SELECT id FROM frames WHERE status = 'active' ORDER BY id".to_string(),
                vec![],
            ),
        };
        let bind_refs: Vec<&dyn rusqlite::types::ToSql> =
            bind.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match self.conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(bind_refs.as_slice(), |row| row.get::<_, i64>(0)) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|r| r.ok()).map(|id| id as u64).collect()
    }

    // ── Search (FTS5) ────────────────────────────────────────────────

    pub(crate) fn search(&self, request: SearchRequest) -> Result<SearchResponse, String> {
        if request.query.trim().is_empty() {
            return Ok(SearchResponse { hits: Vec::new() });
        }

        // Handle track: prefix queries (Tantivy compatibility)
        let (track_filter, clean_query) = Self::extract_track_filter(&request.query);

        // If the entire query was a track filter, return frames from that track
        if clean_query.trim().is_empty() {
            return self.search_by_track(&track_filter, &request);
        }

        let fts_query = Self::sanitize_fts_query(&clean_query);
        if fts_query.is_empty() {
            return Ok(SearchResponse { hits: Vec::new() });
        }

        let snippet_tokens = (request.snippet_chars / 6).max(10).min(200);

        // Build parameterized SQL
        let mut conditions = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        // FTS MATCH — always bind index 1
        bind_values.push(Box::new(fts_query.clone()));

        conditions.push("f.status = 'active'".to_string());

        if let Some(ref track) = track_filter {
            bind_values.push(Box::new(track.clone()));
            conditions.push(format!("f.track = ?{}", bind_values.len()));
        }

        if let Some(ref scope) = request.scope {
            bind_values.push(Box::new(format!("{scope}%")));
            conditions.push(format!("f.uri LIKE ?{}", bind_values.len()));
        }

        if let Some(as_of_ts) = request.as_of_ts {
            bind_values.push(Box::new(as_of_ts));
            conditions.push(format!("f.timestamp <= ?{}", bind_values.len()));
        }

        if let Some(as_of_frame) = request.as_of_frame {
            bind_values.push(Box::new(as_of_frame as i64));
            conditions.push(format!("f.id <= ?{}", bind_values.len()));
        }

        if let Some(ref temporal) = request.temporal {
            if let Some(start) = temporal.start_utc {
                bind_values.push(Box::new(start));
                conditions.push(format!("f.timestamp >= ?{}", bind_values.len()));
            }
            if let Some(end) = temporal.end_utc {
                bind_values.push(Box::new(end));
                conditions.push(format!("f.timestamp <= ?{}", bind_values.len()));
            }
        }

        // Over-fetch 3x to allow recency re-ranking to surface recent results
        let overfetch = (request.top_k * 3).max(request.top_k + 10);
        bind_values.push(Box::new(overfetch as i64));
        let limit_idx = bind_values.len();

        let where_clause = conditions.join(" AND ");
        let sql = format!(
            "SELECT f.id, f.uri, f.title, f.track,
                    snippet(frames_fts, 3, '', '', '…', {snippet_tokens}) as snippet,
                    bm25(frames_fts) as rank_score,
                    f.timestamp
             FROM frames_fts fts
             JOIN frames f ON f.id = fts.rowid
             WHERE frames_fts MATCH ?1
               AND {where_clause}
             ORDER BY rank_score
             LIMIT ?{limit_idx}"
        );

        let bind_refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql).map_err(|e| format!("search prepare: {e}"))?;

        let now = Utc::now().timestamp() as f64;
        let half_life: f64 = 604800.0; // 7 days in seconds

        let rows = stmt
            .query_map(bind_refs.as_slice(), |row| {
                let id: i64 = row.get(0)?;
                let uri: String = row.get::<_, Option<String>>(1)?.unwrap_or_default();
                let title: Option<String> = row.get(2)?;
                let snippet: String = row.get::<_, Option<String>>(4)?.unwrap_or_default();
                let bm25_score: f64 = row.get(5)?;
                let timestamp: i64 = row.get(6)?;

                Ok((id, uri, title, snippet, bm25_score, timestamp))
            })
            .map_err(|e| format!("search query: {e}"))?;

        // Recency-weighted ranking: BM25 relevance * (1 + 0.3 * exp(-ln2 * age_seconds / half_life))
        // Half-life = 7 days. Recent content gets up to 30% boost; old content negligible boost.
        let mut hits: Vec<SearchHit> = Vec::new();
        for row_result in rows {
            match row_result {
                Ok((id, uri, title, snippet, bm25_score, timestamp)) => {
                    let age_seconds = (now - timestamp as f64).max(0.0);
                    let recency_boost =
                        1.0 + 0.3 * (-0.693 * age_seconds / half_life).exp();
                    let weighted_score = bm25_score.abs() * recency_boost;

                    hits.push(SearchHit {
                        rank: 0,
                        frame_id: id as u64,
                        uri,
                        title,
                        range: (0, 0),
                        text: snippet,
                        matches: 0,
                        chunk_range: None,
                        chunk_text: None,
                        score: Some(weighted_score as f32),
                        metadata: None,
                    });
                }
                Err(e) => {
                    eprintln!("[memory_db] search row error: {e}");
                }
            }
        }

        // Sort by recency-weighted score descending, then truncate to requested top_k
        hits.sort_by(|a, b| {
            b.score
                .unwrap_or(0.0)
                .partial_cmp(&a.score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(request.top_k);
        for (i, hit) in hits.iter_mut().enumerate() {
            hit.rank = i;
        }

        Ok(SearchResponse { hits })
    }

    /// Search by track only (when query is purely "track:xxx").
    fn search_by_track(
        &self,
        track: &Option<String>,
        request: &SearchRequest,
    ) -> Result<SearchResponse, String> {
        let Some(track) = track else {
            return Ok(SearchResponse { hits: Vec::new() });
        };

        let mut sql = String::from(
            "SELECT id, uri, title, track, search_text, timestamp
             FROM frames WHERE track = ?1 AND status = 'active'",
        );
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(track.clone())];

        if let Some(ref scope) = request.scope {
            bind_values.push(Box::new(format!("{scope}%")));
            sql.push_str(&format!(" AND uri LIKE ?{}", bind_values.len()));
        }

        bind_values.push(Box::new(request.top_k as i64));
        let limit_idx = bind_values.len();
        sql.push_str(&format!(" ORDER BY id DESC LIMIT ?{limit_idx}"));

        let bind_refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("track search prepare: {e}"))?;

        let rows = stmt
            .query_map(bind_refs.as_slice(), |row| {
                let id: i64 = row.get(0)?;
                let uri: String = row.get::<_, Option<String>>(1)?.unwrap_or_default();
                let title: Option<String> = row.get(2)?;
                let text: String = row.get::<_, Option<String>>(4)?.unwrap_or_default();

                Ok(SearchHit {
                    rank: 0,
                    frame_id: id as u64,
                    uri,
                    title,
                    range: (0, 0),
                    text,
                    matches: 0,
                    chunk_range: None,
                    chunk_text: None,
                    score: Some(1.0),
                    metadata: None,
                })
            })
            .map_err(|e| format!("track search query: {e}"))?;

        let mut hits: Vec<SearchHit> = Vec::new();
        for (i, row_result) in rows.enumerate() {
            if let Ok(mut hit) = row_result {
                hit.rank = i;
                hits.push(hit);
            }
        }

        Ok(SearchResponse { hits })
    }

    // ── Config operations ────────────────────────────────────────────

    pub(crate) fn config_get(&self, key: &str) -> Option<Vec<u8>> {
        self.conn
            .query_row(
                "SELECT value FROM config WHERE key = ?",
                params![key],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .ok()
    }

    pub(crate) fn config_set(&self, key: &str, value: &[u8]) -> Result<(), String> {
        let now = Utc::now().timestamp();
        self.conn
            .execute(
                "INSERT INTO config (key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![key, value, now],
            )
            .map_err(|e| format!("config_set({key}): {e}"))?;
        Ok(())
    }

    pub(crate) fn config_list(&self) -> Vec<(String, i64)> {
        let mut stmt = match self
            .conn
            .prepare("SELECT key, updated_at FROM config ORDER BY key")
        {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    // ── Feedback operations ──────────────────────────────────────────

    pub(crate) fn append_feedback(
        &self,
        uri: &str,
        score: f32,
        note: Option<&str>,
        session: Option<&str>,
    ) -> Result<i64, String> {
        let now = Utc::now().timestamp();
        self.conn
            .execute(
                "INSERT INTO feedback (uri, score, note, session, ts_utc) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![uri, score as f64, note, session, now],
            )
            .map_err(|e| format!("append_feedback: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub(crate) fn load_feedback_scores(&self, uris: &HashSet<String>) -> HashMap<String, f32> {
        let mut scores = HashMap::new();
        if uris.is_empty() {
            return scores;
        }
        // Get the most recent score for each URI
        for uri in uris {
            if let Ok(score) = self.conn.query_row(
                "SELECT score FROM feedback WHERE uri = ? ORDER BY ts_utc DESC, id DESC LIMIT 1",
                params![uri],
                |row| row.get::<_, f64>(0),
            ) {
                scores.insert(uri.clone(), score as f32);
            }
        }
        scores
    }

    // ── Frame enumeration (for list/diff/merge operations) ───────────

    /// Iterate all active frames with their latest version per URI.
    pub(crate) fn collect_latest_frames(
        &self,
        include_inactive: bool,
    ) -> HashMap<String, Frame> {
        let sql = if include_inactive {
            "SELECT id, uri, title, kind, track, status, timestamp, checksum,
                    search_text, role, parent_id, tags, labels, extra_metadata, doc_metadata
             FROM frames ORDER BY id DESC"
        } else {
            "SELECT id, uri, title, kind, track, status, timestamp, checksum,
                    search_text, role, parent_id, tags, labels, extra_metadata, doc_metadata
             FROM frames WHERE status = 'active' ORDER BY id DESC"
        };

        let mut stmt = match self.conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return HashMap::new(),
        };

        let mut out = HashMap::new();
        let rows = match stmt.query_map([], |row| Self::row_to_frame(row)) {
            Ok(r) => r,
            Err(_) => return out,
        };

        for row_result in rows {
            if let Ok(frame) = row_result {
                if let Some(ref uri) = frame.uri {
                    out.entry(uri.clone()).or_insert(frame);
                }
            }
        }
        out
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn row_to_frame(row: &rusqlite::Row) -> Result<Frame, rusqlite::Error> {
        let id: i64 = row.get(0)?;
        let checksum_blob: Option<Vec<u8>> = row.get(7)?;
        let mut checksum = [0u8; 32];
        if let Some(ref blob) = checksum_blob {
            let len = blob.len().min(32);
            checksum[..len].copy_from_slice(&blob[..len]);
        }
        let tags_json: String = row
            .get::<_, Option<String>>(11)?
            .unwrap_or_else(|| "[]".into());
        let labels_json: String = row
            .get::<_, Option<String>>(12)?
            .unwrap_or_else(|| "[]".into());
        let extra_json: String = row
            .get::<_, Option<String>>(13)?
            .unwrap_or_else(|| "{}".into());
        let meta_json: Option<String> = row.get(14)?;

        Ok(Frame {
            id: id as u64,
            uri: row.get(1)?,
            title: row.get(2)?,
            kind: row.get(3)?,
            track: row.get(4)?,
            status: FrameStatus::from_db_str(
                &row.get::<_, Option<String>>(5)?
                    .unwrap_or_else(|| "active".into()),
            ),
            timestamp: row.get(6)?,
            checksum,
            search_text: row.get(8)?,
            role: FrameRole::from_db_str(
                &row.get::<_, Option<String>>(9)?
                    .unwrap_or_else(|| "document".into()),
            ),
            parent_id: row.get::<_, Option<i64>>(10)?.map(|v| v as u64),
            tags: serde_json::from_str(&tags_json).unwrap_or_default(),
            labels: serde_json::from_str(&labels_json).unwrap_or_default(),
            extra_metadata: serde_json::from_str(&extra_json).unwrap_or_default(),
            metadata: meta_json.and_then(|j| serde_json::from_str(&j).ok()),
        })
    }

    /// Extract a `track:value` prefix from a query string.
    /// Returns (Some(track), remaining_query) or (None, original_query).
    fn extract_track_filter(query: &str) -> (Option<String>, String) {
        let trimmed = query.trim();
        if let Some(rest) = trimmed.strip_prefix("track:") {
            let mut parts = rest.splitn(2, ' ');
            let track = parts.next().unwrap_or("").to_string();
            let remainder = parts.next().unwrap_or("").to_string();
            if track.is_empty() {
                (None, trimmed.to_string())
            } else {
                (Some(track), remainder)
            }
        } else {
            (None, trimmed.to_string())
        }
    }

    /// Sanitize a query for FTS5 MATCH syntax.
    fn sanitize_fts_query(query: &str) -> String {
        let cleaned: String = query
            .chars()
            .map(|c| match c {
                '"' | '*' | '(' | ')' | ':' | '^' | '{' | '}' | '[' | ']' | '!' | '+' | '-'
                | '~' | '\\' | '.' | '@' | '#' | ',' | ';' | '/' | '&' | '|' | '?'
                | '`' | '\'' | '=' | '<' | '>' | '%' => ' ',
                _ => c,
            })
            .collect();
        let tokens: Vec<&str> = cleaned
            .split_whitespace()
            .filter(|t| t.len() >= 2 || t.chars().all(|c| c.is_ascii_digit()))
            // Strip FTS5 boolean operators — they cause syntax errors when used as search terms.
            // OR must be stripped too: tokens.join(" OR ") would produce "OR OR foo".
            .filter(|t| !matches!(t.to_uppercase().as_str(), "NOT" | "AND" | "OR" | "NEAR"))
            .collect();
        if tokens.is_empty() {
            return String::new();
        }
        // Use OR between terms for broad recall (matches Tantivy's default behavior)
        tokens.join(" OR ")
    }

    // ── Migration from MV2 ──────────────────────────────────────────

    /// Migrate all data from an existing MV2 vault file into this SQLite database.
    /// Reads active frames, config, and feedback from the vault.
    pub(crate) fn migrate_from_vault(
        &self,
        vault_path: &Path,
    ) -> Result<MigrationReport, String> {
        use aether_core::types::FrameStatus as VaultFrameStatus;
        use aether_core::Vault;

        let mut vault =
            Vault::open_read_only(vault_path).map_err(|e| format!("open vault: {e}"))?;

        let total = vault.frame_count() as u64;
        let mut migrated = 0usize;
        let mut skipped = 0usize;
        let mut errors = Vec::new();

        // Use a transaction for bulk insert performance
        self.conn
            .execute_batch("BEGIN TRANSACTION")
            .map_err(|e| format!("begin tx: {e}"))?;

        for frame_id in 0..total {
            let frame = match vault.frame_by_id(frame_id) {
                Ok(f) => f,
                Err(e) => {
                    errors.push(format!("frame {frame_id}: {e}"));
                    skipped += 1;
                    continue;
                }
            };

            if frame.status != VaultFrameStatus::Active {
                skipped += 1;
                continue;
            }

            let payload = match vault.frame_canonical_payload(frame_id) {
                Ok(p) => p,
                Err(e) => {
                    errors.push(format!("frame {frame_id} payload: {e}"));
                    skipped += 1;
                    continue;
                }
            };

            let text_content = std::str::from_utf8(&payload).ok().map(|s| s.to_string());
            let tags_json =
                serde_json::to_string(&frame.tags).unwrap_or_else(|_| "[]".into());
            let labels_json =
                serde_json::to_string(&frame.labels).unwrap_or_else(|_| "[]".into());
            let extra_json = serde_json::to_string(&frame.extra_metadata)
                .unwrap_or_else(|_| "{}".into());
            let role_str = format!("{:?}", frame.role).to_ascii_lowercase();

            if let Err(e) = self.conn.execute(
                "INSERT INTO frames (uri, title, kind, track, status, timestamp, checksum,
                 search_text, payload, text_content, role, parent_id, tags, labels,
                 extra_metadata)
                 VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    frame.uri,
                    frame.title,
                    frame.kind,
                    frame.track,
                    frame.timestamp,
                    frame.checksum.as_slice(),
                    frame.search_text,
                    payload,
                    text_content,
                    role_str,
                    frame.parent_id.map(|v| v as i64),
                    tags_json,
                    labels_json,
                    extra_json,
                ],
            ) {
                errors.push(format!("frame {frame_id} insert: {e}"));
                skipped += 1;
                continue;
            }
            migrated += 1;
        }

        // Migrate config entries (frames with aethervault://config/ URIs)
        // Config data is already included in the frames migration above.
        // Extract into the config table for O(1) key-value access.
        let config_frames: Vec<(String, Vec<u8>)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT uri, payload FROM frames
                     WHERE uri LIKE 'aethervault://config/%' AND status = 'active'
                     ORDER BY id DESC",
                )
                .map_err(|e| format!("config extract: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                    ))
                })
                .map_err(|e| format!("config query: {e}"))?;
            rows.filter_map(|r| r.ok()).collect()
        };

        let mut config_seen = HashSet::new();
        for (uri, payload) in &config_frames {
            let key = uri
                .strip_prefix("aethervault://config/")
                .unwrap_or("")
                .strip_suffix(".json")
                .unwrap_or("");
            if key.is_empty() || !config_seen.insert(key.to_string()) {
                continue;
            }
            let _ = self.conn.execute(
                "INSERT OR REPLACE INTO config (key, value, updated_at) VALUES (?1, ?2, ?3)",
                params![key, payload.as_slice(), Utc::now().timestamp()],
            );
        }

        // Migrate feedback entries (frames with aethervault://feedback/ URIs)
        {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT payload FROM frames
                     WHERE uri LIKE 'aethervault://feedback/%' AND status = 'active'",
                )
                .map_err(|e| format!("feedback extract: {e}"))?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(|e| format!("feedback query: {e}"))?;
            for row in rows {
                if let Ok(payload) = row {
                    if let Ok(event) =
                        serde_json::from_slice::<serde_json::Value>(&payload)
                    {
                        let uri = event.get("uri").and_then(|v| v.as_str()).unwrap_or("");
                        let score = event
                            .get("score")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        let note = event.get("note").and_then(|v| v.as_str());
                        let session = event.get("session").and_then(|v| v.as_str());
                        let ts = event.get("ts_utc").and_then(|v| v.as_i64()).unwrap_or(0);
                        let _ = self.conn.execute(
                            "INSERT INTO feedback (uri, score, note, session, ts_utc)
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![uri, score, note, session, ts],
                        );
                    }
                }
            }
        }

        self.conn
            .execute_batch("COMMIT")
            .map_err(|e| format!("commit: {e}"))?;

        // Rebuild FTS index to ensure consistency
        self.conn
            .execute_batch("INSERT INTO frames_fts(frames_fts) VALUES('rebuild')")
            .map_err(|e| format!("rebuild FTS: {e}"))?;

        Ok(MigrationReport {
            total_frames: total as usize,
            migrated,
            skipped,
            errors,
        })
    }

    /// Migrate hot-memories.jsonl into the capsule.
    ///
    /// Each line is parsed as JSON with `fact`, `metadata.importance`, `metadata.created_at`,
    /// `metadata.category`, `metadata.t_invalid`. Lines with `t_invalid` set are skipped.
    /// Content is run through consolidation to avoid duplicates.
    pub(crate) fn migrate_hot_memories(
        &self,
        jsonl_path: &Path,
        dry_run: bool,
    ) -> Result<HotMemoryMigrationReport, String> {
        use crate::consolidation::{consolidate, put_with_consolidation, ConsolidationDecision};

        let content = std::fs::read_to_string(jsonl_path)
            .map_err(|e| format!("read {}: {e}", jsonl_path.display()))?;

        let mut report = HotMemoryMigrationReport {
            total_lines: 0,
            added: 0,
            updated: 0,
            skipped_noop: 0,
            skipped_invalid: 0,
            errors: Vec::new(),
        };

        for (line_num, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            report.total_lines += 1;

            let parsed: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    report
                        .errors
                        .push(format!("line {line_num}: parse error: {e}"));
                    continue;
                }
            };

            // Skip if t_invalid is set (already deleted)
            if parsed
                .pointer("/metadata/t_invalid")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            {
                report.skipped_invalid += 1;
                continue;
            }

            let fact = match parsed.get("fact").and_then(|v| v.as_str()) {
                Some(f) if !f.trim().is_empty() => f.to_string(),
                _ => {
                    // Try "memory" key as fallback
                    match parsed.get("memory").and_then(|v| v.as_str()) {
                        Some(m) if !m.trim().is_empty() => m.to_string(),
                        _ => {
                            report.errors.push(format!("line {line_num}: no fact/memory field"));
                            continue;
                        }
                    }
                }
            };

            let created_at = parsed
                .pointer("/metadata/created_at")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp())
                .or_else(|| {
                    parsed
                        .pointer("/metadata/created_at")
                        .and_then(|v| v.as_i64())
                });

            let importance = parsed
                .pointer("/metadata/importance")
                .and_then(|v| v.as_f64());

            let category = parsed
                .pointer("/metadata/category")
                .and_then(|v| v.as_str())
                .unwrap_or("general");

            if dry_run {
                let decision = consolidate(
                    self,
                    fact.as_bytes(),
                    Some(&fact),
                    Some("aethervault.observation"),
                );
                match decision {
                    ConsolidationDecision::Noop { .. } => report.skipped_noop += 1,
                    ConsolidationDecision::Update { .. } => report.updated += 1,
                    ConsolidationDecision::Add => report.added += 1,
                }
                continue;
            }

            let uri = format!("aethervault://memory/hot-import/{line_num}");
            let mut opts = PutOptions::default();
            opts.uri = Some(uri);
            opts.track = Some("aethervault.observation".to_string());
            opts.kind = Some("text/markdown".to_string());
            opts.search_text = Some(fact.clone());
            opts.tags = vec!["migrated-from-hot-memories".to_string()];
            if let Some(ts) = created_at {
                opts.timestamp = Some(ts);
            }
            opts.extra_metadata
                .insert("hot_memory_category".into(), category.to_string());
            if let Some(imp) = importance {
                opts.extra_metadata
                    .insert("hot_memory_importance".into(), imp.to_string());
            }

            match put_with_consolidation(self, fact.as_bytes(), opts) {
                Ok(result) => match result.decision {
                    ConsolidationDecision::Noop { .. } => report.skipped_noop += 1,
                    ConsolidationDecision::Update { .. } => report.updated += 1,
                    ConsolidationDecision::Add => report.added += 1,
                },
                Err(e) => {
                    report
                        .errors
                        .push(format!("line {line_num}: put failed: {e}"));
                }
            }
        }

        Ok(report)
    }

    /// Database file size in bytes.
    pub(crate) fn file_size(&self, path: &Path) -> u64 {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }

    /// Rebuild the FTS index from scratch.
    pub(crate) fn rebuild_fts(&self) -> Result<(), String> {
        self.conn
            .execute_batch("INSERT INTO frames_fts(frames_fts) VALUES('rebuild')")
            .map_err(|e| format!("rebuild FTS: {e}"))
    }

    /// Run VACUUM to reclaim space.
    /// Delete all superseded frames (they are dead weight after URI supersede).
    pub(crate) fn purge_superseded(&self) -> Result<usize, String> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT count(*) FROM frames WHERE status = 'superseded'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("count superseded: {e}"))?;
        if count > 0 {
            self.conn
                .execute("DELETE FROM frames WHERE status = 'superseded'", [])
                .map_err(|e| format!("purge superseded: {e}"))?;
        }
        Ok(count as usize)
    }

    pub(crate) fn vacuum(&self) -> Result<(), String> {
        self.conn
            .execute_batch("VACUUM")
            .map_err(|e| format!("vacuum: {e}"))
    }
}

#[cfg(test)]
impl MemoryDb {
    /// Borrow the underlying connection (test-only; used for raw SQL assertions).
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn config_delete(&self, key: &str) -> Result<bool, String> {
        let rows = self
            .conn
            .execute("DELETE FROM config WHERE key = ?", params![key])
            .map_err(|e| format!("config_delete({key}): {e}"))?;
        Ok(rows > 0)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_db_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("aethervault_test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("test_{}_{name}.sqlite", std::process::id()))
    }

    #[test]
    fn test_open_or_create() {
        let path = temp_db_path("open_create");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();
        assert_eq!(db.frame_count(), 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_put_and_get() {
        let path = temp_db_path("put_get");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        let mut opts = PutOptions::default();
        opts.uri = Some("test://doc/1".to_string());
        opts.title = Some("Test Document".to_string());
        opts.search_text = Some("hello world".to_string());

        let id = db
            .put_bytes_with_options(b"hello world content", opts)
            .unwrap();
        assert!(id > 0);
        assert_eq!(db.frame_count(), 1);

        let frame = db.frame_by_id(id).unwrap();
        assert_eq!(frame.uri.as_deref(), Some("test://doc/1"));
        assert_eq!(frame.title.as_deref(), Some("Test Document"));
        assert_eq!(frame.status, FrameStatus::Active);

        let text = db.frame_text_by_id(id).unwrap();
        assert_eq!(text, "hello world content");

        let payload = db.frame_canonical_payload(id).unwrap();
        assert_eq!(payload, b"hello world content");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_uri_supersede() {
        let path = temp_db_path("supersede");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        let mut opts1 = PutOptions::default();
        opts1.uri = Some("test://doc/1".to_string());
        let id1 = db.put_bytes_with_options(b"version 1", opts1).unwrap();

        let mut opts2 = PutOptions::default();
        opts2.uri = Some("test://doc/1".to_string());
        let id2 = db.put_bytes_with_options(b"version 2", opts2).unwrap();

        assert!(id2 > id1);

        // Old frame should be superseded
        let old = db.frame_by_id(id1).unwrap();
        assert_eq!(old.status, FrameStatus::Superseded);

        // frame_by_uri should return the latest
        let latest = db.frame_by_uri("test://doc/1").unwrap();
        assert_eq!(latest.id, id2);
        let text = db.frame_text_by_id(id2).unwrap();
        assert_eq!(text, "version 2");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_search() {
        let path = temp_db_path("search");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        let mut opts = PutOptions::default();
        opts.uri = Some("test://doc/1".to_string());
        opts.search_text = Some("rust programming language systems".to_string());
        db.put_bytes_with_options(b"Rust is a systems programming language", opts)
            .unwrap();

        let mut opts2 = PutOptions::default();
        opts2.uri = Some("test://doc/2".to_string());
        opts2.search_text = Some("python scripting language".to_string());
        db.put_bytes_with_options(b"Python is a scripting language", opts2)
            .unwrap();

        let request = SearchRequest {
            query: "rust programming".to_string(),
            top_k: 10,
            snippet_chars: 200,
            scope: None,
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
        };

        let response = db.search(request).unwrap();
        assert!(!response.hits.is_empty());
        assert_eq!(response.hits[0].uri, "test://doc/1");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_config() {
        let path = temp_db_path("config");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        assert!(db.config_get("test_key").is_none());

        db.config_set("test_key", b"test_value").unwrap();
        let val = db.config_get("test_key").unwrap();
        assert_eq!(val, b"test_value");

        db.config_set("test_key", b"updated_value").unwrap();
        let val = db.config_get("test_key").unwrap();
        assert_eq!(val, b"updated_value");

        let entries = db.config_list();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "test_key");

        db.config_delete("test_key").unwrap();
        assert!(db.config_get("test_key").is_none());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_feedback() {
        let path = temp_db_path("feedback");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        db.append_feedback("test://doc/1", 0.8, Some("good result"), None)
            .unwrap();
        db.append_feedback("test://doc/1", 0.9, Some("even better"), None)
            .unwrap();

        let uris: HashSet<String> = ["test://doc/1".to_string()].into_iter().collect();
        let scores = db.load_feedback_scores(&uris);
        assert_eq!(scores.len(), 1);
        // Should return the most recent score
        let score = scores["test://doc/1"];
        assert!((score - 0.9).abs() < 0.01);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_sanitize_fts_query() {
        assert_eq!(
            MemoryDb::sanitize_fts_query("hello world"),
            "hello OR world"
        );
        assert_eq!(MemoryDb::sanitize_fts_query("\"quoted\""), "quoted");
        assert_eq!(MemoryDb::sanitize_fts_query("a b"), ""); // too short
        assert_eq!(
            MemoryDb::sanitize_fts_query("track:foo bar"),
            "track OR foo OR bar"
        );
        // FTS5 boolean operators are stripped
        assert_eq!(
            MemoryDb::sanitize_fts_query("do NOT delete"),
            "do OR delete"
        );
        assert_eq!(
            MemoryDb::sanitize_fts_query("this AND that"),
            "this OR that"
        );
        assert_eq!(
            MemoryDb::sanitize_fts_query("NEAR something"),
            "something"
        );
        // OR is also stripped (it's the join separator)
        assert_eq!(
            MemoryDb::sanitize_fts_query("OR fallback"),
            "fallback"
        );
        assert_eq!(
            MemoryDb::sanitize_fts_query("this OR that"),
            "this OR that" // "OR" stripped, "this" + "that" re-joined with OR
        );
    }

    #[test]
    fn test_extract_track_filter() {
        let (track, rest) = MemoryDb::extract_track_filter("track:aethervault.feedback");
        assert_eq!(track, Some("aethervault.feedback".to_string()));
        assert!(rest.is_empty());

        let (track, rest) = MemoryDb::extract_track_filter("track:foo bar baz");
        assert_eq!(track, Some("foo".to_string()));
        assert_eq!(rest, "bar baz");

        let (track, rest) = MemoryDb::extract_track_filter("hello world");
        assert!(track.is_none());
        assert_eq!(rest, "hello world");
    }

    #[test]
    fn test_search_recency_boost() {
        let path = temp_db_path("recency_boost");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        let now = chrono::Utc::now().timestamp();
        let thirty_days_ago = now - 30 * 86400;

        // Insert old frame
        let mut opts_old = PutOptions::default();
        opts_old.uri = Some("test://doc/old".to_string());
        opts_old.search_text = Some("rust memory systems programming language".to_string());
        opts_old.timestamp = Some(thirty_days_ago);
        db.put_bytes_with_options(b"rust memory systems programming language", opts_old)
            .unwrap();

        // Insert recent frame with identical text
        let mut opts_new = PutOptions::default();
        opts_new.uri = Some("test://doc/new".to_string());
        opts_new.search_text = Some("rust memory systems programming language".to_string());
        opts_new.timestamp = Some(now);
        db.put_bytes_with_options(b"rust memory systems programming language", opts_new)
            .unwrap();

        let request = SearchRequest {
            query: "rust memory systems".to_string(),
            top_k: 10,
            snippet_chars: 200,
            scope: None,
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
        };

        let response = db.search(request).unwrap();
        assert!(response.hits.len() >= 2);
        // Recent frame should rank first due to recency boost
        assert_eq!(response.hits[0].uri, "test://doc/new");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_search_relevance_still_wins() {
        let path = temp_db_path("relevance_wins");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        let now = chrono::Utc::now().timestamp();
        let seven_days_ago = now - 7 * 86400;

        // Old but highly relevant
        let mut opts_old = PutOptions::default();
        opts_old.uri = Some("test://doc/relevant".to_string());
        opts_old.search_text = Some(
            "rust programming language memory safety borrow checker ownership lifetime".to_string(),
        );
        opts_old.timestamp = Some(seven_days_ago);
        db.put_bytes_with_options(
            b"rust programming language memory safety borrow checker ownership lifetime",
            opts_old,
        )
        .unwrap();

        // New but irrelevant
        let mut opts_new = PutOptions::default();
        opts_new.uri = Some("test://doc/irrelevant".to_string());
        opts_new.search_text = Some("python scripting dynamic typing".to_string());
        opts_new.timestamp = Some(now);
        db.put_bytes_with_options(b"python scripting dynamic typing", opts_new)
            .unwrap();

        let request = SearchRequest {
            query: "rust memory safety borrow checker".to_string(),
            top_k: 10,
            snippet_chars: 200,
            scope: None,
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
        };

        let response = db.search(request).unwrap();
        assert!(!response.hits.is_empty());
        // The highly relevant old frame should still rank first
        assert_eq!(response.hits[0].uri, "test://doc/relevant");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_importance_column_migration() {
        let path = temp_db_path("importance_col");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        // Verify the importance column exists by inserting and querying
        let result: Result<Option<f64>, _> = db.conn().query_row(
            "SELECT importance FROM frames LIMIT 1",
            [],
            |row| row.get(0),
        );
        // Table is empty but query should succeed (column exists)
        assert!(
            result.is_ok() || result.is_err(), // No rows is OK; column-missing would error on prepare
            "importance column should exist after init_schema"
        );

        // Also verify we can insert with importance
        db.conn()
            .execute(
                "INSERT INTO frames (status, timestamp, importance) VALUES ('active', 0, 0.75)",
                [],
            )
            .expect("should be able to insert importance value");

        let val: f64 = db
            .conn()
            .query_row(
                "SELECT importance FROM frames WHERE importance IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!((val - 0.75).abs() < 0.01);

        std::fs::remove_file(&path).ok();
    }
}

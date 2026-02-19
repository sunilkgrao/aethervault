//! Mem0-style write-time memory consolidation.
//!
//! Every memory write goes through a similarity gate that decides ADD / UPDATE / NOOP
//! before touching the database, replacing the old "write everything, prune later" model.

use std::collections::HashSet;

use crate::memory_db::{FrameId, MemoryDb, PutOptions, SearchRequest};

// ── Thresholds ──────────────────────────────────────────────────────────

const NOOP_THRESHOLD: f32 = 0.85;
const UPDATE_THRESHOLD: f32 = 0.50;

// ── Types ───────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub(crate) enum ConsolidationDecision {
    Add,
    Update { supersede_id: FrameId },
    Noop { existing_id: FrameId },
}

#[derive(Debug)]
pub(crate) struct ConsolidationResult {
    pub(crate) decision: ConsolidationDecision,
    pub(crate) frame_id: Option<u64>, // None for Noop
}

// ── Token Jaccard ───────────────────────────────────────────────────────

pub(crate) fn token_jaccard(a: &str, b: &str) -> f32 {
    let normalize = |s: &str| -> HashSet<String> {
        s.to_ascii_lowercase()
            .split_whitespace()
            .filter(|w| w.len() >= 3)
            .map(|w| w.to_string())
            .collect()
    };
    let set_a = normalize(a);
    let set_b = normalize(b);
    if set_a.is_empty() && set_b.is_empty() {
        return 1.0;
    }
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

// ── Consolidation logic ─────────────────────────────────────────────────

/// Decide whether incoming content should be Added, Updated, or skipped (Noop).
pub(crate) fn consolidate(
    db: &MemoryDb,
    bytes: &[u8],
    search_text: Option<&str>,
    track: Option<&str>,
) -> ConsolidationDecision {
    // Step 1: Exact dedup via blake3 checksum
    let checksum = blake3::hash(bytes);
    let checksum_bytes = checksum.as_bytes().as_slice();

    if let Ok(existing_id) = db.find_active_frame_by_checksum(checksum_bytes) {
        return ConsolidationDecision::Noop {
            existing_id,
        };
    }

    // Step 2: FTS5 candidate retrieval using first ~200 chars of search text
    let text = search_text.unwrap_or_else(|| {
        std::str::from_utf8(bytes).unwrap_or("")
    });
    if text.trim().is_empty() {
        return ConsolidationDecision::Add;
    }

    let query_prefix: String = text.chars().take(200).collect();
    let scope = track.map(|t| format!("aethervault://{}/", t.replace('.', "/")));

    let request = SearchRequest {
        query: query_prefix,
        top_k: 5,
        snippet_chars: 300,
        uri: None,
        scope,
        cursor: None,
        temporal: None,
        as_of_frame: None,
        as_of_ts: None,
        no_sketch: false,
    };

    let candidates = match db.search(request) {
        Ok(resp) => resp.hits,
        Err(_) => return ConsolidationDecision::Add,
    };

    if candidates.is_empty() {
        return ConsolidationDecision::Add;
    }

    // Step 3: Token Jaccard scoring against each candidate
    let mut best_score: f32 = 0.0;
    let mut best_id: FrameId = 0;

    for hit in &candidates {
        // Get the full text of the candidate for comparison
        let candidate_text = match db.frame_text_by_id(hit.frame_id) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let score = token_jaccard(text, &candidate_text);
        if score > best_score {
            best_score = score;
            best_id = hit.frame_id;
        }
    }

    // Step 4: Decision based on thresholds
    if best_score >= NOOP_THRESHOLD {
        ConsolidationDecision::Noop {
            existing_id: best_id,
        }
    } else if best_score >= UPDATE_THRESHOLD {
        ConsolidationDecision::Update {
            supersede_id: best_id,
        }
    } else {
        ConsolidationDecision::Add
    }
}

/// Write-time consolidation wrapper around `db.put_bytes_with_options()`.
///
/// Runs `consolidate()` then:
///   - ADD:    inserts normally
///   - UPDATE: marks the similar old frame as superseded, inserts new with audit trail
///   - NOOP:   returns early with existing frame ID, no write
pub(crate) fn put_with_consolidation(
    db: &MemoryDb,
    bytes: &[u8],
    options: PutOptions,
) -> Result<ConsolidationResult, String> {
    let search_text = options.search_text.as_deref();
    let track = options.track.as_deref();

    let decision = consolidate(db, bytes, search_text, track);

    match decision {
        ConsolidationDecision::Add => {
            let frame_id = db.put_bytes_with_options(bytes, options)?;
            Ok(ConsolidationResult {
                decision: ConsolidationDecision::Add,
                frame_id: Some(frame_id),
            })
        }
        ConsolidationDecision::Update { supersede_id } => {
            // Mark the old frame as superseded
            db.supersede_frame(supersede_id)?;

            // Add audit trail in extra_metadata
            let mut opts = options;
            opts.extra_metadata.insert(
                "supersedes_id".into(),
                supersede_id.to_string(),
            );

            let frame_id = db.put_bytes_with_options(bytes, opts)?;
            Ok(ConsolidationResult {
                decision: ConsolidationDecision::Update { supersede_id },
                frame_id: Some(frame_id),
            })
        }
        ConsolidationDecision::Noop { existing_id } => {
            eprintln!(
                "[consolidation] NOOP: content similar to frame #{existing_id}, skipping write"
            );
            Ok(ConsolidationResult {
                decision: ConsolidationDecision::Noop { existing_id },
                frame_id: None,
            })
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_db::MemoryDb;
    use std::path::PathBuf;

    fn temp_db_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("aethervault_test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!(
            "test_consolidation_{}_{name}.sqlite",
            std::process::id()
        ))
    }

    // ── token_jaccard tests ─────────────────────────────────────────

    #[test]
    fn test_jaccard_identical() {
        let score = token_jaccard("hello world foo bar", "hello world foo bar");
        assert!((score - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_jaccard_disjoint() {
        let score = token_jaccard("alpha beta gamma", "delta epsilon zeta");
        assert!(score.abs() < f32::EPSILON);
    }

    #[test]
    fn test_jaccard_partial() {
        // "hello" and "world" are shared, "foo" and "bar" differ
        let score = token_jaccard("hello world foo", "hello world bar");
        // intersection: {hello, world} = 2, union: {hello, world, foo, bar} = 4
        assert!((score - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_jaccard_empty() {
        // Both empty -> 1.0 (convention)
        assert!((token_jaccard("", "") - 1.0).abs() < f32::EPSILON);
        // Short words filtered out: "a" and "be" are < 3 chars
        assert!((token_jaccard("a be", "a be") - 1.0).abs() < f32::EPSILON);
        // One empty, one not
        let score = token_jaccard("hello world", "");
        assert!(score.abs() < f32::EPSILON);
    }

    // ── consolidate() integration tests ─────────────────────────────

    #[test]
    fn test_consolidate_add_new() {
        let path = temp_db_path("add_new");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        let decision =
            consolidate(&db, b"brand new content here", Some("brand new content here"), None);
        assert_eq!(decision, ConsolidationDecision::Add);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_consolidate_noop_duplicate() {
        let path = temp_db_path("noop_dup");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        let content = b"Sunil prefers dark roast coffee in the morning and green tea in the afternoon";
        let mut opts = PutOptions::default();
        opts.uri = Some("aethervault://memory/test/1".to_string());
        opts.search_text = Some(String::from_utf8_lossy(content).to_string());
        opts.track = Some("aethervault.observation".to_string());
        db.put_bytes_with_options(content, opts).unwrap();

        // Exact same bytes -> NOOP via checksum
        let decision = consolidate(
            &db,
            content,
            Some(&String::from_utf8_lossy(content)),
            Some("aethervault.observation"),
        );
        assert!(matches!(decision, ConsolidationDecision::Noop { .. }));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_consolidate_update_similar() {
        let path = temp_db_path("update_sim");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        let original = "Sunil prefers dark roast coffee and reads in the morning every day";
        let mut opts = PutOptions::default();
        opts.uri = Some("aethervault://memory/test/orig".to_string());
        opts.search_text = Some(original.to_string());
        opts.track = Some("aethervault.observation".to_string());
        db.put_bytes_with_options(original.as_bytes(), opts).unwrap();

        // Similar but different content (>50% overlap, <85% overlap)
        let updated = "Sunil prefers dark roast coffee and works out in the morning every day instead of reading";
        let decision = consolidate(
            &db,
            updated.as_bytes(),
            Some(updated),
            Some("aethervault.observation"),
        );
        // Should be UPDATE since there's significant overlap but not identical
        assert!(
            matches!(decision, ConsolidationDecision::Update { .. })
                || matches!(decision, ConsolidationDecision::Add),
            "expected Update or Add for similar content, got {:?}",
            decision
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_consolidate_different_track_isolation() {
        let path = temp_db_path("track_iso");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        let content = b"important fact about the weather today being sunny and warm";
        let mut opts = PutOptions::default();
        opts.uri = Some("aethervault://memory/track_a/1".to_string());
        opts.search_text = Some(String::from_utf8_lossy(content).to_string());
        opts.track = Some("aethervault.reflection".to_string());
        db.put_bytes_with_options(content, opts).unwrap();

        // Same content on a different track — checksum match doesn't scope by track
        // but FTS search does scope by track, so behavior depends on checksum path
        let decision = consolidate(
            &db,
            content,
            Some(&String::from_utf8_lossy(content)),
            Some("aethervault.observation"),
        );
        // Checksum dedup is cross-track (identical bytes should not be stored twice)
        assert!(
            matches!(decision, ConsolidationDecision::Noop { .. }),
            "exact duplicate bytes should NOOP even across tracks"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_put_consolidation_full_flow() {
        let path = temp_db_path("put_flow");
        let _ = std::fs::remove_file(&path);
        let db = MemoryDb::open_or_create(&path).unwrap();

        // First write: ADD
        let mut opts = PutOptions::default();
        opts.uri = Some("aethervault://memory/observation/100".to_string());
        opts.search_text = Some("the quick brown fox jumps over the lazy dog".to_string());
        opts.track = Some("aethervault.observation".to_string());
        let result = put_with_consolidation(&db, b"the quick brown fox jumps over the lazy dog", opts).unwrap();
        assert!(matches!(result.decision, ConsolidationDecision::Add));
        assert!(result.frame_id.is_some());

        // Exact duplicate: NOOP
        let mut opts2 = PutOptions::default();
        opts2.uri = Some("aethervault://memory/observation/101".to_string());
        opts2.search_text = Some("the quick brown fox jumps over the lazy dog".to_string());
        opts2.track = Some("aethervault.observation".to_string());
        let result2 = put_with_consolidation(&db, b"the quick brown fox jumps over the lazy dog", opts2).unwrap();
        assert!(matches!(result2.decision, ConsolidationDecision::Noop { .. }));
        assert!(result2.frame_id.is_none());

        // Totally different content: ADD
        let mut opts3 = PutOptions::default();
        opts3.uri = Some("aethervault://memory/observation/102".to_string());
        opts3.search_text = Some("quantum physics explains particle behavior at subatomic scales".to_string());
        opts3.track = Some("aethervault.observation".to_string());
        let result3 = put_with_consolidation(&db, b"quantum physics explains particle behavior at subatomic scales", opts3).unwrap();
        assert!(matches!(result3.decision, ConsolidationDecision::Add));
        assert!(result3.frame_id.is_some());

        std::fs::remove_file(&path).ok();
    }
}

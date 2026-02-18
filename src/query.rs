use std::collections::HashMap;
use std::path::Path;

use crate::memory_db::{
    Frame, FrameStatus, MemoryDb, SearchHit, SearchRequest, SearchResponse, TemporalFilter,
};
use chrono::Utc;
use serde_json;

use super::*;

pub(crate) fn frame_to_summary(frame: &Frame) -> Option<FrameSummary> {
    let uri = frame.uri.clone()?;
    Some(FrameSummary {
        uri,
        frame_id: frame.id,
        timestamp: frame.timestamp,
        checksum: checksum_hex(&frame.checksum),
        title: frame.title.clone(),
        track: frame.track.clone(),
        kind: frame.kind.clone(),
        status: frame.status.as_str().to_string(),
    })
}

pub(crate) fn collect_latest_frames(db: &MemoryDb, include_inactive: bool) -> HashMap<String, FrameSummary> {
    let frames = db.collect_latest_frames(include_inactive);
    let mut out = HashMap::new();
    for (uri, frame) in frames {
        if let Some(summary) = frame_to_summary(&frame) {
            out.insert(uri, summary);
        }
    }
    out
}

pub(crate) fn has_strong_signal(hits: &[SearchHit]) -> bool {
    let s1 = hits.first().and_then(|h| h.score).unwrap_or(0.0);
    let s2 = hits.get(1).and_then(|h| h.score).unwrap_or(0.0);
    if s1 <= 0.0 {
        return false;
    }
    if s1 <= 1.5 {
        s1 >= 0.85 && (s1 - s2) >= 0.15
    } else {
        let ratio = if s2 > 0.0 { s1 / s2 } else { 10.0 };
        s1 >= 2.0 && ratio >= 1.3
    }
}

pub(crate) fn build_ranked_list(lane: LaneKind, query: &str, is_base: bool, hits: &[SearchHit]) -> RankedList {
    let items = hits
        .iter()
        .enumerate()
        .map(|(i, hit)| Candidate {
            key: hit.uri.clone(),
            frame_id: hit.frame_id,
            uri: hit.uri.clone(),
            title: hit.title.clone(),
            snippet: hit.text.clone(),
            score: hit.score,
            lane,
            query: query.to_string(),
            rank: i + 1,
        })
        .collect();
    RankedList {
        lane,
        query: query.to_string(),
        is_base,
        items,
    }
}

pub(crate) fn rrf_fuse(lists: &[RankedList], k: f32) -> Vec<FusedCandidate> {
    let mut map: HashMap<String, FusedCandidate> = HashMap::new();

    for list in lists {
        let weight = if list.is_base { 2.0 } else { 1.0 };
        for (i, item) in list.items.iter().enumerate() {
            let rank = i + 1;
            let rrf = weight / (k + rank as f32);
            let bonus = if rank == 1 {
                0.05
            } else if rank <= 3 {
                0.02
            } else {
                0.0
            };

            let entry = map.entry(item.key.clone()).or_insert(FusedCandidate {
                key: item.key.clone(),
                frame_id: item.frame_id,
                uri: item.uri.clone(),
                title: item.title.clone(),
                snippet: item.snippet.clone(),
                best_rank: rank,
                rrf_score: 0.0,
                rrf_bonus: 0.0,
                sources: Vec::new(),
            });

            if rank < entry.best_rank {
                entry.best_rank = rank;
                entry.snippet = item.snippet.clone();
                entry.title = item.title.clone();
                entry.frame_id = item.frame_id;
                entry.uri = item.uri.clone();
            }

            entry.rrf_score += rrf;
            entry.rrf_bonus += bonus;
            entry
                .sources
                .push(format!("{}:{}#{}", list.lane.as_str(), list.query, rank));
        }
    }

    let mut fused: Vec<FusedCandidate> = map.into_values().collect();
    fused.sort_by(|a, b| {
        let sa = a.rrf_score + a.rrf_bonus;
        let sb = b.rrf_score + b.rrf_bonus;
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    fused
}

pub(crate) fn chunk_text(text: &str, max_chars: usize, overlap: usize) -> Vec<(String, usize)> {
    let len = text.len();
    if len == 0 {
        return vec![];
    }
    if len <= max_chars {
        return vec![(text.to_string(), 0)];
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut chunk_count = 0usize;
    let max_chunks = 200usize;
    while start < len && chunk_count < max_chunks {
        let mut end = (start + max_chars).min(len);
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        let chunk = text[start..end].to_string();
        chunks.push((chunk, start));
        if end == len {
            break;
        }
        let mut next_start = end.saturating_sub(overlap);
        while next_start > 0 && !text.is_char_boundary(next_start) {
            next_start -= 1;
        }
        if next_start == start {
            break;
        }
        start = next_start;
        chunk_count += 1;
    }
    chunks
}

pub(crate) fn rerank_score(query: &str, chunk: &str) -> f32 {
    let query_lower = query.to_ascii_lowercase();
    let terms: Vec<String> = query_lower
        .split_whitespace()
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_string())
        .collect();
    if terms.is_empty() {
        return 0.0;
    }

    let chunk_lower = chunk.to_ascii_lowercase();
    let mut matched = 0usize;
    let mut freq = 0usize;
    for term in &terms {
        if chunk_lower.contains(term) {
            matched += 1;
        }
        freq += chunk_lower.matches(term).count();
    }
    let coverage = matched as f32 / terms.len() as f32;
    let phrase_bonus = if chunk_lower.contains(&query_lower) {
        0.2
    } else {
        0.0
    };
    let freq_bonus = (freq as f32).ln_1p() * 0.05;
    let raw = coverage + phrase_bonus + freq_bonus;
    raw / (1.0 + raw)
}

pub(crate) fn print_plan(plan: &QueryPlan) {
    eprintln!("├─ {}", plan.cleaned_query);
    if !plan.lex_queries.is_empty() {
        for (i, q) in plan.lex_queries.iter().enumerate() {
            let prefix = if i == plan.lex_queries.len() - 1 && plan.vec_queries.is_empty() {
                "└─"
            } else {
                "├─"
            };
            eprintln!("{prefix} lex: {q}");
        }
    }
    if !plan.vec_queries.is_empty() {
        for (i, q) in plan.vec_queries.iter().enumerate() {
            let prefix = if i == plan.vec_queries.len() - 1 {
                "└─"
            } else {
                "├─"
            };
            eprintln!("{prefix} vec: {q}");
        }
    }
}

pub(crate) fn load_feedback_scores(
    db: &MemoryDb,
    targets: &std::collections::HashSet<String>,
) -> HashMap<String, f32> {
    db.load_feedback_scores(targets)
}

pub(crate) fn execute_query(
    db: &MemoryDb,
    args: QueryArgs,
) -> Result<QueryResponse, Box<dyn std::error::Error>> {
    let mut warnings = Vec::new();

    let (cleaned_query, parsed) = parse_query_markup(&args.raw_query);
    if cleaned_query.trim().is_empty() {
        return Err("Query is empty after removing markup tokens.".into());
    }

    let config = load_capsule_config(db);
    let hook_config = config.as_ref().and_then(|c| c.hooks.clone());
    let expansion_hook = resolve_hook_spec(
        args.expand_hook.clone(),
        args.expand_hook_timeout_ms,
        hook_config.as_ref().and_then(|h| h.expansion.clone()),
        None,
    );
    let rerank_hook = resolve_hook_spec(
        args.rerank_hook.clone(),
        args.rerank_hook_timeout_ms,
        hook_config.as_ref().and_then(|h| h.rerank.clone()),
        if args.rerank_hook_full_text {
            Some(true)
        } else {
            None
        },
    );

    let scope_collection = args.collection.or(parsed.collection);
    let scope = scope_collection.as_deref().map(scope_prefix);

    let asof_ts = args
        .asof
        .as_deref()
        .and_then(parse_date_to_ts)
        .or(parsed.asof_ts);

    let before_ts = args
        .before
        .as_deref()
        .and_then(parse_date_to_ts)
        .or(parsed.before_ts);
    let after_ts = args
        .after
        .as_deref()
        .and_then(parse_date_to_ts)
        .or(parsed.after_ts);

    let temporal = if before_ts.is_some() || after_ts.is_some() {
        Some(TemporalFilter {
            start_utc: after_ts,
            end_utc: before_ts,
            phrase: None,
            tz: None,
        })
    } else {
        None
    };

    let lane_limit = args.limit.max(20);

    // Probe for strong lexical signal to optionally skip expansion.
    let mut strong_signal = false;
    if !args.no_expand {
        let probe_request = SearchRequest {
            query: cleaned_query.clone(),
            top_k: 2,
            snippet_chars: 80,
            uri: None,
            scope: scope.clone(),
            cursor: None,
            temporal: temporal.clone(),
            as_of_frame: None,
            as_of_ts: asof_ts,
            no_sketch: false,
        };
        match db.search(probe_request) {
            Ok(resp) => {
                strong_signal = has_strong_signal(&resp.hits);
            }
            Err(err) => {
                warnings.push(format!("lex probe failed: {err}"));
            }
        }
    }

    let skipped_expansion = !args.no_expand && strong_signal;
    let (lex_queries, mut vec_queries) = if args.no_expand || strong_signal {
        (vec![cleaned_query.clone()], vec![cleaned_query.clone()])
    } else if let Some(hook) = expansion_hook.as_ref() {
        let input = ExpansionHookInput {
            query: cleaned_query.clone(),
            max_expansions: args.max_expansions,
            scope: scope.clone(),
            temporal: temporal.clone(),
        };
        match run_expansion_hook(hook, &input) {
            Ok(output) => {
                if !output.warnings.is_empty() {
                    warnings.extend(output.warnings);
                }
                let mut lex = output.lex;
                let mut vec = output.vec;
                if lex.is_empty() {
                    lex = vec![cleaned_query.clone()];
                }
                if vec.is_empty() {
                    vec = lex.clone();
                }
                (
                    lex.into_iter().take(args.max_expansions.max(1)).collect(),
                    vec.into_iter().take(args.max_expansions.max(1)).collect(),
                )
            }
            Err(err) => {
                warnings.push(format!("expansion hook failed: {err}"));
                (
                    build_expansions(&cleaned_query, args.max_expansions),
                    build_expansions(&cleaned_query, args.max_expansions),
                )
            }
        }
    } else {
        (
            build_expansions(&cleaned_query, args.max_expansions),
            build_expansions(&cleaned_query, args.max_expansions),
        )
    };
    if args.no_vector {
        vec_queries.clear();
    }
    // Vector search via local HNSW is no longer supported after MV2 removal.
    // Clear local vec queries unconditionally.
    vec_queries.clear();

    let plan_obj = QueryPlan {
        cleaned_query: cleaned_query.clone(),
        scope: scope.clone(),
        as_of_ts: asof_ts,
        temporal: temporal.clone(),
        skipped_expansion,
        lex_queries: lex_queries.clone(),
        vec_queries: vec_queries.clone(),
    };

    if args.plan {
        print_plan(&plan_obj);
    }

    let mut lists: Vec<RankedList> = Vec::new();

    for (i, q) in lex_queries.iter().enumerate() {
        let request = SearchRequest {
            query: q.clone(),
            top_k: lane_limit,
            snippet_chars: args.snippet_chars,
            uri: None,
            scope: scope.clone(),
            cursor: None,
            temporal: temporal.clone(),
            as_of_frame: None,
            as_of_ts: asof_ts,
            no_sketch: false,
        };
        let hits = match db.search(request) {
            Ok(resp) => resp.hits,
            Err(err) => {
                warnings.push(format!("lex search failed for '{q}': {err}"));
                Vec::new()
            }
        };
        if !hits.is_empty() {
            lists.push(build_ranked_list(LaneKind::Lex, q, i == 0, &hits));
        }
    }

    // --- Qdrant external vector lane ---
    if !args.no_vector {
        if let Some(qdrant_url) = env_optional("QDRANT_URL") {
            let collection = env_optional("QDRANT_COLLECTION").unwrap_or_else(|| "aethervault".to_string());
            match qdrant_search_text(&qdrant_url, &collection, &args.raw_query, lane_limit) {
                Ok(hits) if !hits.is_empty() => {
                    lists.push(build_ranked_list(LaneKind::Vec, &args.raw_query, false, &hits));
                }
                Ok(_) => {}
                Err(e) => {
                    warnings.push(format!("qdrant search failed: {e}"));
                }
            }
        }
    }

    if lists.is_empty() {
        return Ok(QueryResponse {
            query: args.raw_query,
            plan: plan_obj,
            warnings,
            results: Vec::new(),
        });
    }

    let fused = rrf_fuse(&lists, 60.0);

    let rerank_mode = if rerank_hook.is_some() {
        "hook"
    } else {
        args.rerank.as_str()
    };
    let mut rerank_scores: HashMap<String, (f32, Option<String>)> = HashMap::new();
    let mut rerank_active = false;

    match rerank_mode {
        "none" => {}
        "local" => {
            for cand in fused.iter().take(args.rerank_docs) {
                let text = match db.frame_text_by_id(cand.frame_id) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let chunks = chunk_text(&text, args.rerank_chunk_chars, args.rerank_chunk_overlap);
                let mut best_score = 0.0f32;
                let mut best_chunk = String::new();
                for (chunk, _) in chunks {
                    let score = rerank_score(&cleaned_query, &chunk);
                    if score > best_score {
                        best_score = score;
                        best_chunk = chunk;
                    }
                }
                rerank_scores.insert(cand.key.clone(), (best_score, Some(best_chunk)));
            }
            rerank_active = !rerank_scores.is_empty();
        }
        "hook" => {
            if let Some(hook) = rerank_hook.as_ref() {
                let include_text = hook.full_text.unwrap_or(false);
                let mut candidates = Vec::new();
                for cand in fused.iter().take(args.rerank_docs) {
                    let text = if include_text {
                        db.frame_text_by_id(cand.frame_id).ok()
                    } else {
                        None
                    };
                    candidates.push(RerankHookCandidate {
                        key: cand.key.clone(),
                        uri: cand.uri.clone(),
                        title: cand.title.clone(),
                        snippet: cand.snippet.clone(),
                        frame_id: cand.frame_id,
                        text,
                    });
                }
                let input = RerankHookInput {
                    query: cleaned_query.clone(),
                    candidates,
                };
                match run_rerank_hook(hook, &input) {
                    Ok(output) => {
                        if !output.warnings.is_empty() {
                            warnings.extend(output.warnings);
                        }
                        for (key, score) in output.scores {
                            let snippet = output.snippets.get(&key).cloned();
                            rerank_scores.insert(key, (score, snippet));
                        }
                        rerank_active = !rerank_scores.is_empty();
                    }
                    Err(err) => {
                        warnings.push(format!("rerank hook failed: {err}"));
                    }
                }
            } else {
                warnings.push("rerank hook selected but no hook configured".to_string());
            }
        }
        other => {
            warnings.push(format!("unknown rerank mode '{other}', defaulting to none"));
        }
    }

    let feedback_weight = args.feedback_weight.clamp(0.0, 1.0);
    let mut feedback_scores: HashMap<String, f32> = HashMap::new();
    if feedback_weight.abs() > 0.0 {
        let targets: std::collections::HashSet<String> =
            fused.iter().map(|c| c.uri.clone()).collect();
        feedback_scores = load_feedback_scores(db, &targets);
    }

    let mut results: Vec<QueryResult> = Vec::new();
    for (idx, cand) in fused.iter().enumerate() {
        let rrf_rank = idx + 1;
        let rrf_total = cand.rrf_score + cand.rrf_bonus;
        let rerank_score_opt = rerank_scores.get(&cand.key).map(|(s, _)| *s);
        let base_score = if rerank_active {
            let weight = if rrf_rank <= 3 {
                0.75
            } else if rrf_rank <= 10 {
                0.60
            } else {
                0.40
            };
            let rrf_rank_score = 1.0 / (rrf_rank as f32);
            let rerank_score = rerank_score_opt.unwrap_or(0.0);
            weight * rrf_rank_score + (1.0 - weight) * rerank_score
        } else {
            rrf_total
        };
        let feedback_score = feedback_scores.get(&cand.uri).copied();
        let score = if let Some(fb) = feedback_score {
            base_score + feedback_weight * fb
        } else {
            base_score
        };

        let mut snippet = cand.snippet.clone();
        if let Some((_, Some(override_snippet))) = rerank_scores.get(&cand.key) {
            if !override_snippet.trim().is_empty() {
                snippet = override_snippet.clone();
            }
        }

        results.push(QueryResult {
            rank: rrf_rank,
            frame_id: cand.frame_id,
            uri: cand.uri.clone(),
            title: cand.title.clone(),
            snippet,
            score,
            rrf_rank,
            rrf_score: rrf_total,
            rerank_score: rerank_score_opt,
            feedback_score,
            sources: cand.sources.clone(),
        });
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(args.limit);
    for (i, r) in results.iter_mut().enumerate() {
        r.rank = i + 1;
    }

    Ok(QueryResponse {
        query: args.raw_query,
        plan: plan_obj,
        warnings,
        results,
    })
}

pub(crate) fn build_context_pack(
    db: &MemoryDb,
    args: QueryArgs,
    max_bytes: usize,
    full: bool,
) -> Result<ContextPack, Box<dyn std::error::Error>> {
    let response = execute_query(db, args)?;
    let mut context = String::new();
    let mut citations = Vec::new();

    for r in &response.results {
        if context.len() >= max_bytes {
            break;
        }
        let header = format!(
            "[{}] {} {}\n",
            r.rank,
            r.uri,
            r.title.clone().unwrap_or_default()
        );
        let mut body = if full {
            db.frame_text_by_id(r.frame_id)
                .unwrap_or_else(|_| r.snippet.clone())
        } else {
            r.snippet.clone()
        };
        let remaining = max_bytes.saturating_sub(context.len() + header.len());
        if remaining == 0 {
            break;
        }
        if body.len() > remaining {
            body.truncate(remaining);
        }
        context.push_str(&header);
        context.push_str(&body);
        context.push_str("\n\n");

        citations.push(ContextCitation {
            rank: r.rank,
            frame_id: r.frame_id,
            uri: r.uri.clone(),
            title: r.title.clone(),
            score: r.score,
        });
    }

    Ok(ContextPack {
        query: response.query,
        plan: response.plan,
        warnings: response.warnings,
        citations,
        context,
    })
}

pub(crate) fn append_agent_log(
    _db: &MemoryDb,
    entry: &AgentLogEntry,
) -> Result<String, Box<dyn std::error::Error>> {
    // JSONL-only: agent logs are audit trail, not searchable knowledge.
    let workspace = resolve_workspace(None, &AgentConfig::default())
        .unwrap_or_else(|| std::path::PathBuf::from(DEFAULT_WORKSPACE_DIR));
    let log_dir = log_dir_path(&workspace);
    if let Err(e) = append_log_jsonl(&log_dir, entry) {
        eprintln!("[agent-log] JSONL write failed: {e}");
    }

    let bytes = serde_json::to_vec(entry)?;
    let ts = Utc::now().timestamp();
    let hash = blake3_hash(&bytes);
    let session_slug = entry
        .session
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let uri = format!(
        "aethervault://agent-log/{session_slug}/{ts}-{}",
        hash.to_hex()
    );

    Ok(uri)
}

pub(crate) fn append_feedback(
    db: &MemoryDb,
    event: &FeedbackEvent,
) -> Result<String, String> {
    let ts = Utc::now().timestamp();
    let bytes = serde_json::to_vec(event).map_err(|e| format!("serialize feedback: {e}"))?;
    let hash = blake3_hash(&bytes);
    let uri_log = format!("aethervault://feedback/{ts}-{}", hash.to_hex());

    db.append_feedback(
        &event.uri,
        event.score,
        event.note.as_deref(),
        event.session.as_deref(),
    )
    .map_err(|e| format!("append_feedback: {e}"))?;

    Ok(uri_log)
}

pub(crate) fn parse_log_ts_from_uri(uri: &str) -> Option<i64> {
    let tail = uri.rsplit('/').next()?;
    let ts_str = tail.split('-').next()?;
    ts_str.parse::<i64>().ok()
}

pub(crate) fn collect_mid_loop_reminders(
    reminder_state: &ReminderState,
    step: usize,
    max_steps: usize,
    token_est: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    if token_est > 60_000 {
        out.push("Context is growing. Switch to compact summaries and avoid verbose raw tool output.".to_string());
    }
    if token_est > 80_000 {
        out.push("Context is high. Keep tool calls minimal and summarize before next major step.".to_string());
    }
    if step > max_steps * 3 / 4 {
        out.push("Approaching step budget. Finish current objective with the smallest safe completion path.".to_string());
    }
    if reminder_state.last_tool_failed {
        out.push("Previous tool call failed. Reflect on what went wrong, then try a different approach.".to_string());
    }
    if reminder_state.same_tool_fail_streak >= 2 {
        out.push("You are retrying the same failing pattern. Try a different tool or different scope.".to_string());
    }
    if reminder_state.approval_required_count >= 2 {
        out.push("Multiple approval-required calls. Combine or batch work, then ask for one concise approval.".to_string());
    }
    if reminder_state.sequential_read_ops >= 2 {
        out.push("Independent read-only calls available; prefer batched parallel execution instead of sequential loops.".to_string());
    }
    if reminder_state.no_progress_streak >= 3 {
        out.push("No observable progress for several turns. Re-state your hypothesis, then pick one high-confidence next step.".to_string());
    }
    if reminder_state.reminder_ignored_count >= 1 {
        out.push("A prior system reminder was not followed. Treat the next instruction as hard constraint.".to_string());
    }
    out
}

pub(crate) fn detect_cycle(actions: &std::collections::VecDeque<String>) -> Option<(usize, usize)> {
    let len = actions.len();
    for cycle_len in 1..=5 {
        if len < cycle_len * 2 {
            continue;
        }
        let min_repeats = if cycle_len == 1 { 3 } else { 2 };
        if len < cycle_len * min_repeats {
            continue;
        }
        let pattern: Vec<&String> = actions.iter().rev().take(cycle_len).collect();
        let mut repeats = 1usize;
        'outer: for rep in 1..min_repeats {
            let start = cycle_len * rep;
            if start + cycle_len > len {
                break;
            }
            for (i, pat_item) in pattern.iter().enumerate() {
                let idx = len - 1 - start - i;
                if actions[idx] != **pat_item {
                    break 'outer;
                }
            }
            repeats += 1;
        }
        if repeats >= min_repeats {
            return Some((cycle_len, repeats));
        }
    }
    None
}

pub(crate) fn compute_drift_score(
    drift: &DriftState,
    reminder_state: &ReminderState,
    _tool_calls: &[AgentToolCall],
) -> f32 {
    let mut score: f32 = 100.0;
    if reminder_state.same_tool_fail_streak >= 2 {
        score -= 12.0;
    }
    if reminder_state.no_progress_streak >= 3 {
        score -= 12.0;
    }
    if reminder_state.last_tool_failed {
        score -= 5.0;
    }
    if drift.reminder_violations >= 1 {
        score -= 10.0;
    }
    if drift.reminder_violations >= 3 {
        score -= 8.0;
    }
    if reminder_state.approval_required_count >= 3 {
        score -= 6.0;
    }
    score.max(0.0)
}

pub(crate) fn scan_confidence_markers(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "fully deployed",
        "fully optimized",
        "fully complete",
        "fully implemented",
        "perfectly",
        "no issues",
        "completely done",
        "all tests pass",
        "everything works",
        "no problems",
        "100% complete",
        "flawlessly",
        "works perfectly",
        "successfully deployed",
        "production ready",
        "no errors",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

pub(crate) fn critic_should_fire(
    step: usize,
    base_interval: usize,
    last_critic_step: &mut usize,
    reminder_state: &ReminderState,
    tool_calls: &[AgentToolCall],
    messages: &[AgentMessage],
    violation_count: usize,
) -> bool {
    if !env_bool("CRITIC_ENABLED", true) {
        return false;
    }

    let interval = if violation_count >= 5 {
        1
    } else if violation_count >= 3 {
        2
    } else {
        base_interval
    };

    let within_interval = step > 0 && step.saturating_sub(*last_critic_step) < interval;

    if within_interval {
        let last_assistant = messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .and_then(|m| m.content.as_deref())
            .unwrap_or("");

        if scan_confidence_markers(last_assistant) {
            eprintln!("[critic] triggered: confidence markers detected");
            *last_critic_step = step;
            return true;
        }

        if reminder_state.last_tool_failed {
            let lower = last_assistant.to_ascii_lowercase();
            let acknowledges = lower.contains("error")
                || lower.contains("fail")
                || lower.contains("issue")
                || lower.contains("problem")
                || lower.contains("sorry");
            if !acknowledges {
                eprintln!("[critic] triggered: unacknowledged tool failure");
                *last_critic_step = step;
                return true;
            }
        }

        return false;
    }

    *last_critic_step = step;

    if tool_calls.len() >= 3 {
        eprintln!("[critic] triggered: periodic + large tool batch ({})", tool_calls.len());
    } else {
        eprintln!("[critic] triggered: periodic (step {})", step);
    }

    true
}

pub(crate) fn tool_autonomy_for(tool_name: &str) -> ToolAutonomyLevel {
    if let Ok(level_str) = std::env::var(format!("TOOL_AUTONOMY_{}", tool_name.to_ascii_uppercase())) {
        match level_str.as_str() {
            "autonomous" => return ToolAutonomyLevel::Autonomous,
            "suggest_only" => return ToolAutonomyLevel::SuggestOnly,
            "background" => return ToolAutonomyLevel::Background,
            _ => return ToolAutonomyLevel::Confirm,
        }
    }
    ToolAutonomyLevel::Confirm
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn detect_cycle_single_repeat() {
        let mut actions = VecDeque::new();
        actions.push_back("read:abc".to_string());
        actions.push_back("read:abc".to_string());
        actions.push_back("read:abc".to_string());
        let result = detect_cycle(&actions);
        assert!(result.is_some());
        let (cycle_len, repeats) = result.unwrap();
        assert_eq!(cycle_len, 1);
        assert_eq!(repeats, 3);
    }

    #[test]
    fn detect_cycle_no_cycle() {
        let mut actions = VecDeque::new();
        actions.push_back("read:abc".to_string());
        actions.push_back("write:def".to_string());
        actions.push_back("query:ghi".to_string());
        assert!(detect_cycle(&actions).is_none());
    }

    #[test]
    fn detect_cycle_two_step_pattern() {
        let mut actions = VecDeque::new();
        actions.push_back("read:abc".to_string());
        actions.push_back("write:def".to_string());
        actions.push_back("read:abc".to_string());
        actions.push_back("write:def".to_string());
        let result = detect_cycle(&actions);
        assert!(result.is_some());
        let (cycle_len, _) = result.unwrap();
        assert_eq!(cycle_len, 2);
    }

    #[test]
    fn detect_cycle_empty() {
        let actions = VecDeque::new();
        assert!(detect_cycle(&actions).is_none());
    }

    #[test]
    fn detect_cycle_too_few() {
        let mut actions = VecDeque::new();
        actions.push_back("read:abc".to_string());
        actions.push_back("read:abc".to_string());
        assert!(detect_cycle(&actions).is_none());
    }

    #[test]
    fn rrf_fuse_single_list() {
        let lists = vec![RankedList {
            lane: LaneKind::Lex,
            query: "test".to_string(),
            is_base: true,
            items: vec![
                Candidate {
                    key: "a".to_string(),
                    frame_id: 1,
                    uri: "uri:a".to_string(),
                    title: Some("A".to_string()),
                    snippet: "snippet a".to_string(),
                    score: Some(1.0),
                    lane: LaneKind::Lex,
                    query: "test".to_string(),
                    rank: 0,
                },
                Candidate {
                    key: "b".to_string(),
                    frame_id: 2,
                    uri: "uri:b".to_string(),
                    title: Some("B".to_string()),
                    snippet: "snippet b".to_string(),
                    score: Some(0.5),
                    lane: LaneKind::Lex,
                    query: "test".to_string(),
                    rank: 1,
                },
            ],
        }];
        let fused = rrf_fuse(&lists, 60.0);
        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].key, "a");
        assert!(fused[0].rrf_score > fused[1].rrf_score);
    }

    #[test]
    fn rrf_fuse_merge_across_lists() {
        let lists = vec![
            RankedList {
                lane: LaneKind::Lex,
                query: "q1".to_string(),
                is_base: true,
                items: vec![Candidate {
                    key: "shared".to_string(),
                    frame_id: 1,
                    uri: "uri:shared".to_string(),
                    title: Some("Shared".to_string()),
                    snippet: "s1".to_string(),
                    score: Some(1.0),
                    lane: LaneKind::Lex,
                    query: "q1".to_string(),
                    rank: 0,
                }],
            },
            RankedList {
                lane: LaneKind::Vec,
                query: "q2".to_string(),
                is_base: false,
                items: vec![Candidate {
                    key: "shared".to_string(),
                    frame_id: 1,
                    uri: "uri:shared".to_string(),
                    title: Some("Shared".to_string()),
                    snippet: "s2".to_string(),
                    score: Some(0.8),
                    lane: LaneKind::Vec,
                    query: "q2".to_string(),
                    rank: 0,
                }],
            },
        ];
        let fused = rrf_fuse(&lists, 60.0);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].sources.len(), 2);
    }

    #[test]
    fn rrf_fuse_empty() {
        let fused = rrf_fuse(&[], 60.0);
        assert!(fused.is_empty());
    }
}

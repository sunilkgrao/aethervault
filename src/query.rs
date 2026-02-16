#[allow(unused_imports)]
use std::collections::{HashMap, HashSet};
use std::path::Path;

use aether_core::types::{Frame, FrameStatus, SearchHit, SearchRequest, TemporalFilter};
use aether_core::{PutOptions, Vault};
use chrono::{TimeZone, Utc};
use serde_json;

#[allow(unused_imports)]
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
        status: format!("{:?}", frame.status).to_ascii_lowercase(),
    })
}

pub(crate) fn collect_latest_frames(mem: &mut Vault, include_inactive: bool) -> HashMap<String, FrameSummary> {
    let mut out = HashMap::new();
    let total = mem.frame_count() as i64;
    for idx in (0..total).rev() {
        let frame_id = idx as u64;
        let frame = match mem.frame_by_id(frame_id) {
            Ok(f) => f,
            Err(_) => continue,
        };
        if !include_inactive && frame.status != FrameStatus::Active {
            continue;
        }
        let summary = match frame_to_summary(&frame) {
            Some(s) => s,
            None => continue,
        };
        if !out.contains_key(&summary.uri) {
            out.insert(summary.uri.clone(), summary);
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
    mem: &mut Vault,
    targets: &std::collections::HashSet<String>,
) -> HashMap<String, f32> {
    let mut scores = HashMap::new();
    if targets.is_empty() {
        return scores;
    }

    let mut remaining = targets.clone();
    let total = mem.frame_count() as i64;
    for idx in (0..total).rev() {
        if remaining.is_empty() {
            break;
        }
        let frame_id = idx as u64;
        let frame = match mem.frame_by_id(frame_id) {
            Ok(f) => f,
            Err(_) => continue,
        };
        if frame.track.as_deref() != Some("aethervault.feedback") {
            continue;
        }

        let bytes = match mem.frame_canonical_payload(frame.id) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let event: FeedbackEvent = match serde_json::from_slice(&bytes) {
            Ok(e) => e,
            Err(_) => continue,
        };

        if remaining.remove(&event.uri) {
            scores.insert(event.uri, event.score);
        }
    }

    scores
}

pub(crate) fn execute_query(
    mem: &mut Vault,
    args: QueryArgs,
) -> Result<QueryResponse, Box<dyn std::error::Error>> {
    let mut warnings = Vec::new();

    let (cleaned_query, parsed) = parse_query_markup(&args.raw_query);
    if cleaned_query.trim().is_empty() {
        return Err("Query is empty after removing markup tokens.".into());
    }

    #[cfg(not(feature = "vec"))]
    let _ = (&args.embed_model, args.embed_cache, args.embed_no_cache);

    let config = load_capsule_config(mem);
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
        match mem.search(probe_request) {
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
    #[cfg(not(feature = "vec"))]
    {
        vec_queries.clear();
    }

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
        let hits = match mem.search(request) {
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

    #[cfg(feature = "vec")]
    if !args.no_vector {
        let embed_config = build_embed_config(
            args.embed_model.as_deref(),
            args.embed_cache,
            !args.embed_no_cache,
        );
        let embedder = match LocalTextEmbedder::new(embed_config) {
            Ok(e) => Some(e),
            Err(err) => {
                warnings.push(format!("vector embedder unavailable: {err}"));
                None
            }
        };

        if let Some(embedder) = embedder {
            let unique_vec_queries = dedup_keep_order(vec_queries.clone());
            let mut embed_map: HashMap<String, Vec<f32>> = HashMap::new();
            if !unique_vec_queries.is_empty() {
                let refs: Vec<&str> = unique_vec_queries.iter().map(|q| q.as_str()).collect();
                match embedder.embed_batch(&refs) {
                    Ok(embeddings) => {
                        for (q, emb) in unique_vec_queries
                            .iter()
                            .cloned()
                            .zip(embeddings.into_iter())
                        {
                            embed_map.insert(q, emb);
                        }
                    }
                    Err(err) => {
                        warnings.push(format!(
                            "embed batch failed ({err}), falling back to single embeddings"
                        ));
                        for q in &unique_vec_queries {
                            match embedder.embed_text(q) {
                                Ok(emb) => {
                                    embed_map.insert(q.clone(), emb);
                                }
                                Err(err) => {
                                    warnings.push(format!("embedding failed for '{q}': {err}"));
                                }
                            }
                        }
                    }
                }
            }

            for (i, q) in vec_queries.iter().enumerate() {
                let Some(embedding) = embed_map.get(q) else {
                    continue;
                };

                let mut resp = match mem.vec_search_with_embedding(
                    q,
                    embedding,
                    lane_limit,
                    args.snippet_chars,
                    scope.as_deref(),
                ) {
                    Ok(r) => r,
                    Err(err) => {
                        warnings.push(format!("vec search failed for '{q}': {err}"));
                        continue;
                    }
                };

                // Manual as-of / temporal filter for vector lane (best-effort).
                if asof_ts.is_some() || before_ts.is_some() || after_ts.is_some() {
                    resp.hits.retain(|hit| {
                        let frame = mem.frame_by_id(hit.frame_id).ok();
                        let Some(frame) = frame else { return false };
                        if let Some(ts) = asof_ts {
                            if frame.timestamp > ts {
                                return false;
                            }
                        }
                        if let Some(after_ts) = after_ts {
                            if frame.timestamp < after_ts {
                                return false;
                            }
                        }
                        if let Some(before_ts) = before_ts {
                            if frame.timestamp > before_ts {
                                return false;
                            }
                        }
                        true
                    });
                }

                if !resp.hits.is_empty() {
                    lists.push(build_ranked_list(LaneKind::Vec, q, i == 0, &resp.hits));
                }
            }
        }
    }

    #[cfg(not(feature = "vec"))]
    if !args.no_vector {
        warnings.push("vector lane disabled (build with --features vec)".to_string());
    }

    // --- Qdrant external vector lane ---
    // When QDRANT_URL is set, query Qdrant for additional vector results.
    // This supplements the local vec lane (if enabled) with a scalable external store.
    if !args.no_vector {
        if let Some(qdrant_url) = env_optional("QDRANT_URL") {
            let collection = env_optional("QDRANT_COLLECTION").unwrap_or_else(|| "aethervault".to_string());
            match qdrant_search_text(&qdrant_url, &collection, &args.raw_query, lane_limit) {
                Ok(hits) if !hits.is_empty() => {
                    lists.push(build_ranked_list(LaneKind::Vec, &args.raw_query, false, &hits));
                }
                Ok(_) => {} // no hits
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
                let text = match mem.frame_text_by_id(cand.frame_id) {
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
                        mem.frame_text_by_id(cand.frame_id).ok()
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
        feedback_scores = load_feedback_scores(mem, &targets);
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
    mem: &mut Vault,
    args: QueryArgs,
    max_bytes: usize,
    full: bool,
) -> Result<ContextPack, Box<dyn std::error::Error>> {
    let response = execute_query(mem, args)?;
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
            mem.frame_text_by_id(r.frame_id)
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
    mem: &mut Vault,
    entry: &AgentLogEntry,
) -> Result<String, Box<dyn std::error::Error>> {
    append_agent_log_with_commit(mem, entry, true)
}

pub(crate) fn append_agent_log_uncommitted(
    mem: &mut Vault,
    entry: &AgentLogEntry,
) -> Result<String, Box<dyn std::error::Error>> {
    append_agent_log_with_commit(mem, entry, false)
}

pub(crate) fn append_agent_log_with_commit(
    mem: &mut Vault,
    entry: &AgentLogEntry,
    commit: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    // Dual-write: JSONL file (primary) + MV2 capsule (legacy)
    let workspace = std::env::var("AETHERVAULT_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(DEFAULT_WORKSPACE_DIR));
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

    let mut options = PutOptions::default();
    options.uri = Some(uri.clone());
    options.title = Some(format!("agent log ({})", entry.role));
    options.kind = Some("application/json".to_string());
    options.track = Some("aethervault.agent".to_string());
    options.search_text = Some(entry.text.clone());
    options
        .extra_metadata
        .insert("session".into(), session_slug);
    options
        .extra_metadata
        .insert("role".into(), entry.role.clone());

    mem.put_bytes_with_options(&bytes, options)?;
    if commit {
        mem.commit()?;
    }
    Ok(uri)
}

pub(crate) fn append_feedback(
    mem: &mut Vault,
    event: &FeedbackEvent,
) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = serde_json::to_vec(event)?;
    let ts = Utc::now().timestamp();
    let hash = blake3_hash(&bytes);
    let uri_log = format!("aethervault://feedback/{ts}-{}", hash.to_hex());

    let mut options = PutOptions::default();
    options.uri = Some(uri_log.clone());
    options.title = Some("aethervault feedback".to_string());
    options.kind = Some("application/json".to_string());
    options.track = Some("aethervault.feedback".to_string());
    let mut search_text = event.uri.clone();
    if let Some(note) = event.note.clone() {
        search_text.push(' ');
        search_text.push_str(&note);
    }
    options.search_text = Some(search_text);
    mem.put_bytes_with_options(&bytes, options)?;
    mem.commit()?;
    Ok(uri_log)
}

pub(crate) fn merge_capsule_into(
    out: &mut Vault,
    src_path: &Path,
    dedup: bool,
    dedup_map: &mut HashMap<String, u64>,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let mut src = Vault::open_read_only(src_path)?;
    let mut written = 0usize;
    let mut deduped = 0usize;
    let mut id_map: HashMap<u64, u64> = HashMap::new();
    let total = src.frame_count() as u64;

    for frame_id in 0..total {
        let frame = match src.frame_by_id(frame_id) {
            Ok(f) => f,
            Err(_) => continue,
        };
        if frame.status != FrameStatus::Active {
            continue;
        }
        let uri = frame.uri.clone().unwrap_or_default();
        let key = format!(
            "{}|{}|{}",
            uri,
            checksum_hex(&frame.checksum),
            frame.timestamp
        );
        if dedup {
            if let Some(existing) = dedup_map.get(&key).copied() {
                id_map.insert(frame_id, existing);
                deduped += 1;
                continue;
            }
        }

        let payload = match src.frame_canonical_payload(frame_id) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("merge: skipping corrupt frame {} ({})", frame_id, e);
                deduped += 1;  // count as skipped
                continue;
            }
        };
        let mut options = PutOptions::default();
        options.timestamp = Some(frame.timestamp);
        options.track = frame.track.clone();
        options.kind = frame.kind.clone();
        options.uri = frame.uri.clone();
        options.title = frame.title.clone();
        options.metadata = frame.metadata.clone();
        options.search_text = frame.search_text.clone();
        options.tags = frame.tags.clone();
        options.labels = frame.labels.clone();
        options.extra_metadata = frame.extra_metadata.clone();
        options.role = frame.role;
        options.parent_id = frame.parent_id.and_then(|pid| id_map.get(&pid).copied());
        options.auto_tag = false;
        options.extract_dates = false;
        options.extract_triplets = false;

        let new_id = out.put_bytes_with_options(&payload, options)?;
        id_map.insert(frame_id, new_id);
        if dedup {
            dedup_map.insert(key, new_id);
        }
        written += 1;
    }

    Ok((written, deduped))
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

/// Heuristic scan for high-confidence markers in assistant text.
/// Returns true if the text contains phrases suggesting unverified claims.
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

/// Determine whether the covert critic should fire this step.
/// Updates `last_critic_step` when it decides to fire.
pub(crate) fn critic_should_fire(
    step: usize,
    interval: usize,
    last_critic_step: &mut usize,
    reminder_state: &ReminderState,
    tool_calls: &[AgentToolCall],
    messages: &[AgentMessage],
) -> bool {
    if !env_bool("CRITIC_ENABLED", true) {
        return false;
    }

    let within_interval = step > 0 && step.saturating_sub(*last_critic_step) < interval;

    if within_interval {
        // Check for high-priority triggers that override the interval
        let last_assistant = messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .and_then(|m| m.content.as_deref())
            .unwrap_or("");

        // Confidence signal: override interval
        if scan_confidence_markers(last_assistant) {
            eprintln!("[critic] triggered: confidence markers detected");
            *last_critic_step = step;
            return true;
        }

        // Tool failure without acknowledgment: override interval
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

    // Periodic trigger
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

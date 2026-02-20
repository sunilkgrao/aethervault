// Module declarations
mod cli;
mod types;
mod tool_args;
mod util;
mod config;
mod query;
mod tool_defs;
mod tool_exec;
mod mcp;
mod claude;
mod agent;
mod bridges;
mod services;
mod agent_log;
mod config_file;
mod memory_db;
mod consolidation;
mod skill_registry;

// Re-export all module items at crate root so cross-module references work.
// Before this split, everything lived in main.rs and shared a single namespace.
// These wildcard re-exports preserve that behavior.
pub(crate) use cli::*;
pub(crate) use types::*;
pub(crate) use tool_args::*;
pub(crate) use util::*;
pub(crate) use config::*;
pub(crate) use query::*;
pub(crate) use tool_defs::*;
pub(crate) use tool_exec::*;
pub(crate) use mcp::*;
pub(crate) use claude::*;
pub(crate) use agent::*;
pub(crate) use bridges::*;
pub(crate) use services::*;
pub(crate) use agent_log::*;
pub(crate) use config_file::*;
pub(crate) use skill_registry::*;

// External crate imports used directly in main()
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::memory_db::{Frame, FrameStatus, MemoryDb, PutOptions, SearchRequest};
use chrono::Utc;
use clap::Parser;
use serde::Serialize;
use walkdir::WalkDir;

#[derive(Debug, Serialize)]
struct ArchiveSummary {
    source: String,
    target: String,
    before: String,
    collection: String,
    scanned: usize,
    eligible: usize,
    archived: usize,
    deleted: usize,
    dry_run: bool,
}

#[derive(Debug, Serialize)]
struct DedupSummary {
    source: String,
    scanned: usize,
    unique_uris: usize,
    duplicates_removed: usize,
    keep_versions: usize,
    dry_run: bool,
}

#[derive(Debug, Serialize)]
struct StatsSummary {
    source: String,
    total_frames: u64,
    active_frames: u64,
    by_collection: Vec<(String, usize)>,
    by_age_days: Vec<(String, usize)>,
    by_size: Vec<(String, usize)>,
    duplicate_uris: usize,
    duplicate_frames: usize,
    total_breakdown: Vec<(String, u64)>,
}

fn frame_collection_name(frame: &Frame) -> String {
    if let Some(track) = frame.track.as_deref() {
        let normalized = normalize_collection(track);
        if !normalized.is_empty() {
            return normalized;
        }
    }

    let Some(uri) = frame.uri.as_deref() else {
        return "<no-uri>".to_string();
    };

    let Some(rest) = uri.strip_prefix("aethervault://") else {
        return "<non-aethervault-uri>".to_string();
    };
    rest
        .split('/')
        .next()
        .filter(|collection| !collection.is_empty())
        .map(normalize_collection)
        .unwrap_or_else(|| "<invalid-uri>".to_string())
}

fn frame_matches_collection(frame: &Frame, collection: &str) -> bool {
    let expected = normalize_collection(collection);
    if frame.track.as_deref() == Some(expected.as_str()) {
        return true;
    }

    frame
        .uri
        .as_deref()
        .is_some_and(|uri| uri.starts_with(&scope_prefix(&expected)))
}

fn frame_age_bucket(age_days: i64) -> String {
    if age_days < 0 {
        return "future".to_string();
    }
    if age_days <= 1 {
        return "0-1 day".to_string();
    }
    if age_days <= 7 {
        return "2-7 days".to_string();
    }
    if age_days <= 30 {
        return "8-30 days".to_string();
    }
    if age_days <= 90 {
        return "31-90 days".to_string();
    }
    if age_days <= 365 {
        return "91-365 days".to_string();
    }
    "> 365 days".to_string()
}

fn frame_size_bucket(size_bytes: u64) -> String {
    if size_bytes < 1_024 {
        return "<1 KB".to_string();
    }
    if size_bytes < 10 * 1_024 {
        return "1-10 KB".to_string();
    }
    if size_bytes < 100 * 1_024 {
        return "10-100 KB".to_string();
    }
    if size_bytes < 1_024 * 1_024 {
        return "100 KB-1 MB".to_string();
    }
    if size_bytes < 10 * 1_024 * 1_024 {
        return "1-10 MB".to_string();
    }
    "10 MB+".to_string()
}

fn to_sorted_stats(map: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut entries: Vec<(String, usize)> = map.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

fn copy_frame_to_archive(
    source: &MemoryDb,
    archive: &MemoryDb,
    frame: &Frame,
) -> Result<u64, Box<dyn std::error::Error>> {
    let payload = source.frame_canonical_payload(frame.id).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
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
    options.parent_id = frame.parent_id;
    let id = archive.put_bytes_with_options(&payload, options).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
    Ok(id)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init { mv2 } => {
            if mv2.exists() {
                eprintln!("Refusing to overwrite existing file: {}", mv2.display());
                std::process::exit(2);
            }
            let _ = open_or_create_db(&mv2)?;
            println!("Created {}", mv2.display());
            Ok(())
        }

        Command::Ingest {
            mv2,
            collection,
            root,
            exts,
            dry_run,
        } => {
            let root = root.canonicalize().unwrap_or(root);
            if !root.exists() {
                eprintln!("Root does not exist: {}", root.display());
                std::process::exit(2);
            }

            let db = open_or_create_db(&mv2)?;

            let mut scanned = 0usize;
            let mut ingested = 0usize;
            let mut updated = 0usize;
            let mut skipped = 0usize;

            for entry in WalkDir::new(&root).follow_links(false) {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if !is_extension_allowed(path, &exts) {
                    continue;
                }

                let Ok(relative) = path.strip_prefix(&root) else {
                    continue;
                };

                scanned += 1;

                let bytes = fs::read(path)?;
                let file_hash = blake3_hash(&bytes);
                let uri = uri_for_path(&collection, relative);
                let title = infer_title(path, &bytes);

                let existing_checksum = db.frame_by_uri(&uri).ok().map(|frame| frame.checksum);

                if existing_checksum.is_some_and(|c| c == *file_hash.as_bytes()) {
                    skipped += 1;
                    continue;
                }

                if dry_run {
                    if existing_checksum.is_some() {
                        updated += 1;
                    } else {
                        ingested += 1;
                    }
                    continue;
                }

                let meta = entry.metadata().ok();
                let mtime_ms = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis().to_string())
                    .unwrap_or_default();
                let size_bytes = meta
                    .as_ref()
                    .map(|m| m.len().to_string())
                    .unwrap_or_default();

                let mut options = PutOptions::default();
                options.uri = Some(uri);
                options.title = Some(title);
                options.track = Some(normalize_collection(&collection));
                options.kind = Some("text/markdown".to_string());
                options
                    .extra_metadata
                    .insert("source_path".into(), path.to_string_lossy().into_owned());
                options.extra_metadata.insert(
                    "relative_path".into(),
                    relative.to_string_lossy().into_owned(),
                );
                if !mtime_ms.is_empty() {
                    options.extra_metadata.insert("mtime_ms".into(), mtime_ms);
                }
                if !size_bytes.is_empty() {
                    options
                        .extra_metadata
                        .insert("size_bytes".into(), size_bytes);
                }

                db.put_bytes_with_options(&bytes, options).map_err(|e| Box::<dyn std::error::Error>::from(e))?;

                if existing_checksum.is_some() {
                    updated += 1;
                } else {
                    ingested += 1;
                }
            }

            if dry_run {
                println!(
                    "Dry run: scanned={scanned} ingest={ingested} update={updated} skip={skipped}"
                );
                return Ok(());
            }

            db.commit().map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            println!("Done: scanned={scanned} ingest={ingested} update={updated} skip={skipped}");
            Ok(())
        }

        Command::Put {
            mv2,
            uri,
            collection,
            path,
            title,
            track,
            kind,
            text,
            file,
            json,
        } => {
            let payload = if let Some(file) = file {
                fs::read(file)?
            } else if let Some(text) = text {
                text.into_bytes()
            } else {
                return Err("put requires --text or --file".into());
            };

            let uri = if let Some(uri) = uri {
                uri
            } else if let (Some(collection), Some(path)) = (collection, path) {
                uri_for_path(&collection, Path::new(&path))
            } else {
                return Err("put requires --uri or --collection + --path".into());
            };

            let inferred_title = uri
                .split('/')
                .next_back()
                .filter(|s| !s.is_empty())
                .unwrap_or(&uri)
                .to_string();

            let mut options = PutOptions::default();
            options.uri = Some(uri.clone());
            options.title = Some(title.unwrap_or(inferred_title));
            options.track = track;
            options.kind = kind;
            if let Ok(text) = String::from_utf8(payload.clone()) {
                options.search_text = Some(text);
            }

            let db = open_or_create_db(&mv2)?;
            let frame_id = db.put_bytes_with_options(&payload, options).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            db.commit().map_err(|e| Box::<dyn std::error::Error>::from(e))?;

            if json {
                let response = serde_json::json!({
                    "frame_id": frame_id,
                    "uri": uri
                });
                println!("{}", serde_json::to_string_pretty(&response)?);
            } else {
                println!("Added frame #{frame_id} {}", uri);
            }
            Ok(())
        }

        Command::Search {
            mv2,
            query,
            limit,
            collection,
            snippet_chars,
            json,
        } => {
            let db = open_or_create_db(&mv2)?;
            let scope = collection.as_deref().map(scope_prefix);

            let request = SearchRequest {
                query: query.clone(),
                top_k: limit,
                snippet_chars,
                scope,
                temporal: None,
                as_of_frame: None,
                as_of_ts: None,
            };

            let response = db.search(request).map_err(|e| Box::<dyn std::error::Error>::from(e))?;

            if json {
                println!("{}", serde_json::to_string_pretty(&response)?);
                return Ok(());
            }

            for hit in response.hits {
                let title = hit.title.unwrap_or_default();
                if let Some(score) = hit.score {
                    println!("{:>2}. {:>6.3}  {}  {}", hit.rank, score, hit.uri, title);
                } else {
                    println!("{:>2}. {}  {}", hit.rank, hit.uri, title);
                }
                if !hit.text.trim().is_empty() {
                    println!("    {}", hit.text.replace('\n', " "));
                }
            }

            Ok(())
        }

        Command::Query {
            mv2,
            query,
            limit,
            collection,
            snippet_chars,
            no_expand,
            max_expansions,
            expand_hook,
            expand_hook_timeout_ms,
            no_vector,
            rerank,
            rerank_hook,
            rerank_hook_timeout_ms,
            rerank_hook_full_text,
            embed_model,
            embed_cache,
            embed_no_cache,
            rerank_docs,
            rerank_chunk_chars,
            rerank_chunk_overlap,
            json,
            files,
            plan,
            log,
            asof,
            before,
            after,
            feedback_weight,
        } => {
            let db = open_or_create_db(&mv2)?;

            let args = QueryArgs {
                raw_query: query.clone(),
                collection,
                limit,
                snippet_chars,
                no_expand,
                max_expansions,
                expand_hook,
                expand_hook_timeout_ms,
                no_vector,
                rerank,
                rerank_hook,
                rerank_hook_timeout_ms,
                rerank_hook_full_text,
                embed_model,
                embed_cache,
                embed_no_cache,
                rerank_docs,
                rerank_chunk_chars,
                rerank_chunk_overlap,
                plan,
                asof,
                before,
                after,
                feedback_weight,
            };

            let response = execute_query(&db, args)?;

            if log {
                #[derive(Serialize)]
                struct QueryLog<'a> {
                    query: &'a str,
                    plan: &'a QueryPlan,
                    results: &'a [QueryResult],
                }

                let log_payload = QueryLog {
                    query: &response.query,
                    plan: &response.plan,
                    results: &response.results,
                };
                let bytes = serde_json::to_vec(&log_payload)?;
                let ts = Utc::now().timestamp();
                let hash = blake3_hash(&bytes);
                let uri = format!("aethervault://query-log/{ts}-{}", hash.to_hex());
                let mut options = PutOptions::default();
                options.uri = Some(uri);
                options.title = Some("aethervault query log".to_string());
                options.kind = Some("application/json".to_string());
                options.track = Some("aethervault.query".to_string());
                options.search_text = Some(response.plan.cleaned_query.clone());
                db.put_bytes_with_options(&bytes, options).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
                db.commit().map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            }

            if !response.warnings.is_empty() && !json {
                for warning in &response.warnings {
                    eprintln!("Warning: {warning}");
                }
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&response)?);
                return Ok(());
            }

            if files {
                for r in response.results {
                    println!(
                        "{:.4}\t{}\t{}\t{}",
                        r.score,
                        r.frame_id,
                        r.uri,
                        r.title.unwrap_or_default()
                    );
                }
                return Ok(());
            }

            if response.results.is_empty() {
                println!("No results found.");
                return Ok(());
            }

            for r in response.results {
                let title = r.title.clone().unwrap_or_default();
                println!("{:>2}. {:>6.3}  {}  {}", r.rank, r.score, r.uri, title);
                if !r.snippet.trim().is_empty() {
                    println!("    {}", r.snippet.replace('\n', " "));
                }
            }

            Ok(())
        }

        Command::Context {
            mv2,
            query,
            collection,
            limit,
            snippet_chars,
            max_bytes,
            full,
            no_expand,
            max_expansions,
            expand_hook,
            expand_hook_timeout_ms,
            no_vector,
            rerank,
            rerank_hook,
            rerank_hook_timeout_ms,
            rerank_hook_full_text,
            embed_model,
            embed_cache,
            embed_no_cache,
            plan,
            asof,
            before,
            after,
            feedback_weight,
        } => {
            let db = open_or_create_db(&mv2)?;
            let args = QueryArgs {
                raw_query: query.clone(),
                collection,
                limit,
                snippet_chars,
                no_expand,
                max_expansions,
                expand_hook,
                expand_hook_timeout_ms,
                no_vector,
                rerank,
                rerank_hook,
                rerank_hook_timeout_ms,
                rerank_hook_full_text,
                embed_model,
                embed_cache,
                embed_no_cache,
                rerank_docs: limit.max(20),
                rerank_chunk_chars: 1200,
                rerank_chunk_overlap: 200,
                plan,
                asof,
                before,
                after,
                feedback_weight,
            };

            let pack = build_context_pack(&db, args, max_bytes, full)?;
            if !pack.warnings.is_empty() {
                for warning in &pack.warnings {
                    eprintln!("Warning: {warning}");
                }
            }
            println!("{}", serde_json::to_string_pretty(&pack)?);
            Ok(())
        }

        Command::Log {
            mv2,
            session,
            role,
            text,
            file,
            meta,
        } => {
            let payload_text = if let Some(path) = file {
                fs::read_to_string(path)?
            } else if let Some(text) = text {
                text
            } else {
                return Err("log requires --text or --file".into());
            };

            let meta_value = if let Some(meta) = meta {
                Some(serde_json::from_str(&meta)?)
            } else {
                None
            };

            let entry = AgentLogEntry {
                session: session.clone(),
                role: role.clone(),
                text: payload_text.clone(),
                meta: meta_value,
                ts_utc: Some(Utc::now().timestamp()),
            };
            let db = open_or_create_db(&mv2)?;
            let _ = append_agent_log(&db, &entry)?;
            println!("Logged agent turn.");
            Ok(())
        }

        Command::Feedback {
            mv2,
            uri,
            score,
            note,
            session,
        } => {
            let score = score.clamp(-1.0, 1.0);
            let event = FeedbackEvent {
                uri: uri.clone(),
                score,
                note: note.clone(),
                session: session.clone(),
                ts_utc: Some(Utc::now().timestamp()),
            };
            let db = open_or_create_db(&mv2)?;
            let _ = append_feedback(&db, &event).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            println!("Feedback recorded.");
            Ok(())
        }

        Command::Embed {
            mv2,
            collection,
            limit,
            batch,
            force,
            model,
            embed_cache,
            embed_no_cache,
            dry_run,
            json,
        } => {
            #[cfg(feature = "vec")]
            {
                let _ = (mv2, collection, limit, batch, force, model, embed_cache, embed_no_cache, dry_run, json);
                eprintln!("Local embedding is not supported with SQLite backend. Use Qdrant for vector search.");
                std::process::exit(2);
            }
            #[cfg(not(feature = "vec"))]
            {
                let _ = (
                    mv2,
                    collection,
                    limit,
                    batch,
                    force,
                    model,
                    embed_cache,
                    embed_no_cache,
                    dry_run,
                    json,
                );
                eprintln!("Embed requires --features vec");
                std::process::exit(2);
            }
        }

        Command::Get { mv2, id, json } => {
            let db = open_or_create_db(&mv2)?;

            let (frame_id, frame) = if let Some(rest) = id.strip_prefix('#') {
                let frame_id: u64 = rest.parse().map_err(|_| -> Box<dyn std::error::Error> {
                    "invalid frame id (expected #123)".into()
                })?;
                let frame = db.frame_by_id(frame_id).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
                (frame_id, frame)
            } else {
                let frame = db.frame_by_uri(&id).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
                (frame.id, frame)
            };

            let text = db.frame_text_by_id(frame_id).map_err(|e| Box::<dyn std::error::Error>::from(e))?;

            if json {
                let payload = GetResponse {
                    frame_id,
                    uri: frame.uri.clone(),
                    title: frame.title.clone(),
                    text,
                };
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }

            println!("{text}");
            Ok(())
        }

        Command::Status { mv2, json } => {
            let db = open_or_create_db(&mv2)?;
            let payload = StatusResponse {
                mv2: mv2.display().to_string(),
                frame_count: db.frame_count(),
                next_frame_id: db.frame_count() as u64,
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("mv2: {}", payload.mv2);
                println!("frames: {}", payload.frame_count);
                println!("next_frame_id: {}", payload.next_frame_id);
            }

            Ok(())
        }

        Command::Config { mv2, command } => match command {
            ConfigCommand::Set {
                key,
                file,
                json,
                pretty,
            } => {
                let bytes = if let Some(path) = file {
                    fs::read(path)?
                } else if let Some(json) = json {
                    json.into_bytes()
                } else {
                    return Err("config set requires --file or --json".into());
                };
                let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                let payload = if pretty {
                    serde_json::to_vec_pretty(&value)?
                } else {
                    serde_json::to_vec(&value)?
                };
                let db = open_or_create_db(&mv2)?;
                save_config_entry(&db, &key, &payload).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
                println!("Stored config {key}");
                Ok(())
            }
            ConfigCommand::Get { key, raw } => {
                let db = open_or_create_db(&mv2)?;
                let Some(bytes) = load_config_entry(&db, &key) else {
                    return Err("config not found".into());
                };
                if raw {
                    io::stdout().write_all(&bytes)?;
                } else {
                    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                    println!("{}", serde_json::to_string_pretty(&value)?);
                }
                Ok(())
            }
            ConfigCommand::List { json } => {
                let db = open_or_create_db(&mv2)?;
                let entries = list_config_entries(&db);
                if json {
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                } else {
                    for entry in entries {
                        println!("{}\t{}\t{}", entry.key, entry.frame_id, entry.timestamp);
                    }
                }
                Ok(())
            }
        },

        Command::Diff {
            left,
            right,
            all,
            limit,
            json,
        } => {
            let left_db = open_or_create_db(&left)?;
            let right_db = open_or_create_db(&right)?;
            let left_map = collect_latest_frames(&left_db, all);
            let right_map = collect_latest_frames(&right_db, all);

            let mut only_left = Vec::new();
            let mut only_right = Vec::new();
            let mut changed = Vec::new();

            for (uri, left_summary) in &left_map {
                if let Some(right_summary) = right_map.get(uri) {
                    if left_summary.checksum != right_summary.checksum {
                        changed.push(DiffChange {
                            uri: uri.clone(),
                            left: left_summary.clone(),
                            right: right_summary.clone(),
                        });
                    }
                } else {
                    only_left.push(left_summary.clone());
                }
            }

            for (uri, right_summary) in &right_map {
                if !left_map.contains_key(uri) {
                    only_right.push(right_summary.clone());
                }
            }

            only_left.sort_by(|a, b| a.uri.cmp(&b.uri));
            only_right.sort_by(|a, b| a.uri.cmp(&b.uri));
            changed.sort_by(|a, b| a.uri.cmp(&b.uri));

            if limit > 0 {
                only_left.truncate(limit);
                only_right.truncate(limit);
                changed.truncate(limit);
            }

            let report = DiffReport {
                left: left.display().to_string(),
                right: right.display().to_string(),
                only_left,
                only_right,
                changed,
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("left: {}", report.left);
                println!("right: {}", report.right);
                println!("only_left: {}", report.only_left.len());
                println!("only_right: {}", report.only_right.len());
                println!("changed: {}", report.changed.len());
            }
            Ok(())
        }

        Command::Merge {
            left,
            right,
            out,
            force,
            no_dedup,
            json,
        } => {
            let _ = (left, right, out, force, no_dedup, json);
            eprintln!("Merge is not supported with SQLite backend. Copy the .sqlite file instead.");
            std::process::exit(2);
        }

        Command::Mcp { mv2, read_only } => run_mcp_server(mv2, read_only),

        Command::Agent {
            mv2,
            prompt,
            file,
            session,
            model_hook,
            system,
            system_file,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log_commit_interval,
            json,
            log, ..
        } => run_agent(
            mv2,
            prompt,
            file,
            session,
            model_hook,
            system,
            system_file,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log_commit_interval,
            json,
            log,
        ),

        Command::Hook { provider } => match provider {
            HookCommand::Claude => run_claude_hook(),
        },

        Command::Bootstrap {
            mv2,
            workspace,
            timezone,
            force,
        } => {
            let workspace = workspace
                .or_else(|| env_optional("AETHERVAULT_WORKSPACE").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            bootstrap_workspace(&mv2, &workspace, timezone, force)?;
            println!(
                "bootstrapped workspace at {} (mv2: {})",
                workspace.display(),
                mv2.display()
            );
            Ok(())
        }

        Command::Schedule {
            mv2,
            workspace,
            timezone,
            telegram_token,
            telegram_chat_id,
            model_hook,
            max_steps,
            log,
            log_commit_interval,
        } => run_schedule_loop(
            mv2,
            workspace,
            timezone,
            telegram_token,
            telegram_chat_id,
            model_hook,
            max_steps,
            log,
            log_commit_interval,
        ),

        Command::Watch {
            mv2,
            workspace,
            timezone,
            model_hook,
            max_steps,
            log,
            log_commit_interval,
            poll_seconds,
        } => run_watch_loop(
            mv2,
            workspace,
            timezone,
            model_hook,
            max_steps,
            log,
            log_commit_interval,
            poll_seconds,
        ),

        Command::Connect {
            mv2,
            provider,
            bind,
            port,
            redirect_base,
        } => run_oauth_broker(mv2, provider, bind, port, redirect_base),

        Command::Approve { mv2, id, execute } => {
            let output = approve_and_maybe_execute(&mv2, &id, execute)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            println!("{output}");
            Ok(())
        }

        Command::Reject { mv2, id } => {
            let output =
                reject_approval(&mv2, &id).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            println!("{output}");
            Ok(())
        }

        Command::Bridge { command } => run_bridge(command),

        Command::Doctor {
            mv2,
            vacuum,
            rebuild_time,
            rebuild_lex,
            rebuild_vec,
            dry_run,
            quiet,
            json,
        } => {
            let _ = (rebuild_time, rebuild_vec, dry_run, quiet);
            let db = open_or_create_db(&mv2)?;
            if rebuild_lex {
                db.rebuild_fts().map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            }
            if vacuum {
                db.vacuum().map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            }
            let size = db.file_size(&mv2);
            if json {
                println!("{}", serde_json::json!({"status": "ok", "size_bytes": size}));
            } else {
                println!("Doctor complete. Size: {} bytes", size);
            }
            Ok(())
        }

        Command::Compact {
            mv2,
            dry_run,
            quiet,
            json,
        } => {
            let _ = (dry_run, quiet);
            let db = open_or_create_db(&mv2)?;
            db.rebuild_fts().map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            db.vacuum().map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            let size = db.file_size(&mv2);
            if json {
                println!("{}", serde_json::json!({"status": "ok", "size_bytes": size}));
            } else {
                println!("Compact complete. Size: {} bytes", size);
            }
            Ok(())
        }

        Command::Archive {
            mv2,
            before,
            collection,
            target,
            dry_run,
        } => {
            let source_path = mv2.display().to_string();
            let target = target.unwrap_or_else(|| {
                mv2.parent()
                    .unwrap_or(Path::new("."))
                    .join("archive.mv2")
            });
            let cutoff = parse_date_to_ts(&before)
                .ok_or_else(|| format!("invalid before date: {before}"))?;

            let target_display = target.display().to_string();
            if target == mv2 {
                return Err("archive destination must differ from source".into());
            }

            let source = open_or_create_db(&mv2)?;
            let target_mem = if dry_run {
                None
            } else {
                Some(open_or_create_db(&target)?)
            };

            let mut scanned = 0usize;
            let mut eligible = 0usize;
            let mut archived = 0usize;
            let mut deleted = 0usize;
            let mut candidates = Vec::new();

            let all_ids = source.collect_active_frame_ids(None);
            for &frame_id in &all_ids {
                let frame = match source.frame_by_id(frame_id) {
                    Ok(frame) => frame,
                    Err(_) => continue,
                };
                if frame.status != FrameStatus::Active {
                    continue;
                }
                scanned += 1;
                if frame.timestamp < cutoff && frame_matches_collection(&frame, &collection) {
                    candidates.push(frame.id);
                    eligible += 1;
                }
            }

            if !dry_run {
                for frame_id in candidates {
                let frame = source.frame_by_id(frame_id).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
                if frame.status != FrameStatus::Active
                    || frame.timestamp >= cutoff
                    || !frame_matches_collection(&frame, &collection)
                {
                    continue;
                }
                copy_frame_to_archive(&source, target_mem.as_ref().unwrap(), &frame)?;
                source.delete_frame(frame_id).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
                archived += 1;
                deleted += 1;
            }
                target_mem
                    .as_ref()
                    .unwrap()
                    .commit()
                    .map_err(|e| Box::<dyn std::error::Error>::from(e))?;
                source.vacuum().map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            }

            let report = ArchiveSummary {
                source: source_path.clone(),
                target: target_display.clone(),
                before: before.clone(),
                collection: collection.clone(),
                scanned,
                eligible,
                archived,
                deleted,
                dry_run,
            };

            let _ = report;
            if dry_run {
                println!(
                    "Dry run: archive {} frames before {} in collection '{}' from {} to {} (scanned active={scanned}, eligible={eligible})",
                    eligible, before, collection, source_path, target_display
                );
            } else {
                println!(
                    "Archived {} frames before {} in collection '{}' from {} to {} (scanned active={}, deleted={})",
                    archived, before, collection, source_path, target_display, scanned, deleted
                );
            }
            Ok(())
        }

        Command::Dedup {
            mv2,
            keep_versions,
            dry_run,
        } => {
            let source_path = mv2.display().to_string();
            let source = open_or_create_db(&mv2)?;

            let mut by_uri_versions: HashMap<String, Vec<(i64, u64)>> = HashMap::new();
            let mut scanned = 0usize;

            let all_ids = source.collect_active_frame_ids(None);
            for &frame_id in &all_ids {
                let frame = match source.frame_by_id(frame_id) {
                    Ok(frame) => frame,
                    Err(_) => continue,
                };
                if frame.status != FrameStatus::Active {
                    continue;
                }
                let Some(uri) = frame.uri.clone() else {
                    continue;
                };
                by_uri_versions
                    .entry(uri)
                    .or_default()
                    .push((frame.timestamp, frame.id));
                scanned += 1;
            }

            let mut duplicate_ids = Vec::new();
            for versions in by_uri_versions.values_mut() {
                versions.sort_by(|left, right| {
                    right
                        .0
                        .cmp(&left.0)
                        .then_with(|| right.1.cmp(&left.1))
                });
                if versions.len() > keep_versions {
                    duplicate_ids.extend(versions.iter().skip(keep_versions).map(|(_, frame_id)| *frame_id));
                }
            }

            let deleted = duplicate_ids.len();
            if !dry_run {
                for frame_id in &duplicate_ids {
                    source.delete_frame(*frame_id).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
                }
                source.vacuum().map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            }

            let report = DedupSummary {
                source: source_path.clone(),
                scanned,
                unique_uris: by_uri_versions.len(),
                duplicates_removed: deleted,
                keep_versions,
                dry_run,
            };

            let _ = report;
            if dry_run {
                println!(
                    "Dry run: dedup would remove {} duplicate frames from {} while keeping {} newest versions per URI",
                    deleted, source_path, keep_versions
                );
            } else {
                println!(
                    "Dedup removed {} duplicate frames from {} while keeping {} newest versions per URI",
                    deleted, source_path, keep_versions
                );
            }

            Ok(())
        }

        Command::Stats { mv2 } => {
            let source_path = mv2.display().to_string();
            let source = open_or_create_db(&mv2)?;
            let now = Utc::now().timestamp();
            let total_frames = source.frame_count() as u64;
            let active_frames = source.active_frame_count() as u64;
            let file_size = source.file_size(&mv2);

            let mut by_collection: HashMap<String, usize> = HashMap::new();
            let mut by_age_days: HashMap<String, usize> = HashMap::new();
            let mut by_size: HashMap<String, usize> = HashMap::new();
            let mut uri_counts: HashMap<String, usize> = HashMap::new();

            let all_ids = source.collect_active_frame_ids(None);
            for &frame_id in &all_ids {
                let frame = match source.frame_by_id(frame_id) {
                    Ok(frame) => frame,
                    Err(_) => continue,
                };
                if frame.status != FrameStatus::Active {
                    continue;
                }
                *by_collection
                    .entry(frame_collection_name(&frame))
                    .or_insert(0) += 1;

                let age_days = now.saturating_sub(frame.timestamp).div_euclid(86_400);
                *by_age_days
                    .entry(frame_age_bucket(age_days))
                    .or_insert(0) += 1;
                let payload_size = source.frame_canonical_payload(frame.id).map(|p| p.len() as u64).unwrap_or(0);
                *by_size
                    .entry(frame_size_bucket(payload_size))
                    .or_insert(0) += 1;
                let uri_key = frame.uri.unwrap_or_else(|| "<no-uri>".to_string());
                *uri_counts.entry(uri_key).or_insert(0) += 1;
            }

            let duplicate_uris = uri_counts.values().filter(|count| **count > 1).count();
            let duplicate_frames: usize = uri_counts
                .values()
                .map(|count| count.saturating_sub(1))
                .sum();
            let deleted_frames = total_frames.saturating_sub(active_frames);
            let total_breakdown = vec![
                ("total_frames".to_string(), total_frames),
                ("active_frames".to_string(), active_frames),
                ("deleted_frames".to_string(), deleted_frames),
                ("size_bytes".to_string(), file_size),
            ];

            let report = StatsSummary {
                source: source_path.clone(),
                total_frames,
                active_frames,
                by_collection: to_sorted_stats(by_collection),
                by_age_days: to_sorted_stats(by_age_days),
                by_size: to_sorted_stats(by_size),
                duplicate_uris,
                duplicate_frames,
                total_breakdown,
            };

            println!("Stats for {}", report.source);
            println!("Active frames: {}", report.active_frames);
            println!("Total frames: {}", report.total_frames);
            println!("Duplicate URIs: {} ({} duplicate frames)", report.duplicate_uris, report.duplicate_frames);
            println!("By collection:");
            for (collection, count) in &report.by_collection {
                println!("  {collection}: {count}");
            }
            println!("By age:");
            for (bucket, count) in &report.by_age_days {
                println!("  {bucket}: {count}");
            }
            println!("By size:");
            for (bucket, count) in &report.by_size {
                println!("  {bucket}: {count}");
            }
            println!("Total breakdown:");
            for (name, value) in &report.total_breakdown {
                println!("  {name}: {value}");
            }

            Ok(())
        }

        Command::MigrateHotMemories {
            mv2,
            jsonl,
            dry_run,
        } => {
            let jsonl_path = jsonl.unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
                PathBuf::from(format!("{home}/.aethervault/data/hot-memories.jsonl"))
            });
            if !jsonl_path.exists() {
                eprintln!(
                    "JSONL file not found: {}",
                    jsonl_path.display()
                );
                std::process::exit(2);
            }
            let db = open_or_create_db(&mv2)?;
            let report = db
                .migrate_hot_memories(&jsonl_path, dry_run)
                .map_err(|e| Box::<dyn std::error::Error>::from(e))?;

            if dry_run {
                println!("Dry run — no writes performed.");
            }
            println!(
                "Hot-memory migration: total={} added={} updated={} noop={} invalid={} errors={}",
                report.total_lines,
                report.added,
                report.updated,
                report.skipped_noop,
                report.skipped_invalid,
                report.errors.len()
            );
            for err in &report.errors {
                eprintln!("  error: {err}");
            }
            if !dry_run {
                db.commit().map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            }
            Ok(())
        }
    }
}

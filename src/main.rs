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

// Re-export all module items at crate root so cross-module references work.
// Before this split, everything lived in main.rs and shared a single namespace.
// These wildcard re-exports preserve that behavior.
#[allow(unused_imports)]
pub(crate) use cli::*;
#[allow(unused_imports)]
pub(crate) use types::*;
#[allow(unused_imports)]
pub(crate) use tool_args::*;
#[allow(unused_imports)]
pub(crate) use util::*;
#[allow(unused_imports)]
pub(crate) use config::*;
#[allow(unused_imports)]
pub(crate) use query::*;
#[allow(unused_imports)]
pub(crate) use tool_defs::*;
#[allow(unused_imports)]
pub(crate) use tool_exec::*;
#[allow(unused_imports)]
pub(crate) use mcp::*;
#[allow(unused_imports)]
pub(crate) use claude::*;
#[allow(unused_imports)]
pub(crate) use agent::*;
#[allow(unused_imports)]
pub(crate) use bridges::*;
#[allow(unused_imports)]
pub(crate) use services::*;

// External crate imports used directly in main()
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use aether_core::types::SearchRequest;
use aether_core::{DoctorOptions, PutOptions, Vault, VaultError};
use chrono::Utc;
use clap::Parser;
use serde::Serialize;
use walkdir::WalkDir;

#[cfg(feature = "vec")]
use aether_core::text_embed::{LocalTextEmbedder, TextEmbedConfig};
#[cfg(feature = "vec")]
use aether_core::types::EmbeddingProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init { mv2 } => {
            if mv2.exists() {
                eprintln!("Refusing to overwrite existing file: {}", mv2.display());
                std::process::exit(2);
            }
            let _ = Vault::create(&mv2)?;
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

            let mut mem = open_or_create(&mv2)?;
            mem.enable_lex()?;

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

                let existing_checksum = mem.frame_by_uri(&uri).ok().map(|frame| frame.checksum);

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

                mem.put_bytes_with_options(&bytes, options)?;

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

            mem.commit()?;
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

            let mut mem = open_or_create(&mv2)?;
            let frame_id = mem.put_bytes_with_options(&payload, options)?;
            mem.commit()?;

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
            let mut mem = Vault::open_read_only(&mv2)?;
            let scope = collection.as_deref().map(scope_prefix);

            let request = SearchRequest {
                query: query.clone(),
                top_k: limit,
                snippet_chars,
                uri: None,
                scope,
                cursor: None,
                temporal: None,
                as_of_frame: None,
                as_of_ts: None,
                no_sketch: false,
            };

            let response = mem.search(request)?;

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
            let mut mem = if log {
                Vault::open(&mv2)?
            } else {
                Vault::open_read_only(&mv2)?
            };

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

            let response = execute_query(&mut mem, args)?;

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
                mem.put_bytes_with_options(&bytes, options)?;
                mem.commit()?;
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
            let mut mem = Vault::open_read_only(&mv2)?;
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

            let pack = build_context_pack(&mut mem, args, max_bytes, full)?;
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
            let mut mem = Vault::open(&mv2)?;
            let _ = append_agent_log(&mut mem, &entry)?;
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
            let mut mem = Vault::open(&mv2)?;
            let _ = append_feedback(&mut mem, &event)?;
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
                let mut mem = Vault::open(&mv2)?;
                mem.enable_vec()?;

                let embed_config =
                    build_embed_config(model.as_deref(), embed_cache, !embed_no_cache);
                let embedder = LocalTextEmbedder::new(embed_config)?;
                mem.set_vec_model(embedder.model())?;

                let scope = collection.as_deref().map(scope_prefix);
                let mut frame_ids = collect_active_frame_ids(&mem, scope.as_deref());
                if limit > 0 && frame_ids.len() > limit {
                    frame_ids.truncate(limit);
                }

                let batch_size = batch.max(1);
                let mut embedded = 0usize;
                let mut skipped = 0usize;
                let mut failed = 0usize;

                for chunk in frame_ids.chunks(batch_size) {
                    let mut targets: Vec<(u64, String)> = Vec::new();
                    for &frame_id in chunk {
                        if !force {
                            match mem.frame_embedding(frame_id) {
                                Ok(Some(_)) => {
                                    skipped += 1;
                                    continue;
                                }
                                Ok(None) => {}
                                Err(_) => {}
                            }
                        }

                        let frame = match mem.frame_by_id(frame_id) {
                            Ok(f) => f,
                            Err(_) => {
                                failed += 1;
                                continue;
                            }
                        };
                        let text = if let Some(search) = frame.search_text.clone() {
                            search
                        } else {
                            match mem.frame_text_by_id(frame_id) {
                                Ok(t) => t,
                                Err(_) => {
                                    failed += 1;
                                    continue;
                                }
                            }
                        };

                        if text.trim().is_empty() {
                            skipped += 1;
                            continue;
                        }
                        targets.push((frame_id, text));
                    }

                    if targets.is_empty() {
                        continue;
                    }

                    let refs: Vec<&str> = targets.iter().map(|(_, t)| t.as_str()).collect();
                    let embeddings = match embedder.embed_batch(&refs) {
                        Ok(e) => e,
                        Err(err) => {
                            eprintln!("Embedding batch failed: {err}");
                            failed += targets.len();
                            continue;
                        }
                    };

                    for ((frame_id, _), embedding) in
                        targets.into_iter().zip(embeddings.into_iter())
                    {
                        if dry_run {
                            embedded += 1;
                            continue;
                        }
                        let mut options = PutOptions::default();
                        options.auto_tag = false;
                        options.extract_dates = false;
                        options.extract_triplets = false;
                        options.instant_index = false;
                        options.enable_embedding = false;
                        if mem
                            .update_frame(frame_id, None, options, Some(embedding))
                            .is_ok()
                        {
                            embedded += 1;
                        } else {
                            failed += 1;
                        }
                    }
                }

                if !dry_run {
                    mem.commit()?;
                }

                if json {
                    #[derive(Serialize)]
                    struct EmbedSummary {
                        total: usize,
                        embedded: usize,
                        skipped: usize,
                        failed: usize,
                        dry_run: bool,
                    }

                    let summary = EmbedSummary {
                        total: frame_ids.len(),
                        embedded,
                        skipped,
                        failed,
                        dry_run,
                    };
                    println!("{}", serde_json::to_string_pretty(&summary)?);
                } else {
                    println!(
                        "Embedding complete: total={} embedded={} skipped={} failed={} dry_run={}",
                        frame_ids.len(),
                        embedded,
                        skipped,
                        failed,
                        dry_run
                    );
                }
                Ok(())
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
            let mut mem = Vault::open_read_only(&mv2)?;

            let (frame_id, frame) = if let Some(rest) = id.strip_prefix('#') {
                let frame_id: u64 = rest.parse().map_err(|_| VaultError::InvalidQuery {
                    reason: "invalid frame id (expected #123)".into(),
                })?;
                let frame = mem.frame_by_id(frame_id)?;
                (frame_id, frame)
            } else {
                let frame = mem.frame_by_uri(&id)?;
                (frame.id, frame)
            };

            let text = mem.frame_text_by_id(frame_id)?;

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
            let mem = Vault::open_read_only(&mv2)?;
            let payload = StatusResponse {
                mv2: mv2.display().to_string(),
                frame_count: mem.frame_count(),
                next_frame_id: mem.next_frame_id(),
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
                let mut mem = open_or_create(&mv2)?;
                let frame_id = save_config_entry(&mut mem, &key, &payload)?;
                println!("Stored config {key} at frame #{frame_id}");
                Ok(())
            }
            ConfigCommand::Get { key, raw } => {
                let mut mem = Vault::open_read_only(&mv2)?;
                let Some(bytes) = load_config_entry(&mut mem, &key) else {
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
                let mut mem = Vault::open_read_only(&mv2)?;
                let entries = list_config_entries(&mut mem);
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
            let mut left_mem = Vault::open_read_only(&left)?;
            let mut right_mem = Vault::open_read_only(&right)?;
            let left_map = collect_latest_frames(&mut left_mem, all);
            let right_map = collect_latest_frames(&mut right_mem, all);

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
            if out.exists() {
                if force {
                    fs::remove_file(&out)?;
                } else {
                    return Err("output file exists (use --force to overwrite)".into());
                }
            }
            let mut out_mem = Vault::create(&out)?;
            let mut dedup_map: HashMap<String, u64> = HashMap::new();
            let mut written = 0usize;
            let mut deduped = 0usize;

            let (w1, d1) = merge_capsule_into(&mut out_mem, &left, !no_dedup, &mut dedup_map)?;
            written += w1;
            deduped += d1;
            let (w2, d2) = merge_capsule_into(&mut out_mem, &right, !no_dedup, &mut dedup_map)?;
            written += w2;
            deduped += d2;
            out_mem.commit()?;

            let report = MergeReport {
                left: left.display().to_string(),
                right: right.display().to_string(),
                out: out.display().to_string(),
                written,
                deduped,
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "merged {} + {} -> {} (written={}, deduped={})",
                    report.left, report.right, report.out, report.written, report.deduped
                );
            }
            Ok(())
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
            log,
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
            let options = DoctorOptions {
                rebuild_time_index: rebuild_time,
                rebuild_lex_index: rebuild_lex,
                rebuild_vec_index: rebuild_vec,
                vacuum,
                dry_run,
                quiet,
            };
            let report = Vault::doctor(&mv2, options)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_doctor_report(&report);
            }
            Ok(())
        }

        Command::Compact {
            mv2,
            dry_run,
            quiet,
            json,
        } => {
            let options = DoctorOptions {
                rebuild_time_index: true,
                rebuild_lex_index: true,
                rebuild_vec_index: cfg!(feature = "vec"),
                vacuum: true,
                dry_run,
                quiet,
            };
            let report = Vault::doctor(&mv2, options)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_doctor_report(&report);
            }
            Ok(())
        }
    }
}

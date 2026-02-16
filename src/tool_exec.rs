use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use aether_core::types::SearchRequest;
use aether_core::{PutOptions, Vault};
use base64::Engine;
use chrono::Utc;
use walkdir::WalkDir;

use std::sync::mpsc;

const NO_TIMEOUT_MS: u64 = u64::MAX;
const PROCESS_POLL_MS: u64 = 250;
const STATUS_REPORT_MS: u64 = 3_000;
/// Kill a process after this long with zero stdout/stderr output.
/// This catches hung SSH, stuck network calls, etc. while allowing
/// legitimately long processes (Codex, cargo build) that produce output.
const STALE_OUTPUT_THRESHOLD_MS: u64 = 600_000; // 10 minutes

/// Result from wait_for_child_monitored — owns the captured output.
struct ChildResult {
    stdout: String,
    stderr: String,
    status: std::process::ExitStatus,
}

/// Waits for a child process while monitoring stdout/stderr activity.
/// If the process produces no output for STALE_OUTPUT_THRESHOLD_MS, it is
/// killed and a descriptive error is returned so the agent can inform the user.
/// Long-running processes that actively produce output are never interrupted.
fn wait_for_child_monitored(
    child: &mut std::process::Child,
    label: &str,
    cancel_token: &Arc<AtomicBool>,
) -> Result<ChildResult, String> {
    let pid = child.id();
    let start = Instant::now();

    // Take stdout/stderr pipes for incremental reading
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    // Shared: milliseconds-since-start of last output activity
    let last_activity = Arc::new(AtomicU64::new(0));
    // Shared output buffers
    let stdout_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

    // Spawn reader thread for stdout
    if let Some(pipe) = stdout_pipe {
        let buf = stdout_buf.clone();
        let activity = last_activity.clone();
        let t0 = start;
        thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        activity.store(t0.elapsed().as_millis() as u64, Ordering::Release);
                        if let Ok(mut guard) = buf.lock() {
                            guard.extend_from_slice(&chunk[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // Spawn reader thread for stderr
    if let Some(pipe) = stderr_pipe {
        let buf = stderr_buf.clone();
        let activity = last_activity.clone();
        let t0 = start;
        thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        activity.store(t0.elapsed().as_millis() as u64, Ordering::Release);
                        if let Ok(mut guard) = buf.lock() {
                            guard.extend_from_slice(&chunk[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    let mut last_report = Instant::now();

    loop {
        // External cancellation
        if cancel_token.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{label} (pid={pid}) canceled"));
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                // Give reader threads a moment to drain remaining pipe data
                thread::sleep(Duration::from_millis(100));
                let stdout = String::from_utf8_lossy(
                    &stdout_buf.lock().unwrap_or_else(|e| e.into_inner()),
                )
                .to_string();
                let stderr = String::from_utf8_lossy(
                    &stderr_buf.lock().unwrap_or_else(|e| e.into_inner()),
                )
                .to_string();
                return Ok(ChildResult { stdout, stderr, status });
            }
            Ok(None) => {
                let now_ms = start.elapsed().as_millis() as u64;
                let last_ms = last_activity.load(Ordering::Acquire);
                let idle_ms = now_ms - last_ms;

                // Stale-process detection: no output for threshold → kill
                if idle_ms >= STALE_OUTPUT_THRESHOLD_MS {
                    let idle_min = idle_ms / 60_000;
                    let total_min = now_ms / 60_000;
                    eprintln!(
                        "[tool:{label}] pid={pid} stale-killed: \
                         no output for {idle_min}m (total runtime {total_min}m)"
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                    thread::sleep(Duration::from_millis(100));
                    let stdout = String::from_utf8_lossy(
                        &stdout_buf.lock().unwrap_or_else(|e| e.into_inner()),
                    )
                    .to_string();
                    let stderr = String::from_utf8_lossy(
                        &stderr_buf.lock().unwrap_or_else(|e| e.into_inner()),
                    )
                    .to_string();
                    let stdout_tail = if stdout.len() > 500 {
                        &stdout[stdout.len() - 500..]
                    } else {
                        &stdout
                    };
                    let stderr_tail = if stderr.len() > 500 {
                        &stderr[stderr.len() - 500..]
                    } else {
                        &stderr
                    };
                    return Err(format!(
                        "Process stale-killed (pid {pid}): no stdout/stderr output for \
                         {idle_min} minutes (ran {total_min} minutes total). \
                         The command appears stuck — consider retrying with a different approach.\n\
                         --- last stdout ---\n{stdout_tail}\n\
                         --- last stderr ---\n{stderr_tail}"
                    ));
                }

                // Periodic status report
                if last_report.elapsed() >= Duration::from_millis(STATUS_REPORT_MS) {
                    let elapsed_s = now_ms / 1000;
                    let idle_s = idle_ms / 1000;
                    let stdout_len = stdout_buf
                        .lock()
                        .map(|g| g.len())
                        .unwrap_or(0);
                    let stderr_len = stderr_buf
                        .lock()
                        .map(|g| g.len())
                        .unwrap_or(0);
                    eprintln!(
                        "[tool:{label}] pid={pid} running {elapsed_s}s \
                         (idle {idle_s}s, stdout={stdout_len}B stderr={stderr_len}B)"
                    );
                    last_report = Instant::now();
                }

                thread::sleep(Duration::from_millis(PROCESS_POLL_MS));
            }
            Err(err) => {
                return Err(format!("{label} wait failed: {err}"));
            }
        }
    }
}

use crate::{
    env_optional,
    with_read_mem,
    with_write_mem,
    load_approvals,
    save_approvals,
    approval_hash,
    requires_approval,
    scope_prefix,
    execute_query,
    build_context_pack,
    append_agent_log,
    append_feedback,
    save_config_to_file,
    sync_workspace_memory,
    export_capsule_memory,
    load_triggers,
    save_triggers,
    allowed_fs_roots,
    resolve_fs_path,
    tool_definitions_json,
    tool_score,
    parse_log_ts_from_uri,
    get_oauth_token,
    load_capsule_config,
    load_subagents_from_config,
    build_bridge_agent_config,
    run_agent_for_bridge,
    build_external_command,
    subprocess_exit_info,
    subprocess_output_text,
    blake3_hash,
    DEFAULT_WORKSPACE_DIR,
    ToolExecution,
    ApprovalEntry,
    TriggerEntry,
    CronExpr,
    AgentLogEntry,
    FeedbackEvent,
    QueryArgs,
    AgentRunOutput,
    ToolQueryArgs,
    ToolContextArgs,
    ToolSearchArgs,
    ToolGetArgs,
    ToolPutArgs,
    ToolLogArgs,
    ToolFeedbackArgs,
    ToolConfigSetArgs,
    ToolMemorySyncArgs,
    ToolMemoryExportArgs,
    ToolMemorySearchArgs,
    ToolMemoryAppendArgs,
    ToolMemoryRememberArgs,
    ToolEmailListArgs,
    ToolEmailReadArgs,
    ToolEmailSendArgs,
    ToolEmailArchiveArgs,
    ToolExecArgs,
    ToolNotifyArgs,
    ToolSignalSendArgs,
    ToolIMessageSendArgs,
    ToolHttpRequestArgs,
    ToolBrowserArgs,
    ToolExcalidrawArgs,
    ToolFsListArgs,
    ToolFsReadArgs,
    ToolFsWriteArgs,
    ToolTriggerAddArgs,
    ToolTriggerRemoveArgs,
    ToolToolSearchArgs,
    ToolSessionContextArgs,
    ToolReflectArgs,
    ToolSkillStoreArgs,
    ToolSkillSearchArgs,
    ToolSubagentInvokeArgs,
    ToolSubagentBatchArgs,
    ToolGmailListArgs,
    ToolGmailReadArgs,
    ToolGmailSendArgs,
    ToolGCalListArgs,
    ToolGCalCreateArgs,
    ToolMsMailListArgs,
    ToolMsMailReadArgs,
    ToolMsCalendarListArgs,
    ToolMsCalendarCreateArgs,
    ToolScaleArgs,
    ToolSelfUpgradeArgs,
    open_skill_db,
    upsert_skill,
    search_skills,
    SkillRecord,
    log_dir_path,
    load_session_logs,
    resolve_workspace,
    AgentConfig,
};

const EXEC_BACKGROUND_THRESHOLD_MS: u64 = 300_000;
const DEFAULT_EXEC_BG_URL: &str = "http://127.0.0.1:8082";

fn background_exec_job_name(command: &str) -> String {
    let short: String = command.chars().take(80).collect();
    if short.len() < command.len() {
        format!("{short}...")
    } else {
        short
    }
}

fn submit_exec_background_job(
    command: &str,
    cwd: Option<&String>,
    timeout_ms: u64,
    estimated_ms: u64,
) -> Result<serde_json::Value, String> {
    let base_url = env_optional("AETHERVAULT_BACKGROUND_URL")
        .unwrap_or_else(|| DEFAULT_EXEC_BG_URL.to_string())
        .trim_end_matches('/')
        .to_string();
    let endpoint = format!("{base_url}/jobs");
    let payload = serde_json::json!({
        "command": command,
        "cwd": cwd,
        "priority": 75,
        "timeout_ms": timeout_ms,
        "estimated_ms": estimated_ms,
        "name": background_exec_job_name(command),
    });

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_write(Duration::from_millis(NO_TIMEOUT_MS))
        .build();
    let response = agent
        .post(&endpoint)
        .set("content-type", "application/json")
        .send_json(payload)
        .map_err(|err| format!("background queue request failed: {err}"))?;
    let response_status = response.status();
    if response_status >= 300 {
        let body = response.into_string().unwrap_or_default();
        return Err(format!(
            "background queue rejected with HTTP {}: {}",
            response_status,
            body
        ));
    }
    response
        .into_json::<serde_json::Value>()
        .map_err(|err| format!("invalid background queue response: {err}"))
}

pub(crate) fn execute_tool(
    name: &str,
    args: serde_json::Value,
    mv2: &Path,
    read_only: bool,
) -> Result<ToolExecution, String> {
    let mut mem_read = None;
    let mut mem_write = None;
    execute_tool_with_handles(name, args, mv2, read_only, &mut mem_read, &mut mem_write)
}

pub(crate) fn execute_tool_with_handles(
    name: &str,
    args: serde_json::Value,
    mv2: &Path,
    read_only: bool,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let is_write = matches!(
        name,
        "put"
            | "log"
            | "feedback"
            | "config_set"
            | "memory_append_daily"
            | "memory_remember"
            | "trigger_add"
            | "trigger_remove"
            | "reflect"
            | "skill_store"
    );
    if read_only && is_write {
        return Err("tool disabled in read-only mode".into());
    }
    let workspace_override = resolve_workspace(None, &AgentConfig::default());
    if requires_approval(name, &args) {
        if read_only {
            return Err("approval required but tool disabled in read-only mode".into());
        }
        let args_hash = approval_hash(name, &args);
        let mut approval_id: Option<String> = None;
        let mut approved = false;
        with_write_mem(mem_read, mem_write, mv2, true, |mem| {
            let mut approvals = load_approvals(mem);
            if let Some(pos) = approvals
                .iter()
                .position(|e| e.tool == name && e.args_hash == args_hash && e.status == "approved")
            {
                approval_id = Some(approvals[pos].id.clone());
                approvals.remove(pos);
                save_approvals(mem, &approvals)?;
                approved = true;
                return Ok(());
            }
            if let Some(existing) = approvals
                .iter()
                .find(|e| e.tool == name && e.args_hash == args_hash && e.status == "pending")
            {
                approval_id = Some(existing.id.clone());
                return Ok(());
            }
            let now = chrono::Utc::now().to_rfc3339();
            let id = format!("apr_{}_{}", now.replace(':', ""), &args_hash[..8]);
            approvals.push(ApprovalEntry {
                id: id.clone(),
                tool: name.to_string(),
                args_hash: args_hash.clone(),
                args: args.clone(),
                status: "pending".to_string(),
                created_at: now,
            });
            save_approvals(mem, &approvals)?;
            approval_id = Some(id);
            Ok(())
        })?;
        if !approved {
            let id = approval_id.clone().unwrap_or_else(|| "unknown".to_string());
            return Ok(ToolExecution {
                output: format!("approval required: {id}\nReply `approve {id}` or `reject {id}`."),
                details: serde_json::json!({
                    "approval_id": approval_id,
                    "tool": name,
                    "args": args
                }),
                is_error: true,
            });
        }
    }

    match name {
        "query" => {
            let parsed: ToolQueryArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let qargs = QueryArgs {
                    raw_query: parsed.query.clone(),
                    collection: parsed.collection,
                    limit: parsed.limit.unwrap_or(10),
                    snippet_chars: parsed.snippet_chars.unwrap_or(300),
                    no_expand: parsed.no_expand.unwrap_or(false),
                    max_expansions: parsed.max_expansions.unwrap_or(2),
                    expand_hook: None,
                    expand_hook_timeout_ms: NO_TIMEOUT_MS,
                    no_vector: parsed.no_vector.unwrap_or(false),
                    rerank: parsed.rerank.unwrap_or_else(|| "local".to_string()),
                    rerank_hook: None,
                    rerank_hook_timeout_ms: NO_TIMEOUT_MS,
                    rerank_hook_full_text: false,
                    embed_model: None,
                    embed_cache: 4096,
                    embed_no_cache: false,
                    rerank_docs: 40,
                    rerank_chunk_chars: 1200,
                    rerank_chunk_overlap: 200,
                    plan: false,
                    asof: parsed.asof,
                    before: parsed.before,
                    after: parsed.after,
                    feedback_weight: parsed.feedback_weight.unwrap_or(0.15),
                };
                let response = execute_query(mem, qargs).map_err(|e| e.to_string())?;
                let mut lines = Vec::new();
                for r in response.results.iter().take(5) {
                    lines.push(format!("{}. {} ({:.3})", r.rank, r.uri, r.score));
                }
                let output = if lines.is_empty() {
                    "No results.".to_string()
                } else {
                    lines.join("\n")
                };
                let details = serde_json::to_value(response).map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })
        }
        "context" => {
            let parsed: ToolContextArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let qargs = QueryArgs {
                    raw_query: parsed.query.clone(),
                    collection: parsed.collection,
                    limit: parsed.limit.unwrap_or(10),
                    snippet_chars: parsed.snippet_chars.unwrap_or(300),
                    no_expand: parsed.no_expand.unwrap_or(false),
                    max_expansions: parsed.max_expansions.unwrap_or(2),
                    expand_hook: None,
                    expand_hook_timeout_ms: NO_TIMEOUT_MS,
                    no_vector: parsed.no_vector.unwrap_or(false),
                    rerank: parsed.rerank.unwrap_or_else(|| "local".to_string()),
                    rerank_hook: None,
                    rerank_hook_timeout_ms: NO_TIMEOUT_MS,
                    rerank_hook_full_text: false,
                    embed_model: None,
                    embed_cache: 4096,
                    embed_no_cache: false,
                    rerank_docs: parsed.limit.unwrap_or(10).max(20),
                    rerank_chunk_chars: 1200,
                    rerank_chunk_overlap: 200,
                    plan: false,
                    asof: parsed.asof,
                    before: parsed.before,
                    after: parsed.after,
                    feedback_weight: parsed.feedback_weight.unwrap_or(0.15),
                };
                let pack = build_context_pack(
                    mem,
                    qargs,
                    parsed.max_bytes.unwrap_or(12_000),
                    parsed.full.unwrap_or(false),
                )
                .map_err(|e| e.to_string())?;
                let output = pack.context.clone();
                let details = serde_json::to_value(pack).map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })
        }
        "search" => {
            let parsed: ToolSearchArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let scope = parsed.collection.as_deref().map(scope_prefix);
                let request = SearchRequest {
                    query: parsed.query.clone(),
                    top_k: parsed.limit.unwrap_or(10),
                    snippet_chars: parsed.snippet_chars.unwrap_or(300),
                    uri: None,
                    scope,
                    cursor: None,
                    temporal: None,
                    as_of_frame: None,
                    as_of_ts: None,
                    no_sketch: false,
                };
                let response = mem.search(request).map_err(|e| e.to_string())?;
                let mut lines = Vec::new();
                for hit in response.hits.iter().take(5) {
                    let title = hit.title.clone().unwrap_or_default();
                    lines.push(format!("{}. {} {}", hit.rank, hit.uri, title));
                }
                let output = if lines.is_empty() {
                    "No results.".to_string()
                } else {
                    lines.join("\n")
                };
                let details = serde_json::to_value(response).map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })
        }
        "get" => {
            let parsed: ToolGetArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let (frame_id, frame) = if let Some(rest) = parsed.id.strip_prefix('#') {
                    let frame_id: u64 = rest.parse().map_err(|_| "invalid frame id")?;
                    let frame = mem.frame_by_id(frame_id).map_err(|e| e.to_string())?;
                    (frame_id, frame)
                } else {
                    let frame = mem.frame_by_uri(&parsed.id).map_err(|e| e.to_string())?;
                    (frame.id, frame)
                };
                let text = mem.frame_text_by_id(frame_id).unwrap_or_default();
                let details = serde_json::json!({
                    "frame_id": frame_id,
                    "uri": frame.uri,
                    "title": frame.title,
                    "text": text
                });
                let output = if details["text"].as_str().unwrap_or("").is_empty() {
                    format!("Frame #{frame_id} (non-text payload)")
                } else {
                    details["text"].as_str().unwrap_or("").to_string()
                };
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })
        }
        "put" => {
            let parsed: ToolPutArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let Some(text) = parsed.text else {
                return Err("put requires text".into());
            };
            let result = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut options = PutOptions::default();
                options.uri = Some(parsed.uri.clone());
                options.title = Some(parsed.title.unwrap_or_else(|| parsed.uri.clone()));
                options.track = parsed.track;
                options.kind = parsed.kind;
                options.search_text = Some(text.clone());
                let frame_id = mem
                    .put_bytes_with_options(text.as_bytes(), options)
                    .map_err(|e| e.to_string())?;
                mem.commit().map_err(|e| e.to_string())?;
                let details = serde_json::json!({
                    "frame_id": frame_id,
                    "uri": parsed.uri
                });
                let output = format!("Stored frame #{frame_id}");
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })?;
            *mem_read = None;
            Ok(result)
        }
        "log" => {
            let parsed: ToolLogArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let entry = AgentLogEntry {
                session: parsed.session.clone(),
                role: parsed.role.unwrap_or_else(|| "user".to_string()),
                text: parsed.text.clone(),
                meta: parsed.meta.clone(),
                ts_utc: Some(Utc::now().timestamp()),
            };
            let result = with_write_mem(mem_read, mem_write, mv2, false, |mem| {
                let uri = append_agent_log(mem, &entry).map_err(|e| e.to_string())?;
                let details = serde_json::json!({ "uri": uri });
                Ok(ToolExecution {
                    output: "Logged agent turn.".to_string(),
                    details,
                    is_error: false,
                })
            })?;
            *mem_read = None;
            Ok(result)
        }
        "feedback" => {
            let parsed: ToolFeedbackArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let event = FeedbackEvent {
                uri: parsed.uri.clone(),
                score: parsed.score.clamp(-1.0, 1.0),
                note: parsed.note.clone(),
                session: parsed.session.clone(),
                ts_utc: Some(Utc::now().timestamp()),
            };
            let result = with_write_mem(mem_read, mem_write, mv2, false, |mem| {
                let uri_log = append_feedback(mem, &event).map_err(|e| e.to_string())?;
                let details = serde_json::json!({ "uri": uri_log });
                Ok(ToolExecution {
                    output: "Feedback recorded.".to_string(),
                    details,
                    is_error: false,
                })
            })?;
            *mem_read = None;
            Ok(result)
        }
        "config_set" => {
            let parsed: ToolConfigSetArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            save_config_to_file(&workspace, &parsed.key, parsed.json.clone())
                .map_err(|e| e.to_string())?;
            Ok(ToolExecution {
                output: format!("Config saved to file ({})", parsed.key),
                details: serde_json::json!({ "file": workspace.join("config.json") }),
                is_error: false,
            })
        }
        "memory_sync" => {
            let parsed: ToolMemorySyncArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = parsed
                .workspace
                .map(PathBuf::from)
                .or_else(|| workspace_override.clone())
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let include_daily = parsed.include_daily.unwrap_or(true);
            let ids =
                sync_workspace_memory(mv2, &workspace, include_daily).map_err(|e| e.to_string())?;
            *mem_read = None;
            Ok(ToolExecution {
                output: format!("Synced {} memory files.", ids.len()),
                details: serde_json::json!({ "frame_ids": ids }),
                is_error: false,
            })
        }
        "memory_export" => {
            let parsed: ToolMemoryExportArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = parsed
                .workspace
                .map(PathBuf::from)
                .or_else(|| workspace_override.clone())
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let include_daily = parsed.include_daily.unwrap_or(true);
            let paths =
                export_capsule_memory(mv2, &workspace, include_daily).map_err(|e| e.to_string())?;
            Ok(ToolExecution {
                output: format!("Exported {} files.", paths.len()),
                details: serde_json::json!({ "paths": paths }),
                is_error: false,
            })
        }
        "memory_search" => {
            let parsed: ToolMemorySearchArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let request = SearchRequest {
                    query: parsed.query.clone(),
                    top_k: parsed.limit.unwrap_or(10),
                    snippet_chars: 300,
                    uri: None,
                    scope: Some("aethervault://memory/".to_string()),
                    cursor: None,
                    temporal: None,
                    as_of_frame: None,
                    as_of_ts: None,
                    no_sketch: false,
                };
                let response = mem.search(request).map_err(|e| e.to_string())?;
                let mut lines = Vec::new();
                for hit in response.hits.iter().take(5) {
                    let title = hit.title.clone().unwrap_or_default();
                    lines.push(format!("{}. {} {}", hit.rank, hit.uri, title));
                }
                let output = if lines.is_empty() {
                    "No results.".to_string()
                } else {
                    lines.join("\n")
                };
                let details = serde_json::to_value(response).map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })
        }
        "memory_append_daily" => {
            let parsed: ToolMemoryAppendArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let date = parsed
                .date
                .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
            let dir = workspace.join("memory");
            fs::create_dir_all(&dir).map_err(|e| format!("workspace: {e}"))?;
            let path = dir.join(format!("{date}.md"));
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| format!("memory open: {e}"))?;
            writeln!(file, "{}", parsed.text).map_err(|e| format!("memory write: {e}"))?;
            let uri = format!("aethervault://memory/daily/{date}.md");
            let result = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut options = PutOptions::default();
                options.uri = Some(uri.clone());
                options.title = Some(format!("memory daily {date}"));
                options.kind = Some("text/markdown".to_string());
                options.track = Some("aethervault.memory".to_string());
                options.search_text = Some(parsed.text.clone());
                let frame_id = mem
                    .put_bytes_with_options(parsed.text.as_bytes(), options)
                    .map_err(|e| e.to_string())?;
                mem.commit().map_err(|e| e.to_string())?;
                Ok(frame_id)
            })?;
            *mem_read = None;
            Ok(ToolExecution {
                output: format!("Appended to {}", path.display()),
                details: serde_json::json!({
                    "path": path.display().to_string(),
                    "uri": uri,
                    "frame_id": result
                }),
                is_error: false,
            })
        }
        "memory_remember" => {
            let parsed: ToolMemoryRememberArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            fs::create_dir_all(&workspace).map_err(|e| format!("workspace: {e}"))?;
            let path = workspace.join("MEMORY.md");
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| format!("memory open: {e}"))?;
            writeln!(file, "{}", parsed.text).map_err(|e| format!("memory write: {e}"))?;
            let uri = "aethervault://memory/longterm.md".to_string();
            let result = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut options = PutOptions::default();
                options.uri = Some(uri.clone());
                options.title = Some("memory longterm".to_string());
                options.kind = Some("text/markdown".to_string());
                options.track = Some("aethervault.memory".to_string());
                options.search_text = Some(parsed.text.clone());
                let frame_id = mem
                    .put_bytes_with_options(parsed.text.as_bytes(), options)
                    .map_err(|e| e.to_string())?;
                mem.commit().map_err(|e| e.to_string())?;
                Ok(frame_id)
            })?;
            *mem_read = None;
            Ok(ToolExecution {
                output: format!("Appended to {}", path.display()),
                details: serde_json::json!({
                    "path": path.display().to_string(),
                    "uri": uri,
                    "frame_id": result
                }),
                is_error: false,
            })
        }
        "email_list" => {
            let parsed: ToolEmailListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let mut cmd = build_external_command("himalaya", &[]);
            cmd.arg("envelope").arg("list").arg("--output").arg("json");
            if let Some(limit) = parsed.limit {
                cmd.arg("--limit").arg(limit.to_string());
            }
            if let Some(folder) = parsed.folder {
                cmd.arg("--folder").arg(folder);
            }
            if let Some(account) = parsed.account {
                cmd.arg("--account").arg(account);
            }
            let output = cmd.output().map_err(|e| format!("himalaya: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("himalaya error: {stderr}"));
            }
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let details = serde_json::from_str(&stdout)
                .unwrap_or_else(|_| serde_json::json!({ "raw": stdout }));
            Ok(ToolExecution {
                output: "Listed envelopes.".to_string(),
                details,
                is_error: false,
            })
        }
        "email_read" => {
            let parsed: ToolEmailReadArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let mut cmd = build_external_command("himalaya", &[]);
            cmd.arg("message")
                .arg("read")
                .arg(parsed.id)
                .arg("--output")
                .arg("json");
            if let Some(folder) = parsed.folder {
                cmd.arg("--folder").arg(folder);
            }
            if let Some(account) = parsed.account {
                cmd.arg("--account").arg(account);
            }
            let output = cmd.output().map_err(|e| format!("himalaya: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("himalaya error: {stderr}"));
            }
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let details = serde_json::from_str(&stdout)
                .unwrap_or_else(|_| serde_json::json!({ "raw": stdout }));
            Ok(ToolExecution {
                output: "Read message.".to_string(),
                details,
                is_error: false,
            })
        }
        "email_send" => {
            let parsed: ToolEmailSendArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let mut template = String::new();
            if let Some(from) = parsed.from {
                template.push_str(&format!("From: {from}\n"));
            }
            template.push_str(&format!("To: {}\n", parsed.to));
            if let Some(cc) = parsed.cc {
                template.push_str(&format!("Cc: {cc}\n"));
            }
            if let Some(bcc) = parsed.bcc {
                template.push_str(&format!("Bcc: {bcc}\n"));
            }
            if let Some(in_reply_to) = parsed.in_reply_to {
                template.push_str(&format!("In-Reply-To: {in_reply_to}\n"));
            }
            if let Some(references) = parsed.references {
                template.push_str(&format!("References: {references}\n"));
            }
            template.push_str(&format!("Subject: {}\n", parsed.subject));
            template.push('\n');
            template.push_str(&parsed.body);
            template.push('\n');

            let mut cmd = build_external_command("himalaya", &[]);
            cmd.arg("template").arg("send");
            let mut child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("himalaya: {e}"))?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(template.as_bytes())
                    .map_err(|e| format!("send stdin: {e}"))?;
            }
            let output = child
                .wait_with_output()
                .map_err(|e| format!("send output: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("himalaya error: {stderr}"));
            }
            Ok(ToolExecution {
                output: "Sent email.".to_string(),
                details: serde_json::json!({ "status": "sent" }),
                is_error: false,
            })
        }
        "email_archive" => {
            let parsed: ToolEmailArchiveArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let mut cmd = build_external_command("himalaya", &[]);
            cmd.arg("message").arg("move").arg(parsed.id).arg("Archive");
            if let Some(folder) = parsed.folder {
                cmd.arg("--folder").arg(folder);
            }
            if let Some(account) = parsed.account {
                cmd.arg("--account").arg(account);
            }
            let output = cmd.output().map_err(|e| format!("himalaya: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("himalaya error: {stderr}"));
            }
            Ok(ToolExecution {
                output: "Archived email.".to_string(),
                details: serde_json::json!({ "status": "archived" }),
                is_error: false,
            })
        }
        "exec" => {
            let parsed: ToolExecArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let timeout_ms = parsed.timeout_ms.unwrap_or(NO_TIMEOUT_MS).max(1);
            let estimated_ms = parsed.estimated_ms.unwrap_or(timeout_ms);
            let is_codex_session = parsed.command.to_ascii_lowercase().contains("codex");
            let should_background = parsed.background.unwrap_or(false)
                || (is_codex_session && estimated_ms >= EXEC_BACKGROUND_THRESHOLD_MS);

            if should_background {
                let response = submit_exec_background_job(
                    &parsed.command,
                    parsed.cwd.as_ref(),
                    timeout_ms,
                    estimated_ms,
                )?;
                let job_id = response
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let status_url = response
                    .get("status_url")
                    .and_then(|value| value.as_str())
                    .map(std::borrow::ToOwned::to_owned)
                    .unwrap_or_else(|| format!("/jobs/{job_id}/status"));
                let details = serde_json::json!({
                    "background": true,
                    "job_id": job_id,
                    "status_url": status_url,
                    "estimated_ms": estimated_ms,
                    "timeout_ms": timeout_ms
                });
                return Ok(ToolExecution {
                    output: format!("background job started: {job_id}"),
                    details,
                    is_error: false,
                });
            }

            let command = if cfg!(windows) {
                vec!["cmd".to_string(), "/C".to_string(), parsed.command]
            } else {
                vec!["sh".to_string(), "-c".to_string(), parsed.command]
            };
            let mut cmd = build_external_command(&command[0], &command[1..]);
            if let Some(cwd) = parsed.cwd {
                cmd.current_dir(cwd);
            }
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| format!("exec spawn: {e}"))?;
            let cancel_token = Arc::new(AtomicBool::new(false));
            let result = wait_for_child_monitored(&mut child, "exec", &cancel_token)?;
            let stdout = result.stdout;
            let stderr = result.stderr;
            let is_error = !result.status.success();
            let exit_code = subprocess_exit_info(&result.status);
            let details = serde_json::json!({
                "exit_code": exit_code,
                "stdout": stdout,
                "stderr": stderr
            });
            let output_text = subprocess_output_text(&stdout, &stderr, is_error);
            Ok(ToolExecution {
                output: output_text,
                details,
                is_error,
            })
        }
        "notify" => {
            let parsed: ToolNotifyArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let channel = parsed
                .channel
                .unwrap_or_else(|| "slack".to_string())
                .to_ascii_lowercase();
            let webhook = parsed.webhook.or_else(|| match channel.as_str() {
                "discord" => env_optional("DISCORD_WEBHOOK_URL"),
                "teams" => env_optional("TEAMS_WEBHOOK_URL"),
                _ => env_optional("SLACK_WEBHOOK_URL"),
            });
            let Some(webhook) = webhook else {
                return Err("notify requires webhook url".into());
            };
            let payload = match channel.as_str() {
                "discord" => serde_json::json!({ "content": parsed.text }),
                "teams" => serde_json::json!({ "text": parsed.text }),
                _ => serde_json::json!({ "text": parsed.text }),
            };
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
                .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                .timeout_write(Duration::from_millis(NO_TIMEOUT_MS))
                .build();
            let response = agent
                .post(&webhook)
                .set("content-type", "application/json")
                .send_json(payload);
            match response {
                Ok(_) => Ok(ToolExecution {
                    output: "Notification sent.".to_string(),
                    details: serde_json::json!({ "channel": channel }),
                    is_error: false,
                }),
                Err(err) => Err(format!("notify error: {err}")),
            }
        }
        "signal_send" => {
            let parsed: ToolSignalSendArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let sender = parsed.sender.or_else(|| env_optional("SIGNAL_SENDER"));
            let Some(sender) = sender else {
                return Err("signal_send requires sender".into());
            };
            let mut cmd = build_external_command("signal-cli", &[]);
            cmd.arg("-u")
                .arg(sender)
                .arg("send")
                .arg("-m")
                .arg(parsed.text)
                .arg(parsed.to);
            let output = cmd.output().map_err(|e| format!("signal-cli: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("signal-cli error: {stderr}"));
            }
            Ok(ToolExecution {
                output: "Signal message sent.".to_string(),
                details: serde_json::json!({ "status": "sent" }),
                is_error: false,
            })
        }
        "imessage_send" => {
            let parsed: ToolIMessageSendArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            if !cfg!(target_os = "macos") {
                return Err("imessage_send requires macOS".into());
            }
            let script = format!(
                "tell application \"Messages\" to send \"{}\" to buddy \"{}\"",
                parsed.text.replace('"', "\\\""),
                parsed.to.replace('"', "\\\"")
            );
            let mut cmd = build_external_command("osascript", &[]);
            cmd.arg("-e").arg(script);
            let output = cmd.output().map_err(|e| format!("osascript: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("osascript error: {stderr}"));
            }
            Ok(ToolExecution {
                output: "iMessage sent.".to_string(),
                details: serde_json::json!({ "status": "sent" }),
                is_error: false,
            })
        }
        "http_request" => {
            let parsed: ToolHttpRequestArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let method = parsed
                .method
                .unwrap_or_else(|| "GET".to_string())
                .to_ascii_uppercase();
            let timeout = parsed.timeout_ms.unwrap_or(NO_TIMEOUT_MS);
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_millis(timeout))
                .timeout_write(Duration::from_millis(timeout))
                .timeout_read(Duration::from_millis(timeout))
                .build();
            let mut req = match method.as_str() {
                "GET" => agent.get(&parsed.url),
                "POST" => agent.post(&parsed.url),
                "PUT" => agent.put(&parsed.url),
                "PATCH" => agent.patch(&parsed.url),
                "DELETE" => agent.delete(&parsed.url),
                _ => return Err(format!("unsupported method: {method}")),
            };
            if let Some(headers) = parsed.headers {
                for (k, v) in headers {
                    req = req.set(&k, &v);
                }
            }
            let resp = if let Some(body) = parsed.body {
                if parsed.json.unwrap_or(false) {
                    req.set("content-type", "application/json")
                        .send_string(&body)
                } else {
                    req.send_string(&body)
                }
            } else {
                req.call()
            };
            let (status, text) = match resp {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.into_string().unwrap_or_default();
                    (status, text)
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    (code, text)
                }
                Err(err) => return Err(format!("http_request failed: {err}")),
            };
            let truncated = if text.len() > 20_000 {
                let safe: String = text.chars().take(20_000).collect();
                format!("{safe}...[truncated]")
            } else {
                text
            };
            Ok(ToolExecution {
                output: format!("http_request {method} {} -> {status}", parsed.url),
                details: serde_json::json!({
                    "status": status,
                    "body": truncated
                }),
                is_error: status >= 400,
            })
        }
        "browser" => {
            let parsed: ToolBrowserArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let _timeout_ms = parsed.timeout_ms.unwrap_or(NO_TIMEOUT_MS);
            let session = parsed.session.unwrap_or_else(|| "default".to_string());

            let mut cmd_args: Vec<String> = vec!["--session".to_string(), session, "--".to_string()];
            let parts = shlex::split(&parsed.command)
                .ok_or_else(|| "browser: malformed command (unmatched quotes)".to_string())?;
            if parts.is_empty() {
                return Err("browser: command is empty".into());
            }
            cmd_args.extend(parts);

            let mut cmd = build_external_command("agent-browser", &cmd_args);
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let mut child = cmd.spawn().map_err(|e| format!("browser spawn: {e}"))?;
            let cancel_token = Arc::new(AtomicBool::new(false));
            let result = wait_for_child_monitored(&mut child, "browser", &cancel_token)?;
            let stdout = result.stdout;
            let stderr = result.stderr;
            let is_error = !result.status.success();
            let exit_code = subprocess_exit_info(&result.status);
            let details = serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code
            });
            let output_text = subprocess_output_text(&stdout, &stderr, is_error);

            Ok(ToolExecution {
                output: output_text,
                details,
                is_error,
            })
        }
        "excalidraw" => {
            let parsed: ToolExcalidrawArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;

            let tool_name = match parsed.action.as_str() {
                "read_me" => "read_me",
                "create_view" => "create_view",
                _ => return Err(format!("excalidraw: unknown action '{}', use 'read_me' or 'create_view'", parsed.action)),
            };
            let tool_args = if tool_name == "create_view" {
                let elements = parsed.elements
                    .ok_or("excalidraw: 'elements' required for create_view")?;
                serde_json::json!({ "elements": elements })
            } else {
                serde_json::json!({})
            };

            // Spawn excalidraw-mcp server via stdio
            let mcp_cmd = env_optional("EXCALIDRAW_MCP_CMD")
                .unwrap_or_else(|| "npx excalidraw-mcp --stdio".to_string());
            let cmd_parts = shlex::split(&mcp_cmd)
                .ok_or("excalidraw: malformed EXCALIDRAW_MCP_CMD")?;
            if cmd_parts.is_empty() {
                return Err("excalidraw: empty EXCALIDRAW_MCP_CMD".into());
            }
            let mut cmd = build_external_command(&cmd_parts[0], &cmd_parts[1..]);
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let mut child = cmd.spawn().map_err(|e| format!("excalidraw spawn: {e}"))?;
            let mut stdin = child.stdin.take().ok_or("excalidraw: no stdin")?;
            let stdout = child.stdout.take().ok_or("excalidraw: no stdout")?;

            // Run MCP interaction in a thread with cancellation-aware polling.
            // read_line can't hang the caller forever.  The closure also
            // guarantees cleanup (kill + wait) on both success and error paths.
            let tool_name = tool_name.to_string();
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let mut reader = BufReader::new(stdout);

                // Helper: send JSON-RPC message with Content-Length framing
                let send_msg = |writer: &mut dyn Write, msg: &serde_json::Value| -> Result<(), String> {
                    let body = serde_json::to_string(msg).map_err(|e| e.to_string())?;
                    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)
                        .map_err(|e| format!("excalidraw write: {e}"))?;
                    writer.flush().map_err(|e| format!("excalidraw flush: {e}"))?;
                    Ok(())
                };

                // Helper: read JSON-RPC response with Content-Length framing.
                // Reads headers until blank line, extracts Content-Length, then reads body.
                let read_msg = |reader: &mut BufReader<std::process::ChildStdout>| -> Result<serde_json::Value, String> {
                    let mut content_length: Option<usize> = None;
                    // Read headers until blank separator line
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).map_err(|e| format!("excalidraw read: {e}"))?;
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            if content_length.is_some() { break; }
                            continue; // skip leading blank lines before headers
                        }
                        if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
                            content_length = Some(len_str.trim().parse()
                                .map_err(|e| format!("excalidraw bad content-length: {e}"))?);
                        }
                        // ignore other headers (Content-Type, etc.)
                    }
                    let len = content_length.ok_or("excalidraw: missing Content-Length header")?;
                    if len > 10 * 1024 * 1024 {
                        return Err(format!("excalidraw: response too large ({len} bytes)"));
                    }
                    let mut body = vec![0u8; len];
                    io::Read::read_exact(reader, &mut body)
                        .map_err(|e| format!("excalidraw read body: {e}"))?;
                    serde_json::from_slice(&body).map_err(|e| format!("excalidraw parse: {e}"))
                };

                let result = (|| -> Result<serde_json::Value, String> {
                    // 1. Send initialize
                    send_msg(&mut stdin, &serde_json::json!({
                        "jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": {
                            "protocolVersion": "2024-11-05",
                            "capabilities": {},
                            "clientInfo": { "name": "aethervault", "version": "0.1" }
                        }
                    }))?;
                    let init_resp = read_msg(&mut reader)?;
                    if let Some(err) = init_resp.get("error") {
                        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
                        return Err(format!("excalidraw: MCP initialize failed: {msg}"));
                    }

                    // 2. Send initialized notification
                    send_msg(&mut stdin, &serde_json::json!({
                        "jsonrpc": "2.0", "method": "notifications/initialized"
                    }))?;

                    // 3. Call the tool
                    send_msg(&mut stdin, &serde_json::json!({
                        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                        "params": { "name": tool_name, "arguments": tool_args }
                    }))?;
                    read_msg(&mut reader)
                })();

                // Cleanup always runs regardless of success/failure
                drop(stdin);
                let _ = tx.send(result);
            });

            let cancel_token = Arc::new(AtomicBool::new(false));
            let mut last_update = Instant::now();
            let tool_resp: serde_json::Value = loop {
                if cancel_token.load(Ordering::Acquire) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("excalidraw: canceled while waiting for MCP response".into());
                }
                match rx.recv_timeout(Duration::from_millis(PROCESS_POLL_MS)) {
                    Ok(result) => {
                        // Thread completed; child will exit after stdin is dropped
                        let _ = child.kill();
                        let _ = child.wait();
                        break result?;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if last_update.elapsed() >= Duration::from_millis(STATUS_REPORT_MS) {
                            eprintln!("[tool:excalidraw] waiting for MCP response (no deadline)");
                            last_update = Instant::now();
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err("excalidraw: MCP worker channel disconnected".into());
                    }
                }
            };

            // Check for JSON-RPC error
            if let Some(err) = tool_resp.get("error") {
                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
                let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                return Err(format!("excalidraw: MCP error {code}: {msg}"));
            }
            let result = tool_resp.get("result")
                .cloned()
                .ok_or("excalidraw: MCP response missing 'result' field")?;
            let content_text = result.get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            let is_error = result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(ToolExecution {
                output: content_text.to_string(),
                details: result,
                is_error,
            })
        }
        "fs_list" => {
            let parsed: ToolFsListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let roots = allowed_fs_roots(&workspace_override);
            let resolved = resolve_fs_path(&parsed.path, &roots)?;
            let mut items = Vec::new();
            let max_entries = parsed.max_entries.unwrap_or(200);
            if parsed.recursive.unwrap_or(false) {
                for entry in WalkDir::new(&resolved).max_depth(6) {
                    let entry = entry.map_err(|e| e.to_string())?;
                    if items.len() >= max_entries {
                        break;
                    }
                    items.push(entry.path().display().to_string());
                }
            } else if resolved.is_dir() {
                for entry in fs::read_dir(&resolved).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    items.push(entry.path().display().to_string());
                    if items.len() >= max_entries {
                        break;
                    }
                }
            } else if resolved.exists() {
                items.push(resolved.display().to_string());
            }
            Ok(ToolExecution {
                output: format!("Listed {} entries.", items.len()),
                details: serde_json::json!({ "entries": items }),
                is_error: false,
            })
        }
        "fs_read" => {
            let parsed: ToolFsReadArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let roots = allowed_fs_roots(&workspace_override);
            let resolved = resolve_fs_path(&parsed.path, &roots)?;
            let max_bytes = parsed.max_bytes.unwrap_or(200_000);
            let file = fs::File::open(&resolved).map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            file.take(max_bytes as u64)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&buf).to_string();
            Ok(ToolExecution {
                output: format!("Read {} bytes.", buf.len()),
                details: serde_json::json!({
                    "path": resolved.display().to_string(),
                    "text": text
                }),
                is_error: false,
            })
        }
        "fs_write" => {
            let parsed: ToolFsWriteArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let roots = allowed_fs_roots(&workspace_override);
            let resolved = resolve_fs_path(&parsed.path, &roots)?;
            if parsed.append.unwrap_or(false) {
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&resolved)
                    .map_err(|e| e.to_string())?;
                file.write_all(parsed.text.as_bytes())
                    .map_err(|e| e.to_string())?;
            } else {
                fs::write(&resolved, parsed.text.as_bytes()).map_err(|e| e.to_string())?;
            }
            Ok(ToolExecution {
                output: "File written.".to_string(),
                details: serde_json::json!({ "path": resolved.display().to_string() }),
                is_error: false,
            })
        }
        "approval_list" => with_read_mem(mem_read, mem_write, mv2, |mem| {
            let approvals = load_approvals(mem);
            let pending: Vec<ApprovalEntry> = approvals
                .into_iter()
                .filter(|a| a.status == "pending")
                .collect();
            Ok(ToolExecution {
                output: format!("{} pending approvals.", pending.len()),
                details: serde_json::json!({ "approvals": pending }),
                is_error: false,
            })
        }),
        "trigger_add" => {
            let parsed: ToolTriggerAddArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut triggers = load_triggers(mem);
                let id = format!(
                    "trg_{}_{}",
                    chrono::Utc::now().timestamp(),
                    triggers.len() + 1
                );
                // Validate kind-specific required fields
                match parsed.kind.as_str() {
                    "cron" => {
                        if parsed.cron.is_none() {
                            return Err("kind=cron requires a 'cron' expression".into());
                        }
                    }
                    "webhook" => {
                        if parsed.webhook_url.is_none() {
                            return Err("kind=webhook requires a 'webhook_url'".into());
                        }
                    }
                    "email" | "calendar_free" => {}
                    other => {
                        return Err(format!("Unknown trigger kind: '{other}'"));
                    }
                }
                // Validate cron expression if provided
                if let Some(ref cron_str) = parsed.cron {
                    if let Err(e) = CronExpr::parse(cron_str) {
                        return Err(format!("Invalid cron expression: {e}"));
                    }
                }
                // Validate webhook URL (SSRF protection)
                if let Some(ref url) = parsed.webhook_url {
                    if !url.starts_with("https://") && !url.starts_with("http://") {
                        return Err("webhook_url must use http:// or https://".into());
                    }
                    let lower = url.to_lowercase();
                    if lower.contains("localhost") || lower.contains("127.0.0.1")
                        || lower.contains("[::1]") || lower.contains("169.254.169.254")
                        || lower.contains("10.0.") || lower.contains("192.168.") {
                        return Err("webhook_url cannot target private/internal addresses".into());
                    }
                }
                // Validate webhook method
                if let Some(ref m) = parsed.webhook_method {
                    let upper = m.to_uppercase();
                    if upper != "GET" && upper != "POST" {
                        return Err(format!("webhook_method must be GET or POST, got '{m}'"));
                    }
                }
                let entry = TriggerEntry {
                    id: id.clone(),
                    kind: parsed.kind,
                    name: parsed.name,
                    query: parsed.query,
                    prompt: parsed.prompt,
                    start: parsed.start,
                    end: parsed.end,
                    enabled: parsed.enabled.unwrap_or(true),
                    last_seen: None,
                    last_fired: None,
                    cron: parsed.cron,
                    webhook_url: parsed.webhook_url,
                    webhook_method: parsed.webhook_method,
                    schedule_name: None,
                };
                triggers.push(entry);
                save_triggers(mem, &triggers)?;
                Ok(ToolExecution {
                    output: "Trigger added.".to_string(),
                    details: serde_json::json!({ "id": id }),
                    is_error: false,
                })
            })
        }
        "trigger_list" => with_write_mem(mem_read, mem_write, mv2, true, |mem| {
            let triggers = load_triggers(mem);
            Ok(ToolExecution {
                output: format!("{} triggers.", triggers.len()),
                details: serde_json::json!({ "triggers": triggers }),
                is_error: false,
            })
        }),
        "trigger_remove" => {
            let parsed: ToolTriggerRemoveArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut triggers = load_triggers(mem);
                let before = triggers.len();
                triggers.retain(|t| t.id != parsed.id);
                let updated = triggers.len() != before;
                if updated {
                    save_triggers(mem, &triggers)?;
                }
                Ok(ToolExecution {
                    output: if updated {
                        "Trigger removed.".to_string()
                    } else {
                        "Trigger not found.".to_string()
                    },
                    details: serde_json::json!({ "id": parsed.id, "updated": updated }),
                    is_error: !updated,
                })
            })
        }
        "tool_search" => {
            let parsed: ToolToolSearchArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let query_tokens: Vec<String> = parsed
                .query
                .to_ascii_lowercase()
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            let mut results = Vec::new();
            for tool in tool_definitions_json() {
                let name = tool
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let desc = tool
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let score = tool_score(&query_tokens, &name, &desc);
                if score > 0 {
                    results.push(serde_json::json!({
                        "name": name,
                        "description": desc,
                        "score": score
                    }));
                }
            }
            results.sort_by(|a, b| {
                b.get("score")
                    .and_then(|v| v.as_i64())
                    .cmp(&a.get("score").and_then(|v| v.as_i64()))
            });
            let limit = parsed.limit.unwrap_or(8);
            let results: Vec<serde_json::Value> = results.into_iter().take(limit).collect();
            Ok(ToolExecution {
                output: format!("Found {} tools.", results.len()),
                details: serde_json::json!({ "results": results }),
                is_error: false,
            })
        }
        "session_context" => {
            let parsed: ToolSessionContextArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let limit = parsed.limit.unwrap_or(20);
            // Try JSONL logs first, fall back to MV2 capsule
            let workspace = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let log_dir = log_dir_path(&workspace);
            let jsonl_entries = load_session_logs(&log_dir, &parsed.session, limit);
            if !jsonl_entries.is_empty() {
                let results: Vec<serde_json::Value> = jsonl_entries
                    .into_iter()
                    .map(|e| {
                        serde_json::json!({
                            "ts": e.ts_utc,
                            "role": e.role,
                            "text": e.text,
                            "meta": e.meta,
                            "source": "jsonl"
                        })
                    })
                    .collect();
                Ok(ToolExecution {
                    output: format!("Loaded {} entries from logs.", results.len()),
                    details: serde_json::json!({ "entries": results }),
                    is_error: false,
                })
            } else {
                // Fallback: search MV2 capsule for legacy data
                let scope = format!("aethervault://agent-log/{}/", parsed.session);
                with_read_mem(mem_read, mem_write, mv2, |mem| {
                    let request = SearchRequest {
                        query: parsed.session.clone(),
                        top_k: 200,
                        snippet_chars: 200,
                        uri: None,
                        scope: Some(scope),
                        cursor: None,
                        temporal: None,
                        as_of_frame: None,
                        as_of_ts: None,
                        no_sketch: true,
                    };
                    let response = mem.search(request).map_err(|e| e.to_string())?;
                    let mut entries = Vec::new();
                    for hit in response.hits {
                        let uri = hit.uri.clone();
                        let ts = parse_log_ts_from_uri(&uri).unwrap_or_default();
                        if let Ok(text) = mem.frame_text_by_id(hit.frame_id) {
                            if let Ok(entry) = serde_json::from_str::<AgentLogEntry>(&text) {
                                entries.push(serde_json::json!({
                                    "ts": entry.ts_utc.unwrap_or(ts),
                                    "role": entry.role,
                                    "text": entry.text,
                                    "meta": entry.meta,
                                    "source": "capsule"
                                }));
                            }
                        }
                    }
                    entries.sort_by(|a, b| {
                        b.get("ts")
                            .and_then(|v| v.as_i64())
                            .cmp(&a.get("ts").and_then(|v| v.as_i64()))
                    });
                    let results: Vec<serde_json::Value> = entries.into_iter().take(limit).collect();
                    Ok(ToolExecution {
                        output: format!("Loaded {} entries from capsule.", results.len()),
                        details: serde_json::json!({ "entries": results }),
                        is_error: false,
                    })
                })
            }
        }
        "reflect" => {
            let parsed: ToolReflectArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let session = parsed
                .session
                .clone()
                .unwrap_or_else(|| "default".to_string());
            let ts = Utc::now().timestamp();
            let payload = serde_json::json!({
                "session": session,
                "text": parsed.text,
                "reason": parsed.reason,
                "ts_utc": ts
            });
            let bytes = serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?;
            let hash = blake3_hash(&bytes);
            let uri = format!(
                "aethervault://memory/reflection/{}/{}-{}",
                session,
                ts,
                hash.to_hex()
            );
            with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut options = PutOptions::default();
                options.uri = Some(uri.clone());
                options.title = Some("reflection".to_string());
                options.kind = Some("application/json".to_string());
                options.track = Some("aethervault.reflection".to_string());
                options.search_text = Some(payload.to_string());
                mem.put_bytes_with_options(&bytes, options)
                    .map_err(|e| e.to_string())?;
                mem.commit().map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output: "Reflection stored.".to_string(),
                    details: serde_json::json!({ "uri": uri }),
                    is_error: false,
                })
            })
        }
        "skill_store" => {
            let parsed: ToolSkillStoreArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let db_path = workspace.join("skills.sqlite");
            let conn = open_skill_db(&db_path).map_err(|e| format!("skill db: {e}"))?;
            let now = Utc::now().to_rfc3339();
            let skill = SkillRecord {
                name: parsed.name.clone(),
                trigger: parsed.trigger,
                steps: parsed.steps.unwrap_or_default(),
                tools: parsed.tools.unwrap_or_default(),
                notes: parsed.notes,
                success_rate: 0.0,
                times_used: 0,
                times_succeeded: 0,
                last_used: None,
                created_at: now,
                contexts: Vec::new(),
            };
            upsert_skill(&conn, &skill).map_err(|e| format!("upsert: {e}"))?;
            Ok(ToolExecution {
                output: format!("Skill '{}' stored in SQLite.", parsed.name),
                details: serde_json::json!({ "name": parsed.name, "db": db_path.display().to_string() }),
                is_error: false,
            })
        }
        "skill_search" => {
            let parsed: ToolSkillSearchArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let db_path = workspace.join("skills.sqlite");
            let limit = parsed.limit.unwrap_or(10);
            let conn = open_skill_db(&db_path).map_err(|e| format!("skill db: {e}"))?;
            let results = search_skills(&conn, &parsed.query, limit);
            let out: Vec<serde_json::Value> = results
                .into_iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "trigger": s.trigger,
                        "steps": s.steps,
                        "tools": s.tools,
                        "notes": s.notes,
                        "success_rate": s.success_rate,
                        "times_used": s.times_used,
                        "last_used": s.last_used,
                    })
                })
                .collect();
            Ok(ToolExecution {
                output: format!("Found {} skills.", out.len()),
                details: serde_json::json!({ "results": out }),
                is_error: false,
            })
        }
        "subagent_list" => {
            let ws = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let cfg_path = crate::config_file_path(&ws);
            let config = if cfg_path.exists() {
                crate::load_config_from_file(&ws)
            } else {
                with_read_mem(mem_read, mem_write, mv2, |mem| {
                    Ok(load_capsule_config(mem).unwrap_or_default())
                })?
            };
            let subagents = load_subagents_from_config(&config);
            let details: Vec<serde_json::Value> = subagents.iter().map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "description": s.description,
                    "tools": s.tools,
                    "disallowed_tools": s.disallowed_tools,
                    "max_steps": s.max_steps,
                    "timeout_secs": s.timeout_secs,
                })
            }).collect();
            Ok(ToolExecution {
                output: if details.is_empty() {
                    "No subagents configured. Define them in workspace config.json under agent.subagents.".to_string()
                } else {
                    format!("{} subagents available.", details.len())
                },
                details: serde_json::json!({ "subagents": details }),
                is_error: false,
            })
        }
        "subagent_invoke" => {
            let parsed: ToolSubagentInvokeArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let ws = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let cfg_path = crate::config_file_path(&ws);
            let config = if cfg_path.exists() {
                crate::load_config_from_file(&ws)
            } else {
                with_read_mem(mem_read, mem_write, mv2, |mem| {
                    Ok(load_capsule_config(mem).unwrap_or_default())
                })?
            };
            let subagents = load_subagents_from_config(&config);
            let mut system = parsed.system.clone();
            let mut model_hook = parsed.model_hook.clone();
            let spec = subagents.iter().find(|s| s.name == parsed.name);
            if let Some(spec) = spec {
                if system.is_none() {
                    system = spec.system.clone();
                }
                if model_hook.is_none() {
                    model_hook = spec.model_hook.clone();
                }
            } else if system.is_none() && model_hook.is_none() {
                return Err(format!("unknown subagent: {}", parsed.name));
            }

            // Resolve max_steps: invocation arg > spec > default 64
            let max_steps = parsed.max_steps
                .or(spec.and_then(|s| s.max_steps))
                .unwrap_or(64);

            // Resolve timeout: invocation arg > spec > none
            let timeout_secs = parsed.timeout_secs
                .or(spec.and_then(|s| s.timeout_secs));

            let cfg = build_bridge_agent_config(
                mv2.to_path_buf(),
                model_hook,
                system,
                false,
                None,
                8,
                12_000,
                max_steps,
                true,
                8,
            )
            .map_err(|e| e.to_string())?;
            // Release all capsule handles before spawning the subagent so it can
            // acquire its own locks without contending with the parent session.
            *mem_read = None;
            *mem_write = None;
            let session = format!("subagent:{}:{}", parsed.name, Utc::now().timestamp());
            let prompt = parsed.prompt.clone();
            let name_for_err = parsed.name.clone();

            // Spawn the bridge agent in a thread so we can apply a timeout if configured.
            let (tx, rx) = std::sync::mpsc::channel();
            thread::spawn(move || {
                let r = run_agent_for_bridge(&cfg, &prompt, session, None, None, None);
                let _ = tx.send(r);
            });

            let result = if let Some(t) = timeout_secs {
                rx.recv_timeout(std::time::Duration::from_secs(t))
                    .map_err(|_| format!("subagent '{}' timed out after {}s", name_for_err, t))?
                    .map_err(|e| e.to_string())?
            } else {
                rx.recv()
                    .map_err(|e| format!("channel error: {e}"))?
                    .map_err(|e| e.to_string())?
            };

            Ok(ToolExecution {
                output: result.final_text.unwrap_or_default(),
                details: serde_json::json!({ "session": result.session, "messages": result.messages.len() }),
                is_error: false,
            })
        }
        "subagent_batch" => {
            let parsed: ToolSubagentBatchArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            if parsed.invocations.is_empty() {
                return Err("subagent_batch requires at least one invocation".into());
            }
            let ws = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let cfg_path = crate::config_file_path(&ws);
            let config_snapshot = if cfg_path.exists() {
                crate::load_config_from_file(&ws)
            } else {
                with_read_mem(mem_read, mem_write, mv2, |mem| {
                    Ok(load_capsule_config(mem).unwrap_or_default())
                })?
            };
            let subagents = load_subagents_from_config(&config_snapshot);
            let ts = Utc::now().timestamp();

            // Release all capsule handles before spawning subagent threads so they can
            // each acquire their own locks without contending with the parent session.
            *mem_read = None;
            *mem_write = None;

            let max_conc = parsed.max_concurrent.unwrap_or(parsed.invocations.len());
            let max_conc = max_conc.max(1); // ensure at least 1

            // Prepare each invocation: resolve spec fields, build config.
            struct PreparedInvocation {
                name: String,
                prompt: String,
                cfg: Result<crate::types::BridgeAgentConfig, String>,
                index: usize,
            }
            let mut prepared: Vec<PreparedInvocation> = Vec::new();
            for (i, inv) in parsed.invocations.into_iter().enumerate() {
                let mut system = inv.system.clone();
                let mut model_hook = inv.model_hook.clone();
                let spec = subagents.iter().find(|s| s.name == inv.name);
                if let Some(spec) = spec {
                    if system.is_none() {
                        system = spec.system.clone();
                    }
                    if model_hook.is_none() {
                        model_hook = spec.model_hook.clone();
                    }
                } else if system.is_none() && model_hook.is_none() {
                    prepared.push(PreparedInvocation {
                        name: inv.name.clone(),
                        prompt: inv.prompt.clone(),
                        cfg: Err(format!("unknown subagent: {}", inv.name)),
                        index: i,
                    });
                    continue;
                }

                // Resolve max_steps: invocation arg > spec > default 64
                let max_steps = inv.max_steps
                    .or(spec.and_then(|s| s.max_steps))
                    .unwrap_or(64);

                let cfg = build_bridge_agent_config(
                    mv2.to_path_buf(),
                    model_hook,
                    system,
                    false,
                    None,
                    8,
                    12_000,
                    max_steps,
                    true,
                    8,
                )
                .map_err(|e| e.to_string());
                prepared.push(PreparedInvocation {
                    name: inv.name.clone(),
                    prompt: inv.prompt.clone(),
                    cfg,
                    index: i,
                });
            }

            // Process invocations in chunks of max_conc for concurrency limiting.
            let mut all_results: Vec<serde_json::Value> = Vec::new();
            let mut all_ok = true;

            for chunk in prepared.chunks(max_conc) {
                let mut handles: Vec<(String, std::thread::JoinHandle<Result<AgentRunOutput, String>>)> = Vec::new();
                for item in chunk {
                    let name = item.name.clone();
                    match &item.cfg {
                        Err(err) => {
                            let err = err.clone();
                            handles.push((name, thread::spawn(move || Err(err))));
                        }
                        Ok(cfg) => {
                            let cfg = cfg.clone();
                            let session = format!("subagent:{}:{}:{}", item.name, ts, item.index);
                            let prompt = item.prompt.clone();
                            handles.push((name, thread::spawn(move || {
                                run_agent_for_bridge(&cfg, &prompt, session, None, None, None)
                            })));
                        }
                    }
                }

                // Collect results from this chunk before starting the next.
                for (name, handle) in handles {
                    match handle.join() {
                        Ok(Ok(output)) => {
                            all_results.push(serde_json::json!({
                                "name": name,
                                "status": "ok",
                                "output": output.final_text.unwrap_or_default(),
                                "session": output.session,
                                "messages": output.messages.len(),
                            }));
                        }
                        Ok(Err(err)) => {
                            all_ok = false;
                            all_results.push(serde_json::json!({
                                "name": name,
                                "status": "error",
                                "error": err,
                            }));
                        }
                        Err(_) => {
                            all_ok = false;
                            all_results.push(serde_json::json!({
                                "name": name,
                                "status": "error",
                                "error": "subagent thread panicked",
                            }));
                        }
                    }
                }
            }

            let summary = if all_ok {
                format!("{} subagents completed successfully.", all_results.len())
            } else {
                let ok_count = all_results.iter().filter(|r| r["status"] == "ok").count();
                let err_count = all_results.len() - ok_count;
                format!("{} subagents completed, {} failed.", ok_count, err_count)
            };
            Ok(ToolExecution {
                output: summary,
                details: serde_json::json!({ "results": all_results }),
                is_error: !all_ok,
            })
        }
        "gmail_list" => {
            let parsed: ToolGmailListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "google").map_err(|e| e.to_string())?;
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                .build();
            let mut url = format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults={}",
                parsed.max_results.unwrap_or(10)
            );
            if let Some(q) = parsed.query {
                url.push_str("&q=");
                url.push_str(&urlencoding::encode(&q));
            }
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("gmail_list error {code}: {text}").into());
                }
                Err(err) => return Err(format!("gmail_list failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Gmail messages listed.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "gmail_read" => {
            let parsed: ToolGmailReadArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "google").map_err(|e| e.to_string())?;
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                .build();
            let url = format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}?format=full",
                parsed.id
            );
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("gmail_read error {code}: {text}").into());
                }
                Err(err) => return Err(format!("gmail_read failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Gmail message read.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "gmail_send" => {
            let parsed: ToolGmailSendArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "google").map_err(|e| e.to_string())?;
            let raw = format!(
                "To: {}\r\nSubject: {}\r\n\r\n{}\r\n",
                parsed.to, parsed.subject, parsed.body
            );
            let encoded = base64::engine::general_purpose::STANDARD
                .encode(raw.as_bytes())
                .replace('+', "-")
                .replace('/', "_")
                .trim_end_matches('=')
                .to_string();
            let payload = serde_json::json!({ "raw": encoded });
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                .build();
            let resp = agent
                .post("https://gmail.googleapis.com/gmail/v1/users/me/messages/send")
                .set("authorization", &format!("Bearer {}", token))
                .set("content-type", "application/json")
                .send_json(payload);
            match resp {
                Ok(resp) => Ok(ToolExecution {
                    output: "Gmail message sent.".to_string(),
                    details: resp
                        .into_json::<serde_json::Value>()
                        .map_err(|e| e.to_string())?,
                    is_error: false,
                }),
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    Err(format!("gmail_send error {code}: {text}").into())
                }
                Err(err) => Err(format!("gmail_send failed: {err}").into()),
            }
        }
        "gcal_list" => {
            let parsed: ToolGCalListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "google").map_err(|e| e.to_string())?;
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                .build();
            let url = format!(
                "https://www.googleapis.com/calendar/v3/calendars/primary/events?maxResults={}",
                parsed.max_results.unwrap_or(10)
            );
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("gcal_list error {code}: {text}").into());
                }
                Err(err) => return Err(format!("gcal_list failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Calendar events listed.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "gcal_create" => {
            let parsed: ToolGCalCreateArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "google").map_err(|e| e.to_string())?;
            let payload = serde_json::json!({
                "summary": parsed.summary,
                "description": parsed.description,
                "start": { "dateTime": parsed.start },
                "end": { "dateTime": parsed.end }
            });
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                .build();
            let resp = agent
                .post("https://www.googleapis.com/calendar/v3/calendars/primary/events")
                .set("authorization", &format!("Bearer {}", token))
                .set("content-type", "application/json")
                .send_json(payload);
            match resp {
                Ok(resp) => Ok(ToolExecution {
                    output: "Calendar event created.".to_string(),
                    details: resp
                        .into_json::<serde_json::Value>()
                        .map_err(|e| e.to_string())?,
                    is_error: false,
                }),
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    Err(format!("gcal_create error {code}: {text}").into())
                }
                Err(err) => Err(format!("gcal_create failed: {err}").into()),
            }
        }
        "ms_mail_list" => {
            let parsed: ToolMsMailListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "microsoft").map_err(|e| e.to_string())?;
            let url = format!(
                "https://graph.microsoft.com/v1.0/me/messages?$top={}",
                parsed.top.unwrap_or(10)
            );
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                .build();
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("ms_mail_list error {code}: {text}").into());
                }
                Err(err) => return Err(format!("ms_mail_list failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Microsoft mail listed.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "ms_mail_read" => {
            let parsed: ToolMsMailReadArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "microsoft").map_err(|e| e.to_string())?;
            let url = format!("https://graph.microsoft.com/v1.0/me/messages/{}", parsed.id);
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                .build();
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("ms_mail_read error {code}: {text}").into());
                }
                Err(err) => return Err(format!("ms_mail_read failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Microsoft mail read.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "ms_calendar_list" => {
            let parsed: ToolMsCalendarListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "microsoft").map_err(|e| e.to_string())?;
            let url = format!(
                "https://graph.microsoft.com/v1.0/me/events?$top={}",
                parsed.top.unwrap_or(10)
            );
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                .build();
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("ms_calendar_list error {code}: {text}").into());
                }
                Err(err) => return Err(format!("ms_calendar_list failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Microsoft calendar listed.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "ms_calendar_create" => {
            let parsed: ToolMsCalendarCreateArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "microsoft").map_err(|e| e.to_string())?;
            let payload = serde_json::json!({
                "subject": parsed.subject,
                "body": {
                    "contentType": "Text",
                    "content": parsed.body.unwrap_or_default()
                },
                "start": { "dateTime": parsed.start, "timeZone": "UTC" },
                "end": { "dateTime": parsed.end, "timeZone": "UTC" }
            });
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                .build();
            let resp = agent
                .post("https://graph.microsoft.com/v1.0/me/events")
                .set("authorization", &format!("Bearer {}", token))
                .set("content-type", "application/json")
                .send_json(payload);
            match resp {
                Ok(resp) => Ok(ToolExecution {
                    output: "Microsoft calendar event created.".to_string(),
                    details: resp
                        .into_json::<serde_json::Value>()
                        .map_err(|e| e.to_string())?,
                    is_error: false,
                }),
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    Err(format!("ms_calendar_create error {code}: {text}").into())
                }
                Err(err) => Err(format!("ms_calendar_create failed: {err}").into()),
            }
        }
        "scale" => {
            let parsed: ToolScaleArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            match parsed.action.as_str() {
                "status" => {
                    // Pure local: read /proc files + df for system stats
                    let cpu_count = std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(1);
                    let (load_1m, load_5m) =
                        std::fs::read_to_string("/proc/loadavg")
                            .ok()
                            .and_then(|s| {
                                let parts: Vec<&str> = s.split_whitespace().collect();
                                if parts.len() >= 2 {
                                    Some((
                                        parts[0].parse::<f64>().unwrap_or(0.0),
                                        parts[1].parse::<f64>().unwrap_or(0.0),
                                    ))
                                } else {
                                    None
                                }
                            })
                            .unwrap_or((0.0, 0.0));
                    let (mem_total_mb, mem_avail_mb) =
                        std::fs::read_to_string("/proc/meminfo")
                            .ok()
                            .map(|s| {
                                let mut total: u64 = 0;
                                let mut avail: u64 = 0;
                                for line in s.lines() {
                                    if line.starts_with("MemTotal:") {
                                        total = line
                                            .split_whitespace()
                                            .nth(1)
                                            .and_then(|v| v.parse::<u64>().ok())
                                            .unwrap_or(0)
                                            / 1024;
                                    } else if line.starts_with("MemAvailable:") {
                                        avail = line
                                            .split_whitespace()
                                            .nth(1)
                                            .and_then(|v| v.parse::<u64>().ok())
                                            .unwrap_or(0)
                                            / 1024;
                                    }
                                }
                                (total, avail)
                            })
                            .unwrap_or((0, 0));
                    let mem_used_pct = if mem_total_mb > 0 {
                        ((mem_total_mb - mem_avail_mb) as f64 / mem_total_mb as f64 * 100.0)
                            .round()
                    } else {
                        0.0
                    };
                    // Disk via df
                    let (disk_total_gb, disk_used_gb, disk_used_pct) = std::process::Command::new("df")
                        .args(["-BG", "/"])
                        .output()
                        .ok()
                        .and_then(|out| {
                            let text = String::from_utf8_lossy(&out.stdout);
                            let line = text.lines().nth(1)?;
                            let parts: Vec<&str> = line.split_whitespace().collect();
                            if parts.len() >= 5 {
                                let total = parts[1]
                                    .trim_end_matches('G')
                                    .parse::<f64>()
                                    .unwrap_or(0.0);
                                let used = parts[2]
                                    .trim_end_matches('G')
                                    .parse::<f64>()
                                    .unwrap_or(0.0);
                                let pct = parts[4]
                                    .trim_end_matches('%')
                                    .parse::<f64>()
                                    .unwrap_or(0.0);
                                Some((total, used, pct))
                            } else {
                                None
                            }
                        })
                        .unwrap_or((0.0, 0.0, 0.0));
                    let details = serde_json::json!({
                        "cpu_count": cpu_count,
                        "load_1m": load_1m,
                        "load_5m": load_5m,
                        "mem_total_mb": mem_total_mb,
                        "mem_avail_mb": mem_avail_mb,
                        "mem_used_pct": mem_used_pct,
                        "disk_total_gb": disk_total_gb,
                        "disk_used_gb": disk_used_gb,
                        "disk_used_pct": disk_used_pct,
                    });
                    Ok(ToolExecution {
                        output: format!(
                            "CPU: {} cores, load {:.1}/{:.1} | RAM: {}MB/{} MB ({:.0}% used) | Disk: {:.0}G/{:.0}G ({:.0}% used)",
                            cpu_count, load_1m, load_5m, mem_total_mb - mem_avail_mb, mem_total_mb, mem_used_pct,
                            disk_used_gb, disk_total_gb, disk_used_pct,
                        ),
                        details,
                        is_error: false,
                    })
                }
                "sizes" => {
                    let do_token = env_optional("DO_TOKEN")
                        .ok_or_else(|| "DO_TOKEN not set — cannot query DigitalOcean API".to_string())?;
                    let out = std::process::Command::new("curl")
                        .args([
                            "-s",
                            "-X", "GET",
                            "https://api.digitalocean.com/v2/sizes",
                            "-H", &format!("Authorization: Bearer {}", do_token),
                        ])
                        .output()
                        .map_err(|e| format!("curl failed: {e}"))?;
                    let body: serde_json::Value =
                        serde_json::from_slice(&out.stdout)
                            .map_err(|e| format!("invalid JSON from DO API: {e}"))?;
                    let sizes = body
                        .get("sizes")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    // Filter to ≤8 vCPU / ≤32GB to prevent cost overruns
                    let filtered: Vec<serde_json::Value> = sizes
                        .into_iter()
                        .filter(|s| {
                            let vcpus = s.get("vcpus").and_then(|v| v.as_u64()).unwrap_or(99);
                            let mem = s.get("memory").and_then(|v| v.as_u64()).unwrap_or(999999);
                            let available = s.get("available").and_then(|v| v.as_bool()).unwrap_or(false);
                            vcpus <= 8 && mem <= 32768 && available
                        })
                        .map(|s| {
                            serde_json::json!({
                                "slug": s.get("slug").and_then(|v| v.as_str()).unwrap_or(""),
                                "vcpus": s.get("vcpus").and_then(|v| v.as_u64()).unwrap_or(0),
                                "memory_mb": s.get("memory").and_then(|v| v.as_u64()).unwrap_or(0),
                                "disk_gb": s.get("disk").and_then(|v| v.as_u64()).unwrap_or(0),
                                "price_monthly": s.get("price_monthly").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            })
                        })
                        .collect();
                    let details = serde_json::json!({ "sizes": filtered });
                    Ok(ToolExecution {
                        output: format!("{} available sizes (≤8 vCPU, ≤32GB).", filtered.len()),
                        details,
                        is_error: false,
                    })
                }
                "resize" => {
                    let target_size = parsed
                        .size
                        .ok_or_else(|| "size parameter is required for resize".to_string())?;
                    let do_token = env_optional("DO_TOKEN")
                        .ok_or_else(|| "DO_TOKEN not set — cannot call DigitalOcean API".to_string())?;
                    // Get droplet ID: env var or auto-detect via DO metadata
                    let droplet_id = env_optional("DO_DROPLET_ID").or_else(|| {
                        std::process::Command::new("curl")
                            .args(["-s", "http://169.254.169.254/metadata/v1/id"])
                            .output()
                            .ok()
                            .and_then(|o| {
                                let id = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
                                    Some(id)
                                } else {
                                    None
                                }
                            })
                    }).ok_or_else(|| "DO_DROPLET_ID not set and metadata API unreachable".to_string())?;
                    let url = format!(
                        "https://api.digitalocean.com/v2/droplets/{}/actions",
                        droplet_id
                    );
                    let payload = serde_json::json!({
                        "type": "resize",
                        "disk": false,
                        "size": target_size,
                    });
                    let out = std::process::Command::new("curl")
                        .args([
                            "-s",
                            "-X", "POST",
                            &url,
                            "-H", &format!("Authorization: Bearer {}", do_token),
                            "-H", "Content-Type: application/json",
                            "-d", &payload.to_string(),
                        ])
                        .output()
                        .map_err(|e| format!("curl failed: {e}"))?;
                    let resp: serde_json::Value =
                        serde_json::from_slice(&out.stdout)
                            .map_err(|e| format!("invalid JSON from DO API: {e}"))?;
                    let action_status = resp
                        .get("action")
                        .and_then(|a| a.get("status"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let action_id = resp
                        .get("action")
                        .and_then(|a| a.get("id"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    if action_status == "errored" || resp.get("id").is_some_and(|v| v.as_str() == Some("not_found")) {
                        let msg = resp.get("message").and_then(|v| v.as_str()).unwrap_or("resize failed");
                        return Err(format!("DO resize error: {msg}"));
                    }
                    Ok(ToolExecution {
                        output: format!(
                            "Resize to {} initiated (action {}, status: {}). Note: CPU resizes require a power cycle to take effect.",
                            target_size, action_id, action_status
                        ),
                        details: resp,
                        is_error: false,
                    })
                }
                other => Err(format!("unknown scale action: {other} (use status, sizes, or resize)")),
            }
        }
        "self_upgrade" => {
            let parsed: ToolSelfUpgradeArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let branch = parsed.branch.as_deref().unwrap_or("main");
            let skip_tests = parsed.skip_tests.unwrap_or(false);
            let upgrade_script = "/opt/aethervault/upgrade.sh";
            if !std::path::Path::new(upgrade_script).exists() {
                return Err("upgrade.sh not found at /opt/aethervault/upgrade.sh — deploy it first".into());
            }
            let mut cmd = std::process::Command::new("bash");
            cmd.arg(upgrade_script)
                .arg("--branch").arg(branch);
            if skip_tests {
                cmd.arg("--skip-tests");
            }
            cmd.stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let output = cmd.output().map_err(|e| format!("failed to run upgrade.sh: {e}"))?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let combined = if stderr.is_empty() {
                stdout.clone()
            } else {
                format!("{stdout}\n--- stderr ---\n{stderr}")
            };
            if output.status.success() {
                Ok(ToolExecution {
                    output: format!("Upgrade succeeded (branch: {branch}). Binary hot-swapped. Service will restart momentarily.\n\n{combined}"),
                    details: serde_json::json!({
                        "branch": branch,
                        "skip_tests": skip_tests,
                        "exit_code": 0,
                    }),
                    is_error: false,
                })
            } else {
                let code = output.status.code().unwrap_or(-1);
                Err(format!("upgrade.sh failed (exit {code}):\n{combined}"))
            }
        }
        _ => Err("unknown tool".into()),
    }
}

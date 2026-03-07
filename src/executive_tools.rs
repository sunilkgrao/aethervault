use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;

use aether_core::types::SearchRequest;
use aether_core::{PutOptions, Vault};
use chrono::Utc;

use crate::agent_runtime::{
    AgentRunOutput, build_subagent_agent_config, load_subagents_from_config, run_agent_for_bridge,
};
use crate::executive_state::{
    ExecutiveStateItem, StateCaptureInput, load_executive_state, render_executive_state_focus,
    save_executive_state, state_markdown_path, sync_executive_state_files,
};
use crate::workspace_state::{export_capsule_memory, sync_memory_file, sync_workspace_memory};
use crate::{
    DEFAULT_WORKSPACE_DIR, ToolExecution, ToolMemoryAppendArgs, ToolMemoryExportArgs,
    ToolMemoryRememberArgs, ToolMemorySearchArgs, ToolMemorySyncArgs, ToolReflectArgs,
    ToolSessionContextArgs, ToolSkillSearchArgs, ToolSkillStoreArgs, ToolStateCaptureArgs,
    ToolStateCloseArgs, ToolStateFocusArgs, ToolStateListArgs, ToolSubagentBatchArgs,
    ToolSubagentInvokeArgs, ToolToolSearchArgs, ToolTriggerAddArgs, ToolTriggerRemoveArgs,
    TriggerEntry, blake3_hash, collect_agent_logs, load_capsule_config, load_triggers,
    save_triggers, tool_definitions_json, tool_score, with_read_mem, with_write_mem,
};

fn resolve_workspace_path(
    explicit: Option<String>,
    workspace_override: &Option<PathBuf>,
) -> PathBuf {
    explicit
        .map(PathBuf::from)
        .or_else(|| workspace_override.clone())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR))
}

fn format_state_line(item: &ExecutiveStateItem) -> String {
    let mut line = format!(
        "{} [{}][{}] {}",
        item.id, item.status, item.kind, item.title
    );
    if let Some(next_action) = item.next_action.as_deref() {
        if !next_action.trim().is_empty() {
            line.push_str(&format!(" | next: {}", next_action.trim()));
        }
    }
    if let Some(due) = item.due.as_deref() {
        if !due.trim().is_empty() {
            line.push_str(&format!(" | due: {}", due.trim()));
        }
    }
    line
}

pub fn handle_memory_sync(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolMemorySyncArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let workspace = resolve_workspace_path(parsed.workspace, workspace_override);
    let include_daily = parsed.include_daily.unwrap_or(true);
    let ids = sync_workspace_memory(mv2, &workspace, include_daily).map_err(|e| e.to_string())?;
    *mem_read = None;
    Ok(ToolExecution {
        output: format!("Synced {} memory files.", ids.len()),
        details: serde_json::json!({ "frame_ids": ids }),
        is_error: false,
    })
}

pub fn handle_memory_export(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
    mv2: &Path,
) -> Result<ToolExecution, String> {
    let parsed: ToolMemoryExportArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let workspace = resolve_workspace_path(parsed.workspace, workspace_override);
    let include_daily = parsed.include_daily.unwrap_or(true);
    let paths = export_capsule_memory(mv2, &workspace, include_daily).map_err(|e| e.to_string())?;
    Ok(ToolExecution {
        output: format!("Exported {} files.", paths.len()),
        details: serde_json::json!({ "paths": paths }),
        is_error: false,
    })
}

pub fn handle_memory_search(
    args: serde_json::Value,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
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

pub fn handle_state_list(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
) -> Result<ToolExecution, String> {
    let parsed: ToolStateListArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let workspace = resolve_workspace_path(None, workspace_override);
    let include_closed = parsed.include_closed.unwrap_or(false);
    let limit = parsed.limit.unwrap_or(20);
    let items = load_executive_state(&workspace).list_items(
        parsed.kind.as_deref(),
        parsed.status.as_deref(),
        include_closed,
        limit,
    );
    let output = if items.is_empty() {
        "No state items.".to_string()
    } else {
        items
            .iter()
            .map(format_state_line)
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(ToolExecution {
        output,
        details: serde_json::json!({ "items": items }),
        is_error: false,
    })
}

pub fn handle_state_focus(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
) -> Result<ToolExecution, String> {
    let parsed: ToolStateFocusArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let workspace = resolve_workspace_path(None, workspace_override);
    let state = load_executive_state(&workspace);
    let output = render_executive_state_focus(
        &state,
        parsed.limit.unwrap_or(8),
        parsed.include_notes.unwrap_or(true),
    );
    Ok(ToolExecution {
        output: if output.trim().is_empty() {
            "No executive state tracked.".to_string()
        } else {
            output
        },
        details: serde_json::json!({ "state": state }),
        is_error: false,
    })
}

pub fn handle_state_capture(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolStateCaptureArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let workspace = resolve_workspace_path(None, workspace_override);
    let mut state = load_executive_state(&workspace);
    let saved_item = state.capture_item(StateCaptureInput {
        id: parsed.id.as_deref(),
        title: parsed.title.as_deref(),
        kind: parsed.kind.as_deref(),
        status: parsed.status.as_deref(),
        next_action: parsed.next_action.as_deref(),
        due: parsed.due.as_deref(),
        waiting_on: parsed.waiting_on.as_deref(),
        note: parsed.note.as_deref(),
        source: parsed.source.as_deref(),
        session: parsed.session.as_deref(),
    })?;
    let item_id = saved_item.id.clone();
    save_executive_state(&workspace, &state).map_err(|e| format!("state save: {e}"))?;
    let frame_ids = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
        sync_executive_state_files(mem, &workspace).map_err(|e| e.to_string())
    })?;
    *mem_read = None;
    Ok(ToolExecution {
        output: format!("Tracked {}.", item_id),
        details: serde_json::json!({
            "item": saved_item,
            "frame_id": frame_ids.last().copied(),
            "frame_ids": frame_ids,
            "path": state_markdown_path(&workspace).display().to_string()
        }),
        is_error: false,
    })
}

pub fn handle_state_close(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolStateCloseArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let workspace = resolve_workspace_path(None, workspace_override);
    let mut state = load_executive_state(&workspace);
    let closed_item = state.close_item(&parsed.id, parsed.resolution.as_deref())?;
    save_executive_state(&workspace, &state).map_err(|e| format!("state save: {e}"))?;
    let frame_ids = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
        sync_executive_state_files(mem, &workspace).map_err(|e| e.to_string())
    })?;
    *mem_read = None;
    Ok(ToolExecution {
        output: format!("Closed {}.", parsed.id),
        details: serde_json::json!({
            "item": closed_item,
            "frame_id": frame_ids.last().copied(),
            "frame_ids": frame_ids,
            "path": state_markdown_path(&workspace).display().to_string()
        }),
        is_error: false,
    })
}

pub fn handle_memory_append_daily(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolMemoryAppendArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let workspace = resolve_workspace_path(None, workspace_override);
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
    drop(file);
    let uri = format!("aethervault://memory/daily/{date}.md");
    let title = format!("memory daily {date}");
    let result = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
        sync_memory_file(mem, &path, uri.clone(), &title, "aethervault.memory")
            .map_err(|e| e.to_string())
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

pub fn handle_memory_remember(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolMemoryRememberArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let workspace = resolve_workspace_path(None, workspace_override);
    fs::create_dir_all(&workspace).map_err(|e| format!("workspace: {e}"))?;
    let path = workspace.join("MEMORY.md");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("memory open: {e}"))?;
    writeln!(file, "{}", parsed.text).map_err(|e| format!("memory write: {e}"))?;
    drop(file);
    let uri = "aethervault://memory/longterm.md".to_string();
    let result = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
        sync_memory_file(
            mem,
            &path,
            uri.clone(),
            "memory longterm",
            "aethervault.memory",
        )
        .map_err(|e| e.to_string())
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

pub fn handle_trigger_add(
    args: serde_json::Value,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolTriggerAddArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    with_write_mem(mem_read, mem_write, mv2, true, |mem| {
        let mut triggers = load_triggers(mem);
        let id = format!("trg_{}_{}", Utc::now().timestamp(), triggers.len() + 1);
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

pub fn handle_trigger_list(
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    with_write_mem(mem_read, mem_write, mv2, true, |mem| {
        let triggers = load_triggers(mem);
        Ok(ToolExecution {
            output: format!("{} triggers.", triggers.len()),
            details: serde_json::json!({ "triggers": triggers }),
            is_error: false,
        })
    })
}

pub fn handle_trigger_remove(
    args: serde_json::Value,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
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

pub fn handle_tool_search(args: serde_json::Value) -> Result<ToolExecution, String> {
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

pub fn handle_session_context(
    args: serde_json::Value,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolSessionContextArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let limit = parsed.limit.unwrap_or(20);
    with_read_mem(mem_read, mem_write, mv2, |mem| {
        let results = collect_agent_logs(mem, Some(&parsed.session), None, limit)
            .map_err(|e| e.to_string())?;
        Ok(ToolExecution {
            output: format!("Loaded {} entries.", results.len()),
            details: serde_json::json!({ "entries": results }),
            is_error: false,
        })
    })
}

pub fn handle_reflect(
    args: serde_json::Value,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolReflectArgs = serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
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

pub fn handle_skill_store(
    args: serde_json::Value,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolSkillStoreArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let ts = Utc::now().timestamp();
    let payload = serde_json::json!({
        "name": parsed.name,
        "trigger": parsed.trigger,
        "steps": parsed.steps,
        "tools": parsed.tools,
        "notes": parsed.notes,
        "ts_utc": ts
    });
    let bytes = serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?;
    let hash = blake3_hash(&bytes);
    let slug = payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("skill")
        .to_ascii_lowercase()
        .replace(' ', "-");
    let uri = format!("aethervault://skills/{}/{}-{}", slug, ts, hash.to_hex());
    with_write_mem(mem_read, mem_write, mv2, true, |mem| {
        let mut options = PutOptions::default();
        options.uri = Some(uri.clone());
        options.title = Some("skill".to_string());
        options.kind = Some("application/json".to_string());
        options.track = Some("aethervault.skill".to_string());
        options.search_text = Some(payload.to_string());
        mem.put_bytes_with_options(&bytes, options)
            .map_err(|e| e.to_string())?;
        mem.commit().map_err(|e| e.to_string())?;
        Ok(ToolExecution {
            output: "Skill stored.".to_string(),
            details: serde_json::json!({ "uri": uri }),
            is_error: false,
        })
    })
}

pub fn handle_skill_search(
    args: serde_json::Value,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolSkillSearchArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    with_read_mem(mem_read, mem_write, mv2, |mem| {
        let request = SearchRequest {
            query: parsed.query.clone(),
            top_k: parsed.limit.unwrap_or(10),
            snippet_chars: 200,
            uri: None,
            scope: Some("aethervault://skills/".to_string()),
            cursor: None,
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: true,
        };
        let response = mem.search(request).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for hit in response.hits {
            out.push(serde_json::json!({
                "uri": hit.uri,
                "title": hit.title,
                "text": hit.text,
                "score": hit.score
            }));
        }
        Ok(ToolExecution {
            output: format!("Found {} skills.", out.len()),
            details: serde_json::json!({ "results": out }),
            is_error: false,
        })
    })
}

pub fn handle_subagent_list(
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    with_read_mem(mem_read, mem_write, mv2, |mem| {
        let config = load_capsule_config(mem).unwrap_or_default();
        let subagents = load_subagents_from_config(&config);
        Ok(ToolExecution {
            output: format!("{} subagents.", subagents.len()),
            details: serde_json::json!({ "subagents": subagents }),
            is_error: false,
        })
    })
}

pub fn handle_subagent_invoke(
    args: serde_json::Value,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolSubagentInvokeArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let config = with_read_mem(mem_read, mem_write, mv2, |mem| {
        Ok(load_capsule_config(mem).unwrap_or_default())
    })?;
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
    let cfg = build_subagent_agent_config(mv2, &config, spec, &parsed, system, model_hook)?;
    *mem_read = None;
    *mem_write = None;
    let session = format!("subagent:{}:{}", parsed.name, Utc::now().timestamp());
    let result = run_agent_for_bridge(&cfg, &parsed.prompt, session, None, None)
        .map_err(|e| e.to_string())?;
    Ok(ToolExecution {
        output: result.final_text.unwrap_or_default(),
        details: serde_json::json!({ "session": result.session, "messages": result.messages.len() }),
        is_error: false,
    })
}

pub fn handle_subagent_batch(
    args: serde_json::Value,
    mv2: &Path,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let parsed: ToolSubagentBatchArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    if parsed.invocations.is_empty() {
        return Err("subagent_batch requires at least one invocation".into());
    }
    let config_snapshot = with_read_mem(mem_read, mem_write, mv2, |mem| {
        Ok(load_capsule_config(mem).unwrap_or_default())
    })?;
    let subagents = load_subagents_from_config(&config_snapshot);
    let ts = Utc::now().timestamp();

    *mem_read = None;
    *mem_write = None;

    let mut handles: Vec<(
        String,
        std::thread::JoinHandle<Result<AgentRunOutput, String>>,
    )> = Vec::new();
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
            handles.push((
                inv.name.clone(),
                thread::spawn(move || Err(format!("unknown subagent: {}", inv.name))),
            ));
            continue;
        }
        let cfg =
            build_subagent_agent_config(mv2, &config_snapshot, spec, &inv, system, model_hook)?;
        let session = format!("subagent:{}:{}:{}", inv.name, ts, i);
        let prompt = inv.prompt.clone();
        let name = inv.name.clone();
        handles.push((
            name,
            thread::spawn(move || run_agent_for_bridge(&cfg, &prompt, session, None, None)),
        ));
    }

    let mut results = Vec::new();
    let mut all_ok = true;
    for (name, handle) in handles {
        match handle.join() {
            Ok(Ok(output)) => {
                results.push(serde_json::json!({
                    "name": name,
                    "status": "ok",
                    "output": output.final_text.unwrap_or_default(),
                    "session": output.session,
                    "messages": output.messages.len(),
                }));
            }
            Ok(Err(err)) => {
                all_ok = false;
                results.push(serde_json::json!({
                    "name": name,
                    "status": "error",
                    "error": err,
                }));
            }
            Err(_) => {
                all_ok = false;
                results.push(serde_json::json!({
                    "name": name,
                    "status": "error",
                    "error": "subagent thread panicked",
                }));
            }
        }
    }
    let summary = if all_ok {
        format!("{} subagents completed successfully.", results.len())
    } else {
        let ok_count = results.iter().filter(|r| r["status"] == "ok").count();
        let err_count = results.len() - ok_count;
        format!("{} subagents completed, {} failed.", ok_count, err_count)
    };
    Ok(ToolExecution {
        output: summary,
        details: serde_json::json!({ "results": results }),
        is_error: !all_ok,
    })
}

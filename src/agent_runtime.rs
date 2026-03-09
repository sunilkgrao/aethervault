use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_core::Vault;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::workspace_state::load_workspace_context;
use crate::{
    AgentHookRequest, AgentLogEntry, AgentMessage, AgentToolResult, CapsuleConfig, ContextPack,
    DEFAULT_WORKSPACE_DIR, HookSpec, QueryArgs, SubagentSpec, ToolExecution,
    ToolSubagentInvokeArgs, append_agent_log_uncommitted, build_context_pack, build_kg_context,
    call_agent_hook, env_optional, env_optional_alias, execute_tool_with_handles,
    find_kg_entities,
    load_capsule_config, load_kg_graph, resolve_hook_spec, resolve_kg_path, resolve_workspace,
    tool_definitions_json,
};

const TOOL_DETAILS_MAX_CHARS: usize = 4_000;
const TOOL_OUTPUT_MAX_FOR_DETAILS: usize = 2_000;
const DEFAULT_AGENT_CONTEXT_RESULTS: usize = 8;
const DEFAULT_AGENT_CONTEXT_MAX_BYTES: usize = 12_000;
const DEFAULT_AGENT_MAX_STEPS: usize = 64;
const DEFAULT_AGENT_LOG_COMMIT_INTERVAL: usize = 8;

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &s[..boundary]
}

fn suffix_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut boundary = s.len() - max_bytes;
    while boundary < s.len() && !s.is_char_boundary(boundary) {
        boundary += 1;
    }
    &s[boundary..]
}

#[derive(Debug, Serialize)]
pub struct AgentSession {
    pub session: Option<String>,
    pub context: Option<ContextPack>,
    pub messages: Vec<AgentMessage>,
    pub tool_results: Vec<AgentToolResult>,
}

pub struct AgentRunOutput {
    pub session: Option<String>,
    pub context: Option<ContextPack>,
    pub messages: Vec<AgentMessage>,
    pub tool_results: Vec<AgentToolResult>,
    pub final_text: Option<String>,
}

#[derive(Clone)]
pub struct BridgeAgentConfig {
    pub mv2: PathBuf,
    pub model_hook: Option<String>,
    pub system: Option<String>,
    pub no_memory: bool,
    pub context_query: Option<String>,
    pub context_results: usize,
    pub context_max_bytes: usize,
    pub max_steps: usize,
    pub log: bool,
    pub log_commit_interval: usize,
    pub session_prefix: String,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionTurn {
    role: String,
    content: String,
    timestamp: i64,
}

fn resolve_session_dir(workspace: Option<&Path>) -> PathBuf {
    if let Some(value) = env_optional_alias(&["OPENCLAW_SESSION_DIR", "AETHERVAULT_SESSION_DIR"])
    {
        return PathBuf::from(value);
    }
    if let Some(ws) = workspace {
        return ws.join("sessions");
    }
    if let Some(value) = env_optional_alias(&["OPENCLAW_WORKSPACE", "AETHERVAULT_WORKSPACE"]) {
        return PathBuf::from(value).join("sessions");
    }
    PathBuf::from(DEFAULT_WORKSPACE_DIR).join("sessions")
}

fn session_file_path(session_id: &str, workspace: Option<&Path>) -> PathBuf {
    let safe_id = session_id.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    resolve_session_dir(workspace).join(format!("{safe_id}.json"))
}

fn load_session_turns(
    session_id: &str,
    workspace: Option<&Path>,
    max_turns: usize,
) -> Vec<SessionTurn> {
    let path = session_file_path(session_id, workspace);
    match fs::read_to_string(&path) {
        Ok(data) => match serde_json::from_str::<Vec<SessionTurn>>(&data) {
            Ok(mut turns) => {
                let keep = max_turns * 2;
                if turns.len() > keep {
                    turns.drain(..turns.len() - keep);
                }
                turns
            }
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    }
}

fn save_session_turns(
    session_id: &str,
    workspace: Option<&Path>,
    turns: &[SessionTurn],
    max_turns: usize,
) {
    let path = session_file_path(session_id, workspace);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let keep = max_turns * 2;
    let to_save: Vec<&SessionTurn> = if turns.len() > keep {
        turns[turns.len() - keep..].iter().collect()
    } else {
        turns.iter().collect()
    };
    if let Ok(json) = serde_json::to_string_pretty(&to_save) {
        let tmp_path = path.with_extension("json.tmp");
        if fs::write(&tmp_path, &json).is_ok() {
            let _ = fs::rename(&tmp_path, &path);
        }
    }
}

fn build_memory_query_seed(prompt_text: &str, session_turns: &[SessionTurn]) -> String {
    let mut parts = Vec::new();
    for turn in session_turns.iter().rev().take(4).rev() {
        let snippet = turn.content.trim();
        if snippet.is_empty() {
            continue;
        }
        let condensed = if snippet.len() > 240 {
            format!("{}...", truncate_utf8(snippet, 240))
        } else {
            snippet.to_string()
        };
        parts.push(format!("{}: {}", turn.role, condensed));
    }
    parts.push(format!("user: {}", prompt_text.trim()));
    let joined = parts.join("\n");
    if joined.len() > 1_200 {
        suffix_utf8(&joined, 1_200).to_string()
    } else {
        joined
    }
}

fn format_tool_message_content(name: &str, output: &str, details: &serde_json::Value) -> String {
    if output.is_empty() {
        return String::new();
    }
    if details.is_null() {
        return output.to_string();
    }
    if output.len() > TOOL_OUTPUT_MAX_FOR_DETAILS {
        return output.to_string();
    }
    if matches!(name, "context") {
        return output.to_string();
    }
    let details_str = match serde_json::to_string(details) {
        Ok(value) => value,
        Err(_) => return output.to_string(),
    };
    if details_str.len() > TOOL_DETAILS_MAX_CHARS {
        return output.to_string();
    }
    format!("{output}\n\n[details]\n{details_str}")
}

pub fn load_subagents_from_config(config: &CapsuleConfig) -> Vec<SubagentSpec> {
    config
        .agent
        .as_ref()
        .map(|a| a.subagents.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.name.trim().is_empty())
        .collect()
}

fn tool_catalog_map(catalog: &[serde_json::Value]) -> HashMap<String, serde_json::Value> {
    let mut map = HashMap::new();
    for tool in catalog {
        if let Some(name) = tool.get("name").and_then(|v| v.as_str()) {
            map.insert(name.to_string(), tool.clone());
        }
    }
    map
}

fn base_tool_names() -> HashSet<String> {
    [
        "tool_search",
        "query",
        "context",
        "search",
        "get",
        "session_context",
        "config_set",
        "memory_append_daily",
        "memory_remember",
        "memory_search",
        "memory_sync",
        "memory_export",
        "state_list",
        "state_focus",
        "state_capture",
        "state_close",
        "reflect",
        "skill_store",
        "skill_search",
        "trigger_add",
        "trigger_list",
        "trigger_remove",
        "subagent_list",
        "subagent_invoke",
        "subagent_batch",
        "approval_list",
        "scale",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}

fn tools_from_active(
    map: &HashMap<String, serde_json::Value>,
    active: &HashSet<String>,
) -> Vec<serde_json::Value> {
    let mut tools = Vec::new();
    for name in active {
        if let Some(tool) = map.get(name) {
            tools.push(tool.clone());
        }
    }
    tools.sort_by(|a, b| {
        a.get("name")
            .and_then(|v| v.as_str())
            .cmp(&b.get("name").and_then(|v| v.as_str()))
    });
    tools
}

fn default_system_prompt() -> String {
    [
        "You are AetherVault, a high-performance personal AI assistant.",
        "Be proactive, concrete, and concise. Prefer action over discussion.",
        "",
        "## Action Protocol",
        "For routine actions (reading, searching): execute immediately, summarize after.",
        "For significant actions (writing, creating): state your plan in one sentence, then execute.",
        "For complex multi-step tasks: outline 2-3 bullet points, then execute step by step.",
        "For irreversible actions (deleting, sending, deploying): describe consequences, wait for confirmation.",
        "Track commitments, deadlines, blockers, and next actions explicitly.",
        "When useful, turn open loops into concrete follow-ups, reminders, or memory entries.",
        "",
        "## Tools",
        "Tools load dynamically — call tool_search when you need a capability not currently available.",
        "When multiple independent tool calls are needed, request them all at once for parallel execution.",
        "Sensitive actions require approval. If a tool returns `approval required: <id>`, ask the user to approve or reject.",
        "Use state_focus, state_list, state_capture, and state_close to maintain executive state: priorities, open loops, waiting-fors, and follow-ups.",
        "Subagents are an elastic worker pool, not a fixed trio. Decide yourself when to spawn none, one, or many specialist workers via subagent_invoke or subagent_batch, then synthesize the result yourself.",
        "",
        "## Error Recovery",
        "When a tool fails, use reflect to record what went wrong, then retry differently.",
        "Never retry the same failing call. If stuck after 2 attempts, ask the user for guidance.",
        "",
        "## Critical Reminders",
        "Investigate before answering — search memory before making claims.",
        "Match the user's energy. Be concise when they're concise, detailed when they need detail.",
        "For irreversible actions, always confirm first.",
    ]
    .join("\n")
}

fn estimate_tokens(messages: &[AgentMessage]) -> usize {
    messages
        .iter()
        .map(|m| m.content.as_ref().map(|c| c.len()).unwrap_or(0) / 4)
        .sum()
}

fn compact_messages(
    messages: &mut Vec<AgentMessage>,
    hook: &HookSpec,
    keep_recent: usize,
) -> Result<(), String> {
    if messages.len() <= keep_recent + 2 {
        return Ok(());
    }
    let system_msg = messages[0].clone();
    let to_summarize: Vec<_> = messages[1..messages.len() - keep_recent].to_vec();
    let recent: Vec<_> = messages[messages.len() - keep_recent..].to_vec();

    let summary_text: String = to_summarize
        .iter()
        .filter_map(|m| {
            let role = &m.role;
            m.content.as_ref().map(|c| {
                let preview = truncate_utf8(c, 300);
                format!("[{role}] {preview}")
            })
        })
        .collect::<Vec<_>>()
        .join("\n");

    let summary_prompt = format!(
        "Summarize this conversation concisely. Preserve: key decisions, file paths, unresolved issues, user preferences. Discard: verbose tool outputs, redundant context.\n\n{summary_text}"
    );

    let summary_request = AgentHookRequest {
        messages: vec![
            AgentMessage {
                role: "system".to_string(),
                content: Some("You are a conversation summarizer. Output only the summary, nothing else. Be concise — use bullet points.".to_string()),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
            },
            AgentMessage {
                role: "user".to_string(),
                content: Some(summary_prompt),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
            },
        ],
        tools: Vec::new(),
        session: None,
    };

    let summary_response = call_agent_hook(hook, &summary_request)?;
    let summary = summary_response
        .content
        .unwrap_or_else(|| "(compaction failed)".to_string());

    *messages = Vec::new();
    messages.push(system_msg);
    messages.push(AgentMessage {
        role: "user".to_string(),
        content: Some(format!(
            "[Context compacted. Summary of prior conversation:]\n{summary}"
        )),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });
    messages.push(AgentMessage {
        role: "assistant".to_string(),
        content: Some(
            "Understood. I have the context from the summary above. Continuing.".to_string(),
        ),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });
    messages.extend(recent);
    Ok(())
}

pub fn run_agent(
    mv2: PathBuf,
    prompt: Option<String>,
    file: Option<PathBuf>,
    session: Option<String>,
    model_hook: Option<String>,
    system: Option<String>,
    system_file: Option<PathBuf>,
    no_memory: bool,
    context_query: Option<String>,
    context_results: usize,
    context_max_bytes: usize,
    max_steps: usize,
    log_commit_interval: usize,
    json: bool,
    log: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let prompt_text = if let Some(file) = file {
        fs::read_to_string(file)?
    } else if let Some(prompt) = prompt {
        prompt
    } else {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer
    };
    let system_text = if let Some(path) = system_file {
        Some(fs::read_to_string(path)?)
    } else {
        system
    };

    let output = run_agent_with_prompt(
        mv2,
        prompt_text,
        session,
        model_hook,
        system_text,
        no_memory,
        context_query,
        context_results,
        context_max_bytes,
        max_steps,
        log_commit_interval,
        false,
        log,
    )?;

    if json {
        let payload = AgentSession {
            session: output.session,
            context: output.context,
            messages: output.messages,
            tool_results: output.tool_results,
        };
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if let Some(text) = output.final_text {
        println!("{text}");
    }
    Ok(())
}

pub fn run_agent_with_prompt(
    mv2: PathBuf,
    prompt_text: String,
    session: Option<String>,
    model_hook: Option<String>,
    system_override: Option<String>,
    no_memory: bool,
    context_query: Option<String>,
    context_results: usize,
    context_max_bytes: usize,
    max_steps: usize,
    log_commit_interval: usize,
    read_only_tools: bool,
    log: bool,
) -> Result<AgentRunOutput, Box<dyn std::error::Error>> {
    if prompt_text.trim().is_empty() {
        return Err("agent prompt is empty".into());
    }

    let mut mem_read = Some(Vault::open_read_only(&mv2)?);
    let config = load_capsule_config(mem_read.as_mut().unwrap()).unwrap_or_default();
    let agent_cfg = config.agent.clone().unwrap_or_default();
    let workspace = resolve_workspace(None, &agent_cfg);
    let hook_cfg = config.hooks.clone().unwrap_or_default();
    let model_spec = resolve_hook_spec(
        model_hook,
        300000,
        agent_cfg.model_hook.clone().or(hook_cfg.llm),
        None,
    )
    .ok_or("agent requires --model-hook or config.agent.model_hook or config.hooks.llm")?;

    let mut system_prompt = if let Some(system) = system_override {
        system
    } else if let Some(system) = agent_cfg.system.clone() {
        system
    } else {
        let system_path = workspace
            .clone()
            .map(|ws| ws.join("SYSTEM.md"))
            .filter(|p| p.exists());
        if let Some(path) = system_path {
            fs::read_to_string(&path).unwrap_or_else(|_| default_system_prompt())
        } else {
            default_system_prompt()
        }
    };

    if agent_cfg.onboarding_complete == Some(false) {
        system_prompt.push_str(
            "\n\n# Onboarding\nYou are in onboarding mode. Guide the user to connect email, calendar, and messaging integrations. Verify tool access. When complete, append a note to MEMORY.md and ask the user to run `openclaw config set --key index` to set `agent.onboarding_complete=true`.",
        );
    }

    if let Some(workspace) = workspace.as_ref() {
        if workspace.exists() {
            let workspace_context = load_workspace_context(workspace);
            if !workspace_context.trim().is_empty() {
                system_prompt.push_str("\n\n# Workspace Context\n");
                system_prompt.push_str(&workspace_context);
            }
        }
    }

    if let Some(ref global_context) = config.context {
        if !global_context.trim().is_empty() {
            system_prompt.push_str("\n\n# Global Context\n");
            system_prompt.push_str(global_context);
        }
    }

    let configured_subagents = load_subagents_from_config(&config);
    if !configured_subagents.is_empty() {
        system_prompt.push_str("\n\n# Dynamic Delegation\n");
        system_prompt.push_str(
            "Subagents are an elastic worker pool. Decide yourself when to use none, one, or many. \
             Use `subagent_list` to inspect reusable templates. \
             Use `subagent_batch` to parallelize independent lines of work. \
             You may create ad-hoc subagents on the fly by supplying a `system` prompt; names do not need to be preconfigured.\n",
        );
        system_prompt.push_str("Configured templates:\n");
        for spec in &configured_subagents {
            system_prompt.push_str("- ");
            system_prompt.push_str(&spec.name);
            if let Some(system) = spec.system.as_deref() {
                let summary = system.lines().next().unwrap_or_default().trim();
                if !summary.is_empty() {
                    system_prompt.push_str(": ");
                    system_prompt.push_str(summary);
                }
            }
            system_prompt.push('\n');
        }
    }

    let session_turns = session
        .as_ref()
        .map(|sess_id| load_session_turns(sess_id, workspace.as_deref(), 8))
        .unwrap_or_default();

    let mut context_pack = None;
    let effective_max_steps = agent_cfg.max_steps.unwrap_or(max_steps);
    let effective_log_commit_interval = agent_cfg
        .log_commit_interval
        .unwrap_or(log_commit_interval)
        .max(1);
    if !no_memory {
        let query = context_query
            .or(agent_cfg.context_query)
            .unwrap_or_else(|| build_memory_query_seed(&prompt_text, &session_turns));
        let qargs = QueryArgs {
            raw_query: query,
            collection: Some("memory".to_string()),
            limit: agent_cfg.max_context_results.unwrap_or(context_results),
            snippet_chars: 300,
            no_expand: false,
            max_expansions: 2,
            expand_hook: None,
            expand_hook_timeout_ms: 2000,
            no_vector: false,
            rerank: "local".to_string(),
            rerank_hook: None,
            rerank_hook_timeout_ms: 6000,
            rerank_hook_full_text: false,
            embed_model: None,
            embed_cache: 4096,
            embed_no_cache: false,
            rerank_docs: 40,
            rerank_chunk_chars: 1200,
            rerank_chunk_overlap: 200,
            plan: false,
            asof: None,
            before: None,
            after: None,
            feedback_weight: 0.15,
            fusion_mode: crate::FusionMode::Rrf,
            bayesian_bm25_weight: 0.5,
            bayesian_vec_weight: 0.5,
        };
        if let Ok(pack) = build_context_pack(
            mem_read.as_mut().unwrap(),
            qargs,
            agent_cfg.max_context_bytes.unwrap_or(context_max_bytes),
            false,
        ) {
            if !pack.context.trim().is_empty() {
                system_prompt.push_str("\n\n# Memory Context\n");
                system_prompt.push_str(&pack.context);
                context_pack = Some(pack);
            }
        }
    }

    mem_read = None;

    let kg_path = resolve_kg_path();
    if kg_path.exists() {
        if let Some(kg) = load_kg_graph(&kg_path) {
            let matched = find_kg_entities(&prompt_text, &kg);
            if !matched.is_empty() {
                let kg_context = build_kg_context(&matched, &kg);
                if !kg_context.trim().is_empty() {
                    system_prompt.push_str("\n\n# Knowledge Graph Context\n");
                    system_prompt
                        .push_str("(Automatically matched entities from the knowledge graph)\n\n");
                    system_prompt.push_str(&kg_context);
                }
            }
        }
    }

    let mut messages = Vec::new();
    messages.push(AgentMessage {
        role: "system".to_string(),
        content: Some(system_prompt),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });

    if session.is_some() {
        for turn in &session_turns {
            messages.push(AgentMessage {
                role: turn.role.clone(),
                content: Some(if turn.content.len() > 500 {
                    format!("{}...", truncate_utf8(&turn.content, 500))
                } else {
                    turn.content.clone()
                }),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
            });
        }
    }

    messages.push(AgentMessage {
        role: "user".to_string(),
        content: Some(prompt_text.clone()),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });

    let tool_catalog = tool_definitions_json();
    let tool_map = tool_catalog_map(&tool_catalog);
    let mut active_tools = base_tool_names();
    let mut tools = tools_from_active(&tool_map, &active_tools);
    let mut tool_results: Vec<AgentToolResult> = Vec::new();
    let should_log = log || agent_cfg.log.unwrap_or(false);
    let mut final_text = None;

    let mut log_buffer: Vec<AgentLogEntry> = Vec::new();
    let mut mem_write: Option<Vault> = None;

    let flush_log_buffer =
        |mv2: &Path, buffer: &mut Vec<AgentLogEntry>, mem_read: &mut Option<Vault>| {
            if buffer.is_empty() {
                return Ok(()) as Result<(), Box<dyn std::error::Error>>;
            }
            *mem_read = None;
            let mut mem = Vault::open(mv2)?;
            for entry in buffer.drain(..) {
                let _ = append_agent_log_uncommitted(&mut mem, &entry);
            }
            mem.commit()?;
            Ok(())
        };

    if should_log {
        let entry = AgentLogEntry {
            session: session.clone(),
            role: "user".to_string(),
            text: prompt_text.clone(),
            meta: None,
            ts_utc: Some(Utc::now().timestamp()),
        };
        log_buffer.push(entry);
        if log_buffer.len() >= effective_log_commit_interval {
            flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
        }
    }

    let mut completed = false;
    for _ in 0..effective_max_steps {
        let token_estimate = estimate_tokens(&messages);
        if token_estimate > 100_000 {
            eprintln!("[harness] context at ~{token_estimate} tokens, compacting...");
            if let Err(e) = compact_messages(&mut messages, &model_spec, 6) {
                eprintln!("[harness] compaction failed: {e}");
            }
        }

        let request = AgentHookRequest {
            messages: messages.clone(),
            tools: tools.clone(),
            session: session.clone(),
        };
        let message = call_agent_hook(&model_spec, &request)?;
        if let Some(content) = message.content.clone() {
            final_text = Some(content.clone());
            if should_log {
                let entry = AgentLogEntry {
                    session: session.clone(),
                    role: "assistant".to_string(),
                    text: content,
                    meta: None,
                    ts_utc: Some(Utc::now().timestamp()),
                };
                log_buffer.push(entry);
                if log_buffer.len() >= effective_log_commit_interval {
                    flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
                }
            }
        }
        let tool_calls = message.tool_calls.clone();
        messages.push(message);
        if tool_calls.is_empty() {
            completed = true;
            break;
        }

        for call in &tool_calls {
            if call.id.trim().is_empty() {
                return Err("tool call is missing an id".into());
            }
            if call.name.trim().is_empty() {
                return Err("tool call is missing a name".into());
            }
        }

        let max_tool_output = 8000;

        if tool_calls.len() == 1 {
            let call = &tool_calls[0];
            let result = match execute_tool_with_handles(
                &call.name,
                call.args.clone(),
                &mv2,
                read_only_tools,
                &workspace,
                &mut mem_read,
                &mut mem_write,
            ) {
                Ok(result) => result,
                Err(err) => ToolExecution {
                    output: format!("Tool error: {err}"),
                    details: serde_json::json!({ "error": err }),
                    is_error: true,
                },
            };

            let result = if result.output.len() > max_tool_output && !result.is_error {
                ToolExecution {
                    output: format!(
                        "{}\n\n[Output truncated: {} chars total, showing first {}. Use a more specific query for full results.]",
                        truncate_utf8(&result.output, max_tool_output),
                        result.output.len(),
                        max_tool_output
                    ),
                    details: result.details,
                    is_error: result.is_error,
                }
            } else {
                result
            };

            let tool_content =
                format_tool_message_content(&call.name, &result.output, &result.details);
            tool_results.push(AgentToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                output: result.output.clone(),
                details: result.details.clone(),
                is_error: result.is_error,
            });
            messages.push(AgentMessage {
                role: "tool".to_string(),
                content: if tool_content.is_empty() {
                    None
                } else {
                    Some(tool_content)
                },
                tool_calls: Vec::new(),
                name: Some(call.name.clone()),
                tool_call_id: Some(call.id.clone()),
                is_error: Some(result.is_error),
            });

            if call.name == "tool_search" && !result.is_error {
                if let Some(results_arr) = result.details.get("results").and_then(|v| v.as_array())
                {
                    let mut changed = false;
                    for item in results_arr {
                        if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                            if active_tools.insert(name.to_string()) {
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        tools = tools_from_active(&tool_map, &active_tools);
                    }
                }
            }

            if should_log {
                log_buffer.push(AgentLogEntry {
                    session: session.clone(),
                    role: "tool".to_string(),
                    text: result.output,
                    meta: Some(result.details),
                    ts_utc: Some(Utc::now().timestamp()),
                });
                if log_buffer.len() >= effective_log_commit_interval {
                    flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
                }
            }

            if matches!(call.name.as_str(), "put" | "log" | "feedback") && !result.is_error {
                flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
            }
        } else {
            let results: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = tool_calls
                    .iter()
                    .map(|call| {
                        let mv2 = &mv2;
                        let workspace_override = workspace.clone();
                        (
                            call,
                            s.spawn(move || {
                                let mut local_mem_read: Option<Vault> = None;
                                let mut local_mem_write: Option<Vault> = None;
                                match execute_tool_with_handles(
                                    &call.name,
                                    call.args.clone(),
                                    mv2,
                                    read_only_tools,
                                    &workspace_override,
                                    &mut local_mem_read,
                                    &mut local_mem_write,
                                ) {
                                    Ok(r) => r,
                                    Err(err) => ToolExecution {
                                        output: format!("Tool error: {err}"),
                                        details: serde_json::json!({ "error": err }),
                                        is_error: true,
                                    },
                                }
                            }),
                        )
                    })
                    .collect();
                let mut results = Vec::with_capacity(handles.len());
                for (call, handle) in handles {
                    match handle.join() {
                        Ok(result) => results.push((call, result)),
                        Err(panic_info) => {
                            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                                s.to_string()
                            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "unknown panic".to_string()
                            };
                            eprintln!("[harness] parallel tool '{}' panicked: {msg}", call.name);
                            results.push((
                                call,
                                ToolExecution {
                                    output: format!("Tool panicked: {msg}"),
                                    details: serde_json::json!({
                                        "error": "panic",
                                        "message": msg,
                                    }),
                                    is_error: true,
                                },
                            ));
                        }
                    }
                }
                results
            });

            for (call, result) in results {
                let result = if result.output.len() > max_tool_output && !result.is_error {
                    ToolExecution {
                        output: format!(
                            "{}\n\n[Output truncated: {} chars total, showing first {}.]",
                            truncate_utf8(&result.output, max_tool_output),
                            result.output.len(),
                            max_tool_output
                        ),
                        details: result.details,
                        is_error: result.is_error,
                    }
                } else {
                    result
                };

                let tool_content =
                    format_tool_message_content(&call.name, &result.output, &result.details);
                tool_results.push(AgentToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    output: result.output.clone(),
                    details: result.details.clone(),
                    is_error: result.is_error,
                });
                messages.push(AgentMessage {
                    role: "tool".to_string(),
                    content: if tool_content.is_empty() {
                        None
                    } else {
                        Some(tool_content)
                    },
                    tool_calls: Vec::new(),
                    name: Some(call.name.clone()),
                    tool_call_id: Some(call.id.clone()),
                    is_error: Some(result.is_error),
                });

                if call.name == "tool_search" && !result.is_error {
                    if let Some(results_arr) =
                        result.details.get("results").and_then(|v| v.as_array())
                    {
                        let mut changed = false;
                        for item in results_arr {
                            if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                                if active_tools.insert(name.to_string()) {
                                    changed = true;
                                }
                            }
                        }
                        if changed {
                            tools = tools_from_active(&tool_map, &active_tools);
                        }
                    }
                }

                if should_log {
                    log_buffer.push(AgentLogEntry {
                        session: session.clone(),
                        role: "tool".to_string(),
                        text: result.output,
                        meta: Some(result.details),
                        ts_utc: Some(Utc::now().timestamp()),
                    });
                    if log_buffer.len() >= effective_log_commit_interval {
                        flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
                    }
                }

                if matches!(call.name.as_str(), "put" | "log" | "feedback") && !result.is_error {
                    flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
                }
            }
        }

        let step_num = messages.iter().filter(|m| m.role == "assistant").count();
        let token_est = estimate_tokens(&messages);
        let mut reminders = Vec::new();

        if token_est > 80_000 {
            reminders.push("Context is large. Be concise in your responses and tool calls.");
        }
        if step_num > effective_max_steps * 3 / 4 {
            reminders
                .push("You are approaching the step limit. Focus on completing the current task.");
        }
        if messages
            .last()
            .map(|m| m.is_error == Some(true))
            .unwrap_or(false)
        {
            reminders.push("The previous tool call failed. Use reflect to analyze what went wrong, then try a different approach. Do not retry the same call.");
        }

        if !reminders.is_empty() {
            messages.push(AgentMessage {
                role: "user".to_string(),
                content: Some(format!("[System Reminder] {}", reminders.join(" "))),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
            });
        }
    }

    if should_log {
        flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
    }

    if !completed {
        let last_action = messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .and_then(|m| m.content.as_ref())
            .map(|c| c.chars().take(200).collect::<String>())
            .unwrap_or_else(|| "(no context available)".to_string());
        return Err(format!(
            "Agent used all {effective_max_steps} steps without finishing. \
            Last action: {last_action}"
        )
        .into());
    }

    if let Some(ref sess_id) = session {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut turns = load_session_turns(sess_id, workspace.as_deref(), 8);
        turns.push(SessionTurn {
            role: "user".to_string(),
            content: prompt_text,
            timestamp: now,
        });
        if let Some(ref reply) = final_text {
            turns.push(SessionTurn {
                role: "assistant".to_string(),
                content: reply.clone(),
                timestamp: now,
            });
        }
        save_session_turns(sess_id, workspace.as_deref(), &turns, 8);
    }

    Ok(AgentRunOutput {
        session,
        context: context_pack,
        messages,
        tool_results,
        final_text,
    })
}

fn resolve_bridge_model_hook(cli: Option<String>) -> Option<String> {
    if cli.is_some() {
        return cli;
    }
    if env_optional("ANTHROPIC_API_KEY").is_some() && env_optional("ANTHROPIC_MODEL").is_some() {
        return Some("builtin:claude".to_string());
    }
    None
}

fn resolve_bridge_timeout() -> Result<Option<Duration>, Box<dyn std::error::Error>> {
    let Some(value) =
        env_optional_alias(&["OPENCLAW_BRIDGE_TIMEOUT_SECS", "AETHERVAULT_BRIDGE_TIMEOUT_SECS"])
    else {
        return Ok(Some(Duration::from_secs(900)));
    };
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "0" | "off" | "none" | "disable" | "disabled"
    ) {
        return Ok(None);
    }
    let secs = normalized.parse::<u64>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid OPENCLAW_BRIDGE_TIMEOUT_SECS / AETHERVAULT_BRIDGE_TIMEOUT_SECS",
        )
    })?;
    Ok(Some(Duration::from_secs(secs.max(1))))
}

pub fn build_bridge_agent_config(
    mv2: PathBuf,
    model_hook: Option<String>,
    system: Option<String>,
    no_memory: bool,
    context_query: Option<String>,
    context_results: usize,
    context_max_bytes: usize,
    max_steps: usize,
    log: bool,
    log_commit_interval: usize,
) -> Result<BridgeAgentConfig, Box<dyn std::error::Error>> {
    let timeout = resolve_bridge_timeout()?;
    Ok(BridgeAgentConfig {
        mv2,
        model_hook: resolve_bridge_model_hook(model_hook),
        system,
        no_memory,
        context_query,
        context_results,
        context_max_bytes,
        max_steps,
        log,
        log_commit_interval: log_commit_interval.max(1),
        session_prefix: String::new(),
        timeout,
    })
}

pub fn build_subagent_agent_config(
    mv2: &Path,
    config: &CapsuleConfig,
    spec: Option<&SubagentSpec>,
    invocation: &ToolSubagentInvokeArgs,
    system: Option<String>,
    model_hook: Option<String>,
) -> Result<BridgeAgentConfig, String> {
    let agent_cfg = config.agent.clone().unwrap_or_default();
    let context_query = invocation
        .context_query
        .clone()
        .or_else(|| spec.and_then(|item| item.context_query.clone()))
        .or_else(|| agent_cfg.context_query.clone());
    let context_results = invocation
        .max_context_results
        .or_else(|| spec.and_then(|item| item.max_context_results))
        .or(agent_cfg.max_context_results)
        .unwrap_or(DEFAULT_AGENT_CONTEXT_RESULTS);
    let context_max_bytes = invocation
        .max_context_bytes
        .or_else(|| spec.and_then(|item| item.max_context_bytes))
        .or(agent_cfg.max_context_bytes)
        .unwrap_or(DEFAULT_AGENT_CONTEXT_MAX_BYTES);
    let max_steps = invocation
        .max_steps
        .or_else(|| spec.and_then(|item| item.max_steps))
        .or(agent_cfg.max_steps)
        .unwrap_or(DEFAULT_AGENT_MAX_STEPS);
    let log = invocation
        .log
        .or_else(|| spec.and_then(|item| item.log))
        .or(agent_cfg.log)
        .unwrap_or(true);
    let log_commit_interval = invocation
        .log_commit_interval
        .or_else(|| spec.and_then(|item| item.log_commit_interval))
        .or(agent_cfg.log_commit_interval)
        .unwrap_or(DEFAULT_AGENT_LOG_COMMIT_INTERVAL);
    let no_memory = invocation
        .no_memory
        .or_else(|| spec.and_then(|item| item.no_memory))
        .unwrap_or(false);

    build_bridge_agent_config(
        mv2.to_path_buf(),
        model_hook,
        system,
        no_memory,
        context_query,
        context_results,
        context_max_bytes,
        max_steps,
        log,
        log_commit_interval,
    )
    .map_err(|e| e.to_string())
}

pub fn run_agent_for_bridge(
    config: &BridgeAgentConfig,
    prompt: &str,
    session: String,
    system_override: Option<String>,
    model_hook_override: Option<String>,
) -> Result<AgentRunOutput, String> {
    let (tx, rx) = mpsc::channel();
    let prompt_text = prompt.to_string();
    let mv2 = config.mv2.clone();
    let model_hook = model_hook_override.or_else(|| config.model_hook.clone());
    let system_text = system_override.or_else(|| config.system.clone());
    let no_memory = config.no_memory;
    let context_query = config.context_query.clone();
    let context_results = config.context_results;
    let context_max_bytes = config.context_max_bytes;
    let max_steps = config.max_steps;
    let log_commit_interval = config.log_commit_interval;
    let log = config.log;

    thread::spawn(move || {
        let result = run_agent_with_prompt(
            mv2,
            prompt_text,
            Some(session),
            model_hook,
            system_text,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log_commit_interval,
            false,
            log,
        )
        .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });

    let result = match config.timeout {
        Some(timeout) => match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "Agent timed out after {}s. Set OPENCLAW_BRIDGE_TIMEOUT_SECS=0 to disable the wall-clock deadline for long-running bridge tasks.",
                    timeout.as_secs()
                ));
            }
            Err(err) => return Err(format!("Agent channel error: {err}")),
        },
        None => rx
            .recv()
            .map_err(|err| format!("Agent channel error: {err}"))?,
    };
    result.map_err(|e| e)
}

#[cfg(test)]
mod tests {
    use super::{SessionTurn, build_memory_query_seed, suffix_utf8, truncate_utf8};

    #[test]
    fn memory_query_seed_keeps_recent_thread_context() {
        let turns = vec![
            SessionTurn {
                role: "user".to_string(),
                content: "Reach out to Dana about rescheduling next week's board prep.".to_string(),
                timestamp: 1,
            },
            SessionTurn {
                role: "assistant".to_string(),
                content:
                    "I drafted a reschedule note and flagged that we still need two agenda items."
                        .to_string(),
                timestamp: 2,
            },
        ];
        let seed = build_memory_query_seed("What still needs follow-up?", &turns);
        assert!(seed.contains("Reach out to Dana"));
        assert!(seed.contains("still need two agenda items"));
        assert!(seed.contains("What still needs follow-up?"));
    }

    #[test]
    fn truncate_utf8_respects_char_boundaries() {
        assert_eq!(truncate_utf8("é🙂b", 1), "");
        assert_eq!(truncate_utf8("é🙂b", 2), "é");
        assert_eq!(truncate_utf8("é🙂b", 6), "é🙂");
    }

    #[test]
    fn suffix_utf8_respects_char_boundaries() {
        assert_eq!(suffix_utf8("aé🙂b", 5), "🙂b");
        assert_eq!(suffix_utf8("aé🙂b", 6), "🙂b");
        assert_eq!(suffix_utf8("aé🙂b", 8), "aé🙂b");
    }
}

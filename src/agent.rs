#[allow(unused_imports)]
use std::collections::{HashMap, HashSet};
use std::fs;
#[allow(unused_imports)]
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_core::{PutOptions, Vault};
#[allow(unused_imports)]
use chrono::{TimeZone, Utc};
use rayon::ThreadPoolBuilder;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde_json;

use crate::claude::{call_agent_hook, call_claude, call_critic};
use crate::{
    agent_log_path, base_tool_names, build_context_pack, build_kg_context,
    collect_mid_loop_reminders, compute_drift_score, critic_should_fire, env_optional,
    execute_tool_with_handles, find_kg_entities, flush_log_to_jsonl,
    format_tool_message_content, load_capsule_config, load_kg_graph, load_session_turns,
    load_workspace_context, requires_approval, resolve_hook_spec, resolve_workspace,
    rotate_log_if_needed, save_session_turns, tool_catalog_map, tool_definitions_json,
    tools_from_active, with_write_mem, AgentHookRequest, AgentLogEntry, AgentMessage,
    AgentProgress, AgentRunOutput, AgentSession, AgentToolCall, AgentToolResult,
    DriftState, HookSpec, McpRegistry, McpServerConfig, QueryArgs, ReminderState, SessionTurn,
    ToolExecution,
};

pub(crate) fn run_agent(
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

    let prompt_for_session = prompt_text.clone();
    let session_for_save = session.clone();
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
        log,
        None,
    )?;

    // Save session turns for CLI agent continuity (mirrors Telegram bridge behaviour)
    if let Some(ref sess_id) = session_for_save {
        let mut turns = load_session_turns(sess_id, 8);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        turns.push(SessionTurn {
            role: "user".to_string(),
            content: prompt_for_session,
            timestamp: now,
        });
        if let Some(ref reply) = output.final_text {
            turns.push(SessionTurn {
                role: "assistant".to_string(),
                content: reply.clone(),
                timestamp: now,
            });
        }
        save_session_turns(sess_id, &turns, 8);
    }

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

pub(crate) fn default_system_prompt() -> String {
    [
        "You are AetherVault, a high-performance personal AI assistant with a rich toolkit.",
        "You are NOT a limited chatbot. You have tools for memory, search, web, email, browser, file system, code execution, notifications, and more.",
        "Be proactive, concrete, and concise. Prefer action over discussion.",
        "",
        "## Action Protocol",
        "For routine actions (reading, searching): execute immediately, summarize after.",
        "For significant actions (writing, creating): state your plan in one sentence, then execute.",
        "For complex multi-step tasks: outline 2-3 bullet points, then execute step by step.",
        "For irreversible actions (deleting, sending, deploying): describe consequences, wait for confirmation.",
        "",
        "## Tools",
        "Your tools are listed in the Available Tools section below. You have a comprehensive toolkit — use it.",
        "Call tool_search to discover additional specialized tools not in the initial set.",
        "When multiple independent tool calls are needed, request them all at once for parallel execution.",
        "Sensitive actions require approval. If a tool returns `approval required: <id>`, ask the user to approve or reject.",
        "Use subagent_invoke or subagent_batch for specialist work when it improves quality or speed.",
        "",
        "## IMPORTANT: Do Not Undersell Yourself",
        "Never say 'my tools are limited', 'I don't have access to', or 'I can't do that' unless you have actually tried the tool and it failed.",
        "If you're unsure whether a tool exists, call tool_search first. Do not guess.",
        "When a tool is available, USE it rather than dumping generic knowledge from training data.",
        "Research with your tools FIRST, then synthesize. Never substitute memory/training data for actual tool use.",
        "",
        "## Communication Style",
        "Before calling tools, briefly say what you're about to do in a natural way (e.g., 'Let me check your calendar' or 'Searching for that...').",
        "These interim messages are sent to the user immediately, so they know you're working on it.",
        "Keep interim messages short and natural — one sentence, no bullet points.",
        "Do NOT narrate every single tool call. Only narrate when starting a new logical step.",
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

/// Estimate token count for messages (rough: chars / 4).
pub(crate) fn estimate_tokens(messages: &[AgentMessage]) -> usize {
    messages.iter().map(|m| {
        m.content.as_ref().map(|c| c.len()).unwrap_or(0) / 4
    }).sum()
}

pub(crate) fn compaction_budget_tokens() -> usize {
    let window: usize = env_optional("ANTHROPIC_CONTEXT_WINDOW_TOKENS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(120_000);
    let ratio: f64 = env_optional("ANTHROPIC_COMPACT_RATIO")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.82);
    ((window as f64) * ratio) as usize
}

pub(crate) fn keep_recent_turns() -> usize {
    env_optional("ANTHROPIC_COMPACT_KEEP_RECENT")
        .and_then(|v| v.parse().ok())
        .unwrap_or(6)
}

/// Compact messages when context is getting large.
/// Preserves all leading system blocks and last `keep_recent` messages verbatim.
/// Summarizes everything in between into a compaction notice.
pub(crate) fn compact_messages(
    messages: &mut Vec<AgentMessage>,
    hook: &HookSpec,
    keep_recent: usize,
) -> Result<(), String> {
    if messages.len() <= keep_recent + 2 {
        return Ok(()); // Nothing to compact
    }
    // Preserve all leading system blocks (supports cache-split: stable prefix + dynamic suffix)
    let system_end = messages.iter().take_while(|m| m.role == "system").count().max(1);
    let system_msgs: Vec<_> = messages[..system_end].to_vec();
    let to_summarize: Vec<_> = messages[system_end..messages.len() - keep_recent].to_vec();
    let recent: Vec<_> = messages[messages.len() - keep_recent..].to_vec();

    // Build a summary request
    let summary_text: String = to_summarize.iter().filter_map(|m| {
        let role = &m.role;
        m.content.as_ref().map(|c| {
            let preview: String = c.chars().take(300).collect();
            format!("[{role}] {preview}")
        })
    }).collect::<Vec<_>>().join("\n");

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
    let summary = summary_response.content.unwrap_or_else(|| "(compaction failed)".to_string());

    // Rebuild messages: system blocks + compaction notice + recent
    *messages = system_msgs;
    messages.push(AgentMessage {
        role: "user".to_string(),
        content: Some(format!("[Context compacted. Summary of prior conversation:]\n{summary}")),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });
    messages.push(AgentMessage {
        role: "assistant".to_string(),
        content: Some("Understood. I have the context from the summary above. Continuing.".to_string()),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });
    messages.extend(recent);
    Ok(())
}

pub(crate) fn run_agent_with_prompt(
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
    log: bool,
    progress: Option<Arc<Mutex<AgentProgress>>>,
) -> Result<AgentRunOutput, Box<dyn std::error::Error>> {
    if prompt_text.trim().is_empty() {
        return Err("agent prompt is empty".into());
    }

    // No external lock — Vault handles shared/exclusive internally.
    let mut mem_read = Some(Vault::open_read_only(&mv2)?);
    let config = load_capsule_config(mem_read.as_mut().unwrap()).unwrap_or_default();
    let agent_cfg = config.agent.clone().unwrap_or_default();
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
        // Load from workspace SYSTEM.md, fall back to inline default
        let system_path = resolve_workspace(None, &agent_cfg)
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
            "\n\n# Onboarding\nYou are in onboarding mode. Guide the user to connect email, calendar, and messaging integrations. Verify tool access. When complete, append a note to MEMORY.md and ask the user to run `aethervault config set --key index` to set `agent.onboarding_complete=true`.",
        );
    }

    if let Some(workspace) = resolve_workspace(None, &agent_cfg) {
        if workspace.exists() {
            let workspace_context = load_workspace_context(&workspace);
            if !workspace_context.trim().is_empty() {
                system_prompt.push_str("\n\n# Workspace Context\n");
                system_prompt.push_str(&workspace_context);
            }
        }
    }

    if let Some(global_context) = config.context {
        if !global_context.trim().is_empty() {
            system_prompt.push_str("\n\n# Global Context\n");
            system_prompt.push_str(&global_context);
        }
    }

    // --- KV-Cache Breakpoint ---
    // Everything above (system_prompt) is stable within a session.
    // Everything below (system_dynamic) churns per-turn (memory, KG).
    // Splitting them enables Anthropic prompt cache reuse on the stable prefix.
    let mut system_dynamic = String::new();

    let mut context_pack = None;
    let effective_max_steps = agent_cfg.max_steps.unwrap_or(max_steps);
    let effective_log_commit_interval = agent_cfg
        .log_commit_interval
        .unwrap_or(log_commit_interval)
        .max(1);
    if !no_memory {
        let query = context_query
            .or(agent_cfg.context_query)
            .unwrap_or_else(|| prompt_text.clone());
            let qargs = QueryArgs {
            raw_query: query,
            collection: session.as_ref().map(|s| format!("agent-log/{s}")),
            limit: agent_cfg.max_context_results.unwrap_or(context_results),
            snippet_chars: 300,
            no_expand: false,
            max_expansions: 2,
            expand_hook: None,
            expand_hook_timeout_ms: u64::MAX,
            no_vector: false,
            rerank: "local".to_string(),
            rerank_hook: None,
            rerank_hook_timeout_ms: u64::MAX,
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
            vault_path: Some(mv2.clone()),
            parallel_lanes: true,
        };
        if let Ok(pack) = build_context_pack(
            mem_read.as_mut().unwrap(),
            qargs,
            agent_cfg.max_context_bytes.unwrap_or(context_max_bytes),
            false,
        ) {
            if !pack.context.trim().is_empty() {
                system_dynamic.push_str("\n\n# Memory Context\n");
                system_dynamic.push_str(&pack.context);
                context_pack = Some(pack);
            }
        }
    }


    // Release the read handle after initialization. Tool calls re-open on demand via
    // with_read_mem/with_write_mem. This prevents a long-lived shared flock from blocking
    // sibling subagents that need exclusive access for writes.
    mem_read = None;

    // Knowledge Graph entity auto-injection
    let kg_path = std::path::PathBuf::from("/root/.aethervault/data/knowledge-graph.json");
    if kg_path.exists() {
        if let Some(kg) = load_kg_graph(&kg_path) {
            let matched = find_kg_entities(&prompt_text, &kg);
            if !matched.is_empty() {
                let kg_context = build_kg_context(&matched, &kg);
                if !kg_context.trim().is_empty() {
                    system_dynamic.push_str("\n\n# Knowledge Graph Context\n");
                    system_dynamic.push_str("(Automatically matched entities from the knowledge graph)\n\n");
                    system_dynamic.push_str(&kg_context);
                }
            }
        }
    }

    // Inject tool capability inventory so the agent knows what it can do
    {
        let all_tools = tool_definitions_json();
        let active_names = base_tool_names();
        let discoverable: Vec<String> = all_tools.iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
            .filter(|n| !active_names.contains(n))
            .collect();
        let mut cap = String::from("\n\n# Available Tools\n");
        cap.push_str("You have the following tools ready to use right now:\n");
        let mut sorted_active: Vec<String> = active_names.iter().cloned().collect();
        sorted_active.sort();
        for name in &sorted_active {
            let desc = all_tools.iter()
                .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name.as_str()))
                .and_then(|t| t.get("description").and_then(|d| d.as_str()))
                .unwrap_or("");
            let short_desc: String = desc.chars().take(80).collect();
            cap.push_str(&format!("- **{name}**: {short_desc}\n"));
        }
        if !discoverable.is_empty() {
            cap.push_str(&format!(
                "\nAdditional tools available via tool_search: {}\n",
                discoverable.join(", ")
            ));
        }
        cap.push_str("\nDo NOT say your tools are limited. You have a full toolkit. ");
        cap.push_str("Use tool_search to discover additional tools if needed. ");
        cap.push_str("Never hallucinate tools that don't exist — only use tools listed above or discovered via tool_search.");
        system_dynamic.push_str(&cap);
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
    if !system_dynamic.trim().is_empty() {
        messages.push(AgentMessage {
            role: "system".to_string(),
            content: Some(system_dynamic),
            tool_calls: Vec::new(),
            name: None,
            tool_call_id: None,
            is_error: None,
        });
    }

    // Insert session history as proper user/assistant messages (not in system prompt)
    if let Some(ref sess_id) = session {
        let session_turns = load_session_turns(sess_id, 8);
        for turn in &session_turns {
            messages.push(AgentMessage {
                role: turn.role.clone(),
                content: Some(if turn.content.len() > 500 {
                    let safe: String = turn.content.chars().take(500).collect();
                    format!("{safe}...")
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
    let mut full_catalog = tool_catalog.clone();

    // --- MCP Client Registry ---
    // Spawn configured MCP servers, discover their tools, and merge into the catalog.
    // Also auto-register excalidraw if EXCALIDRAW_MCP_CMD is set and no explicit mcp_servers config.
    let mcp_configs = {
        let mut cfgs = agent_cfg.mcp_servers.clone();
        // Auto-register excalidraw as MCP server if env var is set and not already configured
        if !cfgs.iter().any(|c| c.name == "excalidraw") {
            if let Some(cmd) = env_optional("EXCALIDRAW_MCP_CMD") {
                cfgs.push(McpServerConfig {
                    name: "excalidraw".to_string(),
                    command: cmd,
                    timeout_secs: Some(u64::MAX),
                    env: HashMap::new(),
                });
            }
        }
        cfgs
    };

    let mut mcp_registry = if !mcp_configs.is_empty() {
        match McpRegistry::start(&mcp_configs) {
            Ok(registry) => {
                let mcp_tools = registry.tool_definitions();
                full_catalog.extend(mcp_tools);
                Some(registry)
            }
            Err(e) => {
                eprintln!("[harness] MCP registry failed: {e}");
                None
            }
        }
    } else {
        None
    };

    let tool_map = tool_catalog_map(&full_catalog);
    let mut active_tools = base_tool_names();
    // Add MCP tool names to active set
    if let Some(ref registry) = mcp_registry {
        for name in registry.route_map.keys() {
            active_tools.insert(name.clone());
        }
    }
    let mut tools = tools_from_active(&tool_map, &active_tools);
    let mut tool_results: Vec<AgentToolResult> = Vec::new();
    let should_log = log || agent_cfg.log.unwrap_or(false);
    let mut final_text = None;

    // Agent logs go to a JSONL file — not the vault. This avoids Tantivy index bloat
    // that caused the vault to grow from 616KB to 7.5GB. JSONL appends are microsecond-fast,
    // require no index updates, and support trivial rotation.
    let log_path = agent_log_path(&mv2);
    let log_max_bytes: u64 = std::env::var("AGENT_LOG_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000_000); // 50MB default
    let mut log_buffer: Vec<AgentLogEntry> = Vec::new();
    let mut mem_write: Option<Vault> = None;

    let flush_log = |path: &Path, buffer: &mut Vec<AgentLogEntry>, max_bytes: u64| {
        rotate_log_if_needed(path, max_bytes);
        if let Err(e) = flush_log_to_jsonl(path, buffer) {
            eprintln!("[harness] failed to write agent log: {e}");
        }
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
            flush_log(&log_path, &mut log_buffer, log_max_bytes);
        }
    }

    let mut reminder_state = ReminderState::default();
    let mut drift_state = DriftState::default();
    let mut turns_since_fact_extract: usize = 0;
    let fact_extract_interval: usize = env_optional("AGENT_FACT_TURNS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);

    let critic_interval: usize = env_optional("CRITIC_INTERVAL")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let mut last_critic_step: usize = 0;

    let mut completed = false;
    let mut current_max_steps = effective_max_steps;
    let mut step = 0;
    let mut wrap_up_injected = false;
    while step < current_max_steps {
        // Check if user extended step budget via checkpoint response
        if let Some(ref prog) = progress {
            if let Ok(p) = prog.lock() {
                if let Some(ext) = p.extended_max_steps {
                    if ext > current_max_steps {
                        current_max_steps = ext;
                    }
                }
                if p.checkpoint_response == Some(false) && !wrap_up_injected {
                    // User said "wrap up" — inject once then let agent finish naturally
                    wrap_up_injected = true;
                    drop(p);
                    messages.push(AgentMessage {
                        role: "user".to_string(),
                        content: Some("[System] The user has asked you to wrap up. Provide a concise summary of what you've accomplished so far and any remaining work. Do NOT start new tool calls.".to_string()),
                        tool_calls: Vec::new(),
                        name: None,
                        tool_call_id: None,
                        is_error: None,
                    });
                }
            }
        }

        // Update progress: thinking phase
        if let Some(ref prog) = progress {
            if let Ok(mut p) = prog.lock() {
                p.step = step;
                p.phase = "thinking".to_string();
            }
        }

        // Auto-compact when context exceeds configurable budget
        let token_estimate = estimate_tokens(&messages);
        let compact_at = compaction_budget_tokens();
        let compact_keep = keep_recent_turns().max(2);
        if token_estimate > compact_at {
            eprintln!("[harness] context at ~{token_estimate} tokens (budget {compact_at}), compacting...");
            if let Err(e) = compact_messages(&mut messages, &model_spec, compact_keep) {
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
            // Update progress: text preview
            if let Some(ref prog) = progress {
                if let Ok(mut p) = prog.lock() {
                    p.text_preview = Some(content.chars().take(100).collect());
                }
            }
            // Track turns for observational memory extraction
            turns_since_fact_extract += 1;

            if should_log {
                let entry = AgentLogEntry {
                    session: session.clone(),
                    role: "assistant".to_string(),
                    text: content.clone(),
                    meta: None,
                    ts_utc: Some(Utc::now().timestamp()),
                };
                log_buffer.push(entry);
                if log_buffer.len() >= effective_log_commit_interval {
                    flush_log(&log_path, &mut log_buffer, log_max_bytes);
                }
            }

            // Observational memory: extract durable facts every N turns
            if turns_since_fact_extract >= fact_extract_interval && !no_memory {
                turns_since_fact_extract = 0;
                let snapshot: String = messages.iter()
                    .filter(|m| m.role == "user" || m.role == "assistant")
                    .rev()
                    .take(8)
                    .filter_map(|m| m.content.as_ref().map(|c| {
                        let preview: String = c.chars().take(300).collect();
                        format!("[{}] {}", m.role, preview)
                    }))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                if !snapshot.trim().is_empty() {
                    let mv2_clone = mv2.clone();
                    let session_clone = session.clone();
                    thread::spawn(move || {
                        let extract_request = AgentHookRequest {
                            messages: vec![
                                AgentMessage {
                                    role: "system".to_string(),
                                    content: Some("You are a fact extractor. Return 3-8 durable, stable facts from the conversation. One fact per line. Only output facts, nothing else. IMPORTANT: Never include passwords, API keys, tokens, private keys, credit card numbers, SSNs, or other sensitive credentials in your output. Redact any PII to general descriptions.".to_string()),
                                    tool_calls: Vec::new(),
                                    name: None,
                                    tool_call_id: None,
                                    is_error: None,
                                },
                                AgentMessage {
                                    role: "user".to_string(),
                                    content: Some(format!("Extract stable facts from:\n{snapshot}")),
                                    tool_calls: Vec::new(),
                                    name: None,
                                    tool_call_id: None,
                                    is_error: None,
                                },
                            ],
                            tools: Vec::new(),
                            session: session_clone,
                        };
                        if let Ok(response) = call_claude(&extract_request) {
                            if let Some(facts) = response.message.content {
                                if !facts.trim().is_empty() {
                                    let uri = format!(
                                        "aethervault://memory/observation/{}",
                                        Utc::now().timestamp()
                                    );
                                    let mut mem_w: Option<Vault> = None;
                                    let mut mem_r: Option<Vault> = None;
                                    let _ = with_write_mem(
                                        &mut mem_r,
                                        &mut mem_w,
                                        &mv2_clone,
                                        true,
                                        |mem| {
                                            let mut opts = PutOptions::default();
                                            opts.uri = Some(uri.clone());
                                            opts.kind = Some("text/markdown".to_string());
                                            opts.track =
                                                Some("aethervault.observation".to_string());
                                            opts.search_text = Some(facts.clone());
                                            mem.put_bytes_with_options(facts.as_bytes(), opts)
                                                .map(|_| ())
                                                .map_err(|e| e.to_string())
                                        },
                                    );
                                }
                            }
                        }
                    });
                }
            }
        }
        let tool_calls = message.tool_calls.clone();
        let has_interim_text = !tool_calls.is_empty() && final_text.as_ref().map(|t| !t.trim().is_empty()).unwrap_or(false);
        messages.push(message);
        if tool_calls.is_empty() {
            completed = true;
            break;
        }

        // Send interim text to user when agent narrates before tool calls
        if has_interim_text {
            if let Some(ref prog) = progress {
                if let Ok(mut p) = prog.lock() {
                    let text = final_text.as_ref().unwrap().clone();
                    // Only send if substantive (not just "OK" or single words)
                    if text.len() > 15 {
                        p.interim_messages.push(text);
                    }
                }
            }
        }

        // Validate all tool calls before execution
        for call in &tool_calls {
            if call.id.trim().is_empty() {
                return Err("tool call is missing an id".into());
            }
            if call.name.trim().is_empty() {
                return Err("tool call is missing a name".into());
            }
        }

        let max_tool_output = 8000; // chars (~2000 tokens)

        // Update progress: tool execution phase + track tools used
        if let Some(ref prog) = progress {
            if let Ok(mut p) = prog.lock() {
                let names: Vec<&str> = tool_calls.iter().map(|c| c.name.as_str()).collect();
                p.phase = format!("tool:{}", names.join(","));
                for call in &tool_calls {
                    *p.tools_used.entry(call.name.clone()).or_insert(0) += 1;
                }
            }
        }

        if tool_calls.len() == 1 {
            // Single tool call — execute directly (no thread overhead)
            let call = &tool_calls[0];
            let result = if call.name.starts_with("mcp__") {
                // Route to MCP registry
                match mcp_registry.as_mut() {
                    Some(registry) => match registry.call_tool(&call.name, call.args.clone()) {
                        Ok(result) => result,
                        Err(err) => ToolExecution {
                            output: format!("Tool error: {err}"),
                            details: serde_json::json!({ "error": err }),
                            is_error: true,
                        },
                    },
                    None => ToolExecution {
                        output: "MCP registry not initialized".to_string(),
                        details: serde_json::json!({ "error": "no registry" }),
                        is_error: true,
                    },
                }
            } else {
                match execute_tool_with_handles(
                    &call.name,
                    call.args.clone(),
                    &mv2,
                    false,
                    &mut mem_read,
                    &mut mem_write,
                ) {
                    Ok(result) => result,
                    Err(err) => ToolExecution {
                        output: format!("Tool error: {err}"),
                        details: serde_json::json!({ "error": err }),
                        is_error: true,
                    },
                }
            };

            // Truncate large tool outputs to prevent context blowout
            let result = if result.output.len() > max_tool_output && !result.is_error {
                let truncated: String = result.output.chars().take(max_tool_output).collect();
                ToolExecution {
                    output: format!(
                        "{truncated}\n\n[Output truncated: {} chars total, showing first {}. Use a more specific query for full results.]",
                        result.output.chars().count(),
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
                content: if tool_content.is_empty() { None } else { Some(tool_content) },
                tool_calls: Vec::new(),
                name: Some(call.name.clone()),
                tool_call_id: Some(call.id.clone()),
                is_error: Some(result.is_error),
            });

            if call.name == "tool_search" && !result.is_error {
                if let Some(results_arr) = result.details.get("results").and_then(|v| v.as_array()) {
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
                    flush_log(&log_path, &mut log_buffer, log_max_bytes);
                }
            }

            // Update reminder state from tool result
            if result.is_error {
                reminder_state.last_tool_failed = true;
                reminder_state.same_tool_fail_streak += 1;
                reminder_state.no_progress_streak += 1;
                // If reminders were given and model still failed, that's a violation
                if drift_state.turns > 0 && drift_state.last_score < 80.0 {
                    drift_state.reminder_violations += 1;
                }
            } else {
                reminder_state.last_tool_failed = false;
                reminder_state.same_tool_fail_streak = 0;
                reminder_state.no_progress_streak = 0;
            }
            if requires_approval(&call.name, &call.args) {
                reminder_state.approval_required_count += 1;
            }
            let read_only_tools = ["search", "query", "get", "list", "tool_search", "skill_search", "reflect"];
            if read_only_tools.iter().any(|t| call.name.contains(t)) {
                reminder_state.sequential_read_ops += 1;
            } else {
                reminder_state.sequential_read_ops = 0;
            }

            if matches!(call.name.as_str(), "put" | "log" | "feedback") && !result.is_error {
                flush_log(&log_path, &mut log_buffer, log_max_bytes);
            }
        } else {
            // Multiple tool calls — execute in parallel (non-MCP), MCP calls sequentially
            let (mcp_calls, regular_calls): (Vec<_>, Vec<_>) = tool_calls.iter()
                .partition(|c| c.name.starts_with("mcp__"));

            let mut results: Vec<(AgentToolCall, ToolExecution)> = Vec::new();

            // Regular tools run in a bounded worker pool.
            if !regular_calls.is_empty() {
                let execute_regular_call = |call: &&AgentToolCall| -> (AgentToolCall, ToolExecution) {
                    let call = *call;
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut local_mem_read: Option<Vault> = None;
                        let mut local_mem_write: Option<Vault> = None;
                        execute_tool_with_handles(
                            &call.name,
                            call.args.clone(),
                            &mv2,
                            false,
                            &mut local_mem_read,
                            &mut local_mem_write,
                        )
                    }));

                    let execution = match result {
                        Ok(Ok(r)) => r,
                        Ok(Err(err)) => ToolExecution {
                            output: format!("Tool error: {err}"),
                            details: serde_json::json!({ "error": err }),
                            is_error: true,
                        },
                        Err(panic_info) => {
                            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                                s.to_string()
                            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "unknown panic".to_string()
                            };
                            eprintln!("[harness] tool thread panicked on '{}': {msg}", call.name);
                            ToolExecution {
                                output: format!(
                                    "Internal error: tool execution panicked: {msg}"
                                ),
                                details: serde_json::json!({ "error": "panic", "message": msg }),
                                is_error: true,
                            }
                        }
                    };

                    (call.clone(), execution)
                };

                let parallel_results: Vec<(AgentToolCall, ToolExecution)> = ThreadPoolBuilder::new()
                    .num_threads(
                        std::thread::available_parallelism()
                            .map(|v| v.get())
                            .unwrap_or(4)
                            .min(regular_calls.len())
                    )
                    .build()
                    .map(|pool| pool.install(|| regular_calls.par_iter().map(execute_regular_call).collect()))
                    .unwrap_or_else(|_| regular_calls.iter().map(execute_regular_call).collect());
                results.extend(parallel_results);
            }

            // MCP tools run sequentially (they share a mutable registry)
            for call in &mcp_calls {
                let result = match mcp_registry.as_mut() {
                    Some(registry) => match registry.call_tool(&call.name, call.args.clone()) {
                        Ok(r) => r,
                        Err(err) => ToolExecution {
                            output: format!("Tool error: {err}"),
                            details: serde_json::json!({ "error": err }),
                            is_error: true,
                        },
                    },
                    None => ToolExecution {
                        output: "MCP registry not initialized".to_string(),
                        details: serde_json::json!({ "error": "no registry" }),
                        is_error: true,
                    },
                };
                results.push(((*call).clone(), result));
            }

            for (call, result) in results {
                // Truncate large tool outputs to prevent context blowout
                let result = if result.output.len() > max_tool_output && !result.is_error {
                    let truncated: String = result.output.chars().take(max_tool_output).collect();
                    ToolExecution {
                        output: format!(
                            "{truncated}\n\n[Output truncated: {} chars total, showing first {}.]",
                            result.output.chars().count(),
                            max_tool_output
                        ),
                        details: result.details,
                        is_error: result.is_error,
                    }
                } else {
                    result
                };

                let tool_content = format_tool_message_content(&call.name, &result.output, &result.details);
                tool_results.push(AgentToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    output: result.output.clone(),
                    details: result.details.clone(),
                    is_error: result.is_error,
                });
                messages.push(AgentMessage {
                    role: "tool".to_string(),
                    content: if tool_content.is_empty() { None } else { Some(tool_content) },
                    tool_calls: Vec::new(),
                    name: Some(call.name.clone()),
                    tool_call_id: Some(call.id.clone()),
                    is_error: Some(result.is_error),
                });

                if call.name == "tool_search" && !result.is_error {
                    if let Some(results_arr) = result.details.get("results").and_then(|v| v.as_array()) {
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
                        flush_log(&log_path, &mut log_buffer, log_max_bytes);
                    }
                }

                // Update reminder state from parallel tool result
                if result.is_error {
                    reminder_state.last_tool_failed = true;
                    reminder_state.same_tool_fail_streak += 1;
                    reminder_state.no_progress_streak += 1;
                    if drift_state.turns > 0 && drift_state.last_score < 80.0 {
                        drift_state.reminder_violations += 1;
                    }
                } else {
                    reminder_state.no_progress_streak = 0;
                }

                if matches!(call.name.as_str(), "put" | "log" | "feedback") && !result.is_error {
                    flush_log(&log_path, &mut log_buffer, log_max_bytes);
                }
            }
        }

        // Mid-loop system reminders (10 rules) + drift detection
        let token_est = estimate_tokens(&messages);
        let reminders = collect_mid_loop_reminders(&reminder_state, step, current_max_steps, token_est);

        // Drift detection scoring
        drift_state.turns += 1;
        let drift_score = compute_drift_score(&drift_state, &reminder_state, &tool_calls);
        drift_state.last_score = drift_score;
        // EMA smoothing: weight recent score 30%
        if drift_state.ema == 0.0 {
            drift_state.ema = drift_score;
        } else {
            drift_state.ema = drift_state.ema * 0.7 + drift_score * 0.3;
        }

        let mut all_reminders = reminders;

        // Drift-based escalation
        if drift_score < 70.0 && drift_score >= 55.0 {
            all_reminders.push("Adherence is degrading. Be more careful and concise with your next action.".to_string());
        } else if drift_score < 55.0 {
            all_reminders.push("Adherence is low. Stop and reflect: re-state the user's goal, then take one careful step.".to_string());
        }
        if drift_state.ema < 40.0 && drift_state.turns >= 3 {
            all_reminders.push("Sustained low adherence. Complete current action and provide a status summary.".to_string());
        }

        // Covert critic: periodic reality grounding via Opus evaluation
        if critic_should_fire(step, critic_interval, &mut last_critic_step, &reminder_state, &tool_calls, &messages) {
            if let Some(correction) = call_critic(
                &prompt_text,
                &messages,
                step,
                current_max_steps,
            ) {
                all_reminders.push(correction);
                drift_state.violations.entry("critic_correction".to_string())
                    .and_modify(|c| *c += 1).or_insert(1);
            }
        }

        if !all_reminders.is_empty() {
            drift_state.reminder_violations = 0;
            messages.push(AgentMessage {
                role: "user".to_string(),
                content: Some(format!("[System Reminder] {}", all_reminders.join(" "))),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
            });
        }
        step += 1;
    }

    if should_log {
        flush_log(&log_path, &mut log_buffer, log_max_bytes);
    }

    if !completed {
        // Extract the last assistant message for context on what was in progress
        let last_action = messages.iter().rev()
            .find(|m| m.role == "assistant")
            .and_then(|m| m.content.as_ref())
            .map(|c| c.chars().take(200).collect::<String>())
            .unwrap_or_else(|| "(no context available)".to_string());
        // Build a richer exhaustion summary with tool stats
        let tool_summary = if let Some(ref prog) = progress {
            if let Ok(p) = prog.lock() {
                let mut sorted: Vec<_> = p.tools_used.iter()
                    .map(|(k, v)| (k.clone(), *v))
                    .collect();
                sorted.sort_by(|a, b| b.1.cmp(&a.1));
                let top: Vec<String> = sorted.into_iter().take(5)
                    .map(|(k, v)| format!("{k}({v}x)"))
                    .collect();
                if top.is_empty() { String::new() } else { format!(" Tools: {}", top.join(", ")) }
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        return Err(format!(
            "Reached {current_max_steps} steps without finishing. \
            Last action: {last_action}.{tool_summary}"
        )
        .into());
    }

    Ok(AgentRunOutput {
        session,
        context: context_pack,
        messages,
        tool_results,
        final_text,
    })
}

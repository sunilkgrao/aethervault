use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::memory_db::PutOptions;
use crate::consolidation::put_with_consolidation;
use chrono::Utc;
use rayon::ThreadPoolBuilder;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde_json;

use crate::claude::{call_agent_hook, call_claude, call_critic};
use crate::{
    append_log_jsonl, base_tool_names, build_context_pack, build_kg_context,
    collect_mid_loop_reminders, compute_drift_score, critic_should_fire, detect_cycle, env_optional,
    execute_tool, find_kg_entities, log_dir_path,
    config_file_path, format_tool_message_content, load_capsule_config, load_config_from_file,
    load_kg_graph, load_session_turns, load_workspace_context, open_or_create_db, requires_approval,
    resolve_hook_spec, resolve_workspace,
    save_session_turns, tool_catalog_map, tool_definitions_json,
    tools_from_active, AgentHookRequest, AgentLogEntry, AgentMessage,
    AgentProgress, AgentRunOutput, AgentSession, AgentToolCall, AgentToolResult,
    ContinuationCheckpoint,
    CommandSpec, DriftState, HookSpec, McpRegistry, McpServerConfig, QueryArgs, ReminderState, SessionTurn,
    ToolExecution,
    open_skill_db, list_skills, search_skills, record_skill_use,
};

/// Tracks blake3 hashes of observations already written this process lifetime.
static OBSERVATION_DEDUP: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Check capsule file size and log a warning if it exceeds 2GB.
fn check_capsule_health(mv2: &Path) {
    let size_bytes = match fs::metadata(mv2) {
        Ok(meta) => meta.len(),
        Err(_) => return,
    };
    let size_mb = size_bytes / (1024 * 1024);
    if size_mb > 2000 {
        eprintln!("[capsule-health] capsule is {size_mb}MB — consider running VACUUM");
    }
}

/// Returns true if an observation is worth persisting to long-term memory.
fn observation_is_useful(text: &str) -> bool {
    let trimmed = text.trim();
    // Too short to be useful
    if trimmed.len() < 30 {
        return false;
    }
    let lower = trimmed.to_lowercase();
    // Meta-observations about the agent itself
    if lower.starts_with("the assistant") || lower.starts_with("the agent") {
        return false;
    }
    // Generic status checks
    let status_noise = [
        "all services are", "everything is running", "everything is working",
        "currently up", "currently running", "currently active",
        "all systems", "is currently ok", "are currently ok",
        "no issues found", "nothing to report",
    ];
    for pattern in &status_noise {
        if lower.contains(pattern) {
            return false;
        }
    }
    // Must contain something specific: a number, a proper noun, a technology name,
    // a concrete preference, or a lesson learned
    let has_number = trimmed.chars().any(|c| c.is_ascii_digit());
    let has_proper_noun = trimmed.split_whitespace().skip(1).any(|w| {
        w.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
            && w.len() > 1
            && !["I", "A", "The", "An", "In", "On", "At", "To", "For", "And", "But", "Or", "Is", "It", "My"].contains(&w)
    });
    let specificity_markers = ["because", "prefers", "always", "never", "important",
        "learned", "rule", "policy", "deadline", "budget", "password", "key",
        "api", "token", "endpoint", "port", "version", "config"];
    let has_specificity = specificity_markers.iter().any(|m| lower.contains(m));

    has_number || has_proper_noun || has_specificity
}

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
        let mut turns = load_session_turns(sess_id, 20);
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
        save_session_turns(sess_id, &turns, 20);
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
        "You are NOT a limited chatbot. You have tools for memory, search, file system, code execution, web requests, email, browser, notifications, and more — all available immediately.",
        "Be proactive, concrete, and concise. Prefer action over discussion.",
        "",
        "## Action Protocol",
        "For routine actions (reading, searching): execute immediately, summarize after.",
        "For significant actions (writing, creating): state your plan in one sentence, then execute.",
        "For complex multi-step tasks: outline 2-3 bullet points, then execute step by step.",
        "For irreversible actions (deleting, sending, deploying): describe consequences, wait for confirmation.",
        "",
        "## Tools",
        "Your tools are listed in the Available Tools section below.",
        "All core tools (memory, search, exec, filesystem, subagents, browser) are available immediately.",
        "Call tool_search to discover additional specialized tools (email providers, calendar, messaging).",
        "Calling tool_search also activates the discovered tools for use in this session.",
        "When multiple independent tool calls are needed, request them all at once for parallel execution.",
        "Sensitive actions require approval. If a tool returns `approval required: <id>`, this is NOT an error — ask the user to approve or reject via `approve <id>` or `reject <id>`.",
        "For parallel or specialist work: use subagent_invoke to spawn an agent with any descriptive name, or subagent_batch for parallel fan-out. Each subagent gets its own session and tools.",
        "",
        "## Subagents",
        "You can spawn subagents dynamically with ANY name — choose names that describe the task (e.g., 'log-analyzer', 'api-tester', 'code-reviewer').",
        "Use subagent_invoke for single delegation, subagent_batch for parallel work.",
        "Subagents use a lighter-weight model, so they're good for heavy lifting while you orchestrate.",
        "",
        "### When to Use Subagents vs Do Directly",
        "- SUBAGENT: large research tasks, multi-file code changes, parallel independent work, long-running analysis",
        "- DIRECTLY: simple tool calls, conversational responses, single file reads, quick commands, anything you can do in 1-3 steps",
        "- Use your judgment. Not every task needs delegation — simple tasks are faster done directly.",
        "",
        "## Self-Modification Workflow",
        "You can modify your own source code, compile, and deploy without human intervention.",
        "The full workflow:",
        "1. Edit source files in /root/aethervault/src/ using `exec` (e.g., `sed`, `cat >`, etc.) or `fs_write`",
        "2. Test: `exec` command `cd /root/aethervault && cargo check` to verify compilation",
        "3. Commit: `exec` commands: `cd /root/aethervault && git add -A && git commit -m \"description\"`",
        "4. Push: `exec` command: `cd /root/aethervault && git push origin main`",
        "5. Deploy: call `self_upgrade` tool (blue-green deploy with automatic rollback)",
        "6. After deploy, you will restart. Your conversation state persists in the capsule.",
        "",
        "Important:",
        "- ALWAYS test with `cargo check` before committing",
        "- ALWAYS commit and push BEFORE calling self_upgrade (it does git reset --hard)",
        "- If the new binary crashes, upgrade.sh auto-rolls back within 30s",
        "- You can check deploy status: `exec` command `cat /opt/aethervault/upgrade.log | tail -20`",
        "",
        "## Autonomous Self-Improvement",
        "A systemd timer runs every 6 hours to trigger autonomous self-improvement cycles.",
        "Each cycle: scans for improvements → implements one → validates → deploys.",
        "Past improvements are logged in /root/.aethervault/data/self-improve-log.jsonl",
        "and stored as reflections in your capsule memory.",
        "",
        "When running a self-improvement scan, prioritize:",
        "1. Reliability fixes (error handling, edge cases, crash prevention)",
        "2. Performance improvements (reduce latency, memory usage)",
        "3. Safety hardening (input validation, timeout handling)",
        "4. Capability additions (new tool integrations, better prompts)",
        "",
        "Never autonomously:",
        "- Remove safety checks or approval gates",
        "- Modify deployment infrastructure (upgrade.sh, systemd configs)",
        "- Change API keys, secrets, or authentication",
        "- Make changes that affect the Telegram bridge protocol",
        "",
        "## Mid-Run User Messages",
        "The user can send messages at any time, even while you are working on a task.",
        "These messages are injected directly into your conversation as they arrive.",
        "Treat every mid-run user message as a potential course correction — read it immediately and adjust your approach.",
        "If the user's message changes what you should be doing, acknowledge it and pivot.",
        "Never ignore a user message or defer it until later.",
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
        "NEVER describe a fallback plan. If something fails, just try the next approach silently.",
        "",
        "## Error Recovery",
        "When a tool fails, try a different approach. Use reflect to record lessons learned.",
        "Never retry the same failing call. If stuck after 2 attempts, ask the user for guidance.",
        "",
        "## Critical Reminders",
        "Investigate before answering — search memory before making claims.",
        "Match the user's energy. Be concise when they're concise, detailed when they need detail.",
        "For irreversible actions, always confirm first.",
        "",
        "## Tool Output Grounding Rule",
        "When reporting what a tool returned, ONLY state information literally present in the output.",
        "NEVER infer details not shown (config values from key names, success from partial output).",
        "NEVER claim error messages, file paths, or identifiers not in the tool output.",
        "NEVER report success when the tool output shows errors or empty results.",
        "If output is ambiguous or incomplete, say so. Quote the relevant output to support claims.",
        "",
        "## Multi-Step Grounding Rules",
        "When executing multi-step tasks:",
        "- NEVER claim a step is complete until the tool output for that step confirms it.",
        "- Report each step's ACTUAL outcome, including failures, before proceeding to the next step.",
        "- If a tool call fails, acknowledge the failure explicitly before retrying or moving on.",
        "- When reporting subagent results, quote the subagent's actual output — do NOT paraphrase or embellish.",
        "- If a subagent returns empty or error results, say so — do NOT fabricate results on its behalf.",
        "",
        "## Request Triage",
        "Before using tools, classify the request:",
        "- Conversational (greeting, thanks, status check): Respond directly. No tools needed.",
        "- Clear bounded task (single file, one command, quick lookup): Execute directly, report results.",
        "- Ambiguous/vague request (unclear scope, vague pronouns like 'this'/'everything'): Ask 1-2 clarifying questions BEFORE acting.",
        "- Complex multi-step task (research, multi-file code, debugging, troubleshooting): Break it down, use subagents for heavy lifting if helpful.",
        "Do NOT launch extensive tool use for greetings or vague requests.",
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
/// Compact messages when context is getting large.
/// Returns the extracted GOAL from the structured summary (if any).
pub(crate) fn compact_messages(
    messages: &mut Vec<AgentMessage>,
    hook: &HookSpec,
    keep_recent: usize,
) -> Result<Option<String>, String> {
    if messages.len() <= keep_recent + 2 {
        return Ok(None); // Nothing to compact
    }
    // Preserve all leading system blocks (supports cache-split: stable prefix + dynamic suffix)
    let system_end = messages.iter().take_while(|m| m.role == "system").count();
    let summary_end = messages.len().saturating_sub(keep_recent);
    let summary_start = system_end.min(summary_end);
    let system_msgs: Vec<_> = messages[..system_end].to_vec();
    let to_summarize: Vec<_> = messages[summary_start..summary_end].to_vec();
    let recent: Vec<_> = messages[summary_end..].to_vec();

    // Build a summary request
    let summary_text: String = to_summarize.iter().filter_map(|m| {
        let role = &m.role;
        m.content.as_ref().map(|c| {
            let preview: String = c.chars().take(300).collect();
            format!("[{role}] {preview}")
        })
    }).collect::<Vec<_>>().join("\n");

    let summary_prompt = format!(
        "Summarize this conversation. Output in this format:\n\
         GOAL: <the user's original goal in one sentence>\n\
         PROGRESS: <what has been accomplished>\n\
         PENDING: <what still needs to be done>\n\
         KEY_FILES: <important file paths mentioned>\n\
         AVOID: <mistakes made or approaches that failed>\n\
         CORRECTIONS: <any grounding violations flagged by the critic, specific false claims made, and what the correct information was>\n\
         SECURITY_INCIDENTS: <any API keys or secrets exposed, security warnings issued>\n\
         CONTEXT: <other important context>\n\n\
         {summary_text}"
    );

    let summary_request = AgentHookRequest {
        messages: vec![
            AgentMessage {
                role: "system".to_string(),
                content: Some("You are a conversation summarizer. Output only the structured summary, nothing else. Be concise.".to_string()),
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

    // Extract the GOAL field from the structured summary
    let extracted_goal = summary.lines()
        .find(|line| line.starts_with("GOAL:"))
        .map(|line| line.trim_start_matches("GOAL:").trim().to_string());

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
    Ok(extracted_goal)
}

/// Truncate large tool outputs to prevent context blowout.
/// Non-error outputs exceeding `max_chars` are trimmed with a notice appended.
fn truncate_tool_output(result: ToolExecution, max_chars: usize) -> ToolExecution {
    if result.output.len() > max_chars && !result.is_error {
        let truncated: String = result.output.chars().take(max_chars).collect();
        ToolExecution {
            output: format!(
                "{truncated}\n\n[Output truncated: {} chars total, showing first {}. Use a more specific query for full results.]",
                result.output.chars().count(),
                max_chars
            ),
            details: result.details,
            is_error: result.is_error,
        }
    } else {
        result
    }
}

/// Post-process a single completed tool execution: push results and messages,
/// activate discovered tools, track skill retrieval, and write log entries.
/// Returns `(is_error, tools_changed)` so the caller can update reminder state
/// and refresh the active tool set as needed.
fn process_tool_result(
    call: &AgentToolCall,
    result: ToolExecution,
    tool_results: &mut Vec<AgentToolResult>,
    messages: &mut Vec<AgentMessage>,
    active_tools: &mut HashSet<String>,
    retrieved_skills: &mut Vec<String>,
    should_log: bool,
    session: &Option<String>,
    log_dir: &Path,
) -> (bool, bool) {
    let is_error = result.is_error;

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

    // Activate newly discovered tools from tool_search results
    let mut tools_changed = false;
    if call.name == "tool_search" && !is_error {
        if let Some(results_arr) = result.details.get("results").and_then(|v| v.as_array()) {
            for item in results_arr {
                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                    if active_tools.insert(name.to_string()) {
                        tools_changed = true;
                    }
                }
            }
        }
    }

    // SkillRL R4: Track skill names retrieved via skill_search
    if call.name == "skill_search" && !is_error {
        if let Some(results_arr) = result.details.get("results").and_then(|v| v.as_array()) {
            for item in results_arr {
                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                    retrieved_skills.push(name.to_string());
                }
            }
        }
    }

    if should_log {
        let entry = AgentLogEntry {
            session: session.clone(),
            role: "tool".to_string(),
            text: result.output,
            meta: Some(result.details),
            ts_utc: Some(Utc::now().timestamp()),
        };
        if let Err(e) = append_log_jsonl(log_dir, &entry) {
            eprintln!("[harness] failed to write agent log: {e}");
        }
    }

    (is_error, tools_changed)
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
    _log_commit_interval: usize,
    log: bool,
    progress: Option<Arc<Mutex<AgentProgress>>>,
) -> Result<AgentRunOutput, Box<dyn std::error::Error>> {
    if prompt_text.trim().is_empty() {
        return Err("agent prompt is empty".into());
    }

    // One-time capsule size check at session start
    check_capsule_health(&mv2);

    let db = open_or_create_db(&mv2)?;

    // Try flat file config first (workspace/config.json), fall back to capsule.
    let workspace_env = std::env::var("AETHERVAULT_WORKSPACE").ok().map(PathBuf::from);
    let config = if let Some(ref ws) = workspace_env {
        let cfg_path = config_file_path(ws);
        if cfg_path.exists() {
            load_config_from_file(ws)
        } else {
            load_capsule_config(&db).unwrap_or_default()
        }
    } else {
        load_capsule_config(&db).unwrap_or_default()
    };
    let agent_cfg = config.agent.clone().unwrap_or_default();
    let agent_workspace = resolve_workspace(None, &agent_cfg);
    let hook_cfg = config.hooks.clone().unwrap_or_default();
    // No wall-clock deadline for model hooks — zombie detection handles stuck processes.
    // The old 300s timeout killed complex Codex tasks (CRM ingestion, VM repair, KG growth)
    // before they could finish.  Subagent steps are bounded by max_steps, not wall-clock.
    let base_model_spec = resolve_hook_spec(
        model_hook,
        u64::MAX,
        agent_cfg.model_hook.clone().or(hook_cfg.llm),
        None,
    )
    .ok_or("agent requires --model-hook or config.agent.model_hook or config.hooks.llm")?;
    let mut model_spec = base_model_spec.clone();

    // Opus escalation: build a fallback HookSpec for when critic fires
    let opus_escalation_spec: Option<HookSpec> = {
        // Only useful if the base model isn't already Opus
        let base_cmd = match &base_model_spec.command {
            CommandSpec::String(s) => s.trim().to_ascii_lowercase(),
            CommandSpec::Array(a) => a.first().map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default(),
        };
        let is_already_opus = base_cmd == "builtin:claude" || base_cmd == "claude";
        if is_already_opus {
            None
        } else {
            Some(HookSpec {
                command: CommandSpec::String("builtin:claude".to_string()),
                timeout_ms: base_model_spec.timeout_ms,
                full_text: base_model_spec.full_text,
            })
        }
    };
    let opus_escalation_steps: usize = env_optional("OPUS_ESCALATION_STEPS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let mut opus_escalation_remaining: usize = 0;

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

    // --- SkillRL R1: Auto-inject top skills into stable prefix ---
    if let Some(workspace) = resolve_workspace(None, &agent_cfg) {
        let db_path = workspace.join("skills.sqlite");
        if db_path.exists() {
            if let Ok(conn) = open_skill_db(&db_path) {
                let general = list_skills(&conn, 3);
                let prompt_words: String = prompt_text
                    .split_whitespace()
                    .take(10)
                    .collect::<Vec<_>>()
                    .join(" ");
                let task_specific = search_skills(&conn, &prompt_words, 3);
                let mut skill_block = String::new();
                let mut seen: HashSet<String> = HashSet::new();
                for s in general.iter().chain(task_specific.iter()) {
                    if !seen.insert(s.name.clone()) {
                        continue;
                    }
                    skill_block.push_str(&format!(
                        "- **{}**: {} (trigger: {}, success: {:.0}%, used {}x)\n",
                        s.name,
                        s.notes.as_deref().unwrap_or(""),
                        s.trigger.as_deref().unwrap_or("any"),
                        s.success_rate * 100.0,
                        s.times_used
                    ));
                }
                if !skill_block.is_empty() {
                    system_prompt.push_str("\n\n# Learned Skills\n");
                    system_prompt.push_str("These are proven strategies from past experience. Apply them when their trigger conditions match.\n\n");
                    system_prompt.push_str(&skill_block);
                }
            }
        }
    }

    if let Some(global_context) = config.context {
        if !global_context.trim().is_empty() {
            system_prompt.push_str("\n\n# Global Context\n");
            system_prompt.push_str(&global_context);
        }
    }

    // Resource-aware orchestration: inject compute delegation guide for long-running tasks
    let is_continuation = prompt_text.contains("[Continuation from previous session");
    let long_run_mode = env_optional("AGENT_LONG_RUN").map(|v| v == "1").unwrap_or(false)
        || is_continuation;
    if long_run_mode {
        system_prompt.push_str(concat!(
            "\n\n## Resource Guide — Long-Running Tasks\n",
            "For long-running or complex tasks, subagents help you parallelize and offload heavy work.\n\n",
            "### Spawning Subagents\n",
            "Use subagent_invoke with ANY descriptive name. The name should describe what the agent does:\n",
            "- subagent_invoke(name=\"log-analyzer\", prompt=\"...\") — analyzes logs.\n",
            "- subagent_invoke(name=\"api-tester\", prompt=\"...\") — tests API endpoints.\n",
            "- subagent_invoke(name=\"code-reviewer\", prompt=\"...\") — reviews code changes.\n",
            "- subagent_batch(invocations=[...]) — run multiple agents in parallel.\n",
            "Choose names that describe the TASK, not a generic role. Be specific.\n\n",
            "### Cost Model\n",
            "- Your main loop uses a more expensive model. Good for orchestration, synthesis, user communication.\n",
            "- Subagents use a lighter model. Good for research, code changes, analysis, and batch work.\n",
            "- Use subagent_batch for independent parallel tasks.\n\n",
            "### Guidelines\n",
            "- Use exec for shell commands, file operations, service management.\n",
            "- Use subagent_invoke for LLM-powered work (research, coding, analysis).\n",
            "- Do NOT use exec to invoke LLM processes (codex, ollama) — use subagent_invoke instead.\n",
            "- Simple tasks (1-3 steps) are usually faster done directly than delegated.\n",
        ));
    }

    // --- KV-Cache Breakpoint ---
    // Everything above (system_prompt) is stable within a session.
    // Everything below (system_dynamic) churns per-turn (memory, KG).
    // Splitting them enables Anthropic prompt cache reuse on the stable prefix.
    let mut system_dynamic = String::new();

    let mut context_pack = None;
    let effective_max_steps = agent_cfg.max_steps.unwrap_or(max_steps);
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
        };
        if let Ok(pack) = build_context_pack(
            &db,
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
    // Knowledge Graph entity auto-injection
    let kg_path = agent_workspace.as_ref()
        .map(|ws| ws.join("data/knowledge-graph.json"))
        .unwrap_or_else(|| PathBuf::from("/root/.aethervault/data/knowledge-graph.json"));
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
        let session_turns = load_session_turns(sess_id, 20);
        for turn in &session_turns {
            messages.push(AgentMessage {
                role: turn.role.clone(),
                content: Some(if turn.content.len() > 2000 {
                    let safe: String = turn.content.chars().take(2000).collect();
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

    // Agent logs go to date-based JSONL files in workspace/logs/agent-YYYY-MM-DD.jsonl.
    // This avoids Tantivy index bloat and naturally partitions logs by day.
    // Resolve workspace from env var (already computed earlier) or agent config.
    let log_dir = workspace_env.as_ref()
        .cloned()
        .or_else(|| agent_cfg.workspace.as_ref().map(PathBuf::from))
        .map(|ws| log_dir_path(&ws))
        .unwrap_or_else(|| {
            // Fallback: derive from vault parent (legacy path)
            let dir = mv2.parent().unwrap_or(Path::new(".")).join("logs");
            let _ = std::fs::create_dir_all(&dir);
            dir
        });
    if should_log {
        let entry = AgentLogEntry {
            session: session.clone(),
            role: "user".to_string(),
            text: prompt_text.clone(),
            meta: None,
            ts_utc: Some(Utc::now().timestamp()),
        };
        if let Err(e) = append_log_jsonl(&log_dir, &entry) {
            eprintln!("[harness] failed to write agent log: {e}");
        }
    }

    let mut reminder_state = ReminderState::default();
    let mut drift_state = DriftState::default();
    // Load persisted violations from previous sessions.
    // Cap loaded violations so a new session doesn't start in LEVEL 4 from prior runs.
    let drift_path = log_dir.join("drift_state.json");
    if let Ok(data) = std::fs::read_to_string(&drift_path) {
        if let Ok(persisted) = serde_json::from_str::<DriftState>(&data) {
            // Only carry forward critic_history, NOT violation counts.
            // Each session starts with a clean violation slate — accumulated
            // violations from previous sessions were causing new sessions to
            // immediately hit LEVEL 3/4 thresholds.
            drift_state.critic_history = persisted.critic_history;
            let prev_count = persisted.violations.get("critic_correction").copied().unwrap_or(0);
            eprintln!("[drift] loaded {prev_count} persisted violations (reset to 0 for new session)");
        }
    }
    let mut recent_actions: VecDeque<String> = VecDeque::with_capacity(30);
    let mut retrieved_skills: Vec<String> = Vec::new();
    let mut turns_since_fact_extract: usize = 0;
    let fact_extract_interval: usize = env_optional("AGENT_FACT_TURNS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);

    let critic_interval: usize = env_optional("CRITIC_INTERVAL")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let mut last_critic_step: usize = 0;

    // Goal recitation: extract and periodically re-inject the user's goal
    let mut current_plan: Option<String> = Some(prompt_text.chars().take(500).collect());
    let plan_recite_interval: usize = env_optional("PLAN_RECITE_INTERVAL")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);

    let mut completed = false;
    let mut current_max_steps = effective_max_steps;
    let mut step = 0;
    let mut wrap_up_injected = false;
    let mut consecutive_hook_failures: usize = 0;
    const MAX_CONSECUTIVE_HOOK_FAILURES: usize = 3;
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

        // Drain steering messages: user sent messages mid-run that should
        // alter the agent's course. Inject them as user messages so the LLM
        // sees them immediately at the next step.
        if let Some(ref prog) = progress {
            if let Ok(mut p) = prog.lock() {
                let steering: Vec<String> = p.steering_messages.drain(..).collect();
                if !steering.is_empty() {
                    let combined = steering.join("\n\n");
                    drop(p);
                    eprintln!("[harness] injecting {} steering message(s) from user", steering.len());
                    messages.push(AgentMessage {
                        role: "user".to_string(),
                        content: Some(combined),
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
            match compact_messages(&mut messages, &model_spec, compact_keep) {
                Ok(Some(goal)) => {
                    current_plan = Some(goal);
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("[harness] compaction failed: {e}");
                }
            }
        }

        // Model escalation: count down Opus steps and revert to base model
        if opus_escalation_remaining > 0 {
            opus_escalation_remaining -= 1;
            if opus_escalation_remaining == 0 {
                eprintln!("[harness] Opus escalation window ended, reverting to base model");
                model_spec = base_model_spec.clone();
            }
        }

        let request = AgentHookRequest {
            messages: messages.clone(),
            tools: tools.clone(),
            session: session.clone(),
        };
        let message = match call_agent_hook(&model_spec, &request) {
            Ok(msg) => {
                consecutive_hook_failures = 0;
                msg
            }
            Err(e) => {
                consecutive_hook_failures += 1;
                eprintln!(
                    "[harness] hook failed ({consecutive_hook_failures}/{MAX_CONSECUTIVE_HOOK_FAILURES}): {e}"
                );
                if consecutive_hook_failures >= MAX_CONSECUTIVE_HOOK_FAILURES {
                    eprintln!(
                        "[harness] {MAX_CONSECUTIVE_HOOK_FAILURES} consecutive failures, ending run"
                    );
                    final_text = Some(format!(
                        "(Agent terminated: model hook failed {MAX_CONSECUTIVE_HOOK_FAILURES} \
                         consecutive times. Last error: {e})"
                    ));
                    break; // Exits loop -> continuation checkpoint created
                }
                // Inject error as assistant message — agent sees it next iteration
                AgentMessage {
                    role: "assistant".to_string(),
                    content: Some(format!(
                        "(Model hook error on attempt {consecutive_hook_failures}: {e}. \
                         Will retry on next step.)"
                    )),
                    tool_calls: Vec::new(),
                    name: None,
                    tool_call_id: None,
                    is_error: None,
                }
            }
        };
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
                if let Err(e) = append_log_jsonl(&log_dir, &entry) {
                    eprintln!("[harness] failed to write agent log: {e}");
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
                                if !facts.trim().is_empty() && observation_is_useful(&facts) {
                                    // Dedup guard: skip if we already wrote identical observation this session
                                    let hash = blake3::hash(facts.as_bytes()).to_hex().to_string();
                                    {
                                        let mut seen = OBSERVATION_DEDUP.lock().unwrap_or_else(|e| e.into_inner());
                                        if !seen.insert(hash) {
                                            eprintln!("[observation-dedup] skipped duplicate: {}...", &facts.chars().take(60).collect::<String>());
                                            return;
                                        }
                                    }
                                    let uri = format!(
                                        "aethervault://memory/observation/{}",
                                        Utc::now().timestamp()
                                    );
                                    if let Ok(obs_db) = open_or_create_db(&mv2_clone) {
                                        let mut opts = PutOptions::default();
                                        opts.uri = Some(uri.clone());
                                        opts.kind = Some("text/markdown".to_string());
                                        opts.track = Some("aethervault.observation".to_string());
                                        opts.search_text = Some(facts.clone());
                                        match put_with_consolidation(&obs_db, facts.as_bytes(), opts) {
                                            Ok(result) => {
                                                let decision_str = format!("{:?}", result.decision);
                                                if result.frame_id.is_none() {
                                                    eprintln!("[observation-consolidation] NOOP: {decision_str}");
                                                } else {
                                                    eprintln!("[observation-consolidation] {decision_str}");
                                                }
                                            }
                                            Err(e) => {
                                                eprintln!("[observation] consolidation failed: {e}");
                                            }
                                        }
                                        if let Err(e) = obs_db.commit() {
                                            eprintln!("[observation] commit failed: {e}");
                                        }
                                    }
                                } else if !facts.trim().is_empty() {
                                    eprintln!("[observation-gate] skipped: {}...", &facts.chars().take(60).collect::<String>());
                                }
                            }
                        }
                    });
                }
            }
        }
        let tool_calls = message.tool_calls.clone();
        messages.push(message);
        if tool_calls.is_empty() {
            completed = true;
            break;
        }

        // Send interim text to user when agent narrates before tool calls
        if let Some(ref text) = final_text {
            if let Some(ref prog) = progress {
                if let Ok(mut p) = prog.lock() {
                    // Only send if substantive (not just "OK" or single words)
                    if text.len() > 15 {
                        p.interim_messages.push(text.clone());
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

        // Update progress: tool execution phase + track tools used + delegation tracking
        if let Some(ref prog) = progress {
            if let Ok(mut p) = prog.lock() {
                let names: Vec<&str> = tool_calls.iter().map(|c| c.name.as_str()).collect();
                p.phase = format!("tool:{}", names.join(","));
                for call in &tool_calls {
                    *p.tools_used.entry(call.name.clone()).or_insert(0) += 1;
                    // Track delegation: exec calls containing codex/ollama are delegated
                    if call.name == "exec" {
                        let args_str = call.args.to_string().to_lowercase();
                        if args_str.contains("codex") || args_str.contains("ollama") {
                            p.delegated_steps += 1;
                        } else {
                            p.opus_steps += 1;
                        }
                    } else {
                        p.opus_steps += 1;
                    }
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
                match execute_tool(
                    &call.name,
                    call.args.clone(),
                    &mv2,
                    &db,
                    false,
                ) {
                    Ok(result) => result,
                    Err(err) => ToolExecution {
                        output: format!("Tool error: {err}"),
                        details: serde_json::json!({ "error": err }),
                        is_error: true,
                    },
                }
            };

            let result = truncate_tool_output(result, max_tool_output);
            let (is_error, tools_changed) = process_tool_result(
                call, result,
                &mut tool_results, &mut messages, &mut active_tools,
                &mut retrieved_skills, should_log, &session, &log_dir,
            );
            if tools_changed {
                tools = tools_from_active(&tool_map, &active_tools);
            }

            // Update reminder state from tool result
            if is_error {
                reminder_state.last_tool_failed = true;
                reminder_state.same_tool_fail_streak += 1;
                reminder_state.no_progress_streak += 1;
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
        } else {
            // Multiple tool calls — execute in parallel (non-MCP), MCP calls sequentially
            let (mcp_calls, regular_calls): (Vec<_>, Vec<_>) = tool_calls.iter()
                .partition(|c| c.name.starts_with("mcp__"));

            let mut results: Vec<(AgentToolCall, ToolExecution)> = Vec::new();

            // Regular tools run in a bounded worker pool.
            if !regular_calls.is_empty() {
                let mv2_ref = &mv2;
                let execute_regular_call = |call: &&AgentToolCall| -> (AgentToolCall, ToolExecution) {
                    let call = *call;
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let local_db = open_or_create_db(mv2_ref).map_err(|e| e.to_string())?;
                        execute_tool(&call.name, call.args.clone(), mv2_ref, &local_db, false)
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
                let result = truncate_tool_output(result, max_tool_output);
                let (is_error, tools_changed) = process_tool_result(
                    &call, result,
                    &mut tool_results, &mut messages, &mut active_tools,
                    &mut retrieved_skills, should_log, &session, &log_dir,
                );
                if tools_changed {
                    tools = tools_from_active(&tool_map, &active_tools);
                }

                // Update reminder state from parallel tool result
                if is_error {
                    reminder_state.last_tool_failed = true;
                    reminder_state.same_tool_fail_streak += 1;
                    reminder_state.no_progress_streak += 1;
                    if drift_state.turns > 0 && drift_state.last_score < 80.0 {
                        drift_state.reminder_violations += 1;
                    }
                } else {
                    reminder_state.no_progress_streak = 0;
                }
            }
        }

        // Track recent actions for cycle detection
        for call in &tool_calls {
            let args_preview: String = call.args.to_string().chars().take(200).collect();
            let hash = blake3::hash(args_preview.as_bytes()).to_hex()[..16].to_string();
            let action_key = format!("{}:{}", call.name, hash);
            if recent_actions.len() >= 30 {
                recent_actions.pop_front();
            }
            recent_actions.push_back(action_key);
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

        // Budget tracking: inject step budget awareness
        let budget_msg = format!(
            "Steps used: {}/{} | Remaining: {}",
            step + 1, current_max_steps, current_max_steps.saturating_sub(step + 1)
        );
        all_reminders.push(budget_msg);

        // Resource-awareness: nudge delegation to free compute when in long-run mode
        if long_run_mode {
            if let Some(ref prog) = progress {
                if let Ok(p) = prog.lock() {
                    if step > 20 && p.delegated_steps == 0 {
                        all_reminders.push("Reminder: subagent_invoke and subagent_batch are available for parallelizing or offloading heavy work.".to_string());
                    } else if step > 30 && p.opus_steps > 0 {
                        let total = p.opus_steps + p.delegated_steps;
                        let opus_ratio = p.opus_steps as f64 / total.max(1) as f64;
                        if opus_ratio > 0.9 {
                            all_reminders.push("Subagents are available for offloading heavy work if useful.".to_string());
                        }
                    }
                }
            }
        }

        // Cycle detection: catch repeated action patterns
        if let Some((cycle_len, _repeats)) = detect_cycle(&recent_actions) {
            if cycle_len == 1 {
                all_reminders.push("You are repeating the same action 3 times. Try a completely different approach.".to_string());
            } else {
                all_reminders.push(format!("You are in a {cycle_len}-step loop. Break out by trying a fundamentally different strategy."));
            }
            reminder_state.no_progress_streak += 3;
        }

        // Goal recitation: periodically re-inject the user's goal
        if plan_recite_interval > 0 && step > 0 && step % plan_recite_interval == 0 {
            if let Some(ref plan) = current_plan {
                all_reminders.push(format!(
                    "[Plan Check] Your current goal: {}. Progress: step {}/{current_max_steps}. Remain focused on the objective.",
                    plan, step
                ));
            }
        }

        // Drift-based escalation
        if drift_score < 70.0 && drift_score >= 55.0 {
            all_reminders.push("Adherence is degrading. Be more careful and concise with your next action.".to_string());
        } else if drift_score < 55.0 {
            all_reminders.push("Adherence is low. Stop and reflect: re-state the user's goal, then take one careful step.".to_string());
        }
        if drift_state.ema < 40.0 && drift_state.turns >= 3 {
            all_reminders.push("Sustained low adherence. Complete current action and provide a status summary.".to_string());
        }

        // SkillRL R6: Behavioral anchoring — inject proven skills when drifting
        if drift_score < 70.0 {
            if let Some(ref workspace) = agent_workspace {
                let db_path = workspace.join("skills.sqlite");
                if db_path.exists() {
                    if let Ok(conn) = open_skill_db(&db_path) {
                        let top_skills = list_skills(&conn, 3);
                        if !top_skills.is_empty() {
                            let anchor: String = top_skills
                                .iter()
                                .filter_map(|s| {
                                    s.notes.as_ref().map(|n| format!("- {}: {}", s.name, n))
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            if !anchor.is_empty() {
                                all_reminders.push(format!(
                                    "Re-anchor with proven strategies:\n{anchor}"
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Inject routine reminders (budget, drift, cycle, etc.) — excludes critic corrections
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

        // Covert critic: periodic reality grounding via Opus evaluation
        // Critic corrections are injected as a SEPARATE message from routine reminders
        let current_violation_count = drift_state.violations.get("critic_correction").copied().unwrap_or(0);
        if critic_should_fire(step, critic_interval, &mut last_critic_step, &reminder_state, &tool_calls, &messages, current_violation_count) {
            if let Some(correction) = call_critic(
                &prompt_text,
                &messages,
                step,
                current_max_steps,
            ) {
                // Don't add to all_reminders. Inject as separate message.
                let critic_msg = format!(
                    "[CRITICAL CORRECTION — Grounding Violation]\n{}\nYou MUST acknowledge this correction before continuing.",
                    correction
                );
                messages.push(AgentMessage {
                    role: "user".to_string(),
                    content: Some(critic_msg),
                    tool_calls: Vec::new(),
                    name: None,
                    tool_call_id: None,
                    is_error: None,
                });
                // Track in drift state
                drift_state.violations.entry("critic_correction".to_string())
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
                // Persist violations to disk
                if let Ok(json) = serde_json::to_string(&drift_state) {
                    let _ = std::fs::write(&drift_path, json);
                }

                // Model escalation: swap to Opus for next N steps when critic fires
                if let Some(ref opus_spec) = opus_escalation_spec {
                    if opus_escalation_remaining == 0 {
                        eprintln!("[harness] critic fired — escalating to Opus for {opus_escalation_steps} steps");
                        model_spec = opus_spec.clone();
                        opus_escalation_remaining = opus_escalation_steps;
                    }
                }

                // Progressive escalation based on violation count
                let violation_count = drift_state.violations.get("critic_correction").copied().unwrap_or(0);
                match violation_count {
                    0..=2 => { /* Standard correction — already injected above */ }
                    3..=4 => {
                        // Level 2: Stronger language
                        messages.push(AgentMessage {
                            role: "user".to_string(),
                            content: Some(format!("[ESCALATION WARNING] This is correction #{violation_count}. Repeated grounding violations detected. You MUST quote specific tool output for every factual claim. Failure to comply will result in reduced capabilities.")),
                            tool_calls: Vec::new(),
                            name: None,
                            tool_call_id: None,
                            is_error: None,
                        });
                    }
                    5..=6 => {
                        // Level 3: Log severe warning
                        eprintln!("[critic] LEVEL 3 escalation: {violation_count} violations — consider tool restriction");
                        messages.push(AgentMessage {
                            role: "user".to_string(),
                            content: Some(format!("[SEVERE WARNING] {violation_count} grounding violations this session. STOP making claims not supported by tool output. Before EVERY response, re-read the most recent tool output and ONLY report what it literally says.")),
                            tool_calls: Vec::new(),
                            name: None,
                            tool_call_id: None,
                            is_error: None,
                        });
                        // Enforce: reduce remaining step budget by 1/3 (was halved — too aggressive)
                        let remaining = current_max_steps.saturating_sub(step);
                        current_max_steps = step + (remaining * 2 / 3).max(6);
                        eprintln!("[critic] LEVEL 3 enforcement: step budget reduced to {current_max_steps} (was {})", step + remaining);
                    }
                    _ => {
                        // Level 4: Graceful wind-down instead of hard kill.
                        // Give the agent enough steps to write partial results.
                        eprintln!("[critic] LEVEL 4 escalation: {violation_count} violations — winding down gracefully");
                        messages.push(AgentMessage {
                            role: "user".to_string(),
                            content: Some(format!("[CRITICAL — GRACEFUL WIND-DOWN] {violation_count} grounding violations. You have 6 steps remaining. IMMEDIATELY:\n1. Write any partial results to disk (files the user requested).\n2. Summarize what you actually accomplished vs. what failed.\n3. Do NOT make new claims — only report verified facts from tool outputs.\nAfter these 6 steps, the session will end.")),
                            tool_calls: Vec::new(),
                            name: None,
                            tool_call_id: None,
                            is_error: None,
                        });
                        // Enforce: allow 6 steps for graceful output (was 3 — too aggressive)
                        current_max_steps = step + 6;
                        eprintln!("[critic] LEVEL 4 enforcement: graceful wind-down in 6 steps (step={step}, max={current_max_steps})");
                    }
                }
            }
        }

        // Checkpoint-and-report every 10 steps
        if step > 0 && step % 10 == 0 {
            messages.push(AgentMessage {
                role: "user".to_string(),
                content: Some(format!("[Checkpoint — Step {}] Summarize what you have accomplished so far and what you plan to do next. If the user's request was vague, confirm you are on the right track.", step)),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
            });
        }

        step += 1;
    }

    // SkillRL R4: Record usage of retrieved skills based on session outcome
    if !retrieved_skills.is_empty() {
        if let Some(ref workspace) = agent_workspace {
            let db_path = workspace.join("skills.sqlite");
            if let Ok(conn) = open_skill_db(&db_path) {
                for skill_name in &retrieved_skills {
                    let _ = record_skill_use(&conn, skill_name, completed);
                }
            }
        }
    }

    if !completed {
        // Self-continuation: instead of erroring, create a checkpoint for session chaining
        // Compact to get a tight summary for the checkpoint
        let compact_keep = keep_recent_turns().max(2);
        let compacted_goal = compact_messages(&mut messages, &model_spec, compact_keep)
            .ok()
            .flatten();
        let goal = compacted_goal
            .or_else(|| current_plan.clone())
            .unwrap_or_else(|| prompt_text.chars().take(500).collect());

        // Build the summary from the compacted context
        let summary = messages.iter()
            .find(|m| m.role == "user" && m.content.as_ref().map(|c| c.contains("[Context compacted")).unwrap_or(false))
            .and_then(|m| m.content.clone())
            .unwrap_or_else(|| {
                messages.iter().rev()
                    .find(|m| m.role == "assistant")
                    .and_then(|m| m.content.as_ref())
                    .map(|c| c.chars().take(500).collect::<String>())
                    .unwrap_or_default()
            });

        let remaining_work = messages.iter().rev()
            .find(|m| m.role == "assistant")
            .and_then(|m| m.content.as_ref())
            .map(|c| c.chars().take(300).collect::<String>())
            .unwrap_or_else(|| "Continue working toward the goal.".to_string());

        let chain_depth = session.as_ref()
            .and_then(|s| s.rsplit(":chain:").next())
            .and_then(|d| d.parse::<usize>().ok())
            .unwrap_or(0);

        let checkpoint = ContinuationCheckpoint {
            session: session.clone().unwrap_or_else(|| "default".to_string()),
            summary,
            goal: goal.clone(),
            remaining_work,
            key_decisions: Vec::new(),
            total_steps: step,
            chain_depth: chain_depth + 1,
        };

        // Save checkpoint to file
        let checkpoint_dir = PathBuf::from("/root/.aethervault/workspace/checkpoints");
        let _ = fs::create_dir_all(&checkpoint_dir);
        let checkpoint_path = checkpoint_dir.join(format!(
            "{}.json",
            checkpoint.session.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
        ));
        if let Ok(json) = serde_json::to_string_pretty(&checkpoint) {
            let _ = fs::write(&checkpoint_path, &json);
        }

        let continuation_marker = format!(
            "[CONTINUATION_NEEDED:{}]",
            checkpoint_path.display()
        );

        return Ok(AgentRunOutput {
            session,
            context: context_pack,
            messages,
            tool_results,
            final_text: Some(continuation_marker),
        });
    }

    Ok(AgentRunOutput {
        session,
        context: context_pack,
        messages,
        tool_results,
        final_text,
    })
}

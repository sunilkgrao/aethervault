use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::memory_db::PutOptions;
use crate::consolidation::put_with_consolidation;
use chrono::Utc;
use rayon::ThreadPoolBuilder;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde_json;

use crate::claude::{call_agent_hook, call_claude, call_claude_with_model, call_critic};
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
    ToolExecution, BackgroundTaskRegistry, SessionRegistry,
    SessionTaint, FailureKind, classify_failure, LearnedFailure, detect_invisible_unicode,
    open_skill_db, list_skills, record_skill_use,
    match_skills_for_prompt, bootstrap_skills,
    prune_low_performing_skills, rebuild_fts5_index,
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

    // Auto-continuation loop: re-run the agent when it hits max_steps
    const MAX_CHAIN_DEPTH: usize = 5;
    let mut current_prompt = prompt_text;
    let mut current_session = session;
    let mut chain_depth: usize = 0;
    let output = loop {
        let output = run_agent_with_prompt(
            mv2.clone(),
            current_prompt.clone(),
            current_session.clone(),
            model_hook.clone(),
            system_text.clone(),
            no_memory,
            context_query.clone(),
            context_results,
            context_max_bytes,
            max_steps,
            log_commit_interval,
            log,
            None,
            None, // tool_filter: no restrictions for CLI agent
        )?;

        let needs_continuation = output.final_text.as_ref()
            .map(|t| t.contains("[CONTINUATION_NEEDED:"))
            .unwrap_or(false);

        if needs_continuation && chain_depth < MAX_CHAIN_DEPTH {
            // Parse checkpoint and build continuation prompt
            if let Some(ref text) = output.final_text {
                if let Some(start) = text.find("[CONTINUATION_NEEDED:") {
                    let after = &text[start + "[CONTINUATION_NEEDED:".len()..];
                    if let Some(end) = after.find(']') {
                        let checkpoint_path = &after[..end];
                        if let Ok(checkpoint_json) = fs::read_to_string(checkpoint_path) {
                            if let Ok(checkpoint) = serde_json::from_str::<ContinuationCheckpoint>(&checkpoint_json) {
                                chain_depth = checkpoint.chain_depth;
                                eprintln!(
                                    "[auto-continuation] chaining session (depth {}/{}): {}",
                                    chain_depth, MAX_CHAIN_DEPTH,
                                    checkpoint.goal.chars().take(80).collect::<String>()
                                );
                                current_prompt = format!(
                                    "[Continuation from previous session — chain depth {}/{}]\n\n\
                                     ## Goal\n{}\n\n\
                                     ## Summary of work so far\n{}\n\n\
                                     ## Remaining work\n{}\n\n\
                                     Continue from where you left off. Do NOT repeat completed work.",
                                    chain_depth, MAX_CHAIN_DEPTH,
                                    checkpoint.goal, checkpoint.summary, checkpoint.remaining_work,
                                );
                                current_session = current_session.map(|s| {
                                    if s.contains(":chain:") {
                                        let base = s.rsplit(":chain:").last().unwrap_or(&s);
                                        format!("{base}:chain:{chain_depth}")
                                    } else {
                                        format!("{s}:chain:{chain_depth}")
                                    }
                                });
                                continue; // Loop back for the next chain
                            }
                        }
                    }
                }
            }
        }

        break output;
    };

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
        "## Subagents (Background Tasks)",
        "You can spawn subagents dynamically with ANY name — choose names that describe the task (e.g., 'log-analyzer', 'api-tester', 'code-reviewer').",
        "Use subagent_invoke for single delegation, subagent_batch for parallel work.",
        "Subagents use a lighter-weight model, so they're good for heavy lifting while you orchestrate.",
        "When you need to delegate complex work, use subagent_invoke — tasks run in the background automatically.",
        "After spawning background tasks, tell the user what you started and finish your response.",
        "Do not wait for background results. The user can check /status anytime.",
        "When you see '[Background task completed]' steering messages, synthesize results concisely.",
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
        "When a task requires accounts, credentials, or setup you don't have — try to obtain them yourself.",
        "Use the browser tool to navigate dashboards, sign up, generate API keys. Check env vars, config files, CLI auth tools.",
        "Only involve the user after you've tried at least two approaches.",
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
        "After destructive operations (docker rebuild, db reset, rm, reinstall), re-verify all dependent functionality.",
        "Never assume prior test results still hold after a destructive operation — test again.",
        "When a process crashes or a service fails to start, IMMEDIATELY check logs yourself:",
        "- Docker: `docker logs <container>` or `docker compose logs --tail=50`",
        "- System: `journalctl -u <service> --no-pager -n 50`",
        "Do NOT ask the user to check logs for you. Diagnose first, report findings, then fix.",
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
        "",
        "## Behavioral Rules (Constitutional)",
        "These rules are non-negotiable. Follow them EVERY step:",
        "1. DIAGNOSE BEFORE RETRY: When something fails, investigate WHY before trying again. Check logs, read error messages, verify assumptions. Never retry the exact same action that just failed.",
        "2. QUOTE BEFORE CLAIMING: Before stating that something worked, quote the specific tool output that proves it. If the output shows an error or is empty, say so. Never claim success based on assumption.",
        "3. POLL SPARINGLY: For long-running tasks (training, builds, deployments), check status at most once every 5 minutes. Do not burn steps polling repeatedly.",
        "4. ESCALATE AFTER 2 FAILURES: If the same approach fails twice, try a fundamentally different strategy. If that also fails, report the situation clearly instead of continuing to brute-force.",
        "5. VERIFY MUTATIONS: After any state change (file write, API call, config change), verify the result with a read/check operation before moving on.",
        "6. PRE-FLIGHT ON REMOTE: When connecting to any remote machine (SSH, RunPod, cloud instance), ALWAYS run environment discovery first: df -h, nvidia-smi, python3 --version, echo $HF_HOME. Never assume a remote environment matches your expectations.",
        "7. READ BEFORE CALLING: Before using any API endpoint for the first time, read its documentation or schema. Never guess request payloads — verify the expected format first.",
        "",
        "## Project Build Protocol (Orchestrator/Coder Separation)",
        "You are the ORCHESTRATOR. You write prompts and delegate to coding agents. You do NOT write code directly.",
        "When building a full application or multi-file feature:",
        "1. ALWAYS use `swarm_create` first. This activates Orchestrator Mode — exec/fs_write are stripped.",
        "2. Decompose into parallel subtasks. Write DETAILED prompts for each coder (include file paths, expected behavior, test commands).",
        "3. Use `subagent_batch` with multiple swarm-coder agents. Set branch='swarm/{task-id}-{subtask}' for worktree isolation.",
        "4. The swarm monitor checks status every 60s automatically. You'll get injected updates — no need to poll.",
        "5. When CI passes, dispatch a cross-model reviewer (Codex coder → Claude reviewer, Claude coder → Codex reviewer).",
        "6. When a task FAILS: rewrite the prompt with failure context and spawn a new agent. Don't just retry blindly.",
        "7. DEFINITION OF DONE: PR created + CI passing + cross-model review passed + you verified with exec (tools restored when all tasks complete).",
        "8. If a build step (docker rebuild, db reset) destroys state, re-verify everything that depended on it.",
    ]
    .join("\n")
}

/// Estimate token count for messages (rough: chars / 4).
pub(crate) fn estimate_tokens(messages: &[AgentMessage]) -> usize {
    messages.iter().map(|m| {
        let content_chars = m.content.as_ref().map(|c| c.len()).unwrap_or(0);
        let tool_call_chars: usize = m.tool_calls.iter().map(|tc| {
            tc.name.len() + tc.id.len() + tc.args.to_string().len()
        }).sum();
        let thinking_chars: usize = m.thinking_blocks.iter().map(|tb| {
            tb.to_string().len()
        }).sum();
        // ~4 chars per token, plus per-message overhead (~20 tokens for role/structure)
        (content_chars + tool_call_chars + thinking_chars) / 4 + 20
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
/// Summarizes everything in between via a lightweight Sonnet call (no thinking).
/// Returns the extracted GOAL from the structured summary (if any).
pub(crate) fn compact_messages(
    messages: &mut Vec<AgentMessage>,
    _hook: &HookSpec,
    keep_recent: usize,
) -> Result<Option<String>, String> {
    if messages.len() <= keep_recent + 2 {
        return Ok(None); // Nothing to compact
    }
    // Preserve all leading system blocks (supports cache-split: stable prefix + dynamic suffix)
    let system_end = messages.iter().take_while(|m| m.role == "system").count();
    let mut summary_end = messages.len().saturating_sub(keep_recent);
    // Ensure we don't split in the middle of a tool_use→tool_result pair.
    // If `recent` would start with a "tool" role message, back up to include the
    // preceding assistant message with the corresponding tool_calls.
    while summary_end > system_end && summary_end < messages.len()
        && messages[summary_end].role == "tool"
    {
        summary_end = summary_end.saturating_sub(1);
    }
    let summary_start = system_end.min(summary_end);
    let system_msgs: Vec<_> = messages[..system_end].to_vec();
    let to_summarize: Vec<_> = messages[summary_start..summary_end].to_vec();
    let recent: Vec<_> = messages[summary_end..].to_vec();

    // Build summary text with hard cap to prevent the summarizer prompt itself from blowing up.
    // 150 chars/msg keeps it manageable; total capped at ~120K chars (~30K tokens).
    const PER_MSG_CHARS: usize = 150;
    const MAX_SUMMARY_CHARS: usize = 120_000;
    let mut summary_text = String::new();
    for m in &to_summarize {
        let role = &m.role;
        if let Some(c) = &m.content {
            let preview: String = c.chars().take(PER_MSG_CHARS).collect();
            let line = format!("[{role}] {preview}\n");
            if summary_text.len() + line.len() > MAX_SUMMARY_CHARS {
                summary_text.push_str("[... earlier messages truncated for compaction ...]\n");
                break;
            }
            summary_text.push_str(&line);
        }
    }

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

    // Use Sonnet directly for compaction — lightweight, no thinking, won't blow up on token limits
    let sonnet_model = env_optional("SONNET_MODEL")
        .unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string());
    let summary_request = AgentHookRequest {
        messages: vec![
            AgentMessage {
                role: "system".to_string(),
                content: Some("You are a conversation summarizer. Output only the structured summary, nothing else. Be concise.".to_string()),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
                thinking_blocks: vec![],
            },
            AgentMessage {
                role: "user".to_string(),
                content: Some(summary_prompt),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
                thinking_blocks: vec![],
            },
        ],
        tools: Vec::new(),
        session: None,
    };

    let summary_response = call_claude_with_model(&summary_request, Some(&sonnet_model))
        .map_err(|e| format!("compaction summarizer failed: {e}"))?;
    let summary = summary_response.message.content.unwrap_or_else(|| "(compaction failed)".to_string());

    // Extract the GOAL field from the structured summary
    let extracted_goal = summary.lines()
        .find(|line| line.starts_with("GOAL:"))
        .map(|line| line.trim_start_matches("GOAL:").trim().to_string());

    // Rebuild messages: system blocks + compaction notice + recent (thinking blocks stripped)
    *messages = system_msgs;
    messages.push(AgentMessage {
        role: "user".to_string(),
        content: Some(format!("[Context compacted. Summary of prior conversation:]\n{summary}")),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
        thinking_blocks: vec![],
    });
    messages.push(AgentMessage {
        role: "assistant".to_string(),
        content: Some("Understood. I have the context from the summary above. Continuing.".to_string()),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
        thinking_blocks: vec![],
    });
    // Strip thinking blocks from recent messages — they're huge and stale post-compaction
    for mut msg in recent {
        msg.thinking_blocks.clear();
        messages.push(msg);
    }
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
/// Returns `(is_error, tools_changed, deferred_messages)`.
/// Deferred messages (failure hints) must be pushed AFTER all tool results for the
/// current step to avoid breaking the tool_use→tool_result adjacency required by the API.
fn process_tool_result(
    call: &AgentToolCall,
    result: ToolExecution,
    tool_results: &mut Vec<AgentToolResult>,
    messages: &mut Vec<AgentMessage>,
    active_tools: &mut HashSet<String>,
    retrieved_skills: &mut Vec<String>,
    session_taint: &mut SessionTaint,
    should_log: bool,
    session: &Option<String>,
    log_dir: &Path,
) -> (bool, bool, Vec<AgentMessage>) {
    let is_error = result.is_error;
    let mut deferred: Vec<AgentMessage> = Vec::new();

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
        thinking_blocks: vec![],
    });

    // ── Taint tracking (Rule of Two / AgentArmor) ──
    // Mark session as having untrusted input when browser/http returns external content
    match call.name.as_str() {
        "browser" | "http_request" | "exa_search" => {
            session_taint.mark_untrusted(&call.name);
        }
        _ => {}
    }
    // Mark session as accessing private data when memory/files are read
    match call.name.as_str() {
        "memory_search" | "search" | "query" | "get" | "fs_read" | "skill_search" | "context" => {
            session_taint.mark_private_data();
        }
        _ => {}
    }

    // ── Invisible Unicode detection — scan ALL tool outputs for injection markers ──
    // Browser/HTTP outputs are already stripped by sanitize_external_content(), but exec,
    // fs_read, and other tool outputs are not. Detect and warn the LLM when invisible
    // chars are found — this is strong evidence of prompt injection.
    if let Some(warning) = detect_invisible_unicode(&result.output) {
        eprintln!("[security] invisible unicode detected in {} output", call.name);
        session_taint.mark_untrusted(&format!("{} (invisible unicode)", call.name));
        deferred.push(AgentMessage {
            role: "user".to_string(),
            content: Some(warning),
            tool_calls: Vec::new(),
            name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
        });
    }

    // ── Failure classification — defer retry hints until after all tool results ──
    if is_error {
        let failure_kind = classify_failure(&call.name, &result.output, &result.details);

        // Root-Cause Analysis: inject structured diagnostic questions before retry hints
        let rca_prompt = match call.name.as_str() {
            "exec" => {
                let exit_code = result.details.get("exit_code").and_then(|v| v.as_i64());
                Some(format!(
                    "[Root-Cause Analysis Required]\n\
                     The `exec` command failed{}.\n\
                     Before retrying, answer these questions:\n\
                     1. What EXACT error message was returned? Quote it.\n\
                     2. Is this a missing dependency, wrong path, permission issue, or syntax error?\n\
                     3. On a REMOTE machine, have you verified: disk space (df -h), available tools (which <cmd>), environment vars?\n\
                     4. What is ONE specific thing you will change before retrying?",
                    exit_code.map(|c| format!(" (exit code {c})")).unwrap_or_default()
                ))
            }
            "http_request" => {
                let status = result.details.get("status").and_then(|v| v.as_u64()).unwrap_or(0);
                Some(format!(
                    "[Root-Cause Analysis Required]\n\
                     HTTP request failed with status {status}.\n\
                     Before retrying, answer these questions:\n\
                     1. What does status {status} mean for THIS specific API?\n\
                     2. Did you READ the API docs/schema first, or are you guessing the endpoint/payload?\n\
                     3. Is the request body schema correct? Compare your payload against the documented schema.\n\
                     4. What is ONE specific thing you will change before retrying?"
                ))
            }
            _ => None,
        };
        if let Some(rca) = rca_prompt {
            deferred.push(AgentMessage {
                role: "user".to_string(),
                content: Some(rca),
                tool_calls: Vec::new(),
                name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
            });
        }

        match failure_kind {
            FailureKind::Transient => {
                deferred.push(AgentMessage {
                    role: "user".to_string(),
                    content: Some("[System] The previous tool call failed with a transient error (timeout, rate limit, or temporary unavailability). You may retry after a brief pause. Consider using a different approach if retry also fails.".to_string()),
                    tool_calls: Vec::new(),
                    name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
                });
            }
            FailureKind::Permanent => {
                deferred.push(AgentMessage {
                    role: "user".to_string(),
                    content: Some("[System] The previous tool call failed with a permanent error (unauthorized, not found, or invalid request). Do NOT retry the same call. Either fix the inputs, try a different approach, or ask the user for help.".to_string()),
                    tool_calls: Vec::new(),
                    name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
                });
            }
            FailureKind::ApiMisuse => {
                deferred.push(AgentMessage {
                    role: "user".to_string(),
                    content: Some(
                        "[System] API MISUSE DETECTED. Your request was rejected because the schema/parameters are WRONG. \
                         Do NOT retry with the same payload. You MUST:\n\
                         1. READ the API documentation or schema (use http_request GET on the docs endpoint, or search for the API spec)\n\
                         2. Compare your request against the documented schema\n\
                         3. Fix the specific validation error before retrying\n\
                         NEVER guess API schemas. Always read docs first.".to_string()
                    ),
                    tool_calls: Vec::new(),
                    name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
                });
            }
            FailureKind::Semantic => {
                // Let the LLM figure it out — no additional hint needed
            }
        }
    }

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

    (is_error, tools_changed, deferred)
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
    tool_filter: Option<Vec<String>>,
) -> Result<AgentRunOutput, Box<dyn std::error::Error>> {
    if prompt_text.trim().is_empty() {
        return Err("agent prompt is empty".into());
    }

    // One-time capsule size check at session start
    check_capsule_health(&mv2);

    let db = open_or_create_db(&mv2)?;

    // Try flat file config first (workspace/config.json), fall back to capsule.
    let workspace_env = resolve_workspace(None, &crate::AgentConfig::default());
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

    let mut injected_skill_names: Vec<String> = Vec::new();
    let mut swarm_skill_matched = false;
    // --- SkillRL R1: Auto-inject top skills into stable prefix ---
    if let Some(workspace) = resolve_workspace(None, &agent_cfg) {
        let db_path = workspace.join("skills.sqlite");
        if let Ok(conn) = open_skill_db(&db_path) {
            // Bootstrap essential skills on first run
            bootstrap_skills(&conn);

            // Prune skills with <30% success rate after 5+ uses
            let pruned = prune_low_performing_skills(&conn, 5, 0.3);
            if pruned > 0 {
                rebuild_fts5_index(&conn);
            }

            // Match skills against session context (not just current message)
            // so follow-up messages like "try again" still match earlier topics.
            let match_context = if let Some(ref sess_id) = session {
                let turns = load_session_turns(sess_id, 20);
                let recent: String = turns.iter().rev().take(6)
                    .map(|t| t.content.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{} {}", recent, &prompt_text)
            } else {
                prompt_text.clone()
            };
            let matched = match_skills_for_prompt(&conn, &match_context, 5);
            // Also get top general skills by success rate
            let general = list_skills(&conn, 3);

            let mut seen: HashSet<String> = HashSet::new();
            let mut skill_block = String::new();
            let mut inline_count = 0usize;
            // Inline full steps for top 3 matched skills; one-liner for the rest
            for s in matched.iter().chain(general.iter()) {
                if !seen.insert(s.name.clone()) { continue; }
                let is_matched = matched.iter().any(|m| m.name == s.name);
                if is_matched && inline_count < 3 && !s.steps.is_empty() {
                    // Full inline expansion
                    skill_block.push_str(&format!("### {}", s.name));
                    if let Some(ref desc) = s.description {
                        skill_block.push_str(&format!(" — {}", desc));
                    }
                    if s.times_used > 0 {
                        skill_block.push_str(&format!(" ({:.0}% success)", s.success_rate * 100.0));
                    }
                    skill_block.push('\n');
                    if let Some(ref trigger) = s.trigger {
                        skill_block.push_str(&format!("**When:** {}\n", trigger));
                    }
                    skill_block.push_str("**Steps:**\n");
                    for (i, step) in s.steps.iter().enumerate() {
                        skill_block.push_str(&format!("{}. {}\n", i + 1, step));
                    }
                    if !s.tools.is_empty() {
                        skill_block.push_str(&format!("**Tools:** {}\n", s.tools.join(", ")));
                    }
                    if let Some(ref notes) = s.notes {
                        if !notes.is_empty() {
                            skill_block.push_str(&format!("**Notes:** {}\n", notes));
                        }
                    }
                    skill_block.push('\n');
                    inline_count += 1;
                } else {
                    // One-liner summary
                    skill_block.push_str(&format!("- **{}**", s.name));
                    if let Some(ref desc) = s.description {
                        skill_block.push_str(&format!(": {}", desc));
                    } else if let Some(ref trigger) = s.trigger {
                        skill_block.push_str(&format!(" — {}", trigger));
                    }
                    if !s.contexts.is_empty() {
                        skill_block.push_str(&format!(" [{}]", s.contexts.join(", ")));
                    }
                    if s.times_used > 0 {
                        skill_block.push_str(&format!(" ({:.0}% success)", s.success_rate * 100.0));
                    }
                    skill_block.push('\n');
                }
            }
            // Track auto-injected skills for SkillRL R4 end-of-session recording
            for s in matched.iter().chain(general.iter()) {
                if seen.contains(&s.name) {
                    injected_skill_names.push(s.name.clone());
                }
            }

            // Detect swarm-dev-task skill match for proactive orchestrator enforcement
            if matched.iter().any(|s| s.name == "bootstrap:swarm-dev-task") {
                swarm_skill_matched = true;
            }

            if !skill_block.is_empty() {
                system_prompt.push_str("\n\n# Available Procedures\n");
                if inline_count > 0 {
                    system_prompt.push_str("Follow the steps below directly when the procedure matches. For other procedures, call `skill_search` with its name to load full steps.\n\n");
                } else {
                    system_prompt.push_str("You have access to these proven procedures. To use one, call `skill_search` with its name to load the full steps.\n\n");
                }
                system_prompt.push_str(&skill_block);
                system_prompt.push_str("\nWhen you need a credential, API key, or account you don't have:\n1. Check env vars and config files first\n2. Try the browser tool to navigate the service's dashboard\n3. Only ask the user as a last resort — give them the exact URL and key name\n\nYou are resourceful. Figure things out. Use your tools creatively. Don't stop at the first obstacle.\n");
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
        thinking_blocks: vec![],
    });
    if !system_dynamic.trim().is_empty() {
        messages.push(AgentMessage {
            role: "system".to_string(),
            content: Some(system_dynamic),
            tool_calls: Vec::new(),
            name: None,
            tool_call_id: None,
            is_error: None,
            thinking_blocks: vec![],
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
                thinking_blocks: vec![],
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
        thinking_blocks: vec![],
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
                    timeout_secs: None,
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

    // Runtime tool enforcement: if tool_filter is set, strip tools not in the allowlist.
    // This is the API-level enforcement — tools not in the list are never sent to the model.
    if let Some(ref filter) = tool_filter {
        let allowed: std::collections::HashSet<&str> = filter.iter().map(|s| s.as_str()).collect();
        full_catalog.retain(|t| {
            t.get("name")
                .and_then(|n| n.as_str())
                .map(|name| allowed.contains(name))
                .unwrap_or(false)
        });
        eprintln!("[harness] tool_filter active: {} tools allowed (of {} in filter)", full_catalog.len(), allowed.len());
    }

    let tool_map = tool_catalog_map(&full_catalog);
    let mut active_tools = base_tool_names();
    // Add MCP tool names to active set
    if let Some(ref registry) = mcp_registry {
        for name in registry.route_map.keys() {
            active_tools.insert(name.clone());
        }
    }
    // If tool_filter is active, also restrict active_tools to the filter
    if let Some(ref filter) = tool_filter {
        let allowed: std::collections::HashSet<String> = filter.iter().cloned().collect();
        active_tools.retain(|t| allowed.contains(t));
    }
    // --- Proactive Orchestrator Enforcement ---
    // When the swarm-dev-task skill matched the user's prompt AND this is the main agent
    // (not a subagent), strip exec/fs_write BEFORE the first LLM call.
    // This makes it structurally impossible for the orchestrator to code directly —
    // it MUST delegate to swarm-coder agents.
    let is_subagent_early = session.as_deref().map(|s| s.starts_with("subagent:")).unwrap_or(false);
    if swarm_skill_matched && !is_subagent_early && tool_filter.is_none() {
        active_tools.remove("exec");
        active_tools.remove("fs_write");
        eprintln!("[harness] PROACTIVE ORCHESTRATOR: swarm-dev-task skill matched — exec/fs_write stripped before first response");
        // Inject explicit notice so the model never attempts exec/fs_write at step 0.
        // Without this, models hallucinate tool calls for tools not in their tool list.
        messages.push(AgentMessage {
            role: "user".to_string(),
            content: Some(
                "[IMPORTANT — TOOL AVAILABILITY] The tools `exec` and `fs_write` are NOT available to you in this session. \
                 They have been removed from your tool list. Do NOT attempt to call them — they will fail. \
                 To accomplish coding tasks, you MUST use `swarm_create` to register tasks and `subagent_batch` \
                 with name='swarm-coder' to spawn coding agents that have exec/fs_write access. \
                 Start by analyzing the situation with your available tools (browser, http_request, swarm_list, etc.), \
                 then delegate coding work to swarm agents."
                .to_string()
            ),
            tool_calls: Vec::new(),
            name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
        });
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
            // Carry forward learned failures (cap at 20, FIFO)
            drift_state.learned_failures = persisted.learned_failures;
            if drift_state.learned_failures.len() > 20 {
                drift_state.learned_failures = drift_state.learned_failures
                    .split_off(drift_state.learned_failures.len() - 20);
            }
            if !drift_state.learned_failures.is_empty() {
                eprintln!("[drift] loaded {} learned failures from previous sessions",
                    drift_state.learned_failures.len());
            }
            let prev_count = persisted.violations.get("critic_correction").copied().unwrap_or(0);
            eprintln!("[drift] loaded {prev_count} persisted violations (reset to 0 for new session)");
        }
    }
    let mut recent_actions: VecDeque<String> = VecDeque::with_capacity(30);
    let mut retrieved_skills: Vec<String> = Vec::new();
    retrieved_skills.extend(injected_skill_names);
    let mut session_taint = SessionTaint::default();
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

    // Extract background task registry from progress (if running via bridge)
    let bg_registry_ref: Option<(i64, Arc<Mutex<BackgroundTaskRegistry>>)> = progress.as_ref().and_then(|p| {
        let guard = p.lock().ok()?;
        Some((guard.chat_id?, guard.bg_registry.clone()?))
    });

    // Extract session registry from progress (if running via bridge)
    let session_registry_ref: Option<Arc<Mutex<SessionRegistry>>> = progress.as_ref().and_then(|p| {
        p.lock().ok().and_then(|g| g.session_registry.clone())
    });

    // --- Orchestrator Mode ---
    // When the main agent (not a subagent) has active swarm tasks, enter orchestrator mode:
    // strip exec and fs_write so the orchestrator can only plan, delegate, and verify.
    // This enforces the OpenClaw pattern: orchestrator writes prompts, not code.
    let is_subagent = session.as_deref().map(|s| s.starts_with("subagent:")).unwrap_or(false);
    let mut orchestrator_mode = swarm_skill_matched && !is_subagent_early && tool_filter.is_none();
    if orchestrator_mode {
        eprintln!("[harness] ORCHESTRATOR MODE (proactive): swarm-dev-task skill matched, tools already stripped");
    }
        if !is_subagent && tool_filter.is_none() {
            if let Some(ref ws) = workspace_env {
                if let Ok(sdb) = crate::swarm::open_swarm_db(ws) {
                    let running = crate::swarm::swarm_list_tasks(&sdb, Some("running"), Some(100));
                    let queued = crate::swarm::swarm_list_tasks(&sdb, Some("queued"), Some(100));
                    let pr_open = crate::swarm::swarm_list_tasks(&sdb, Some("pr_open"), Some(100));
                    let reviewing = crate::swarm::swarm_list_tasks(&sdb, Some("reviewing"), Some(100));
                    let active_count = running.len() + queued.len() + pr_open.len() + reviewing.len();
                    if active_count > 0 {
                        orchestrator_mode = true;
                        // Strip direct coding tools — orchestrator delegates, doesn't code
                        active_tools.remove("exec");
                        active_tools.remove("fs_write");
                        tools = tools_from_active(&tool_map, &active_tools);
                        eprintln!("[harness] ORCHESTRATOR MODE: {} active swarm tasks, exec/fs_write stripped", active_count);
                        // Inject orchestrator mode notification
                        let task_summary: Vec<String> = running.iter().chain(queued.iter())
                            .chain(pr_open.iter())
                            .chain(reviewing.iter())
                            .map(|t| format!("  - {} ({}): {}", t.id, t.status.as_str(), t.name))
                            .collect();
                        messages.push(AgentMessage {
                        role: "user".to_string(),
                        content: Some(format!(
                            "[System] ORCHESTRATOR MODE ACTIVE. You have {} coding agents working. \
                             You cannot use exec or fs_write — delegate all coding to swarm agents. \
                             Your role: write prompts, monitor progress, verify results, dispatch reviews.\n\
                             Active tasks:\n{}",
                            active_count, task_summary.join("\n")
                        )),
                        tool_calls: Vec::new(),
                        name: None,
                        tool_call_id: None,
                        is_error: None,
                        thinking_blocks: vec![],
                    });
                }
            }
        }
    }

    // --- Deterministic Swarm Monitor ---
    // Track when we last checked swarm status. Check every 60s (deterministic, not LLM-driven).
    let swarm_monitor_interval = std::time::Duration::from_secs(60);
    let mut last_swarm_check = std::time::Instant::now();

    let mut completed = false;
    let mut current_max_steps = effective_max_steps;
    let mut step = 0;
    let session_start = std::time::Instant::now();
    const SESSION_HARD_TIMEOUT_SECS: u64 = 600; // 10 minute hard ceiling
    let mut wrap_up_injected = false;
    let mut consecutive_hook_failures: usize = 0;
    const MAX_CONSECUTIVE_HOOK_FAILURES: usize = 3;
    let mut subagent_tools_restricted = false;
    let mut orchestrator_blocked_count: usize = 0;
    // Circuit breaker: track tool+args failure counts. After 3 identical failures,
    // block the call and force the agent to try a different approach.
    let mut tool_failure_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    // Failed attempts scratchpad: track what was tried and why it failed.
    // Injected periodically so the agent doesn't repeat the same mistakes.
    let mut failed_attempts: Vec<String> = Vec::new();

    // Inject learned failures from previous sessions as context
    if !drift_state.learned_failures.is_empty() {
        let lessons: Vec<String> = drift_state.learned_failures.iter()
            .map(|lf| format!("- {}: {} → {}", lf.tool, lf.pattern, lf.lesson))
            .collect();
        messages.push(AgentMessage {
            role: "user".to_string(),
            content: Some(format!(
                "[Lessons from Previous Sessions]\n\
                 These patterns caused repeated failures before. Do NOT repeat them:\n{}",
                lessons.join("\n")
            )),
            tool_calls: Vec::new(),
            name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
        });
    }

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
                        thinking_blocks: vec![],
                    });
                }
            }
        }

        // Hard wall-clock timeout: prevent infinite sessions
        if session_start.elapsed().as_secs() > SESSION_HARD_TIMEOUT_SECS {
            let mins = session_start.elapsed().as_secs() / 60;
            eprintln!("[harness] SESSION TIMEOUT after {mins}m — forcing wrap-up");
            if !wrap_up_injected {
                wrap_up_injected = true;
                messages.push(AgentMessage {
                    role: "user".to_string(),
                    content: Some(
                        "[SYSTEM: SESSION TIMEOUT] You have exceeded the maximum session time. \
                         Provide your best answer NOW with whatever information you have. \
                         Do NOT make any more tool calls. Respond directly.".to_string()
                    ),
                    tool_calls: Vec::new(),
                    name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
                });
                // Give the LLM one more turn to respond, then the step limit will end it
                current_max_steps = step + 2;
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
                        thinking_blocks: vec![],
                    });
                }
            }
        }

        // --- Deterministic Swarm Monitor: periodic check ---
        // Every 60s, check swarm task status via gh CLI (deterministic, not LLM-driven).
        // Inject status updates + action instructions as system messages.
        if !is_subagent && last_swarm_check.elapsed() >= swarm_monitor_interval {
            last_swarm_check = std::time::Instant::now();
            if let Some(ref ws) = workspace_env {
                if let Ok(sdb) = crate::swarm::open_swarm_db(ws) {
                    let check_result = crate::swarm::swarm_check_open_tasks(&sdb);
                    if !check_result.contains("No open PR tasks") && !check_result.contains("no status changes") {
                        eprintln!("[swarm-monitor] {}", check_result);
                        messages.push(AgentMessage {
                            role: "user".to_string(),
                            content: Some(format!("[SWARM MONITOR — automatic, deterministic check]\n{check_result}")),
                            tool_calls: Vec::new(),
                            name: None,
                            tool_call_id: None,
                            is_error: None,
                            thinking_blocks: vec![],
                        });
                    }
                    // Check for failed tasks that need retry (smart retry with failure classification)
                    let failed = crate::swarm::swarm_list_tasks(&sdb, Some("failed"), Some(50));
                    for task in &failed {
                        if task.retry_count < task.max_retries {
                            let error_ctx = task.error_context.as_deref().unwrap_or("unknown error");
                            let failure_kind = classify_failure("swarm_task", error_ctx, &serde_json::json!({}));
                            let strategy = match failure_kind {
                                FailureKind::Transient =>
                                    "STRATEGY: Transient error (timeout/rate-limit). Retry with same approach but add \
                                     error handling or timeout extension in the prompt.",
                                FailureKind::Permanent =>
                                    "STRATEGY: Permanent error (auth/permission/not-found). Do NOT retry the same approach. \
                                     Investigate root cause first, then try a fundamentally different method. \
                                     If the task is impossible, mark it done with an explanation.",
                                FailureKind::ApiMisuse =>
                                    "STRATEGY: API misuse (wrong request shape/schema). The request payload doesn't match the API spec. \
                                     Rewrite the prompt to include the EXACT API schema. Tell the agent to read the API docs first, \
                                     then construct the request. Include the validation error so the agent knows what field is wrong.",
                                FailureKind::Semantic =>
                                    "STRATEGY: Logic/parsing error. Rewrite the prompt with more specific instructions. \
                                     Include the exact error so the new agent avoids the same mistake.",
                            };
                            let retry_prompt = format!(
                                "[SWARM MONITOR — RETRY NEEDED]\n\
                                 Task '{}' (id: {}) FAILED (attempt {}/{}) | Type: {:?}\n\
                                 Error: {}\n\n\
                                 {}\n\n\
                                 Original prompt (first 500 chars): {}",
                                task.name, task.id, task.retry_count + 1, task.max_retries,
                                failure_kind, error_ctx, strategy,
                                task.prompt.chars().take(500).collect::<String>()
                            );
                            messages.push(AgentMessage {
                                role: "user".to_string(),
                                content: Some(retry_prompt),
                                tool_calls: Vec::new(),
                                name: None,
                                tool_call_id: None,
                                is_error: None,
                                thinking_blocks: vec![],
                            });
                            eprintln!("[swarm-monitor] injected retry for task {} (failure: {:?})", task.id, failure_kind);
                        }
                    }
                    // Check for CI-passing tasks that need cross-model review
                    let reviewing = crate::swarm::swarm_list_tasks(&sdb, Some("reviewing"), Some(50));
                    for task in &reviewing {
                        if task.review_status.as_deref() == Some("pending") || task.review_status.is_none() {
                            if let Some(pr_num) = task.pr_number {
                                let backend = task.agent_backend.as_deref().unwrap_or("unknown");
                                let reviewer = if backend.contains("codex") {
                                    "swarm-reviewer-claude"
                                } else {
                                    "swarm-reviewer-codex"
                                };
                                messages.push(AgentMessage {
                                    role: "user".to_string(),
                                    content: Some(format!(
                                        "[SWARM MONITOR — REVIEW DISPATCH NEEDED]\n\
                                         Task '{}' (PR #{}) has CI passing but no review yet.\n\
                                         ACTION REQUIRED: Dispatch '{}' to review PR #{} via subagent_invoke.",
                                        task.name, pr_num, reviewer, pr_num
                                    )),
                                    tool_calls: Vec::new(),
                                    name: None,
                                    tool_call_id: None,
                                    is_error: None,
                                    thinking_blocks: vec![],
                                });
                                eprintln!("[swarm-monitor] injected review dispatch for task {} (PR #{})", task.id, pr_num);
                            }
                        }
                    }
                    // Re-check orchestrator mode: if all tasks done, restore tools.
                    // BUT: if orchestrator was activated proactively (skill match), only restore
                    // when tasks were actually created AND completed — not just because the DB
                    // has no active tasks (which is the initial state).
                    if orchestrator_mode {
                        let running = crate::swarm::swarm_list_tasks(&sdb, Some("running"), Some(1));
                        let queued = crate::swarm::swarm_list_tasks(&sdb, Some("queued"), Some(1));
                        let pr_open = crate::swarm::swarm_list_tasks(&sdb, Some("pr_open"), Some(1));
                        let reviewing = crate::swarm::swarm_list_tasks(&sdb, Some("reviewing"), Some(1));
                        let no_active = running.is_empty() && queued.is_empty() && pr_open.is_empty() && reviewing.is_empty();
                        // When proactively enforced via skill match, never auto-restore tools.
                        // The agent must complete its swarm tasks; tools are restored only
                        // when orchestrator mode was triggered by existing DB tasks (not skill match).
                        let can_restore = no_active;
                        if can_restore {
                            orchestrator_mode = false;
                            active_tools = base_tool_names();
                            if let Some(ref registry) = mcp_registry {
                                for name in registry.route_map.keys() {
                                    active_tools.insert(name.clone());
                                }
                            }
                            tools = tools_from_active(&tool_map, &active_tools);
                            eprintln!("[harness] ORCHESTRATOR MODE OFF: all swarm tasks complete, full tools restored");
                            messages.push(AgentMessage {
                                role: "user".to_string(),
                                content: Some("[System] All swarm tasks complete. Full tool access restored. You can now verify results directly with exec, curl, docker logs, etc.".to_string()),
                                tool_calls: Vec::new(),
                                name: None,
                                tool_call_id: None,
                                is_error: None,
                                thinking_blocks: vec![],
                            });
                        }
                    }
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
                    thinking_blocks: vec![],
                }
            }
        };
        if let Some(content) = message.content.clone() {
            final_text = Some(content.clone());
            // Update progress: text preview + last_output for session status
            if let Some(ref prog) = progress {
                if let Ok(mut p) = prog.lock() {
                    p.text_preview = Some(content.chars().take(100).collect());
                    if let Some(ref lo) = p.last_output {
                        if let Ok(mut out) = lo.lock() {
                            *out = Some(content.clone());
                        }
                    }
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
                                    thinking_blocks: vec![],
                                },
                                AgentMessage {
                                    role: "user".to_string(),
                                    content: Some(format!("Extract stable facts from:\n{snapshot}")),
                                    tool_calls: Vec::new(),
                                    name: None,
                                    tool_call_id: None,
                                    is_error: None,
                                    thinking_blocks: vec![],
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
        let mut tool_calls = message.tool_calls.clone();

        // Block ungrounded subagent status claims (phantom status detection)
        if let Some(ref content) = message.content {
            let lower = content.to_lowercase();
            let claims_subagent_success = lower.contains("completed successfully")
                || lower.contains("finished processing")
                || lower.contains("results are ready");
            let has_recent_status_check = tool_calls.iter().any(|c|
                c.name == "session_status" || c.name.contains("subagent") || c.name == "bg_status"
            );
            if claims_subagent_success && !has_recent_status_check {
                messages.push(AgentMessage {
                    role: "user".to_string(),
                    content: Some("[Grounding Violation — Phantom Status] You claimed subagent results without checking status first. You MUST call session_status or check background task results BEFORE reporting subagent outcomes. Retract your previous claim and check actual status now.".to_string()),
                    tool_calls: Vec::new(),
                    name: None,
                    tool_call_id: None,
                    is_error: None,
                    thinking_blocks: vec![],
                });
            }
        }

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

        // ── Orchestrator Mode: Block exec/fs_write at dispatch level ──
        // Belt-and-suspenders: even if tools leak back into active_tools,
        // block them here with a clear error message directing the agent to use swarm tools.
        if orchestrator_mode && !is_subagent_early {
            let orchestrator_blocked: Vec<String> = tool_calls.iter()
                .filter(|c| matches!(c.name.as_str(), "exec" | "fs_write"))
                .map(|c| c.name.clone())
                .collect();
            if !orchestrator_blocked.is_empty() {
                let mut has_non_blocked = false;
                for call in &tool_calls {
                    if orchestrator_blocked.contains(&call.name) {
                        tool_results.push(AgentToolResult {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            output: format!(
                                "BLOCKED: ORCHESTRATOR MODE is active. You CANNOT use {} directly. \
                                 Use swarm_create to register tasks, then subagent_batch with swarm-coder agents \
                                 to execute them. Your role is to orchestrate, not code.",
                                call.name
                            ),
                            details: serde_json::json!({ "blocked": true, "reason": "orchestrator_mode" }),
                            is_error: true,
                        });
                        messages.push(AgentMessage {
                            role: "tool".to_string(),
                            content: Some(format!(
                                "BLOCKED: ORCHESTRATOR MODE — {} is disabled. \
                                 You must delegate coding to swarm-coder agents via swarm_create + subagent_batch. \
                                 You cannot write code or run commands directly.",
                                call.name
                            )),
                            tool_calls: Vec::new(),
                            name: Some(call.name.clone()),
                            tool_call_id: Some(call.id.clone()),
                            is_error: Some(true),
                            thinking_blocks: vec![],
                        });
                        eprintln!("[orchestrator] BLOCKED {} — agent tried to bypass orchestrator mode", call.name);
                    } else {
                        has_non_blocked = true;
                    }
                }
                if !has_non_blocked {
                    // All calls were blocked — inject orchestrator reminder and retry without
                    // burning a step. The agent gets ONE free pass (the model may hallucinate
                    // exec despite it not being in the tool list). On repeated offenses, count
                    // the step so we don't loop forever.
                    orchestrator_blocked_count += 1;
                    messages.push(AgentMessage {
                        role: "user".to_string(),
                        content: Some(
                            "[ORCHESTRATOR MODE] You are in orchestrator mode. exec and fs_write are DISABLED. \
                             To fix code issues, you MUST:\n\
                             1. Use swarm_create to register each task\n\
                             2. Use subagent_batch with name='swarm-coder' to spawn coding agents\n\
                             3. Each agent gets a branch parameter for isolation\n\
                             4. Monitor with swarm_list, verify results when done\n\
                             Do NOT attempt exec or fs_write again — they will always be blocked."
                            .to_string()
                        ),
                        tool_calls: Vec::new(),
                        name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
                    });
                    if orchestrator_blocked_count > 1 {
                        step += 1; // Only count as a step on repeat offenses
                    }
                    continue;
                }
                tool_calls.retain(|c| !orchestrator_blocked.contains(&c.name));
            }
        }

        // ── Rule of Two: Block exfil tools when session is tainted ──
        // When both untrusted input AND private data are present, block
        // external communication tools to prevent data exfiltration.
        if session_taint.is_tainted() {
            let exfil_blocked: Vec<String> = tool_calls.iter()
                .filter(|c| matches!(c.name.as_str(),
                    "exec" | "email_send" | "gmail_send" | "signal_send" | "imessage_send" | "notify"
                ) || (c.name == "http_request" && {
                    let method = c.args.get("method").and_then(|v| v.as_str()).unwrap_or("GET").to_ascii_uppercase();
                    method != "GET"
                }))
                .map(|c| c.name.clone())
                .collect();
            if !exfil_blocked.is_empty() {
                let sources = session_taint.untrusted_sources.join(", ");
                // Push tool_result messages for blocked calls FIRST (maintains adjacency)
                let mut has_non_blocked = false;
                for call in &tool_calls {
                    if exfil_blocked.contains(&call.name) {
                        tool_results.push(AgentToolResult {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            output: format!("BLOCKED: Session taint — untrusted input ({sources}) + private data. Ask user for approval."),
                            details: serde_json::json!({ "blocked": true, "reason": "session_taint" }),
                            is_error: true,
                        });
                        messages.push(AgentMessage {
                            role: "tool".to_string(),
                            content: Some(format!("BLOCKED: Session is tainted (untrusted input from {sources} + private data accessed). This tool call was blocked to prevent potential data exfiltration. Ask the user for explicit approval if this action is intended.")),
                            tool_calls: Vec::new(),
                            name: Some(call.name.clone()),
                            tool_call_id: Some(call.id.clone()),
                            is_error: Some(true),
                            thinking_blocks: vec![],
                        });
                    } else {
                        has_non_blocked = true;
                    }
                }
                // If ALL calls were blocked, push security notice AFTER tool results, then skip
                if !has_non_blocked {
                    messages.push(AgentMessage {
                        role: "user".to_string(),
                        content: Some(format!(
                            "[SECURITY] This session has ingested untrusted external content (from: {sources}) \
                             AND accessed private data. All requested tools ({}) were blocked to prevent data exfiltration. \
                             If you need to use these tools, explain to the user what you intend to send and why, \
                             and ask for explicit approval.",
                            exfil_blocked.join(", ")
                        )),
                        tool_calls: Vec::new(),
                        name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
                    });
                    step += 1;
                    continue;
                }
                // Filter out blocked calls and continue with the rest
                tool_calls.retain(|c| !exfil_blocked.contains(&c.name));
            }
        }

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

        // ── Circuit Breaker: block identical tool+args that have failed 3+ times ──
        {
            let mut circuit_broken = Vec::new();
            for call in &tool_calls {
                let key = format!("{}:{:x}", call.name, {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    call.args.to_string().hash(&mut h);
                    h.finish()
                });
                if let Some(&count) = tool_failure_counts.get(&key) {
                    if count >= 3 {
                        circuit_broken.push((call.clone(), key));
                    }
                }
            }
            if !circuit_broken.is_empty() {
                for (call, key) in &circuit_broken {
                    let count = tool_failure_counts.get(key).copied().unwrap_or(0);
                    let blocked_msg = format!(
                        "CIRCUIT BREAKER: `{}` with these arguments has failed {} times. \
                         This exact call is BLOCKED. You must try a fundamentally different approach: \
                         different tool, different arguments, or investigate the root cause first.",
                        call.name, count
                    );
                    tool_results.push(AgentToolResult {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        output: blocked_msg.clone(),
                        details: serde_json::json!({ "circuit_breaker": true, "failures": count }),
                        is_error: true,
                    });
                    messages.push(AgentMessage {
                        role: "tool".to_string(),
                        content: Some(blocked_msg),
                        tool_calls: Vec::new(),
                        name: Some(call.name.clone()),
                        tool_call_id: Some(call.id.clone()),
                        is_error: Some(true),
                        thinking_blocks: vec![],
                    });
                    eprintln!("[circuit-breaker] blocked {}:{} after {count} failures", call.name, &key[call.name.len()+1..std::cmp::min(key.len(), call.name.len()+9)]);

                    // Extract a learned failure lesson when circuit breaker triggers
                    let pattern_detail = match call.name.as_str() {
                        "exec" => call.args.get("command")
                            .and_then(|v| v.as_str())
                            .map(|s| s.chars().take(120).collect::<String>())
                            .unwrap_or_else(|| call.args.to_string().chars().take(80).collect()),
                        "http_request" => {
                            let method = call.args.get("method").and_then(|v| v.as_str()).unwrap_or("?");
                            let url = call.args.get("url").and_then(|v| v.as_str()).unwrap_or("?");
                            format!("{} {}", method, url.chars().take(100).collect::<String>())
                        }
                        _ => call.args.to_string().chars().take(80).collect(),
                    };
                    let lesson = LearnedFailure {
                        tool: call.name.clone(),
                        pattern: pattern_detail,
                        lesson: format!("Failed {} times and was blocked by circuit breaker. Try a fundamentally different approach or verify prerequisites.", count),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    };
                    if !drift_state.learned_failures.iter().any(|lf| lf.tool == lesson.tool && lf.pattern == lesson.pattern) {
                        drift_state.learned_failures.push(lesson);
                    }
                }
                let broken_ids: std::collections::HashSet<String> = circuit_broken.iter().map(|(c, _)| c.id.clone()).collect();
                tool_calls.retain(|c| !broken_ids.contains(&c.id));
                if tool_calls.is_empty() {
                    messages.push(AgentMessage {
                        role: "user".to_string(),
                        content: Some("[Circuit Breaker] All attempted tool calls were blocked due to repeated failures. \
                            You MUST change your approach. Review what has failed (see failed attempts below) and try something different.".to_string()),
                        tool_calls: Vec::new(),
                        name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
                    });
                    step += 1;
                    continue;
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
                    bg_registry_ref.clone(),
                    session_registry_ref.clone(),
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
            let (is_error, tools_changed, deferred_msgs) = process_tool_result(
                call, result,
                &mut tool_results, &mut messages, &mut active_tools,
                &mut retrieved_skills, &mut session_taint,
                should_log, &session, &log_dir,
            );
            if tools_changed {
                // Re-strip orchestrator-blocked tools if they leaked back via tool_search
                if orchestrator_mode && !is_subagent_early {
                    active_tools.remove("exec");
                    active_tools.remove("fs_write");
                }
                tools = tools_from_active(&tool_map, &active_tools);
            }

            // Push deferred messages (failure hints) AFTER the tool result
            // Safe here because single-call path has only one tool_use/tool_result pair
            messages.extend(deferred_msgs);

            // Detect SSH connection to remote host
            if call.name == "exec" && !is_error && !reminder_state.remote_host_seen {
                let args_str = call.args.to_string().to_lowercase();
                if args_str.contains("ssh ") || args_str.contains("scp ") {
                    reminder_state.remote_host_seen = true;
                    messages.push(AgentMessage {
                        role: "user".to_string(),
                        content: Some(
                            "[System: Remote Environment Detected]\n\
                             You just connected to a remote machine. BEFORE doing any real work, run these discovery commands:\n\
                             1. `df -h` — check available disk space (especially /tmp and working directories)\n\
                             2. `nvidia-smi` or equivalent — check GPU availability and memory\n\
                             3. `which python3 && python3 --version` — verify runtime availability\n\
                             4. `echo $HF_HOME $CUDA_HOME` — check critical environment variables\n\
                             5. `free -h` — check available RAM\n\
                             Do NOT skip this step. Many failures come from wrong assumptions about the remote environment.".to_string()
                        ),
                        tool_calls: Vec::new(),
                        name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
                    });
                }
            }
            // Track env verification
            if call.name == "exec" && !is_error && reminder_state.remote_host_seen {
                if let Some(last_result) = tool_results.last() {
                    let output_lower = last_result.output.to_lowercase();
                    if output_lower.contains("filesystem") || output_lower.contains("nvidia-smi") || output_lower.contains("mem:") {
                        reminder_state.remote_env_verified = true;
                    }
                }
            }

            // Inject grounding requirement after subagent-related tool results
            if call.name == "subagent_invoke" || call.name == "subagent_batch" || call.name == "session_status" {
                messages.push(AgentMessage {
                    role: "user".to_string(),
                    content: Some("[Grounding Rule] You just received subagent results. When reporting these to the user, you MUST directly quote the output text above. Do NOT paraphrase, embellish, or add details not present in the tool output. If the output shows errors or empty results, report that honestly.".to_string()),
                    tool_calls: Vec::new(),
                    name: None,
                    tool_call_id: None,
                    is_error: None,
                    thinking_blocks: vec![],
                });
            }

            // Circuit breaker: track failure counts per tool+args
            {
                let cb_key = format!("{}:{:x}", call.name, {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    call.args.to_string().hash(&mut h);
                    h.finish()
                });
                if is_error {
                    let count = tool_failure_counts.entry(cb_key).or_insert(0);
                    *count += 1;
                    if *count == 1 {
                        // First failure — record in scratchpad
                        let output_preview: String = tool_results.last()
                            .map(|r| r.output.chars().take(200).collect())
                            .unwrap_or_default();
                        failed_attempts.push(format!(
                            "FAILED: {}({}) → {}",
                            call.name,
                            call.args.to_string().chars().take(100).collect::<String>(),
                            output_preview
                        ));
                    }
                    // Extract learned failure on 2nd consecutive failure (before circuit breaker at 3)
                    if *count == 2 {
                        let err_snippet: String = tool_results.last()
                            .map(|r| r.output.chars().take(150).collect())
                            .unwrap_or_default();
                        let pattern_detail = match call.name.as_str() {
                            "exec" => call.args.get("command")
                                .and_then(|v| v.as_str())
                                .map(|s| s.chars().take(120).collect::<String>())
                                .unwrap_or_else(|| call.args.to_string().chars().take(80).collect()),
                            "http_request" => {
                                let method = call.args.get("method").and_then(|v| v.as_str()).unwrap_or("?");
                                let url = call.args.get("url").and_then(|v| v.as_str()).unwrap_or("?");
                                format!("{} {}", method, url.chars().take(100).collect::<String>())
                            }
                            _ => call.args.to_string().chars().take(80).collect(),
                        };
                        let lesson = LearnedFailure {
                            tool: call.name.clone(),
                            pattern: pattern_detail,
                            lesson: format!("Failed twice with: {}. Change approach or verify prerequisites.", err_snippet),
                            created_at: chrono::Utc::now().to_rfc3339(),
                        };
                        if !drift_state.learned_failures.iter().any(|lf| lf.tool == lesson.tool && lf.pattern == lesson.pattern) {
                            drift_state.learned_failures.push(lesson);
                        }
                    }
                } else {
                    // Success — clear failure count for this key
                    tool_failure_counts.remove(&cb_key);
                }
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
                let bg_reg_ref = &bg_registry_ref;
                let sess_reg_ref = &session_registry_ref;
                let execute_regular_call = |call: &&AgentToolCall| -> (AgentToolCall, ToolExecution) {
                    let call = *call;
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        let local_db = open_or_create_db(mv2_ref).map_err(|e| e.to_string())?;
                        execute_tool(&call.name, call.args.clone(), mv2_ref, &local_db, false, bg_reg_ref.clone(), sess_reg_ref.clone())
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
                            ToolExecution {
                                output: format!("Tool panicked: {msg}"),
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

            let mut all_deferred: Vec<AgentMessage> = Vec::new();
            for (call, result) in results {
                let result = truncate_tool_output(result, max_tool_output);
                let (is_error, tools_changed, deferred_msgs) = process_tool_result(
                    &call, result,
                    &mut tool_results, &mut messages, &mut active_tools,
                    &mut retrieved_skills, &mut session_taint,
                    should_log, &session, &log_dir,
                );
                // Collect deferred messages — pushed AFTER all tool results
                all_deferred.extend(deferred_msgs);
                if tools_changed {
                    // Re-strip orchestrator-blocked tools if they leaked back via tool_search
                    if orchestrator_mode && !is_subagent_early {
                        active_tools.remove("exec");
                        active_tools.remove("fs_write");
                    }
                    tools = tools_from_active(&tool_map, &active_tools);
                }

                // Circuit breaker: track failure counts per tool+args (parallel path)
                {
                    let cb_key = format!("{}:{:x}", call.name, {
                        use std::hash::{Hash, Hasher};
                        let mut h = std::collections::hash_map::DefaultHasher::new();
                        call.args.to_string().hash(&mut h);
                        h.finish()
                    });
                    if is_error {
                        let count = tool_failure_counts.entry(cb_key).or_insert(0);
                        *count += 1;
                        if *count == 1 {
                            let output_preview: String = tool_results.last()
                                .map(|r| r.output.chars().take(200).collect())
                                .unwrap_or_default();
                            failed_attempts.push(format!(
                                "FAILED: {}({}) → {}",
                                call.name,
                                call.args.to_string().chars().take(100).collect::<String>(),
                                output_preview
                            ));
                        }
                        // Extract learned failure on 2nd consecutive failure (parallel path)
                        if *count == 2 {
                            let err_snippet: String = tool_results.last()
                                .map(|r| r.output.chars().take(150).collect())
                                .unwrap_or_default();
                            let pattern_detail = match call.name.as_str() {
                                "exec" => call.args.get("command")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.chars().take(120).collect::<String>())
                                    .unwrap_or_else(|| call.args.to_string().chars().take(80).collect()),
                                "http_request" => {
                                    let method = call.args.get("method").and_then(|v| v.as_str()).unwrap_or("?");
                                    let url = call.args.get("url").and_then(|v| v.as_str()).unwrap_or("?");
                                    format!("{} {}", method, url.chars().take(100).collect::<String>())
                                }
                                _ => call.args.to_string().chars().take(80).collect(),
                            };
                            let lesson = LearnedFailure {
                                tool: call.name.clone(),
                                pattern: pattern_detail,
                                lesson: format!("Failed twice with: {}. Change approach or verify prerequisites.", err_snippet),
                                created_at: chrono::Utc::now().to_rfc3339(),
                            };
                            if !drift_state.learned_failures.iter().any(|lf| lf.tool == lesson.tool && lf.pattern == lesson.pattern) {
                                drift_state.learned_failures.push(lesson);
                            }
                        }
                    } else {
                        tool_failure_counts.remove(&cb_key);
                    }
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
            // Push deferred messages (failure hints) AFTER all tool results are in place.
            // This preserves tool_use→tool_result adjacency required by the Claude API.
            messages.extend(all_deferred);

            // Detect SSH connection to remote host (parallel path)
            for call in &tool_calls {
                if call.name == "exec" && !reminder_state.remote_host_seen {
                    let args_str = call.args.to_string().to_lowercase();
                    if args_str.contains("ssh ") || args_str.contains("scp ") {
                        reminder_state.remote_host_seen = true;
                        messages.push(AgentMessage {
                            role: "user".to_string(),
                            content: Some(
                                "[System: Remote Environment Detected]\n\
                                 You just connected to a remote machine. BEFORE doing any real work, run these discovery commands:\n\
                                 1. `df -h` — check available disk space (especially /tmp and working directories)\n\
                                 2. `nvidia-smi` or equivalent — check GPU availability and memory\n\
                                 3. `which python3 && python3 --version` — verify runtime availability\n\
                                 4. `echo $HF_HOME $CUDA_HOME` — check critical environment variables\n\
                                 5. `free -h` — check available RAM\n\
                                 Do NOT skip this step. Many failures come from wrong assumptions about the remote environment.".to_string()
                            ),
                            tool_calls: Vec::new(),
                            name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
                        });
                        break;
                    }
                }
            }
            // Track env verification (parallel path)
            if reminder_state.remote_host_seen && !reminder_state.remote_env_verified {
                for tr in tool_results.iter().rev().take(tool_calls.len()) {
                    let output_lower = tr.output.to_lowercase();
                    if output_lower.contains("filesystem") || output_lower.contains("nvidia-smi") || output_lower.contains("mem:") {
                        reminder_state.remote_env_verified = true;
                        break;
                    }
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
                thinking_blocks: vec![],
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
                    thinking_blocks: vec![],
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
                    0..=3 => { /* Standard correction — already injected above */ }
                    4..=7 => {
                        // Level 2: Stronger language (raised from 3-4 to 4-7 for browser workflows)
                        messages.push(AgentMessage {
                            role: "user".to_string(),
                            content: Some(format!("[ESCALATION WARNING] This is correction #{violation_count}. Repeated grounding violations detected. You MUST quote specific tool output for every factual claim. For browser interactions: ALWAYS call `browser snapshot` after click/fill/type BEFORE claiming what happened. Failure to comply will result in reduced capabilities.")),
                            tool_calls: Vec::new(),
                            name: None,
                            tool_call_id: None,
                            is_error: None,
                            thinking_blocks: vec![],
                        });
                    }
                    8..=11 => {
                        // Level 3: Log severe warning + conditionally restrict subagent tools
                        eprintln!("[critic] LEVEL 3 escalation: {violation_count} violations");
                        // In orchestrator mode, subagent tools are the ONLY way the agent can
                        // do useful work (exec/fs_write are already stripped).  Restricting them
                        // would completely hamstring the agent.  Only restrict subagent tools
                        // when orchestrator mode is NOT active.
                        let skip_subagent_restriction = orchestrator_mode;
                        let l3_message = if skip_subagent_restriction {
                            format!("[SEVERE WARNING] {violation_count} grounding violations this session. STOP making claims not supported by tool output. Before EVERY response, re-read the most recent tool output and ONLY report what it literally says. For browser: call snapshot after EVERY action. Your subagent tools remain available — use them to delegate work, but ground ALL claims in tool output.")
                        } else {
                            format!("[SEVERE WARNING] {violation_count} grounding violations this session. STOP making claims not supported by tool output. Before EVERY response, re-read the most recent tool output and ONLY report what it literally says. For browser: call snapshot after EVERY action. Subagent tools have been REVOKED.")
                        };
                        messages.push(AgentMessage {
                            role: "user".to_string(),
                            content: Some(l3_message),
                            tool_calls: Vec::new(),
                            name: None,
                            tool_call_id: None,
                            is_error: None,
                            thinking_blocks: vec![],
                        });
                        // Enforce: restrict subagent tools ONLY when not in orchestrator mode
                        if !subagent_tools_restricted && !skip_subagent_restriction {
                            subagent_tools_restricted = true;
                            let subagent_tool_names = ["subagent_invoke", "subagent_batch", "session_start"];
                            for tool_name in &subagent_tool_names {
                                active_tools.remove(*tool_name);
                            }
                            tools = tools_from_active(&tool_map, &active_tools);
                            eprintln!("[critic] LEVEL 3: subagent tools restricted");
                        } else if skip_subagent_restriction {
                            eprintln!("[critic] LEVEL 3: subagent tools PRESERVED (orchestrator mode active)");
                        }
                        // Enforce: reduce remaining step budget by 1/4 (was 1/3)
                        let remaining = current_max_steps.saturating_sub(step);
                        current_max_steps = step + (remaining * 3 / 4).max(8);
                        eprintln!("[critic] LEVEL 3 enforcement: step budget reduced to {current_max_steps} (was {})", step + remaining);
                    }
                    _ => {
                        // Level 4: Graceful wind-down (raised from 7+ to 12+)
                        eprintln!("[critic] LEVEL 4 escalation: {violation_count} violations — winding down gracefully");
                        messages.push(AgentMessage {
                            role: "user".to_string(),
                            content: Some(format!("[CRITICAL — GRACEFUL WIND-DOWN] {violation_count} grounding violations. You have 8 steps remaining. IMMEDIATELY:\n1. Write any partial results to disk (files the user requested).\n2. Summarize what you actually accomplished vs. what failed.\n3. Do NOT make new claims — only report verified facts from tool outputs.\nAfter these 8 steps, the session will end.")),
                            tool_calls: Vec::new(),
                            name: None,
                            tool_call_id: None,
                            is_error: None,
                            thinking_blocks: vec![],
                        });
                        // Enforce: allow 8 steps for graceful output (was 6)
                        current_max_steps = step + 8;
                        eprintln!("[critic] LEVEL 4 enforcement: graceful wind-down in 8 steps (step={step}, max={current_max_steps})");
                    }
                }
            }
        }

        // Checkpoint-and-report every 10 steps
        if step > 0 && step % 10 == 0 {
            let mut checkpoint_msg = format!(
                "[Checkpoint — Step {}] Summarize what you have accomplished so far and what you plan to do next. \
                 If the user's request was vague, confirm you are on the right track.", step
            );
            // Inject failed attempts scratchpad so the agent doesn't repeat mistakes
            if !failed_attempts.is_empty() {
                checkpoint_msg.push_str(&format!(
                    "\n\n[Failed Attempts — DO NOT RETRY these exact approaches]\n{}",
                    failed_attempts.iter()
                        .enumerate()
                        .map(|(i, a)| format!("{}. {}", i + 1, a))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
            // Include learned failures in checkpoint for cross-session awareness
            if !drift_state.learned_failures.is_empty() {
                checkpoint_msg.push_str(&format!(
                    "\n\n[Lessons Learned — avoid these patterns]\n{}",
                    drift_state.learned_failures.iter()
                        .map(|lf| format!("- {}: {}", lf.tool, lf.lesson))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
            messages.push(AgentMessage {
                role: "user".to_string(),
                content: Some(checkpoint_msg),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
                thinking_blocks: vec![],
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

    // SkillRL R5: Auto skill distillation — on successful multi-step tasks,
    // extract a reusable procedure and store it as a learned skill.
    // Only fires when: (a) task completed successfully, (b) took 3+ steps,
    // (c) used substantive tools (not just search/query).
    if completed && step >= 3 {
        let substantive_tools: HashSet<&str> = ["exec", "http_request", "browser", "fs_write", "skill_store"]
            .iter().cloned().collect();
        let used_substantive = tool_results.iter().any(|r| substantive_tools.contains(r.name.as_str()));
        if used_substantive {
            // Build a compact summary of what was done for distillation
            let action_summary: String = tool_results.iter()
                .filter(|r| !r.is_error)
                .take(10)
                .map(|r| format!("- {}: {}", r.name, r.output.chars().take(100).collect::<String>()))
                .collect::<Vec<_>>()
                .join("\n");
            if !action_summary.is_empty() {
                let distill_prompt = format!(
                    "Based on this successful task execution, extract a reusable skill.\n\
                     Original request: {}\n\n\
                     Actions taken:\n{}\n\n\
                     Respond with ONLY a JSON object: {{\"name\": \"learned:short-name\", \"description\": \"one line\", \
                     \"steps\": [\"step1\", \"step2\"], \"notes\": \"key gotchas\"}}",
                    prompt_text.chars().take(200).collect::<String>(),
                    action_summary
                );
                // Use a lightweight LLM call for distillation (best effort)
                let distill_request = crate::AgentHookRequest {
                    messages: vec![
                        AgentMessage {
                            role: "user".to_string(),
                            content: Some(distill_prompt),
                            tool_calls: Vec::new(),
                            name: None, tool_call_id: None, is_error: None, thinking_blocks: vec![],
                        },
                    ],
                    tools: vec![],
                    session: session.clone(),
                };
                if let Ok(response) = call_claude_with_model(&distill_request, None) {
                    if let Some(ref text) = response.message.content {
                        // Try to parse the JSON and store as a learned skill
                        if let Ok(skill_json) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let (Some(name), Some(desc)) = (
                                skill_json.get("name").and_then(|v| v.as_str()),
                                skill_json.get("description").and_then(|v| v.as_str()),
                            ) {
                                if let Some(ref workspace) = agent_workspace {
                                    let db_path = workspace.join("skills.sqlite");
                                    if let Ok(conn) = open_skill_db(&db_path) {
                                        // Check for duplicates before storing
                                        if crate::find_similar_skill(&conn, desc, 0.85).is_none() {
                                            let steps: Vec<String> = skill_json.get("steps")
                                                .and_then(|v| v.as_array())
                                                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                                .unwrap_or_default();
                                            let notes = skill_json.get("notes").and_then(|v| v.as_str()).map(String::from);
                                            let skill = crate::SkillRecord {
                                                name: name.to_string(),
                                                description: Some(desc.to_string()),
                                                trigger: None,
                                                steps,
                                                tools: vec![],
                                                notes,
                                                success_rate: 0.5, // Laplace prior
                                                times_used: 0,
                                                times_succeeded: 0,
                                                last_used: None,
                                                created_at: chrono::Utc::now().to_rfc3339(),
                                                contexts: vec![],
                                            };
                                            if crate::upsert_skill(&conn, &skill).is_ok() {
                                                eprintln!("[skill-distill] learned new skill: {name}");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
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

        // Clean up browser daemons before returning
        if let Some(ref sess_id) = session {
            let _ = std::process::Command::new("agent-browser")
                .args(["--session", sess_id, "close"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        let _ = std::process::Command::new("agent-browser")
            .args(["--session", "default", "close"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        return Ok(AgentRunOutput {
            session,
            context: context_pack,
            messages,
            tool_results,
            final_text: Some(continuation_marker),
            step_count: step,
        });
    }

    // Clean up browser daemons that this session may have spawned.
    // Without this, each agent session leaves a detached Node.js daemon +
    // Chromium renderer running, consuming 50%+ CPU indefinitely.
    if let Some(ref sess_id) = session {
        let _ = std::process::Command::new("agent-browser")
            .args(["--session", sess_id, "close"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = std::process::Command::new("agent-browser")
        .args(["--session", "default", "close"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    Ok(AgentRunOutput {
        session,
        context: context_pack,
        messages,
        tool_results,
        final_text,
        step_count: step,
    })
}

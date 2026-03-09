use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, mpsc as std_mpsc};
use std::thread;
use std::time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH};

use crate::consolidation::put_with_consolidation;
use crate::memory_db::PutOptions;
use chrono::Utc;
use rayon::ThreadPoolBuilder;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde_json;

use crate::claude::{
    call_agent_hook, call_agent_hook_streaming, call_claude, call_claude_with_model, call_critic,
};
use crate::{
    AgentHookRequest, AgentLogEntry, AgentMessage, AgentProgress, AgentRunOutput, AgentSession,
    AgentToolCall, AgentToolResult, BackgroundTaskRegistry, ClaudeStreamEvent, CommandSpec,
    ContinuationCheckpoint, DriftState, FailureKind, HookSpec, LearnedFailure, McpRegistry,
    McpServerConfig, QueryArgs, ReminderState, SessionRegistry, SessionTaint, SessionTurn,
    StreamPhase, ToolExecution, append_log_jsonl, base_tool_names, bootstrap_skills,
    build_context_pack, build_kg_context, classify_failure, collect_mid_loop_reminders,
    compute_drift_score, config_file_path, critic_should_fire, detect_cycle,
    detect_invisible_unicode, env_optional, env_optional_alias, execute_tool, find_kg_entities,
    format_tool_message_content, list_skills, load_capsule_config, load_config_from_file,
    load_kg_graph, load_session_turns, load_workspace_context, log_dir_path,
    match_skills_for_prompt, open_or_create_db, open_skill_db, prune_low_performing_skills,
    rebuild_fts5_index, record_skill_use, requires_approval, resolve_hook_spec, resolve_workspace,
    save_session_turns, tool_catalog_map, tool_definitions_json, tools_from_active,
};

/// Tracks blake3 hashes of observations already written this process lifetime.
const OBSERVATION_DEDUP_CAP: usize = 10_000;
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
    // Keep concise facts (e.g. short names or year statements) while filtering out tiny chatter.
    if trimmed.len() < 10 {
        return false;
    }
    let lower = trimmed.to_lowercase();

    // Filter strategy: drop obvious meta-phrases and status boilerplate,
    // while requiring a concrete signal (number, proper noun, or explicit marker) for everything else.
    let blocked_prefix = [
        "the assistant",
        "the agent",
        "i will now",
        "let me help",
        "here is",
        "here are",
        "as an assistant",
        "as your assistant",
    ];

    let has_prefix = |text: &str, phrase: &str| {
        text.starts_with(phrase)
            && text
                .get(phrase.len()..)
                .and_then(|rest| rest.chars().next())
                .map_or(true, |next| !next.is_alphabetic())
    };

    if blocked_prefix
        .iter()
        .any(|phrase| has_prefix(&lower, phrase))
    {
        return false;
    }
    // Generic status check phrases that are typically non-actionable
    if lower == "nothing to report" || lower == "nothing to report." {
        return false;
    }
    if lower == "no issues found" || lower == "no issues found." {
        return false;
    }

    let proper_noun_lookalikes = [
        "i", "a", "the", "an", "in", "on", "at", "to", "for", "and", "but", "or", "is", "it", "my",
        "this", "that", "these", "those", "there", "here", "we", "you", "your",
    ];

    let is_title_case_word = |token: &str| {
        let cleaned = token.trim_matches(|c: char| !c.is_alphanumeric());
        cleaned.len() > 1
            && cleaned.chars().next().is_some_and(|c| c.is_uppercase())
            && !proper_noun_lookalikes.contains(&cleaned)
    };

    // Must contain something specific: a number, a proper noun, a technology name, a concrete preference, or a lesson learned.
    let has_number = trimmed.chars().any(|c| c.is_ascii_digit());
    let has_proper_noun = trimmed.split_whitespace().any(is_title_case_word);
    let specificity_markers = [
        "because",
        "prefers",
        "always",
        "never",
        "important",
        "learned",
        "rule",
        "policy",
        "deadline",
        "budget",
        "password",
        "key",
        "api",
        "token",
        "endpoint",
        "port",
        "version",
        "config",
    ];
    let has_specificity = specificity_markers.iter().any(|m| lower.contains(m));

    has_number || has_proper_noun || has_specificity
}

/// Consume a streaming channel from call_claude_streaming and assemble an AgentMessage.
/// Updates progress.stream_thinking / stream_response / stream_phase live so the
/// Telegram progress reporter can push edits to the user in real-time.
fn consume_stream(
    rx: std_mpsc::Receiver<ClaudeStreamEvent>,
    progress: &Arc<Mutex<AgentProgress>>,
) -> Result<AgentMessage, String> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<AgentToolCall> = Vec::new();
    let mut thinking_blocks: Vec<serde_json::Value> = Vec::new();

    // Per-block accumulators (keyed by block index)
    let mut block_types: HashMap<usize, String> = HashMap::new();
    let mut block_texts: HashMap<usize, String> = HashMap::new();
    let mut block_tool_ids: HashMap<usize, String> = HashMap::new();
    let mut block_tool_names: HashMap<usize, String> = HashMap::new();
    let mut block_signatures: HashMap<usize, String> = HashMap::new();

    let timeout_secs: u64 = std::env::var("ANTHROPIC_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180);
    let recv_timeout = StdDuration::from_secs(timeout_secs);

    loop {
        let event = rx
            .recv_timeout(recv_timeout)
            .map_err(|_| format!("stream recv timeout ({}s)", timeout_secs))?;

        match event {
            ClaudeStreamEvent::BlockStart {
                index,
                block_type,
                tool_id,
                tool_name,
            } => {
                block_types.insert(index, block_type.clone());
                block_texts.insert(index, String::new());
                if let Some(id) = tool_id {
                    block_tool_ids.insert(index, id);
                }
                if let Some(name) = tool_name {
                    block_tool_names.insert(index, name);
                }

                match block_type.as_str() {
                    "thinking" => {
                        if let Ok(mut p) = progress.lock() {
                            p.stream_phase = StreamPhase::Thinking;
                            if p.stream_thinking.is_none() {
                                p.stream_thinking = Some(String::new());
                            }
                        }
                    }
                    "text" => {
                        if let Ok(mut p) = progress.lock() {
                            p.stream_phase = StreamPhase::Responding;
                            if p.stream_response.is_none() {
                                p.stream_response = Some(String::new());
                            }
                        }
                    }
                    _ => {} // tool_use, redacted_thinking — no streaming display
                }
            }
            ClaudeStreamEvent::BlockDelta {
                index,
                delta_type,
                text,
                signature,
            } => {
                if let Some(buf) = block_texts.get_mut(&index) {
                    buf.push_str(&text);
                }
                if let Some(sig) = signature {
                    block_signatures.insert(index, sig);
                }
                let btype = block_types.get(&index).map(|s| s.as_str()).unwrap_or("");
                match (btype, delta_type.as_str()) {
                    ("thinking", "thinking_delta") => {
                        if let Ok(mut p) = progress.lock() {
                            if let Some(ref mut t) = p.stream_thinking {
                                t.push_str(&text);
                            }
                            p.stream_revision += 1;
                            // Also update text_preview with latest thinking snippet
                            if let Some(ref t) = p.stream_thinking {
                                let chars: Vec<char> = t.chars().collect();
                                let snippet = if chars.len() > 100 {
                                    format!(
                                        "...{}",
                                        chars[chars.len() - 97..].iter().collect::<String>()
                                    )
                                } else {
                                    t.clone()
                                };
                                p.text_preview = Some(snippet);
                            }
                        }
                    }
                    ("text", "text_delta") => {
                        if let Ok(mut p) = progress.lock() {
                            if let Some(ref mut r) = p.stream_response {
                                r.push_str(&text);
                            }
                            p.stream_revision += 1;
                        }
                    }
                    _ => {} // input_json_delta for tool_use, etc.
                }
            }
            ClaudeStreamEvent::BlockStop { index } => {
                let btype = block_types
                    .get(&index)
                    .map(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                let accumulated = block_texts.remove(&index).unwrap_or_default();

                match btype.as_str() {
                    "thinking" => {
                        if !accumulated.is_empty() {
                            let mut block = serde_json::json!({
                                "type": "thinking",
                                "thinking": accumulated,
                            });
                            if let Some(sig) = block_signatures.remove(&index) {
                                block["signature"] = serde_json::json!(sig);
                            }
                            thinking_blocks.push(block);
                        }
                    }
                    "redacted_thinking" => {
                        thinking_blocks.push(serde_json::json!({
                            "type": "redacted_thinking",
                        }));
                    }
                    "text" => {
                        if !accumulated.is_empty() {
                            text_parts.push(accumulated);
                        }
                    }
                    "tool_use" => {
                        let id = block_tool_ids.remove(&index).unwrap_or_default();
                        let name = block_tool_names.remove(&index).unwrap_or_default();
                        let args: serde_json::Value = serde_json::from_str(&accumulated)
                            .unwrap_or_else(|_| serde_json::json!({}));
                        tool_calls.push(AgentToolCall { id, name, args });
                    }
                    _ => {}
                }
            }
            ClaudeStreamEvent::MessageDelta { .. } => {
                // stop_reason available here but we wait for MessageStop
            }
            ClaudeStreamEvent::MessageStop => {
                if let Ok(mut p) = progress.lock() {
                    p.stream_phase = StreamPhase::Done;
                    p.stream_revision += 1;
                }
                break;
            }
            ClaudeStreamEvent::Error(e) => {
                return Err(format!("stream error: {e}"));
            }
        }
    }

    let content_text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    };

    Ok(AgentMessage {
        role: "assistant".to_string(),
        content: content_text,
        tool_calls,
        name: None,
        tool_call_id: None,
        is_error: None,
        thinking_blocks,
    })
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
    const MAX_CHECKPOINT_CHAIN_DEPTH: usize = 10;
    const CONTINUATION_MARKER_PREFIX: &str = "[CONTINUATION_NEEDED:";
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

        let continuation_marker_line = output.final_text.as_ref().and_then(|text| {
            text.lines()
                .find(|line| line.starts_with(CONTINUATION_MARKER_PREFIX))
        });
        let needs_continuation = continuation_marker_line.is_some();

        if needs_continuation && chain_depth < MAX_CHAIN_DEPTH {
            // Parse checkpoint and build continuation prompt
            if let Some(marker_line) = continuation_marker_line {
                let after = &marker_line[CONTINUATION_MARKER_PREFIX.len()..];
                if let Some(end) = after.find(']') {
                    let checkpoint_path = &after[..end];
                    if let Ok(checkpoint_json) = fs::read_to_string(checkpoint_path) {
                        if let Ok(checkpoint) =
                            serde_json::from_str::<ContinuationCheckpoint>(&checkpoint_json)
                        {
                            chain_depth = if checkpoint.chain_depth <= MAX_CHECKPOINT_CHAIN_DEPTH {
                                checkpoint.chain_depth
                            } else {
                                eprintln!(
                                    "[auto-continuation] checkpoint chain_depth {} outside 0..={}; resetting to 0",
                                    checkpoint.chain_depth, MAX_CHECKPOINT_CHAIN_DEPTH
                                );
                                0
                            };
                            eprintln!(
                                "[auto-continuation] chaining session (depth {}/{}): {}",
                                chain_depth,
                                MAX_CHAIN_DEPTH,
                                checkpoint.goal.chars().take(80).collect::<String>()
                            );
                            current_prompt = format!(
                                "[Continuation from previous session — chain depth {}/{}]\n\n\
                                 ## Goal\n{}\n\n\
                                 ## Summary of work so far\n{}\n\n\
                                 ## Remaining work\n{}\n\n\
                                 Continue from where you left off. Do NOT repeat completed work.",
                                chain_depth,
                                MAX_CHAIN_DEPTH,
                                checkpoint.goal,
                                checkpoint.summary,
                                checkpoint.remaining_work,
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
        "You are OpenClaw, a high-performance personal AI assistant with a rich toolkit.",
        "You are not a limited chatbot; memory, search, filesystem, code execution, web, email, browser, notifications, subagents, and more are available immediately.",
        "Be proactive, concrete, concise, and action-first.",
        "",
        "## Triage and Action",
        "Classify first: conversational/status/thanks -> reply directly; clear bounded task -> execute and report; vague scope or pronouns -> ask 1-2 clarifying questions first; complex multi-step work -> break it down and use subagents if helpful. Do not launch extensive tool use for greetings or vague requests.",
        "Routine reads/searches: execute immediately, summarize after.",
        "Writes/creates: state a one-sentence plan, then execute.",
        "Complex tasks: give 2-3 bullets, then execute step by step.",
        "Irreversible actions (delete, send, deploy): explain consequences and wait for confirmation.",
        "Search memory before making claims, investigate before answering, and match the user's energy.",
        "",
        "## Tools and Delegation",
        "Tools are listed in Available Tools. Core tools (memory, search, exec/filesystem, browser, subagents) are already active.",
        "Use tool_search to discover specialized tools (email, calendar, messaging, etc.); discovered tools activate in this session. If unsure a tool exists, search instead of guessing.",
        "Prefer tools over training-data recall: research with tools first, then synthesize. Do not claim you lack access unless you tried the tool and it failed.",
        "If a task needs accounts, credentials, or setup, try to obtain them yourself via browser dashboards/signups/API keys, env vars, config files, or CLI auth tools; only involve the user after two approaches.",
        "Request independent tool calls in parallel whenever possible.",
        "Sensitive actions may require approval; if a tool returns `approval required: <id>`, that is not an error. Ask the user to approve or reject with `approve <id>` or `reject <id>`.",
        "Use subagent_invoke for single delegation and subagent_batch for parallel fan-out; subagents may have any descriptive name, each with its own session/tools and a lighter model suited for heavy lifting while you orchestrate.",
        "Use subagents for large research, multi-file code changes, parallel independent work, and long analysis. Do simple tool calls, conversational replies, single-file reads, quick commands, and 1-3 step tasks directly.",
        "Delegated work runs in the background automatically. After spawning it, tell the user what started and end your response; do not wait. The user can check /status anytime.",
        "When '[Background task completed]' messages arrive, synthesize the actual results concisely.",
        "",
        "## Communication and Interrupts",
        "Before a new logical tool step, send one short natural sentence about what you're doing. Do not narrate every tool call, use bullet points in interim updates, or describe fallback plans.",
        "User messages can arrive mid-run and are immediate possible course corrections. Read them at once, acknowledge them, and pivot if needed. Never ignore or defer a user message.",
        "",
        "## Grounding and Recovery",
        "The rules below are constitutional and apply at every step.",
        "Only report what tool or subagent output literally shows. Do not infer hidden config, file paths, identifiers, errors, or success from partial output.",
        "Before claiming success, quote or cite the specific output that proves it.",
        "Never claim success when output is empty or shows errors. If output is ambiguous or incomplete, say so and quote the relevant text.",
        "For multi-step work, do not mark a step complete until output confirms it; acknowledge failures explicitly and report each step's actual outcome before continuing.",
        "When reporting subagent results, quote the actual subagent output; if it is empty or errored, say that exactly. Do not paraphrase or embellish.",
        "Diagnose before retrying: read errors, verify assumptions, inspect logs/docs, and never repeat the exact failing call.",
        "Use reflect to record lessons learned.",
        "After two failures with the same approach, switch to a fundamentally different strategy; if that also fails, report clearly and ask the user for guidance instead of brute-forcing.",
        "Poll long-running tasks at most once every 5 minutes.",
        "After any mutation (file write, API call, config change), verify the result with a read/check before continuing.",
        "After destructive operations (docker rebuild, db reset, rm, reinstall), re-verify all dependent functionality; prior test results no longer count.",
        "When a process crashes or a service will not start, check logs yourself immediately: Docker -> `docker logs <container>` or `docker compose logs --tail=50`; system -> `journalctl -u <service> --no-pager -n 50`. Diagnose first, report findings, then fix; never ask the user to fetch logs for you.",
        "Before using any API endpoint for the first time, read its docs or schema; never guess payloads.",
        "Before connecting to any remote machine (SSH, RunPod, cloud), always run `df -h`, `nvidia-smi`, `python3 --version`, and `echo $HF_HOME`. Never assume the environment matches expectations.",
        "",
        "## Self-Modification",
        "You may modify your own source code, compile, and deploy without human intervention.",
        "Workflow: 1) edit the active repo with exec or fs_write; 2) run `cargo check`; 3) commit and push; 4) call self_upgrade for blue-green deploy with automatic rollback; 5) after deploy you restart, and conversation state persists in the capsule.",
        "Always commit and push before self_upgrade because it runs `git reset --hard`.",
        "If the new binary crashes, upgrade.sh auto-rolls back within 30s.",
        "Check deploy status with exec against the current deployment log or systemd journal.",
        "",
        "## Autonomous Self-Improvement",
        "A systemd timer runs every 6 hours: scan for improvements, implement one, validate, and deploy.",
        "Log each improvement in the runtime home data directory and store it as capsule reflections.",
        "Prioritize, in order: reliability fixes, performance improvements, safety hardening, capability additions.",
        "Never autonomously remove safety checks or approval gates, modify deployment infrastructure (upgrade.sh, systemd configs), change API keys/secrets/authentication, or alter the Telegram bridge protocol.",
        "",
        "## Project Build Protocol",
        "For full applications or multi-file features, you are the ORCHESTRATOR: write prompts and delegate to coding agents; do not write code directly.",
        "1. Always start with swarm_create; this enables Orchestrator Mode and strips exec/fs_write.",
        "2. Decompose the work into parallel subtasks and write detailed coder prompts with file paths, expected behavior, and test commands.",
        "3. Use subagent_batch with multiple swarm-coder agents and set branch='swarm/{task-id}-{subtask}' for worktree isolation.",
        "4. The swarm monitor injects status every 60s automatically; do not poll.",
        "5. When CI passes, dispatch a cross-model reviewer (Codex coder -> Claude reviewer, Claude coder -> Codex reviewer).",
        "6. If a task fails, rewrite the prompt with failure context and spawn a new agent; do not blindly retry.",
        "7. Definition of done: PR created, CI passing, cross-model review passing, and final verification with exec after tools are restored.",
        "8. If a build step destroys state, re-verify everything that depended on it.",
    ]
    .join("\n")
}

/// Estimate token count for messages (rough: chars / 4).
pub(crate) fn estimate_tokens(messages: &[AgentMessage]) -> usize {
    messages
        .iter()
        .map(|m| {
            let content_chars = m.content.as_ref().map(|c| c.len()).unwrap_or(0);
            let tool_call_chars: usize = m
                .tool_calls
                .iter()
                .map(|tc| tc.name.len() + tc.id.len() + tc.args.to_string().len())
                .sum();
            let thinking_chars: usize = m
                .thinking_blocks
                .iter()
                .map(|tb| tb.to_string().len())
                .sum();
            // ~4 chars per token, plus per-message overhead (~20 tokens for role/structure)
            (content_chars + tool_call_chars + thinking_chars) / 4 + 20
        })
        .sum()
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
    while summary_end > system_end
        && summary_end < messages.len()
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
    let sonnet_model =
        env_optional("SONNET_MODEL").unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string());
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
    let summary = summary_response
        .message
        .content
        .unwrap_or_else(|| "(compaction failed)".to_string());

    // Extract the GOAL field from the structured summary
    let extracted_goal = summary
        .lines()
        .find(|line| line.starts_with("GOAL:"))
        .map(|line| line.trim_start_matches("GOAL:").trim().to_string());

    // Rebuild messages: system blocks + compaction notice + recent (thinking blocks stripped)
    *messages = system_msgs;
    messages.push(AgentMessage {
        role: "user".to_string(),
        content: Some(format!(
            "[Context compacted. Summary of prior conversation:]\n{summary}"
        )),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
        thinking_blocks: vec![],
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
        content: if tool_content.is_empty() {
            None
        } else {
            Some(tool_content)
        },
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
        eprintln!(
            "[security] invisible unicode detected in {} output",
            call.name
        );
        session_taint.mark_untrusted(&format!("{} (invisible unicode)", call.name));
        deferred.push(AgentMessage {
            role: "user".to_string(),
            content: Some(warning),
            tool_calls: Vec::new(),
            name: None,
            tool_call_id: None,
            is_error: None,
            thinking_blocks: vec![],
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
                    exit_code
                        .map(|c| format!(" (exit code {c})"))
                        .unwrap_or_default()
                ))
            }
            "http_request" => {
                let status = result
                    .details
                    .get("status")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
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
                name: None,
                tool_call_id: None,
                is_error: None,
                thinking_blocks: vec![],
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

fn prompt_file_reference_count(prompt: &str) -> usize {
    let file_extensions = [
        ".rs", ".py", ".js", ".ts", ".tsx", ".jsx", ".go", ".java", ".rb", ".cpp", ".c", ".h",
        ".css", ".html", ".json", ".toml", ".yaml", ".yml", ".md", ".sh", ".sql",
    ];
    let code_dirs = [
        "src", "app", "apps", "bin", "lib", "tests", "test", "public", "config", "docs", "scripts",
    ];
    prompt
        .split_whitespace()
        .filter(|w| {
            let cleaned: String = w
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '/' || *c == '_' || *c == '-')
                .collect();
            let c_lower = cleaned.to_lowercase();
            if file_extensions.iter().any(|ext| c_lower.ends_with(ext)) {
                return true;
            }
            if !cleaned.contains('/') || cleaned.len() <= 2 {
                return false;
            }

            let looks_like_explicit_path = cleaned.starts_with('/')
                || cleaned.starts_with("./")
                || cleaned.starts_with("../")
                || cleaned.starts_with("~/");
            if looks_like_explicit_path {
                return true;
            }

            let segments: Vec<&str> = c_lower
                .split('/')
                .filter(|segment| !segment.is_empty())
                .collect();
            if segments.len() < 2 {
                return false;
            }

            let has_code_dir = segments.iter().any(|segment| code_dirs.contains(segment));
            let has_fileish_segment = segments.iter().any(|segment| {
                segment.contains('.')
                    || segment.chars().any(|ch| ch.is_ascii_digit())
                    || segment.contains('_')
                    || segment.contains('-')
            });

            has_code_dir || has_fileish_segment
        })
        .count()
}

fn build_user_intent_context(prompt_text: &str, session_turns: &[SessionTurn]) -> String {
    let mut user_turns: Vec<String> = session_turns
        .iter()
        .rev()
        .filter(|turn| turn.role == "user")
        .filter_map(|turn| {
            let trimmed = turn.content.trim();
            if trimmed.is_empty() || trimmed.starts_with('[') {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .take(4)
        .collect();
    user_turns.reverse();
    user_turns.push(prompt_text.trim().to_string());
    user_turns.join(" ")
}

fn is_intent_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn contains_whole_term(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }

    let mut search_from = 0usize;
    while let Some(offset) = haystack[search_from..].find(needle) {
        let start = search_from + offset;
        let end = start + needle.len();
        let left_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|ch| !is_intent_word_char(ch));
        let right_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|ch| !is_intent_word_char(ch));
        if left_ok && right_ok {
            return true;
        }
        search_from = start + needle.len();
    }

    false
}

fn prompt_contains_intent_term(lower: &str, term: &str) -> bool {
    if term.chars().all(is_intent_word_char) {
        return contains_whole_term(lower, term);
    }
    lower.contains(term)
}

fn prompt_has_explicit_orchestration_request(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    let explicit_orchestration = [
        "orchestrate",
        "swarm",
        "subagent",
        "sub-agent",
        "parallel agents",
        "coordinate agents",
        "use orchestrator",
        "orchestrator mode",
    ];
    explicit_orchestration
        .iter()
        .any(|kw| prompt_contains_intent_term(&lower, kw))
}

fn prompt_has_executive_assistant_intent(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    let domain_terms = [
        "flight",
        "travel",
        "hotel",
        "airline",
        "airport",
        "parents",
        "mom",
        "dad",
        "mother",
        "father",
        "doctor",
        "appointment",
        "referral",
        "insurance",
        "provider",
        "specialist",
        "clinic",
        "calendar",
        "meeting",
        "restaurant",
        "reservation",
        "itinerary",
        "guest",
        "hosting",
        "vendor",
        "partner",
        "visit",
        "home",
        "rhaine",
        "inbox",
        "email",
        "slack",
        "follow-up",
        "follow up",
        "phone call",
        "birthday",
        "gift",
    ];
    let coordination_terms = [
        "schedule",
        "book",
        "plan",
        "proposal",
        "coordinate",
        "delegate",
        "draft",
        "availability",
        "confirm",
        "handoff",
        "follow-up",
        "follow up",
        "respond",
        "call",
    ];

    let domain_hits = domain_terms
        .iter()
        .filter(|term| prompt_contains_intent_term(&lower, term))
        .count();
    let coordination_hits = coordination_terms
        .iter()
        .filter(|term| prompt_contains_intent_term(&lower, term))
        .count();

    domain_hits >= 2 || (domain_hits >= 1 && coordination_hits >= 1)
}

fn prompt_requests_structured_json_response(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    (lower.contains("respond with only") || lower.contains("reply with only"))
        && (lower.contains("begin_json") || (lower.contains("json") && lower.contains('{')))
}

fn prompt_has_engineering_intent(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    if prompt_has_explicit_orchestration_request(prompt) {
        return true;
    }

    let file_refs = prompt_file_reference_count(prompt);
    if file_refs > 0 {
        return true;
    }

    let strong_terms = [
        "codebase",
        "repository",
        "repo",
        "package.json",
        "cargo",
        "npm",
        "pnpm",
        "yarn",
        "stack trace",
        "panic",
        "pull request",
        "unit test",
        "integration test",
        "test suite",
        "compile",
        "binary",
        "function",
        "class",
        "module",
        "endpoint",
        "api",
        "database",
        "schema",
        "migration",
        "frontend",
        "backend",
        "cli",
        "http server",
        "bug",
        "refactor",
        "debug",
        "implement",
        "oauth",
        "auth flow",
        "docker",
        "ci",
    ];
    let medium_terms = [
        "build an app",
        "build a project",
        "application",
        "feature",
        "service",
        "server",
        "route",
        "handler",
        "workspace",
        "tests",
        "readme",
        "public/",
        "src/",
        "git",
        "commit",
        "deploy",
        "configure",
        "install",
    ];
    let human_ops_terms = [
        "flight",
        "travel",
        "hotel",
        "airline",
        "airport",
        "parents",
        "mom",
        "dad",
        "mother",
        "father",
        "inbox",
        "email",
        "calendar",
        "meeting",
        "rhaine",
        "slack",
        "visit",
        "home",
        "follow-up",
        "follow up",
        "proposal",
        "itinerary",
        "restaurant",
        "reservation",
        "gift",
        "birthday",
    ];

    let strong_hits = strong_terms
        .iter()
        .filter(|term| prompt_contains_intent_term(&lower, term))
        .count();
    let medium_hits = medium_terms
        .iter()
        .filter(|term| prompt_contains_intent_term(&lower, term))
        .count();
    let human_hits = human_ops_terms
        .iter()
        .filter(|term| prompt_contains_intent_term(&lower, term))
        .count();

    let engineering_score = strong_hits * 2 + medium_hits;
    if engineering_score == 0 {
        return false;
    }
    if human_hits >= engineering_score && strong_hits == 0 {
        return false;
    }
    engineering_score >= 2
}

/// Complexity gate: determines whether a prompt warrants full orchestrator mode
/// (swarm delegation with exec/fs_write stripped) vs. simple direct execution.
///
/// Returns `true` only when the prompt shows genuine multi-file/multi-step complexity.
/// Uses a scoring system: explicit orchestration requests always pass, hard negatives
/// return false immediately, then positive complexity signals are counted (need 2+).
fn prompt_is_complex(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    let words: Vec<&str> = prompt.split_whitespace().collect();
    let word_count = words.len();

    // ── Explicit override (checked FIRST, before any negative gates) ──
    // If the user explicitly requests orchestration, always honor it
    // regardless of prompt length or form.
    let explicit_orchestration = [
        "orchestrate",
        "swarm",
        "subagent",
        "sub-agent",
        "parallel agents",
        "coordinate agents",
        "use orchestrator",
        "orchestrator mode",
    ];
    if explicit_orchestration.iter().any(|kw| lower.contains(kw)) {
        return true;
    }

    // ── Hard negative gates: bail immediately for clearly simple prompts ──

    // Very short prompts (< 12 words) are almost never complex enough.
    // We use 12 instead of 30 because concise complex requests like
    // "refactor the agent module across agent.rs and tool_exec.rs" are ~10-20 words.
    if word_count < 12 {
        return false;
    }

    // Question-form prompts: reading/understanding, not multi-file coding
    let question_prefixes = [
        "what ",
        "how ",
        "why ",
        "explain ",
        "describe ",
        "show ",
        "list ",
        "tell me",
        "is there",
        "does ",
        "can you tell",
        "where ",
        "which ",
        "who ",
        "when ",
        "could you explain",
        "help me understand",
    ];
    let trimmed = lower.trim_start();
    if question_prefixes.iter().any(|q| trimmed.starts_with(q)) {
        return false;
    }

    // Read-only intent: no write action implied
    let read_only_verbs = [
        "read ",
        "check ",
        "look at",
        "review ",
        "inspect ",
        "view ",
        "examine ",
        "analyze ",
        "print ",
        "display ",
        "grep ",
        "search ",
        "find ",
        "locate ",
        "cat ",
        "show me",
        "what's in",
    ];
    if read_only_verbs.iter().any(|v| trimmed.starts_with(v)) {
        return false;
    }

    // Count files/paths, stripping trailing punctuation so "agent.rs," still matches
    let file_count = prompt_file_reference_count(prompt);

    // Single-file/single-line fix: explicit "line N" or "on line" with a file ref
    // e.g., "fix the typo on line 5 in main.rs"
    let has_line_ref = lower.contains("line ") || lower.contains("on line");
    if has_line_ref && file_count <= 1 {
        return false;
    }

    // ── Positive complexity signals: need 2+ to trigger ──
    let mut signals: usize = 0;

    // Signal: multiple files or directories mentioned
    if file_count >= 2 {
        signals += 1;
    }

    // Signal: complexity keywords suggesting multi-file/architectural work
    let complexity_keywords = [
        "refactor",
        "redesign",
        "architect",
        "migrate",
        "migration",
        "across",
        "all files",
        "multiple files",
        "codebase",
        "entire",
        "overhaul",
        "rewrite",
        "restructure",
        "modularize",
        "integrate",
        "end-to-end",
        "full-stack",
        "cross-cutting",
    ];
    if complexity_keywords.iter().any(|kw| lower.contains(kw)) {
        signals += 1;
    }

    // Signal: implementation/feature creation keywords
    let implement_words = [
        "build",
        "implement",
        "build out",
        "develop",
        "create a feature",
        "add a feature",
        "new feature",
    ];
    if implement_words.iter().any(|kw| lower.contains(kw)) {
        signals += 1;
    }

    // Signal: long prompt (>60 words) with write intent
    let write_verbs = [
        "create",
        "build",
        "implement",
        "write",
        "add",
        "modify",
        "update",
        "change",
        "refactor",
        "fix",
        "deploy",
        "set up",
        "configure",
        "install",
        "generate",
    ];
    let has_write_intent = write_verbs.iter().any(|v| lower.contains(v));
    if word_count > 60 && has_write_intent {
        signals += 1;
    }

    // Signal: multiple subtasks (numbered lists, bullets, "and then", "also need")
    let multi_task_indicators = [
        "1.",
        "2.",
        "- ",
        "* ",
        "and then",
        "also need",
        "additionally",
        "after that",
        "next step",
        "followed by",
        "as well as",
        "on top of that",
        "plus ",
    ];
    let multi_task_count = multi_task_indicators
        .iter()
        .filter(|ind| lower.contains(*ind))
        .count();
    if multi_task_count >= 2 {
        signals += 1;
    }

    // Signal: mentions multiple components/modules/services
    let component_words = [
        "frontend",
        "backend",
        "api",
        "database",
        "server",
        "client",
        "component",
        "module",
        "service",
        "endpoint",
        "route",
        "model",
        "controller",
        "middleware",
        "schema",
    ];
    let component_count = component_words
        .iter()
        .filter(|cw| lower.contains(*cw))
        .count();
    if component_count >= 2 {
        signals += 1;
    }

    signals >= 2
}

fn prompt_is_trivial_chat(prompt: &str) -> bool {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return true;
    }

    let normalized = trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return true;
    }

    let exact_matches = [
        "hi",
        "hello",
        "hey",
        "yo",
        "ping",
        "test",
        "ok",
        "okay",
        "kk",
        "thanks",
        "thank you",
        "thx",
        "cool",
        "great",
        "nice",
        "sure",
        "yep",
        "yes",
        "no",
        "continue",
        "keep going",
        "go on",
    ];
    if exact_matches.contains(&normalized.as_str()) {
        return true;
    }

    let filler_words = [
        "hi", "hello", "hey", "yo", "ok", "okay", "kk", "thanks", "thank", "you", "thx", "cool",
        "great", "nice", "sure", "yep", "yes", "no", "continue", "keep", "going", "go", "on",
        "please", "ping", "test",
    ];
    let words: Vec<&str> = normalized.split_whitespace().collect();
    words.len() <= 3 && words.iter().all(|word| filler_words.contains(word))
}

fn build_memory_query_seed(prompt_text: &str, session_turns: &[SessionTurn]) -> String {
    let mut parts = Vec::new();
    for turn in session_turns.iter().rev().take(4).rev() {
        let snippet = turn.content.trim();
        if snippet.is_empty() {
            continue;
        }
        let condensed = if snippet.len() > 240 {
            format!("{}...", &snippet[..240])
        } else {
            snippet.to_string()
        };
        parts.push(format!("{}: {}", turn.role, condensed));
    }
    parts.push(format!("user: {}", prompt_text.trim()));
    let joined = parts.join("\n");
    if joined.len() > 1_200 {
        joined[joined.len() - 1_200..].to_string()
    } else {
        joined
    }
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
    let prompt_setup_started = Instant::now();

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
    let session_turns = session
        .as_ref()
        .map(|sess_id| load_session_turns(sess_id, 20))
        .unwrap_or_default();
    let skip_trivial_prefetch = prompt_is_trivial_chat(&prompt_text);
    let session_label = session.as_deref().unwrap_or("<none>");

    // Opus escalation: build a fallback HookSpec for when critic fires
    let opus_escalation_spec: Option<HookSpec> = {
        // Only useful if the base model isn't already Opus
        let base_cmd = match &base_model_spec.command {
            CommandSpec::String(s) => s.trim().to_ascii_lowercase(),
            CommandSpec::Array(a) => a
                .first()
                .map(|s| s.trim().to_ascii_lowercase())
                .unwrap_or_default(),
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
            "\n\n# Onboarding\nYou are in onboarding mode. Guide the user to connect email, calendar, and messaging integrations. Verify tool access. When complete, append a note to MEMORY.md and ask the user to run `openclaw config set --key index` to set `agent.onboarding_complete=true`.",
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
    let mut user_intent_context = build_user_intent_context(&prompt_text, &session_turns);
    let mut explicit_orchestration_intent =
        prompt_has_explicit_orchestration_request(&user_intent_context);
    let mut executive_assistant_intent =
        prompt_has_executive_assistant_intent(&user_intent_context);
    let mut engineering_orchestration_intent = prompt_has_engineering_intent(&user_intent_context);
    let mut orchestration_complexity_intent = prompt_is_complex(&user_intent_context);
    if executive_assistant_intent && !explicit_orchestration_intent {
        engineering_orchestration_intent = false;
    }
    let trace_intent = env_optional_alias(&["OPENCLAW_TRACE_INTENT", "AETHERVAULT_TRACE_INTENT"])
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false);
    // --- SkillRL R1: Auto-inject top skills into stable prefix ---
    if !skip_trivial_prefetch {
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
                let match_context = if session.is_some() {
                    let recent: String = session_turns
                        .iter()
                        .rev()
                        .take(6)
                        .map(|t| t.content.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("{} {}", recent, &prompt_text)
                } else {
                    prompt_text.clone()
                };
                let matched = match_skills_for_prompt(&conn, &match_context, 5);
                user_intent_context = build_user_intent_context(&prompt_text, &session_turns);
                explicit_orchestration_intent =
                    prompt_has_explicit_orchestration_request(&user_intent_context);
                executive_assistant_intent =
                    prompt_has_executive_assistant_intent(&user_intent_context);
                engineering_orchestration_intent =
                    prompt_has_engineering_intent(&user_intent_context);
                orchestration_complexity_intent = prompt_is_complex(&user_intent_context);
                if executive_assistant_intent && !explicit_orchestration_intent {
                    engineering_orchestration_intent = false;
                }
                if trace_intent {
                    eprintln!(
                        "[intent] session={} explicit={} executive_assistant={} engineering={} complexity={} file_refs={} preview={}",
                        session_label,
                        explicit_orchestration_intent,
                        executive_assistant_intent,
                        engineering_orchestration_intent,
                        orchestration_complexity_intent,
                        prompt_file_reference_count(&user_intent_context),
                        prompt_text
                            .chars()
                            .take(120)
                            .collect::<String>()
                            .replace('\n', " "),
                    );
                }
                // Also get top general skills by success rate
                let general = list_skills(&conn, 3);

                let mut seen: HashSet<String> = HashSet::new();
                let mut skill_block = String::new();
                let mut inline_count = 0usize;
                // Inline full steps for top 3 matched skills; one-liner for the rest
                for s in matched.iter().chain(general.iter()) {
                    if !engineering_orchestration_intent && s.name == "bootstrap:swarm-dev-task" {
                        continue;
                    }
                    if !seen.insert(s.name.clone()) {
                        continue;
                    }
                    let is_matched = matched.iter().any(|m| m.name == s.name);
                    if is_matched && inline_count < 3 && !s.steps.is_empty() {
                        // Full inline expansion
                        skill_block.push_str(&format!("### {}", s.name));
                        if let Some(ref desc) = s.description {
                            skill_block.push_str(&format!(" — {}", desc));
                        }
                        if s.times_used > 0 {
                            skill_block
                                .push_str(&format!(" ({:.0}% success)", s.success_rate * 100.0));
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
                            skill_block
                                .push_str(&format!(" ({:.0}% success)", s.success_rate * 100.0));
                        }
                        skill_block.push('\n');
                    }
                }
                // Track auto-injected skills for SkillRL R4 end-of-session recording
                for s in matched.iter().chain(general.iter()) {
                    if !engineering_orchestration_intent && s.name == "bootstrap:swarm-dev-task" {
                        continue;
                    }
                    if seen.contains(&s.name) {
                        injected_skill_names.push(s.name.clone());
                    }
                }

                // Detect swarm-dev-task skill match for proactive orchestrator enforcement.
                // COMPLEXITY GATE: Only activate orchestrator mode for genuinely complex
                // multi-file tasks. Simple prompts (Q&A, single-file fixes, short requests)
                // go through normal agent mode with full tool access.
                if engineering_orchestration_intent
                    && matched.iter().any(|s| s.name == "bootstrap:swarm-dev-task")
                    && (!executive_assistant_intent || explicit_orchestration_intent)
                {
                    if orchestration_complexity_intent {
                        swarm_skill_matched = true;
                        eprintln!(
                            "[complexity-gate] PASS — prompt is complex, orchestrator mode will activate"
                        );
                    } else {
                        eprintln!(
                            "[complexity-gate] BLOCKED — prompt too simple for orchestrator mode, using normal agent"
                        );
                    }
                }

                if !skill_block.is_empty() {
                    system_prompt.push_str("\n\n# Available Procedures\n");
                    if inline_count > 0 {
                        system_prompt.push_str("Follow the steps below directly when the procedure matches. For other procedures, call `skill_search` with its name to load full steps.\n\n");
                    } else {
                        system_prompt.push_str("You have access to these proven procedures. To use one, call `skill_search` with its name to load the full steps.\n\n");
                    }
                    system_prompt.push_str(&skill_block);
                    system_prompt.push_str("\nFor missing credentials: check env vars/config first, try browser dashboard, ask user only as last resort with exact URL and key name.\n");
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
    let long_run_mode = env_optional("AGENT_LONG_RUN")
        .map(|v| v == "1")
        .unwrap_or(false)
        || is_continuation;
    if long_run_mode {
        system_prompt.push_str(concat!(
            "\n\n## Resource Guide — Long-Running Tasks\n",
            "Use subagents to parallelize or offload heavy work.\n",
            "subagent_invoke(name=\"<task-name>\", prompt=\"...\") for single tasks; subagent_batch for parallel. Choose task-specific names.\n",
            "Main loop uses expensive model (orchestration/synthesis); subagents use lighter model (research/coding/analysis).\n",
            "Use exec for shell/file/service ops. Use subagent_invoke for LLM work. Do NOT exec LLM CLIs (codex, ollama).\n",
            "Simple 1-3 step tasks are faster done directly.\n",
        ));
    }

    // --- KV-Cache Breakpoint ---
    // Everything above (system_prompt) is stable within a session.
    // Everything below (system_dynamic) churns per-turn (memory, KG).
    // Splitting them enables Anthropic prompt cache reuse on the stable prefix.
    let mut system_dynamic = String::new();

    let mut context_pack = None;
    let ea_structured_discovery_mode = executive_assistant_intent
        && !engineering_orchestration_intent
        && prompt_requests_structured_json_response(&prompt_text);
    let ea_structured_step_cap = env_optional_alias(&[
        "OPENCLAW_EA_DISCOVERY_MAX_STEPS",
        "AETHERVAULT_EA_DISCOVERY_MAX_STEPS",
    ])
    .and_then(|value| value.parse::<usize>().ok())
    .unwrap_or(8);
    let effective_max_steps = if ea_structured_discovery_mode {
        agent_cfg
            .max_steps
            .unwrap_or(max_steps)
            .min(ea_structured_step_cap.max(1))
    } else {
        agent_cfg.max_steps.unwrap_or(max_steps)
    };
    if trace_intent && ea_structured_discovery_mode {
        eprintln!(
            "[intent] bounded_ea_steps session={} cap={} original_max_steps={}",
            session_label, effective_max_steps, max_steps
        );
    }
    let explicit_context_query = context_query.clone().or(agent_cfg.context_query.clone());
    let auto_memory_prefetch = env_optional_alias(&[
        "OPENCLAW_AUTO_MEMORY_PREFETCH",
        "AETHERVAULT_AUTO_MEMORY_PREFETCH",
    ])
    .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
    .unwrap_or(false);
    let should_load_memory_context = !no_memory
        && (explicit_context_query.is_some() || (auto_memory_prefetch && !skip_trivial_prefetch));
    if should_load_memory_context {
        let memory_context_started = Instant::now();
        let qargs = QueryArgs {
            raw_query: explicit_context_query
                .unwrap_or_else(|| build_memory_query_seed(&prompt_text, &session_turns)),
            collection: Some("memory".to_string()),
            limit: agent_cfg
                .max_context_results
                .unwrap_or(context_results)
                .clamp(1, 6),
            snippet_chars: 220,
            no_expand: true,
            max_expansions: 1,
            expand_hook: None,
            expand_hook_timeout_ms: 1_500,
            no_vector: true,
            rerank: "none".to_string(),
            rerank_hook: None,
            rerank_hook_timeout_ms: 2_000,
            rerank_hook_full_text: false,
            embed_model: None,
            embed_cache: 4096,
            embed_no_cache: false,
            rerank_docs: 0,
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
        match build_context_pack(
            &db,
            qargs,
            agent_cfg
                .max_context_bytes
                .unwrap_or(context_max_bytes)
                .min(6_000),
            false,
        ) {
            Ok(pack) if !pack.context.trim().is_empty() => {
                eprintln!(
                    "[latency] memory-context session={} citations={} bytes={} elapsed_ms={}",
                    session_label,
                    pack.citations.len(),
                    pack.context.len(),
                    memory_context_started.elapsed().as_millis()
                );
                system_dynamic.push_str("\n\n# Memory Context\n");
                system_dynamic.push_str(&pack.context);
                context_pack = Some(pack);
            }
            Ok(_) => {
                eprintln!(
                    "[latency] memory-context session={} citations=0 bytes=0 elapsed_ms={}",
                    session_label,
                    memory_context_started.elapsed().as_millis()
                );
            }
            Err(err) => {
                eprintln!(
                    "[latency] memory-context session={} status=error elapsed_ms={} error={}",
                    session_label,
                    memory_context_started.elapsed().as_millis(),
                    err
                );
            }
        }
    } else if !no_memory {
        let reason = if explicit_context_query.is_none() && !auto_memory_prefetch {
            "auto-prefetch-disabled"
        } else {
            "trivial-prompt"
        };
        eprintln!(
            "[latency] memory-context session={} skipped={}",
            session_label, reason
        );
    }
    // Knowledge Graph entity auto-injection
    let kg_path = agent_workspace
        .as_ref()
        .map(|ws| ws.join("data/knowledge-graph.json"))
        .unwrap_or_else(|| crate::aethervault_home_dir().join("data/knowledge-graph.json"));
    if !skip_trivial_prefetch && kg_path.exists() {
        if let Some(kg) = load_kg_graph(&kg_path) {
            let matched = find_kg_entities(&prompt_text, &kg);
            if !matched.is_empty() {
                let kg_context = build_kg_context(&matched, &kg);
                if !kg_context.trim().is_empty() {
                    system_dynamic.push_str("\n\n# Knowledge Graph Context\n");
                    system_dynamic
                        .push_str("(Automatically matched entities from the knowledge graph)\n\n");
                    system_dynamic.push_str(&kg_context);
                }
            }
        }
    }
    eprintln!(
        "[latency] prompt-assembly session={} trivial_prefetch={} auto_memory_prefetch={} session_turns={} elapsed_ms={}",
        session_label,
        skip_trivial_prefetch,
        auto_memory_prefetch,
        session_turns.len(),
        prompt_setup_started.elapsed().as_millis()
    );

    // Inject tool capability inventory so the agent knows what it can do
    {
        let all_tools = tool_definitions_json();
        let active_names = base_tool_names();
        let discoverable: Vec<String> = all_tools
            .iter()
            .filter_map(|t| {
                t.get("name")
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string())
            })
            .filter(|n| !active_names.contains(n))
            .collect();
        let mut cap = String::from("\n\n# Available Tools\n");
        let mut sorted_active: Vec<String> = active_names.iter().cloned().collect();
        sorted_active.sort();
        for name in &sorted_active {
            let desc = all_tools
                .iter()
                .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name.as_str()))
                .and_then(|t| t.get("description").and_then(|d| d.as_str()))
                .unwrap_or("");
            let short_desc: String = desc.chars().take(80).collect();
            cap.push_str(&format!("- **{name}**: {short_desc}\n"));
        }
        if !discoverable.is_empty() {
            cap.push_str(&format!(
                "\nDiscoverable via tool_search: {}\n",
                discoverable.join(", ")
            ));
        }
        cap.push_str("\nOnly use tools listed above or discovered via tool_search.");
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
    if session.is_some() {
        let keep_from = session_turns.len().saturating_sub(10);
        for turn in &session_turns[keep_from..] {
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
        eprintln!(
            "[harness] tool_filter active: {} tools allowed (of {} in filter)",
            full_catalog.len(),
            allowed.len()
        );
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
    let is_subagent_early = session
        .as_deref()
        .map(|s| s.starts_with("subagent:"))
        .unwrap_or(false);
    if swarm_skill_matched && !is_subagent_early && tool_filter.is_none() {
        active_tools.remove("exec");
        active_tools.remove("fs_write");
        eprintln!(
            "[harness] PROACTIVE ORCHESTRATOR: swarm-dev-task skill matched — exec/fs_write stripped before first response"
        );
        // Inject explicit notice so the model never attempts exec/fs_write at step 0.
        // Without this, models hallucinate tool calls for tools not in their tool list.
        messages.push(AgentMessage {
            role: "user".to_string(),
            content: Some(
                "[TOOL NOTICE] exec and fs_write are removed. Use swarm_create + subagent_batch(name='swarm-coder') for coding tasks. Analyze with available tools first, then delegate."
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
    let log_dir = workspace_env
        .as_ref()
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
                drift_state.learned_failures = drift_state
                    .learned_failures
                    .split_off(drift_state.learned_failures.len() - 20);
            }
            if !drift_state.learned_failures.is_empty() {
                eprintln!(
                    "[drift] loaded {} learned failures from previous sessions",
                    drift_state.learned_failures.len()
                );
            }
            let prev_count = persisted
                .violations
                .get("critic_correction")
                .copied()
                .unwrap_or(0);
            eprintln!(
                "[drift] loaded {prev_count} persisted violations (reset to 0 for new session)"
            );
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
    let bg_registry_ref: Option<(i64, Arc<Mutex<BackgroundTaskRegistry>>)> =
        progress.as_ref().and_then(|p| {
            let guard = p.lock().ok()?;
            Some((guard.chat_id?, guard.bg_registry.clone()?))
        });

    // Extract session registry from progress (if running via bridge)
    let session_registry_ref: Option<Arc<Mutex<SessionRegistry>>> = progress
        .as_ref()
        .and_then(|p| p.lock().ok().and_then(|g| g.session_registry.clone()));

    // --- Orchestrator Mode ---
    // When the main agent (not a subagent) has active swarm tasks, enter orchestrator mode:
    // strip exec and fs_write so the orchestrator can only plan, delegate, and verify.
    // This enforces the OpenClaw pattern: orchestrator writes prompts, not code.
    let is_subagent = session
        .as_deref()
        .map(|s| s.starts_with("subagent:"))
        .unwrap_or(false);
    let session_has_orchestrator_history = session_turns.iter().any(|turn| {
        turn.content.contains("[Orchestrator]")
            || turn.content.contains("[SWARM MONITOR")
            || turn.content.contains("subagent_batch")
            || turn.content.contains("swarm_create")
    });
    let mut orchestrator_mode = engineering_orchestration_intent
        && (!executive_assistant_intent || explicit_orchestration_intent)
        && swarm_skill_matched
        && !is_subagent_early
        && tool_filter.is_none();
    let swarm_monitor_enabled = !is_subagent
        && (orchestrator_mode || engineering_orchestration_intent || explicit_orchestration_intent);
    if trace_intent {
        eprintln!(
            "[intent] gate session={} explicit={} executive_assistant={} engineering={} complexity={} swarm_skill_matched={} swarm_monitor_enabled={} orchestrator_history={} tool_filter={} subagent={}",
            session_label,
            explicit_orchestration_intent,
            executive_assistant_intent,
            engineering_orchestration_intent,
            orchestration_complexity_intent,
            swarm_skill_matched,
            swarm_monitor_enabled,
            session_has_orchestrator_history,
            tool_filter.is_some(),
            is_subagent,
        );
    }
    if orchestrator_mode {
        eprintln!(
            "[harness] ORCHESTRATOR MODE (proactive): swarm-dev-task skill matched, tools already stripped"
        );
    }
    if engineering_orchestration_intent
        && (!executive_assistant_intent || explicit_orchestration_intent)
        && orchestration_complexity_intent
        && session_has_orchestrator_history
        && !is_subagent
        && tool_filter.is_none()
    {
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
                    eprintln!(
                        "[harness] ORCHESTRATOR MODE: {} active swarm tasks, exec/fs_write stripped",
                        active_count
                    );
                    // Inject orchestrator mode notification
                    let task_summary: Vec<String> = running
                        .iter()
                        .chain(queued.iter())
                        .chain(pr_open.iter())
                        .chain(reviewing.iter())
                        .map(|t| format!("  - {} ({}): {}", t.id, t.status.as_str(), t.name))
                        .collect();
                    messages.push(AgentMessage {
                        role: "user".to_string(),
                        content: Some(format!(
                            "[Orchestrator] {} agents active. exec/fs_write disabled. Delegate, monitor, verify.\nTasks:\n{}",
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
    let mut tool_failure_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    // Failed attempts scratchpad: track what was tried and why it failed.
    // Injected periodically so the agent doesn't repeat the same mistakes.
    let mut failed_attempts: Vec<String> = Vec::new();

    // Inject learned failures from previous sessions as context
    if !drift_state.learned_failures.is_empty() {
        let lessons: Vec<String> = drift_state
            .learned_failures
            .iter()
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
            name: None,
            tool_call_id: None,
            is_error: None,
            thinking_blocks: vec![],
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
                         Do NOT make any more tool calls. Respond directly."
                            .to_string(),
                    ),
                    tool_calls: Vec::new(),
                    name: None,
                    tool_call_id: None,
                    is_error: None,
                    thinking_blocks: vec![],
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
                    eprintln!(
                        "[harness] injecting {} steering message(s) from user",
                        steering.len()
                    );
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
        if swarm_monitor_enabled && last_swarm_check.elapsed() >= swarm_monitor_interval {
            last_swarm_check = std::time::Instant::now();
            if let Some(ref ws) = workspace_env {
                if let Ok(sdb) = crate::swarm::open_swarm_db(ws) {
                    let check_result = crate::swarm::swarm_check_open_tasks(&sdb);
                    if !check_result.contains("No open PR tasks")
                        && !check_result.contains("no status changes")
                    {
                        eprintln!("[swarm-monitor] {}", check_result);
                        messages.push(AgentMessage {
                            role: "user".to_string(),
                            content: Some(format!(
                                "[SWARM MONITOR — automatic, deterministic check]\n{check_result}"
                            )),
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
                            let error_ctx =
                                task.error_context.as_deref().unwrap_or("unknown error");
                            let failure_kind =
                                classify_failure("swarm_task", error_ctx, &serde_json::json!({}));
                            let strategy = match failure_kind {
                                FailureKind::Transient => {
                                    "STRATEGY: Transient error (timeout/rate-limit). Retry with same approach but add \
                                     error handling or timeout extension in the prompt."
                                }
                                FailureKind::Permanent => {
                                    "STRATEGY: Permanent error (auth/permission/not-found). Do NOT retry the same approach. \
                                     Investigate root cause first, then try a fundamentally different method. \
                                     If the task is impossible, mark it done with an explanation."
                                }
                                FailureKind::ApiMisuse => {
                                    "STRATEGY: API misuse (wrong request shape/schema). The request payload doesn't match the API spec. \
                                     Rewrite the prompt to include the EXACT API schema. Tell the agent to read the API docs first, \
                                     then construct the request. Include the validation error so the agent knows what field is wrong."
                                }
                                FailureKind::Semantic => {
                                    "STRATEGY: Logic/parsing error. Rewrite the prompt with more specific instructions. \
                                     Include the exact error so the new agent avoids the same mistake."
                                }
                            };
                            let retry_prompt = format!(
                                "[SWARM MONITOR — RETRY NEEDED]\n\
                                 Task '{}' (id: {}) FAILED (attempt {}/{}) | Type: {:?}\n\
                                 Error: {}\n\n\
                                 {}\n\n\
                                 Original prompt (first 500 chars): {}",
                                task.name,
                                task.id,
                                task.retry_count + 1,
                                task.max_retries,
                                failure_kind,
                                error_ctx,
                                strategy,
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
                            eprintln!(
                                "[swarm-monitor] injected retry for task {} (failure: {:?})",
                                task.id, failure_kind
                            );
                        }
                    }
                    // Check for CI-passing tasks that need cross-model review
                    let reviewing =
                        crate::swarm::swarm_list_tasks(&sdb, Some("reviewing"), Some(50));
                    for task in &reviewing {
                        if task.review_status.as_deref() == Some("pending")
                            || task.review_status.is_none()
                        {
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
                                eprintln!(
                                    "[swarm-monitor] injected review dispatch for task {} (PR #{})",
                                    task.id, pr_num
                                );
                            }
                        }
                    }
                    // Re-check orchestrator mode: if all tasks done, restore tools.
                    // BUT: if orchestrator was activated proactively (skill match), only restore
                    // when tasks were actually created AND completed — not just because the DB
                    // has no active tasks (which is the initial state).
                    if orchestrator_mode {
                        // Auto-fail stale "running" tasks (>2 hours with no update)
                        {
                            let stale_running =
                                crate::swarm::swarm_list_tasks(&sdb, Some("running"), Some(100));
                            let now_utc = chrono::Utc::now();
                            for task in &stale_running {
                                if let Ok(updated) = chrono::NaiveDateTime::parse_from_str(
                                    &task.updated_at,
                                    "%Y-%m-%dT%H:%M:%S%.fZ",
                                )
                                .or_else(|_| {
                                    chrono::NaiveDateTime::parse_from_str(
                                        &task.updated_at,
                                        "%Y-%m-%dT%H:%M:%SZ",
                                    )
                                }) {
                                    let age = now_utc.naive_utc() - updated;
                                    if age > chrono::Duration::hours(2) {
                                        eprintln!(
                                            "[harness] Auto-failing stale swarm task {} (running for {}h)",
                                            task.id,
                                            age.num_hours()
                                        );
                                        let _ = crate::swarm::swarm_update_task(
                                            &sdb,
                                            &task.id,
                                            Some("failed"),
                                            None,
                                            None,
                                            None,
                                            None,
                                            None,
                                            None,
                                            Some("auto-failed: stale, no update for 2+ hours"),
                                            None,
                                            None,
                                        );
                                    }
                                }
                            }
                        }
                        let running =
                            crate::swarm::swarm_list_tasks(&sdb, Some("running"), Some(1));
                        let queued = crate::swarm::swarm_list_tasks(&sdb, Some("queued"), Some(1));
                        let pr_open =
                            crate::swarm::swarm_list_tasks(&sdb, Some("pr_open"), Some(1));
                        let reviewing =
                            crate::swarm::swarm_list_tasks(&sdb, Some("reviewing"), Some(1));
                        let no_active = running.is_empty()
                            && queued.is_empty()
                            && pr_open.is_empty()
                            && reviewing.is_empty();
                        // Restore tools when no active tasks remain (including after stale auto-fail).
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
                            eprintln!(
                                "[harness] ORCHESTRATOR MODE OFF: all swarm tasks complete, full tools restored"
                            );
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
            eprintln!(
                "[harness] context at ~{token_estimate} tokens (budget {compact_at}), compacting..."
            );
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
        let hook_started = Instant::now();

        // Try streaming first when a progress handle is available (Telegram bridge).
        // This enables live thinking/response display. Falls back to blocking on any error.
        let message = if progress.is_some() {
            match call_agent_hook_streaming(&model_spec, &request) {
                Ok(rx) => {
                    let prog = progress.as_ref().unwrap();
                    // Set phase to Thinking before consuming
                    if let Ok(mut p) = prog.lock() {
                        p.stream_phase = StreamPhase::Thinking;
                        p.stream_thinking = None;
                        p.stream_response = None;
                        p.stream_message_id = None;
                    }
                    match consume_stream(rx, prog) {
                        Ok(msg) => {
                            consecutive_hook_failures = 0;
                            // Reset streaming state after successful consumption
                            if let Ok(mut p) = prog.lock() {
                                p.stream_phase = StreamPhase::Idle;
                                p.stream_thinking = None;
                                p.stream_response = None;
                                p.stream_message_id = None;
                            }
                            eprintln!(
                                "[latency] model-hook session={} step={} mode=streaming elapsed_ms={}",
                                session_label,
                                step,
                                hook_started.elapsed().as_millis()
                            );
                            msg
                        }
                        Err(e) => {
                            eprintln!("[harness] streaming failed ({e}), falling back to blocking");
                            if let Ok(mut p) = prog.lock() {
                                p.stream_phase = StreamPhase::Idle;
                                p.stream_thinking = None;
                                p.stream_response = None;
                                p.stream_message_id = None;
                            }
                            // Fall back to blocking call
                            match call_agent_hook(&model_spec, &request) {
                                Ok(msg) => {
                                    consecutive_hook_failures = 0;
                                    eprintln!(
                                        "[latency] model-hook session={} step={} mode=blocking-fallback elapsed_ms={}",
                                        session_label,
                                        step,
                                        hook_started.elapsed().as_millis()
                                    );
                                    msg
                                }
                                Err(e2) => {
                                    consecutive_hook_failures += 1;
                                    eprintln!(
                                        "[latency] model-hook session={} step={} mode=blocking-fallback status=error elapsed_ms={}",
                                        session_label,
                                        step,
                                        hook_started.elapsed().as_millis()
                                    );
                                    eprintln!(
                                        "[harness] hook failed ({consecutive_hook_failures}/{MAX_CONSECUTIVE_HOOK_FAILURES}): {e2}"
                                    );
                                    if consecutive_hook_failures >= MAX_CONSECUTIVE_HOOK_FAILURES {
                                        final_text = Some(format!(
                                            "(Agent terminated: model hook failed {MAX_CONSECUTIVE_HOOK_FAILURES} \
                                             consecutive times. Last error: {e2})"
                                        ));
                                        break;
                                    }
                                    AgentMessage {
                                        role: "assistant".to_string(),
                                        content: Some(format!(
                                            "(Model hook error on attempt {consecutive_hook_failures}: {e2}. Will retry on next step.)"
                                        )),
                                        tool_calls: Vec::new(),
                                        name: None,
                                        tool_call_id: None,
                                        is_error: None,
                                        thinking_blocks: vec![],
                                    }
                                }
                            }
                        }
                    }
                }
                Err(_) => {
                    // Non-claude hook or streaming not supported — use blocking path
                    match call_agent_hook(&model_spec, &request) {
                        Ok(msg) => {
                            consecutive_hook_failures = 0;
                            eprintln!(
                                "[latency] model-hook session={} step={} mode=blocking elapsed_ms={}",
                                session_label,
                                step,
                                hook_started.elapsed().as_millis()
                            );
                            msg
                        }
                        Err(e) => {
                            consecutive_hook_failures += 1;
                            eprintln!(
                                "[latency] model-hook session={} step={} mode=blocking status=error elapsed_ms={}",
                                session_label,
                                step,
                                hook_started.elapsed().as_millis()
                            );
                            eprintln!(
                                "[harness] hook failed ({consecutive_hook_failures}/{MAX_CONSECUTIVE_HOOK_FAILURES}): {e}"
                            );
                            if consecutive_hook_failures >= MAX_CONSECUTIVE_HOOK_FAILURES {
                                final_text = Some(format!(
                                    "(Agent terminated: model hook failed {MAX_CONSECUTIVE_HOOK_FAILURES} \
                                     consecutive times. Last error: {e})"
                                ));
                                break;
                            }
                            AgentMessage {
                                role: "assistant".to_string(),
                                content: Some(format!(
                                    "(Model hook error on attempt {consecutive_hook_failures}: {e}. Will retry on next step.)"
                                )),
                                tool_calls: Vec::new(),
                                name: None,
                                tool_call_id: None,
                                is_error: None,
                                thinking_blocks: vec![],
                            }
                        }
                    }
                }
            }
        } else {
            // No progress handle — blocking path only (CLI mode, subagents)
            match call_agent_hook(&model_spec, &request) {
                Ok(msg) => {
                    consecutive_hook_failures = 0;
                    eprintln!(
                        "[latency] model-hook session={} step={} mode=blocking elapsed_ms={}",
                        session_label,
                        step,
                        hook_started.elapsed().as_millis()
                    );
                    msg
                }
                Err(e) => {
                    consecutive_hook_failures += 1;
                    eprintln!(
                        "[latency] model-hook session={} step={} mode=blocking status=error elapsed_ms={}",
                        session_label,
                        step,
                        hook_started.elapsed().as_millis()
                    );
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
                        break;
                    }
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
            }
        };
        // Update progress text_preview from thinking blocks (even without text content)
        if !message.thinking_blocks.is_empty() {
            if let Some(ref prog) = progress {
                if let Ok(mut p) = prog.lock() {
                    let thinking_text: String = message
                        .thinking_blocks
                        .iter()
                        .filter_map(|tb| tb.get("thinking").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !thinking_text.is_empty() {
                        // Take the LAST 100 chars of thinking (most recent reasoning)
                        let chars: Vec<char> = thinking_text.chars().collect();
                        let snippet = if chars.len() > 100 {
                            format!(
                                "...{}",
                                chars[chars.len() - 97..].iter().collect::<String>()
                            )
                        } else {
                            thinking_text
                        };
                        p.text_preview = Some(snippet);
                    }
                }
            }
        }
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
                let snapshot: String = messages
                    .iter()
                    .filter(|m| m.role == "user" || m.role == "assistant")
                    .rev()
                    .take(8)
                    .filter_map(|m| {
                        m.content.as_ref().map(|c| {
                            let preview: String = c.chars().take(300).collect();
                            format!("[{}] {}", m.role, preview)
                        })
                    })
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
                                        let mut seen = OBSERVATION_DEDUP
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        if seen.len() >= OBSERVATION_DEDUP_CAP {
                                            seen.clear();
                                        }
                                        if !seen.insert(hash) {
                                            eprintln!(
                                                "[observation-dedup] skipped duplicate: {}...",
                                                &facts.chars().take(60).collect::<String>()
                                            );
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
                                        match put_with_consolidation(
                                            &obs_db,
                                            facts.as_bytes(),
                                            opts,
                                        ) {
                                            Ok(result) => {
                                                let decision_str = format!("{:?}", result.decision);
                                                if result.frame_id.is_none() {
                                                    eprintln!(
                                                        "[observation-consolidation] NOOP: {decision_str}"
                                                    );
                                                } else {
                                                    eprintln!(
                                                        "[observation-consolidation] {decision_str}"
                                                    );
                                                }
                                            }
                                            Err(e) => {
                                                eprintln!(
                                                    "[observation] consolidation failed: {e}"
                                                );
                                            }
                                        }
                                        if let Err(e) = obs_db.commit() {
                                            eprintln!("[observation] commit failed: {e}");
                                        }
                                    }
                                } else if !facts.trim().is_empty() {
                                    eprintln!(
                                        "[observation-gate] skipped: {}...",
                                        &facts.chars().take(60).collect::<String>()
                                    );
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
            let has_recent_status_check = tool_calls.iter().any(|c| {
                c.name == "session_status" || c.name.contains("subagent") || c.name == "bg_status"
            });
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
            let orchestrator_blocked: Vec<String> = tool_calls
                .iter()
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
                                "BLOCKED: {} disabled in orchestrator mode. Use swarm_create + subagent_batch.",
                                call.name
                            ),
                            details: serde_json::json!({ "blocked": true, "reason": "orchestrator_mode" }),
                            is_error: true,
                        });
                        messages.push(AgentMessage {
                            role: "tool".to_string(),
                            content: Some(format!(
                                "BLOCKED: {} disabled in orchestrator mode. Delegate via swarm_create + subagent_batch.",
                                call.name
                            )),
                            tool_calls: Vec::new(),
                            name: Some(call.name.clone()),
                            tool_call_id: Some(call.id.clone()),
                            is_error: Some(true),
                            thinking_blocks: vec![],
                        });
                        eprintln!(
                            "[orchestrator] BLOCKED {} — agent tried to bypass orchestrator mode",
                            call.name
                        );
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
                            "[ORCHESTRATOR MODE] exec/fs_write DISABLED. Use swarm_create + subagent_batch(name='swarm-coder') with branch params. Do not retry exec/fs_write."
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
            let exfil_blocked: Vec<String> = tool_calls
                .iter()
                .filter(|c| {
                    matches!(
                        c.name.as_str(),
                        "exec"
                            | "email_send"
                            | "gmail_send"
                            | "signal_send"
                            | "imessage_send"
                            | "notify"
                    ) || (c.name == "http_request" && {
                        let method = c
                            .args
                            .get("method")
                            .and_then(|v| v.as_str())
                            .unwrap_or("GET")
                            .to_ascii_uppercase();
                        method != "GET"
                    })
                })
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
                            content: Some(format!("BLOCKED: Tainted session (untrusted: {sources} + private data). Ask user for approval.")),
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
                            "[SECURITY] Tainted session (untrusted: {sources} + private data). Tools blocked: {}. Ask user for explicit approval.",
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
                    eprintln!(
                        "[circuit-breaker] blocked {}:{} after {count} failures",
                        call.name,
                        &key[call.name.len() + 1..std::cmp::min(key.len(), call.name.len() + 9)]
                    );

                    // Extract a learned failure lesson when circuit breaker triggers
                    let pattern_detail = match call.name.as_str() {
                        "exec" => call
                            .args
                            .get("command")
                            .and_then(|v| v.as_str())
                            .map(|s| s.chars().take(120).collect::<String>())
                            .unwrap_or_else(|| call.args.to_string().chars().take(80).collect()),
                        "http_request" => {
                            let method = call
                                .args
                                .get("method")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            let url = call.args.get("url").and_then(|v| v.as_str()).unwrap_or("?");
                            format!("{} {}", method, url.chars().take(100).collect::<String>())
                        }
                        _ => call.args.to_string().chars().take(80).collect(),
                    };
                    let lesson = LearnedFailure {
                        tool: call.name.clone(),
                        pattern: pattern_detail,
                        lesson: format!(
                            "Failed {} times and was blocked by circuit breaker. Try a fundamentally different approach or verify prerequisites.",
                            count
                        ),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    };
                    if !drift_state
                        .learned_failures
                        .iter()
                        .any(|lf| lf.tool == lesson.tool && lf.pattern == lesson.pattern)
                    {
                        drift_state.learned_failures.push(lesson);
                    }
                }
                let broken_ids: std::collections::HashSet<String> =
                    circuit_broken.iter().map(|(c, _)| c.id.clone()).collect();
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
                call,
                result,
                &mut tool_results,
                &mut messages,
                &mut active_tools,
                &mut retrieved_skills,
                &mut session_taint,
                should_log,
                &session,
                &log_dir,
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
                            "[Remote detected] Run discovery first: `df -h`, `nvidia-smi`, `python3 --version`, `echo $HF_HOME $CUDA_HOME`, `free -h`. Do not skip.".to_string()
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
                    if output_lower.contains("filesystem")
                        || output_lower.contains("nvidia-smi")
                        || output_lower.contains("mem:")
                    {
                        reminder_state.remote_env_verified = true;
                    }
                }
            }

            // Inject grounding requirement after subagent-related tool results
            if call.name == "subagent_invoke"
                || call.name == "subagent_batch"
                || call.name == "session_status"
            {
                messages.push(AgentMessage {
                    role: "user".to_string(),
                    content: Some("[Grounding] Quote subagent output exactly. Do not paraphrase or embellish. Report errors/empty results honestly.".to_string()),
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
                        let output_preview: String = tool_results
                            .last()
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
                        let err_snippet: String = tool_results
                            .last()
                            .map(|r| r.output.chars().take(150).collect())
                            .unwrap_or_default();
                        let pattern_detail = match call.name.as_str() {
                            "exec" => call
                                .args
                                .get("command")
                                .and_then(|v| v.as_str())
                                .map(|s| s.chars().take(120).collect::<String>())
                                .unwrap_or_else(|| {
                                    call.args.to_string().chars().take(80).collect()
                                }),
                            "http_request" => {
                                let method = call
                                    .args
                                    .get("method")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("?");
                                let url =
                                    call.args.get("url").and_then(|v| v.as_str()).unwrap_or("?");
                                format!("{} {}", method, url.chars().take(100).collect::<String>())
                            }
                            _ => call.args.to_string().chars().take(80).collect(),
                        };
                        let lesson = LearnedFailure {
                            tool: call.name.clone(),
                            pattern: pattern_detail,
                            lesson: format!(
                                "Failed twice with: {}. Change approach or verify prerequisites.",
                                err_snippet
                            ),
                            created_at: chrono::Utc::now().to_rfc3339(),
                        };
                        if !drift_state
                            .learned_failures
                            .iter()
                            .any(|lf| lf.tool == lesson.tool && lf.pattern == lesson.pattern)
                        {
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
            let read_only_tools = [
                "search",
                "query",
                "get",
                "list",
                "tool_search",
                "skill_search",
                "reflect",
            ];
            if read_only_tools.iter().any(|t| call.name.contains(t)) {
                reminder_state.sequential_read_ops += 1;
            } else {
                reminder_state.sequential_read_ops = 0;
            }
        } else {
            // Multiple tool calls — execute in parallel (non-MCP), MCP calls sequentially
            let (mcp_calls, regular_calls): (Vec<_>, Vec<_>) =
                tool_calls.iter().partition(|c| c.name.starts_with("mcp__"));

            let mut results: Vec<(AgentToolCall, ToolExecution)> = Vec::new();

            // Regular tools run in a bounded worker pool.
            if !regular_calls.is_empty() {
                let mv2_ref = &mv2;
                let bg_reg_ref = &bg_registry_ref;
                let sess_reg_ref = &session_registry_ref;
                let execute_regular_call =
                    |call: &&AgentToolCall| -> (AgentToolCall, ToolExecution) {
                        let call = *call;
                        let result = catch_unwind(AssertUnwindSafe(|| {
                            let local_db = open_or_create_db(mv2_ref).map_err(|e| e.to_string())?;
                            execute_tool(
                                &call.name,
                                call.args.clone(),
                                mv2_ref,
                                &local_db,
                                false,
                                bg_reg_ref.clone(),
                                sess_reg_ref.clone(),
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
                                ToolExecution {
                                    output: format!("Tool panicked: {msg}"),
                                    details: serde_json::json!({ "error": "panic", "message": msg }),
                                    is_error: true,
                                }
                            }
                        };

                        (call.clone(), execution)
                    };

                let parallel_results: Vec<(AgentToolCall, ToolExecution)> =
                    ThreadPoolBuilder::new()
                        .num_threads(
                            std::thread::available_parallelism()
                                .map(|v| v.get())
                                .unwrap_or(4)
                                .min(regular_calls.len()),
                        )
                        .build()
                        .map(|pool| {
                            pool.install(|| {
                                regular_calls.par_iter().map(execute_regular_call).collect()
                            })
                        })
                        .unwrap_or_else(|_| {
                            regular_calls.iter().map(execute_regular_call).collect()
                        });
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
                    &call,
                    result,
                    &mut tool_results,
                    &mut messages,
                    &mut active_tools,
                    &mut retrieved_skills,
                    &mut session_taint,
                    should_log,
                    &session,
                    &log_dir,
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
                            let output_preview: String = tool_results
                                .last()
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
                            let err_snippet: String = tool_results
                                .last()
                                .map(|r| r.output.chars().take(150).collect())
                                .unwrap_or_default();
                            let pattern_detail = match call.name.as_str() {
                                "exec" => call
                                    .args
                                    .get("command")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.chars().take(120).collect::<String>())
                                    .unwrap_or_else(|| {
                                        call.args.to_string().chars().take(80).collect()
                                    }),
                                "http_request" => {
                                    let method = call
                                        .args
                                        .get("method")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("?");
                                    let url = call
                                        .args
                                        .get("url")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("?");
                                    format!(
                                        "{} {}",
                                        method,
                                        url.chars().take(100).collect::<String>()
                                    )
                                }
                                _ => call.args.to_string().chars().take(80).collect(),
                            };
                            let lesson = LearnedFailure {
                                tool: call.name.clone(),
                                pattern: pattern_detail,
                                lesson: format!(
                                    "Failed twice with: {}. Change approach or verify prerequisites.",
                                    err_snippet
                                ),
                                created_at: chrono::Utc::now().to_rfc3339(),
                            };
                            if !drift_state
                                .learned_failures
                                .iter()
                                .any(|lf| lf.tool == lesson.tool && lf.pattern == lesson.pattern)
                            {
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
                    if output_lower.contains("filesystem")
                        || output_lower.contains("nvidia-smi")
                        || output_lower.contains("mem:")
                    {
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
        let reminders =
            collect_mid_loop_reminders(&reminder_state, step, current_max_steps, token_est);

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
        let budget_msg = format!("Step {}/{}", step + 1, current_max_steps);
        all_reminders.push(budget_msg);

        // Resource-awareness: nudge delegation to free compute when in long-run mode
        if long_run_mode {
            if let Some(ref prog) = progress {
                if let Ok(p) = prog.lock() {
                    if step > 20 && p.delegated_steps == 0 {
                        all_reminders.push(
                            "Consider subagent_invoke/subagent_batch for heavy work.".to_string(),
                        );
                    } else if step > 30 && p.opus_steps > 0 {
                        let total = p.opus_steps + p.delegated_steps;
                        let opus_ratio = p.opus_steps as f64 / total.max(1) as f64;
                        if opus_ratio > 0.9 {
                            all_reminders.push("Consider using subagents.".to_string());
                        }
                    }
                }
            }
        }

        // Cycle detection: catch repeated action patterns
        if let Some((cycle_len, _repeats)) = detect_cycle(&recent_actions) {
            if cycle_len == 1 {
                all_reminders
                    .push("Same action repeated 3x. Try a different approach.".to_string());
            } else {
                all_reminders.push(format!(
                    "{cycle_len}-step loop detected. Try a fundamentally different strategy."
                ));
            }
            reminder_state.no_progress_streak += 3;
        }

        // Goal recitation: periodically re-inject the user's goal
        if plan_recite_interval > 0 && step > 0 && step % plan_recite_interval == 0 {
            if let Some(ref plan) = current_plan {
                all_reminders.push(format!(
                    "Goal: {}. Step {}/{current_max_steps}. Stay focused.",
                    plan, step
                ));
            }
        }

        // Drift-based escalation
        if drift_score < 70.0 && drift_score >= 55.0 {
            all_reminders.push("Adherence degrading. Be careful and concise.".to_string());
        } else if drift_score < 55.0 {
            all_reminders.push("Low adherence. Re-state goal, then one careful step.".to_string());
        }
        if drift_state.ema < 40.0 && drift_state.turns >= 3 {
            all_reminders
                .push("Sustained low adherence. Finish current action, give status.".to_string());
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
                                all_reminders
                                    .push(format!("Re-anchor with proven strategies:\n{anchor}"));
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
        let current_violation_count = drift_state
            .violations
            .get("critic_correction")
            .copied()
            .unwrap_or(0);
        if critic_should_fire(
            step,
            critic_interval,
            &mut last_critic_step,
            &reminder_state,
            &tool_calls,
            &messages,
            current_violation_count,
        ) {
            if let Some(correction) = call_critic(&prompt_text, &messages, step, current_max_steps)
            {
                // Don't add to all_reminders. Inject as separate message.
                let critic_msg = format!(
                    "[CORRECTION]\n{}\nAcknowledge before continuing.",
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
                drift_state
                    .violations
                    .entry("critic_correction".to_string())
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
                // Persist violations to disk
                if let Ok(json) = serde_json::to_string(&drift_state) {
                    let _ = std::fs::write(&drift_path, json);
                }

                // Model escalation: swap to Opus for next N steps when critic fires
                if let Some(ref opus_spec) = opus_escalation_spec {
                    if opus_escalation_remaining == 0 {
                        eprintln!(
                            "[harness] critic fired — escalating to Opus for {opus_escalation_steps} steps"
                        );
                        model_spec = opus_spec.clone();
                        opus_escalation_remaining = opus_escalation_steps;
                    }
                }

                // Progressive escalation based on violation count
                let violation_count = drift_state
                    .violations
                    .get("critic_correction")
                    .copied()
                    .unwrap_or(0);
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
                            format!(
                                "[SEVERE WARNING] {violation_count} grounding violations this session. STOP making claims not supported by tool output. Before EVERY response, re-read the most recent tool output and ONLY report what it literally says. For browser: call snapshot after EVERY action. Your subagent tools remain available — use them to delegate work, but ground ALL claims in tool output."
                            )
                        } else {
                            format!(
                                "[SEVERE WARNING] {violation_count} grounding violations this session. STOP making claims not supported by tool output. Before EVERY response, re-read the most recent tool output and ONLY report what it literally says. For browser: call snapshot after EVERY action. Subagent tools have been REVOKED."
                            )
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
                            let subagent_tool_names =
                                ["subagent_invoke", "subagent_batch", "session_start"];
                            for tool_name in &subagent_tool_names {
                                active_tools.remove(*tool_name);
                            }
                            tools = tools_from_active(&tool_map, &active_tools);
                            eprintln!("[critic] LEVEL 3: subagent tools restricted");
                        } else if skip_subagent_restriction {
                            eprintln!(
                                "[critic] LEVEL 3: subagent tools PRESERVED (orchestrator mode active)"
                            );
                        }
                        // Enforce: reduce remaining step budget by 1/4 (was 1/3)
                        let remaining = current_max_steps.saturating_sub(step);
                        current_max_steps = step + (remaining * 3 / 4).max(8);
                        eprintln!(
                            "[critic] LEVEL 3 enforcement: step budget reduced to {current_max_steps} (was {})",
                            step + remaining
                        );
                    }
                    _ => {
                        // Level 4: Graceful wind-down (raised from 7+ to 12+)
                        eprintln!(
                            "[critic] LEVEL 4 escalation: {violation_count} violations — winding down gracefully"
                        );
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
                        eprintln!(
                            "[critic] LEVEL 4 enforcement: graceful wind-down in 8 steps (step={step}, max={current_max_steps})"
                        );
                    }
                }
            }
        }

        // Checkpoint-and-report every 10 steps
        if step > 0 && step % 10 == 0 {
            let mut checkpoint_msg = format!(
                "[Checkpoint — Step {}] Summarize what you have accomplished so far and what you plan to do next. \
                 If the user's request was vague, confirm you are on the right track.",
                step
            );
            // Inject failed attempts scratchpad so the agent doesn't repeat mistakes
            if !failed_attempts.is_empty() {
                checkpoint_msg.push_str(&format!(
                    "\n\n[Failed Attempts — DO NOT RETRY these exact approaches]\n{}",
                    failed_attempts
                        .iter()
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
                    drift_state
                        .learned_failures
                        .iter()
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
        let substantive_tools: HashSet<&str> =
            ["exec", "http_request", "browser", "fs_write", "skill_store"]
                .iter()
                .cloned()
                .collect();
        let used_substantive = tool_results
            .iter()
            .any(|r| substantive_tools.contains(r.name.as_str()));
        if used_substantive {
            // Build a compact summary of what was done for distillation
            let action_summary: String = tool_results
                .iter()
                .filter(|r| !r.is_error)
                .take(10)
                .map(|r| {
                    format!(
                        "- {}: {}",
                        r.name,
                        r.output.chars().take(100).collect::<String>()
                    )
                })
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
                    messages: vec![AgentMessage {
                        role: "user".to_string(),
                        content: Some(distill_prompt),
                        tool_calls: Vec::new(),
                        name: None,
                        tool_call_id: None,
                        is_error: None,
                        thinking_blocks: vec![],
                    }],
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
                                            let steps: Vec<String> = skill_json
                                                .get("steps")
                                                .and_then(|v| v.as_array())
                                                .map(|arr| {
                                                    arr.iter()
                                                        .filter_map(|v| {
                                                            v.as_str().map(String::from)
                                                        })
                                                        .collect()
                                                })
                                                .unwrap_or_default();
                                            let notes = skill_json
                                                .get("notes")
                                                .and_then(|v| v.as_str())
                                                .map(String::from);
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
                                                eprintln!(
                                                    "[skill-distill] learned new skill: {name}"
                                                );
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
        let summary = messages
            .iter()
            .find(|m| {
                m.role == "user"
                    && m.content
                        .as_ref()
                        .map(|c| c.contains("[Context compacted"))
                        .unwrap_or(false)
            })
            .and_then(|m| m.content.clone())
            .unwrap_or_else(|| {
                messages
                    .iter()
                    .rev()
                    .find(|m| m.role == "assistant")
                    .and_then(|m| m.content.as_ref())
                    .map(|c| c.chars().take(500).collect::<String>())
                    .unwrap_or_default()
            });

        let remaining_work = messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .and_then(|m| m.content.as_ref())
            .map(|c| c.chars().take(300).collect::<String>())
            .unwrap_or_else(|| "Continue working toward the goal.".to_string());

        let chain_depth = session
            .as_ref()
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
        let checkpoint_dir = crate::checkpoint_store_dir();
        let _ = fs::create_dir_all(&checkpoint_dir);
        let checkpoint_path = checkpoint_dir.join(format!(
            "{}.json",
            checkpoint
                .session
                .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
        ));
        if let Ok(json) = serde_json::to_string_pretty(&checkpoint) {
            let _ = fs::write(&checkpoint_path, &json);
        }

        let continuation_marker = format!("[CONTINUATION_NEEDED:{}]", checkpoint_path.display());

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

#[cfg(test)]
mod tests {
    use super::{
        build_user_intent_context, prompt_file_reference_count, prompt_has_engineering_intent,
        prompt_has_executive_assistant_intent, prompt_is_complex,
    };
    use crate::SessionTurn;

    #[test]
    fn engineering_intent_detects_multi_file_build_prompt() {
        let prompt = r#"
        Build a real application in /tmp/linus-battery-av-123.
        Include package.json, src/, public/index.html, node:test coverage,
        a CLI, HTTP endpoints, and run the test suite yourself.
        "#;
        assert!(prompt_has_engineering_intent(prompt));
        assert!(prompt_is_complex(prompt));
    }

    #[test]
    fn engineering_intent_rejects_flight_planning_prompt() {
        let prompt = r#"
        I need to find flights for my parents. Figure out who they are from memory
        or my inbox if needed. Ask the smart questions you actually need, infer
        whether the likely destination is my home, and only ask what remains ambiguous.
        "#;
        assert!(prompt_has_executive_assistant_intent(prompt));
        assert!(!prompt_has_engineering_intent(prompt));
    }

    #[test]
    fn engineering_intent_ignores_assistant_oauth_noise_for_ea_followup() {
        let session_turns = vec![
            SessionTurn {
                role: "user".to_string(),
                content: "Help me find flights for my parents to visit me in Boca.".to_string(),
                timestamp: 0,
            },
            SessionTurn {
                role: "assistant".to_string(),
                content: "I inferred the likely route but Gmail OAuth failed while checking inbox."
                    .to_string(),
                timestamp: 1,
            },
        ];
        let prompt = "They should come around April 18 and stay about a week.";
        let intent_context = build_user_intent_context(prompt, &session_turns);
        assert!(!prompt_has_engineering_intent(&intent_context));
    }

    #[test]
    fn engineering_intent_does_not_match_short_terms_inside_human_words() {
        let prompt = "Search my inbox for prior specialists and help me schedule the right doctor appointment.";
        assert!(prompt_has_executive_assistant_intent(prompt));
        assert!(!prompt_has_engineering_intent(prompt));
    }

    #[test]
    fn slash_separated_human_phrases_are_not_counted_as_file_references() {
        let prompt =
            "Do not ask the dumb baseline version of origin/destination if you can infer better.";
        assert_eq!(prompt_file_reference_count(prompt), 0);
        assert!(!prompt_has_engineering_intent(prompt));
    }

    #[test]
    fn doctor_appointment_prompt_stays_out_of_complex_orchestrator_mode() {
        let prompt = r#"
        I need to get a doctor appointment sorted out. Search memory and inbox for
        prior specialists, referral context, and anything Rhaine handled before.
        Ask only the smart missing questions and propose whether email, portal,
        Rhaine, or a direct phone call is the right next action.
        "#;
        assert!(!prompt_has_engineering_intent(prompt));
        assert!(!prompt_is_complex(prompt));
    }
}

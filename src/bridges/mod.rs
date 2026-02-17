pub(crate) mod slack;
pub(crate) mod telegram;
pub(crate) mod whatsapp;
pub(crate) mod webhook;

pub(crate) use telegram::*;

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use chrono::Utc;

use crate::{
    call_claude, env_optional, open_skill_db, run_agent_with_prompt, upsert_skill,
    AgentHookRequest, AgentMessage, AgentProgress, AgentRunOutput,
    BridgeAgentConfig, BridgeCommand, DistilledSkill, SkillRecord, DEFAULT_WORKSPACE_DIR,
};
use self::telegram::run_telegram_bridge;
use self::whatsapp::run_whatsapp_bridge;
use self::slack::run_slack_bridge;
use self::webhook::{
    extract_discord_event, extract_imessage_event, extract_matrix_event, extract_signal_event,
    extract_teams_event, reply_none, run_webhook_bridge,
};

pub(crate) fn resolve_mv2_path(cli_mv2: Option<PathBuf>) -> PathBuf {
    if let Some(path) = cli_mv2 {
        return path;
    }
    if let Some(value) = env_optional("AETHERVAULT_MV2") {
        return PathBuf::from(value);
    }
    PathBuf::from("./data/knowledge.mv2")
}

pub(crate) fn resolve_bridge_model_hook(cli: Option<String>) -> Option<String> {
    if cli.is_some() {
        return cli;
    }
    if env_optional("ANTHROPIC_API_KEY").is_some() && env_optional("ANTHROPIC_MODEL").is_some() {
        return Some("builtin:claude".to_string());
    }
    None
}

pub(crate) fn build_bridge_agent_config(
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
    let model_hook = resolve_bridge_model_hook(model_hook);
    let system = system;
    let no_memory = no_memory;
    let context_query = context_query;
    let context_results = context_results;
    let context_max_bytes = context_max_bytes;
    let max_steps = max_steps;
    let log_commit_interval = log_commit_interval.max(1);
    let log = log;
    let session_prefix = String::new();

    Ok(BridgeAgentConfig {
        mv2,
        model_hook,
        system,
        no_memory,
        context_query,
        context_results,
        context_max_bytes,
        max_steps,
        log,
        log_commit_interval,
        session_prefix,
    })
}

pub(crate) fn run_agent_for_bridge(
    config: &BridgeAgentConfig,
    prompt: &str,
    session: String,
    system_override: Option<String>,
    model_hook_override: Option<String>,
    progress: Option<Arc<Mutex<AgentProgress>>>,
) -> Result<AgentRunOutput, String> {
    let (tx, rx) = mpsc::channel();
    let session_for_distill = session.clone();
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
            log,
            progress,
        )
        .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });

    // No timeout — let the agent run as long as it needs.
    // The agent is bounded by max_steps, not wall-clock time.
    // Long-running tasks (dev work, swarms, batch processing) can take hours.
    let result = rx.recv().map_err(|err| format!("Agent channel error: {err}"))?;

    // SkillRL R2+R3: Fire-and-forget skill distillation based on outcome
    let workspace = std::env::var("AETHERVAULT_WORKSPACE")
        .ok()
        .map(PathBuf::from);
    match &result {
        Ok(output) => {
            distill_skills_from_run(output, workspace.clone());
        }
        Err(err_msg) => {
            distill_failure_skill(err_msg, &session_for_distill, workspace);
        }
    }

    result
}

// --- SkillRL: Distillation prompts ---

const SKILL_DISTILL_PROMPT: &str = r#"You are a skill distiller. Analyze the agent trajectory and extract 1-3 reusable behavioral patterns.

For each skill, output JSON:
[
  {
    "title": "3-5 word name",
    "principle": "1-2 sentence actionable strategy",
    "when_to_apply": "conditions when this skill applies"
  }
]

Rules:
- Only extract genuinely reusable patterns, not task-specific trivia
- Focus on strategies that succeeded, not routine operations
- Each skill should be applicable across different tasks
- Output ONLY the JSON array, nothing else"#;

const FAILURE_DISTILL_PROMPT: &str = r#"You are a failure analyst. Given an agent session that failed, extract 1-2 prevention skills.

For each skill, output JSON:
[
  {
    "title": "3-5 word name (what to avoid)",
    "principle": "What went wrong and how to prevent it",
    "when_to_apply": "Conditions that signal this failure pattern"
  }
]

Rules:
- Focus on the root cause, not symptoms
- Make the prevention actionable and specific
- Output ONLY the JSON array, nothing else"#;

/// SkillRL R2: Distill reusable skills from successful agent runs.
/// Runs in a background thread — fire-and-forget.
fn distill_skills_from_run(
    result: &AgentRunOutput,
    workspace: Option<PathBuf>,
) {
    let messages = result.messages.clone();
    let session = result
        .session
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let ws = workspace.unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));

    thread::spawn(move || {
        // Only distill from sessions with 5+ tool calls (non-trivial tasks)
        let tool_count = messages.iter().filter(|m| m.role == "tool").count();
        if tool_count < 5 {
            return;
        }

        // Build a compact trajectory summary
        let trajectory: String = messages
            .iter()
            .filter(|m| m.role == "assistant" || m.role == "tool")
            .filter_map(|m| {
                let role = &m.role;
                m.content.as_ref().map(|c| {
                    let preview: String = c.chars().take(200).collect();
                    format!("[{role}] {preview}")
                })
            })
            .collect::<Vec<_>>()
            .join("\n");

        let distill_request = AgentHookRequest {
            messages: vec![
                AgentMessage {
                    role: "system".to_string(),
                    content: Some(SKILL_DISTILL_PROMPT.to_string()),
                    tool_calls: Vec::new(),
                    name: None,
                    tool_call_id: None,
                    is_error: None,
                },
                AgentMessage {
                    role: "user".to_string(),
                    content: Some(format!(
                        "Session: {session}\n\nTrajectory:\n{trajectory}"
                    )),
                    tool_calls: Vec::new(),
                    name: None,
                    tool_call_id: None,
                    is_error: None,
                },
            ],
            tools: Vec::new(),
            session: Some(format!("distill:{session}")),
        };

        if let Ok(response) = call_claude(&distill_request) {
            if let Some(text) = response.message.content {
                // Strip markdown fences if present
                let json_text = text
                    .trim()
                    .trim_start_matches("```json")
                    .trim_start_matches("```")
                    .trim_end_matches("```")
                    .trim();
                if let Ok(skills) = serde_json::from_str::<Vec<DistilledSkill>>(json_text) {
                    let db_path = ws.join("skills.sqlite");
                    if let Ok(conn) = open_skill_db(&db_path) {
                        for skill in &skills {
                            let record = SkillRecord {
                                name: skill.title.clone(),
                                trigger: Some(skill.when_to_apply.clone()),
                                steps: Vec::new(),
                                tools: Vec::new(),
                                notes: Some(skill.principle.clone()),
                                success_rate: 0.0,
                                times_used: 0,
                                times_succeeded: 0,
                                last_used: None,
                                created_at: Utc::now().to_rfc3339(),
                                contexts: vec![session.clone()],
                            };
                            let _ = upsert_skill(&conn, &record);
                        }
                    }
                }
            }
        }
    });
}

/// SkillRL R3: Distill prevention skills from failed agent runs.
/// Runs in a background thread — fire-and-forget.
fn distill_failure_skill(
    error_msg: &str,
    session: &str,
    workspace: Option<PathBuf>,
) {
    let error = error_msg.to_string();
    let session = session.to_string();
    let ws = workspace.unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));

    thread::spawn(move || {
        let distill_request = AgentHookRequest {
            messages: vec![
                AgentMessage {
                    role: "system".to_string(),
                    content: Some(FAILURE_DISTILL_PROMPT.to_string()),
                    tool_calls: Vec::new(),
                    name: None,
                    tool_call_id: None,
                    is_error: None,
                },
                AgentMessage {
                    role: "user".to_string(),
                    content: Some(format!("Session: {session}\nFailure: {error}")),
                    tool_calls: Vec::new(),
                    name: None,
                    tool_call_id: None,
                    is_error: None,
                },
            ],
            tools: Vec::new(),
            session: Some(format!("distill-fail:{session}")),
        };

        if let Ok(response) = call_claude(&distill_request) {
            if let Some(text) = response.message.content {
                let json_text = text
                    .trim()
                    .trim_start_matches("```json")
                    .trim_start_matches("```")
                    .trim_end_matches("```")
                    .trim();
                if let Ok(skills) = serde_json::from_str::<Vec<DistilledSkill>>(json_text) {
                    let db_path = ws.join("skills.sqlite");
                    if let Ok(conn) = open_skill_db(&db_path) {
                        for skill in &skills {
                            let record = SkillRecord {
                                name: format!("AVOID: {}", skill.title),
                                trigger: Some(skill.when_to_apply.clone()),
                                steps: Vec::new(),
                                tools: Vec::new(),
                                notes: Some(format!("FAILURE LESSON: {}", skill.principle)),
                                success_rate: 0.0,
                                times_used: 0,
                                times_succeeded: 0,
                                last_used: None,
                                created_at: Utc::now().to_rfc3339(),
                                contexts: vec![session.clone()],
                            };
                            let _ = upsert_skill(&conn, &record);
                        }
                    }
                }
            }
        }
    });
}

pub(crate) fn run_bridge(command: BridgeCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        BridgeCommand::Telegram {
            mv2,
            token,
            poll_timeout,
            poll_limit,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let token = token
                .or_else(|| env_optional("TELEGRAM_BOT_TOKEN"))
                .ok_or("Missing TELEGRAM_BOT_TOKEN")?;
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_telegram_bridge(token, poll_timeout, poll_limit, config)
        }
        BridgeCommand::Whatsapp {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_whatsapp_bridge(bind, port, config)
        }
        BridgeCommand::Slack {
            mv2,
            bot_token,
            app_token,
            signing_secret,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_slack_bridge(config, bot_token, app_token, signing_secret)
        }
        BridgeCommand::Discord {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge(
                "discord",
                bind,
                port,
                config,
                extract_discord_event,
                reply_none,
            )
        }
        BridgeCommand::Teams {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge("teams", bind, port, config, extract_teams_event, reply_none)
        }
        BridgeCommand::Signal {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
            sender: _,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge(
                "signal",
                bind,
                port,
                config,
                extract_signal_event,
                reply_none,
            )
        }
        BridgeCommand::Matrix {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
            room: _,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge(
                "matrix",
                bind,
                port,
                config,
                extract_matrix_event,
                reply_none,
            )
        }
        BridgeCommand::IMessage {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge(
                "imessage",
                bind,
                port,
                config,
                extract_imessage_event,
                reply_none,
            )
        }
    }
}

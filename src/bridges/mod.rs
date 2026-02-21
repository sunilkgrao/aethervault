pub(crate) mod telegram;
pub(crate) mod slack;
pub(crate) mod whatsapp;
pub(crate) mod webhook;

pub(crate) use telegram::*;

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;


use crate::{
    env_optional, run_agent_with_prompt,
    AgentProgress, AgentRunOutput, BridgeAgentConfig, BridgeCommand,
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
    // Prefer Sonnet for the orchestrator — fast + cheap.
    // Opus kicks in automatically via critic-triggered escalation.
    if env_optional("ANTHROPIC_API_KEY").is_some() {
        if env_optional("SONNET_MODEL").is_some() || env_optional("ANTHROPIC_MODEL").is_some() {
            return Some("builtin:sonnet".to_string());
        }
    }
    None
}

pub(crate) fn build_bridge_agent_config(
    db_path: PathBuf,
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
    Ok(BridgeAgentConfig {
        db_path,
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
    })
}

pub(crate) fn split_text_chunks(text: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        if count >= max_chars {
            chunks.push(current);
            current = String::new();
            count = 0;
        }
        current.push(ch);
        count += 1;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

pub(crate) fn cleanup_orphaned_temp_files(db_path: &Path) {
    if let Some(parent) = db_path.parent() {
        if let Some(stem) = db_path.file_name().and_then(|f| f.to_str()) {
            let prefix = format!(".{}.", stem);
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.starts_with(&prefix) {
                            let _ = std::fs::remove_file(entry.path());
                            eprintln!("[bridge] cleaned up orphaned temp file: {}", name);
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn check_vault_health(db_path: &Path) {
    if let Ok(meta) = std::fs::metadata(db_path) {
        let size_mb = meta.len() / (1024 * 1024);
        if size_mb > 2000 {
            eprintln!("[bridge] vault is {size_mb}MB — consider running VACUUM");
        }
    }
}

pub(crate) fn panic_to_string(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "agent panicked".to_string()
    }
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
    let prompt_text = prompt.to_string();
    let mv2 = config.db_path.clone();
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
        let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_agent_with_prompt(
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
            .map_err(|e| e.to_string())
        })) {
            Ok(result) => result,
            Err(panic_info) => {
                Err(format!("Agent crashed: {}", panic_to_string(panic_info)))
            }
        };
        let _ = tx.send(result);
    });

    // No timeout — let the agent run as long as it needs.
    // The agent is bounded by max_steps, not wall-clock time.
    // Long-running tasks (dev work, swarms, batch processing) can take hours.
    rx.recv().map_err(|err| format!("Agent channel error: {err}"))?.map_err(|e| e)
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
            run_slack_bridge(
                config,
                bot_token,
                app_token,
                signing_secret,
            )
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

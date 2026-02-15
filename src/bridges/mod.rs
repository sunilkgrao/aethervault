pub(crate) mod telegram;
pub(crate) mod whatsapp;
pub(crate) mod webhook;

pub(crate) use telegram::*;

#[allow(unused_imports)]
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;


use crate::{
    env_optional, run_agent_with_prompt,
    AgentProgress, AgentRunOutput, BridgeAgentConfig, BridgeCommand,
};
use self::telegram::run_telegram_bridge;
use self::whatsapp::run_whatsapp_bridge;
use self::webhook::{
    extract_discord_event, extract_imessage_event, extract_matrix_event, extract_signal_event,
    extract_slack_event, extract_teams_event, reply_none, reply_slack, run_webhook_bridge,
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
                "slack",
                bind,
                port,
                config,
                extract_slack_event,
                reply_slack,
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::{env_optional, AgentHookRequest, AgentHookResponse, AgentMessage};

// ---------------------------------------------------------------------------
// Auth profiles config (deserialized from config/auth-profiles.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AuthProfilesConfig {
    #[serde(default)]
    pub(crate) profiles: HashMap<String, AuthProfile>,
    #[serde(default)]
    pub(crate) pools: HashMap<String, PoolConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AuthProfile {
    #[serde(rename = "type")]
    pub(crate) auth_type: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) codex_config_dir: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
    #[serde(flatten)]
    pub(crate) extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PoolConfig {
    pub(crate) accounts: Vec<String>,
    #[serde(default = "default_cooldown")]
    pub(crate) cooldown_secs: u64,
}

fn default_cooldown() -> u64 {
    300
}

// ---------------------------------------------------------------------------
// Runtime pool state (in-memory, thread-safe)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct AccountState {
    service: String,
    rate_limited_until: Option<Instant>,
    consecutive_failures: u32,
}

#[derive(Debug)]
pub(crate) struct PoolState {
    accounts: HashMap<String, AccountState>,
    profiles: HashMap<String, AuthProfile>,
    pools: HashMap<String, PoolConfig>,
}

impl PoolState {
    fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            profiles: HashMap::new(),
            pools: HashMap::new(),
        }
    }

    fn load_from_config(&mut self, config: AuthProfilesConfig) {
        self.profiles = config.profiles;
        self.pools = config.pools.clone();

        // Initialize account state from pool config
        for (service, pool) in &config.pools {
            for account in &pool.accounts {
                self.accounts
                    .entry(account.clone())
                    .or_insert_with(|| AccountState {
                        service: service.clone(),
                        rate_limited_until: None,
                        consecutive_failures: 0,
                    });
            }
        }
    }

    fn pick_best_account(&self, service: &str) -> Option<String> {
        let pool = self.pools.get(service)?;
        let now = Instant::now();

        // Return first non-rate-limited account in config order (preserves priority).
        for account in &pool.accounts {
            if let Some(state) = self.accounts.get(account) {
                if let Some(until) = state.rate_limited_until {
                    if now < until {
                        continue; // skip rate-limited
                    }
                }
            }
            // Either no state yet (unknown = available) or not rate-limited
            return Some(account.clone());
        }
        None // all accounts rate-limited
    }

    fn mark_rate_limited(&mut self, account: &str) {
        if let Some(state) = self.accounts.get_mut(account) {
            let cooldown = self
                .pools
                .get(&state.service)
                .map(|p| p.cooldown_secs)
                .unwrap_or(300);
            state.rate_limited_until = Some(Instant::now() + Duration::from_secs(cooldown));
            state.consecutive_failures += 1;
        }
    }

    fn mark_success(&mut self, account: &str) {
        if let Some(state) = self.accounts.get_mut(account) {
            state.rate_limited_until = None;
            state.consecutive_failures = 0;
        }
    }

    fn get_profile(&self, account: &str) -> Option<&AuthProfile> {
        self.profiles.get(account)
    }

    fn has_available_account(&self, service: &str) -> bool {
        self.pick_best_account(service).is_some()
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static POOL: OnceLock<Arc<Mutex<PoolState>>> = OnceLock::new();

fn global_pool() -> Arc<Mutex<PoolState>> {
    POOL.get_or_init(|| {
        let mut state = PoolState::new();

        // Try to load auth-profiles.json from AETHERVAULT_HOME
        let home = env_optional("AETHERVAULT_HOME")
            .unwrap_or_else(|| {
                dirs_home().unwrap_or_else(|| "/root/.aethervault".to_string())
            });
        let config_path = PathBuf::from(&home).join("config").join("auth-profiles.json");
        if config_path.exists() {
            match std::fs::read_to_string(&config_path) {
                Ok(raw) => match serde_json::from_str::<AuthProfilesConfig>(&raw) {
                    Ok(config) => {
                        state.load_from_config(config);
                        eprintln!(
                            "[pool_state] Loaded {} profiles, {} pools from {}",
                            state.profiles.len(),
                            state.pools.len(),
                            config_path.display()
                        );
                    }
                    Err(e) => {
                        eprintln!("[pool_state] Failed to parse {}: {e}", config_path.display());
                    }
                },
                Err(e) => {
                    eprintln!("[pool_state] Failed to read {}: {e}", config_path.display());
                }
            }
        }
        Arc::new(Mutex::new(state))
    })
    .clone()
}

fn dirs_home() -> Option<String> {
    std::env::var("HOME").ok().map(|h| format!("{h}/.aethervault"))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub(crate) fn pool_pick_best_account(service: &str) -> Option<String> {
    let pool = global_pool();
    let state = pool.lock().ok()?;
    state.pick_best_account(service)
}

/// Returns the total number of unique accounts across all pools.
/// Used by BackgroundTaskRegistry to auto-size concurrency to match pool capacity.
pub(crate) fn pool_total_accounts() -> usize {
    let pool = global_pool();
    let state = match pool.lock() {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let mut unique = std::collections::HashSet::new();
    for pool_cfg in state.pools.values() {
        for acct in &pool_cfg.accounts {
            unique.insert(acct.clone());
        }
    }
    unique.len()
}

pub(crate) fn pool_mark_rate_limited(account: &str) {
    if let Ok(mut state) = global_pool().lock() {
        state.mark_rate_limited(account);
    }
}

pub(crate) fn pool_mark_success(account: &str) {
    if let Ok(mut state) = global_pool().lock() {
        state.mark_success(account);
    }
}

pub(crate) fn pool_get_profile(account: &str) -> Option<AuthProfile> {
    let pool = global_pool();
    let state = pool.lock().ok()?;
    state.get_profile(account).cloned()
}

pub(crate) fn pool_has_available(service: &str) -> bool {
    global_pool()
        .lock()
        .map(|s| s.has_available_account(service))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Rate limit detection (shared between backends)
// ---------------------------------------------------------------------------

const RATE_LIMIT_PATTERNS: &[&str] = &[
    "rate limit",
    "429",
    "too many requests",
    "quota exceeded",
    "ratelimiterror",
    "overloaded",
    "capacity",
];

pub(crate) fn is_rate_limit_error(text: &str, exit_code: Option<i32>) -> bool {
    if exit_code == Some(429) {
        return true;
    }
    let lower = text.to_lowercase();
    RATE_LIMIT_PATTERNS.iter().any(|pat| lower.contains(pat))
}

// ---------------------------------------------------------------------------
// Native Codex CLI backend
// ---------------------------------------------------------------------------

pub(crate) fn run_codex_native(prompt: &str, read_only: bool) -> Result<AgentMessage, String> {
    let mut tried: Vec<String> = Vec::new();

    loop {
        let account = match pool_pick_best_account("codex") {
            Some(a) if !tried.contains(&a) => a,
            _ => {
                if tried.is_empty() {
                    return Err("No Codex accounts configured".into());
                }
                return Err("All Codex accounts are rate-limited".into());
            }
        };
        tried.push(account.clone());

        match run_codex_once(prompt, &account, read_only) {
            Ok(msg) => return Ok(msg),
            Err(e) => {
                eprintln!("[codex] Account {account} failed: {e}, trying next...");
                continue;
            }
        }
    }
}

fn run_codex_once(prompt: &str, account: &str, read_only: bool) -> Result<AgentMessage, String> {
    let profile = pool_get_profile(account)
        .ok_or_else(|| format!("No profile for account: {account}"))?;

    let config_dir = profile
        .codex_config_dir
        .as_deref()
        .unwrap_or("/root/.codex");
    let model = profile
        .model
        .as_deref()
        .unwrap_or("gpt-5.3-codex-spark");
    let reasoning = profile
        .reasoning_effort
        .as_deref()
        .unwrap_or("xhigh");

    let effective_prompt = if read_only {
        format!(
            "[ORCHESTRATOR MODE] You are a READ-ONLY analyst. You CANNOT write files or run commands. \
             Your job is to analyze the codebase and return a diagnosis. Do NOT attempt to fix anything — \
             just report what you find.\n\n{prompt}"
        )
    } else {
        prompt.to_string()
    };

    let reasoning_cfg = format!("model_reasoning_effort=\"{reasoning}\"");

    let mut cmd = std::process::Command::new("codex");
    let mut args = vec!["exec", "-m", model];
    if read_only {
        args.extend(["--sandbox", "read-only"]);
    } else {
        args.push("--dangerously-bypass-approvals-and-sandbox");
    }
    args.extend(["--json", "--skip-git-repo-check", "-c", &reasoning_cfg, &effective_prompt]);
    cmd.args(&args);
    // Codex CLI reads config from $HOME/.codex/ — set HOME to the parent
    // of the config dir so each account gets its own auth.
    let config_path = std::path::Path::new(config_dir);
    if let Some(home_dir) = config_path.parent() {
        cmd.env("HOME", home_dir);
    }

    // Use workspace as cwd if available
    let workspace = env_optional("AETHERVAULT_WORKSPACE")
        .or_else(|| env_optional("AETHERVAULT_HOME").map(|h| format!("{h}/workspace")));
    if let Some(ref cwd) = workspace {
        if Path::new(cwd).exists() {
            cmd.current_dir(cwd);
        }
    }

    eprintln!("[codex] Running with account={account}, model={model}, read_only={read_only}");

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to spawn codex: {e}"))?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    if is_rate_limit_error(&stderr, output.status.code()) {
        pool_mark_rate_limited(account);
        return Err("rate limit".into());
    }

    if !output.status.success() && output.stdout.is_empty() {
        // Mark account as temporarily unavailable so pick_best_account
        // skips it on the next iteration of the retry loop.
        pool_mark_rate_limited(account);
        return Err(format!(
            "codex account {account} failed: exit code {:?}: {}",
            output.status.code(),
            stderr.trim()
        ));
    }

    // Parse JSONL output — extract text from item.completed events
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut text_parts: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
            if event.get("type").and_then(|t| t.as_str()) == Some("item.completed") {
                if let Some(text) = event
                    .get("item")
                    .and_then(|item| item.get("text"))
                    .and_then(|t| t.as_str())
                {
                    text_parts.push(text.to_string());
                }
            }
        } else {
            // Non-JSON line — include as raw text
            text_parts.push(line.to_string());
        }
    }

    pool_mark_success(account);

    let content = if text_parts.is_empty() {
        "(Codex returned no output)".to_string()
    } else {
        text_parts.join("\n")
    };

    Ok(AgentMessage {
        role: "assistant".to_string(),
        content: Some(content),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
        thinking_blocks: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Native Claude Code CLI backend
// ---------------------------------------------------------------------------

pub(crate) fn run_claude_code_native(prompt: &str, read_only: bool) -> Result<AgentMessage, String> {
    let account = pool_pick_best_account("claude-code")
        .ok_or("All Claude Code accounts are rate-limited")?;

    let profile = pool_get_profile(&account)
        .ok_or_else(|| format!("No profile for account: {account}"))?;

    let model = profile
        .model
        .as_deref()
        .unwrap_or("claude-sonnet-4-6");

    let effective_prompt = if read_only {
        format!(
            "[ORCHESTRATOR MODE] You are a READ-ONLY analyst. You CANNOT write files or run commands. \
             Your job is to analyze the codebase and return a diagnosis. Do NOT attempt to fix anything — \
             just report what you find.\n\n{prompt}"
        )
    } else {
        prompt.to_string()
    };

    eprintln!("[claude-code] Running with account={account}, model={model}, read_only={read_only}");

    let mut args = vec!["-p", &effective_prompt, "--output-format", "json", "--model", model];
    if read_only {
        args.extend(["--allowedTools", "Read,Grep,Glob,WebSearch,WebFetch"]);
    }

    let output = std::process::Command::new("claude")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to spawn claude: {e}"))?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    if is_rate_limit_error(&stderr, output.status.code()) {
        pool_mark_rate_limited(&account);
        return Err("Claude Code rate-limited".into());
    }

    if !output.status.success() {
        let err = stderr.trim();
        let err_msg = if err.is_empty() {
            format!("exit code {:?}", output.status.code())
        } else {
            err.to_string()
        };
        return Err(format!("Claude Code error: {err_msg}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();

    if stdout.is_empty() {
        pool_mark_success(&account);
        return Ok(AgentMessage {
            role: "assistant".to_string(),
            content: Some("(Claude Code returned no output)".to_string()),
            tool_calls: Vec::new(),
            name: None,
            tool_call_id: None,
            is_error: None,
            thinking_blocks: Vec::new(),
        });
    }

    // Parse JSON output: { "result": "...", "is_error": false }
    let content = match serde_json::from_str::<serde_json::Value>(stdout) {
        Ok(result) if result.is_object() => {
            let is_error = result
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let response_text = result
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if is_error {
                pool_mark_success(&account);
                return Ok(AgentMessage {
                    role: "assistant".to_string(),
                    content: Some(format!("(Claude Code error: {response_text})")),
                    tool_calls: Vec::new(),
                    name: None,
                    tool_call_id: None,
                    is_error: Some(true),
                    thinking_blocks: Vec::new(),
                });
            }

            if response_text.is_empty() {
                "(Claude Code returned empty result)".to_string()
            } else {
                response_text
            }
        }
        _ => stdout.to_string(),
    };

    pool_mark_success(&account);

    Ok(AgentMessage {
        role: "assistant".to_string(),
        content: Some(content),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
        thinking_blocks: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Pool router: tries backends in priority order
// ---------------------------------------------------------------------------

/// Backend priority for pool routing
const POOL_BACKEND_ORDER: &[&str] = &["codex", "claude-code"];

pub(crate) fn run_pool_routed(request: &AgentHookRequest, read_only: bool) -> Result<AgentMessage, String> {
    let prompt = extract_prompt_from_request(request);
    if prompt.is_empty() {
        return Err("No user prompt found in messages".into());
    }

    let mut errors: Vec<String> = Vec::new();

    for service in POOL_BACKEND_ORDER {
        if !pool_has_available(service) {
            errors.push(format!("{service}: all accounts rate-limited"));
            continue;
        }

        let result = match *service {
            "codex" => run_codex_native(&prompt, read_only),
            "claude-code" => run_claude_code_native(&prompt, read_only),
            _ => {
                errors.push(format!("{service}: unknown backend"));
                continue;
            }
        };

        match result {
            Ok(msg) => return Ok(msg),
            Err(e) => {
                errors.push(format!("{service}: {e}"));
                continue;
            }
        }
    }

    Err(format!(
        "All subagent backends exhausted. Errors: {}",
        errors.join("; ")
    ))
}

pub(crate) fn extract_prompt_from_request(request: &AgentHookRequest) -> String {
    for msg in request.messages.iter().rev() {
        if msg.role == "user" {
            if let Some(ref content) = msg.content {
                if !content.is_empty() {
                    return content.clone();
                }
            }
        }
    }
    String::new()
}

/// Inspect the tools array on an AgentHookRequest to decide whether a CLI
/// hook should run in read-only / sandboxed mode.  When the orchestrator
/// strips exec and fs_write the array will be empty (or missing those
/// tools), which means the hook must NOT be allowed to write files.
pub(crate) fn should_hook_be_read_only(request: &AgentHookRequest) -> bool {
    if request.tools.is_empty() {
        return true;
    }
    let has_exec = request.tools.iter().any(|t| {
        t.get("name").and_then(|n| n.as_str()) == Some("exec")
    });
    let has_fs_write = request.tools.iter().any(|t| {
        t.get("name").and_then(|n| n.as_str()) == Some("fs_write")
    });
    !has_exec && !has_fs_write
}

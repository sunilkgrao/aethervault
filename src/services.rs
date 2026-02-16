use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use aether_core::types::SearchHit;
use aether_core::{PutOptions, Vault};
use chrono::{Datelike, Timelike, Utc};
use serde::Deserialize;
use serde_json;
use url::form_urlencoded;

#[cfg(feature = "vec")]
use aether_core::text_embed::{LocalTextEmbedder, TextEmbedConfig};
#[cfg(feature = "vec")]
use aether_core::types::EmbeddingProvider;
#[cfg(feature = "vec")]
use aether_core::types::FrameStatus;

// Re-imports from main (crate-internal helpers and types)
use crate::{
    open_or_create, save_config_entry, load_config_entry, blake3_hash, execute_tool,
    env_optional, tool_autonomy_for, ToolAutonomyLevel, ApprovalEntry, TriggerEntry,
    AgentConfig, CapsuleConfig, CronExpr, load_capsule_config, load_config_from_file,
    config_file_path, resolve_workspace, build_bridge_agent_config, run_agent_for_bridge,
    telegram_send_message, load_file_config, save_config_to_file,
};
use tiny_http::{Response, Server};
use walkdir::WalkDir;

// 2 minutes — prevents hanging on unresponsive external services
const NO_TIMEOUT_MS: u64 = 120_000;

// ── Memory helpers ──────────────────────────────────────────────────────

pub(crate) fn read_optional_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().and_then(|text| {
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    })
}

pub(crate) fn daily_memory_path(workspace: &Path) -> PathBuf {
    let date = Utc::now().format("%Y-%m-%d").to_string();
    workspace.join("memory").join(format!("{date}.md"))
}

pub(crate) fn memory_uri(kind: &str) -> String {
    format!("aethervault://memory/{kind}.md")
}

pub(crate) fn memory_daily_uri(date: &str) -> String {
    format!("aethervault://memory/daily/{date}.md")
}

pub(crate) fn sync_memory_file(
    mem: &mut Vault,
    path: &Path,
    uri: String,
    title: &str,
    track: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut options = PutOptions::default();
    options.uri = Some(uri);
    options.title = Some(title.to_string());
    options.kind = Some("text/markdown".to_string());
    options.track = Some(track.to_string());
    options.search_text = Some(text.clone());
    let id = mem.put_bytes_with_options(text.as_bytes(), options)?;
    mem.commit()?;
    Ok(id)
}

pub(crate) fn sync_workspace_memory(
    mv2: &Path,
    workspace: &Path,
    include_daily: bool,
) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    let mut mem = open_or_create(mv2)?;
    let mut ids = Vec::new();
    let soul = workspace.join("SOUL.md");
    let user = workspace.join("USER.md");
    let memory = workspace.join("MEMORY.md");
    if soul.exists() {
        ids.push(sync_memory_file(
            &mut mem,
            &soul,
            memory_uri("soul"),
            "memory soul",
            "aethervault.memory",
        )?);
    }
    if user.exists() {
        ids.push(sync_memory_file(
            &mut mem,
            &user,
            memory_uri("user"),
            "memory user",
            "aethervault.memory",
        )?);
    }
    if memory.exists() {
        ids.push(sync_memory_file(
            &mut mem,
            &memory,
            memory_uri("longterm"),
            "memory longterm",
            "aethervault.memory",
        )?);
    }
    if include_daily {
        let daily_dir = workspace.join("memory");
        if daily_dir.exists() {
            for entry in WalkDir::new(&daily_dir).max_depth(1) {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let uri = memory_daily_uri(stem);
                let title = format!("memory daily {stem}");
                ids.push(sync_memory_file(
                    &mut mem,
                    path,
                    uri,
                    &title,
                    "aethervault.memory",
                )?);
            }
        }
    }
    Ok(ids)
}

pub(crate) fn export_capsule_memory(
    mv2: &Path,
    workspace: &Path,
    include_daily: bool,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut mem = Vault::open_read_only(mv2)?;
    let mut paths = Vec::new();
    let items = vec![
        (memory_uri("soul"), workspace.join("SOUL.md")),
        (memory_uri("user"), workspace.join("USER.md")),
        (memory_uri("longterm"), workspace.join("MEMORY.md")),
    ];
    for (uri, path) in items {
        if let Ok(frame) = mem.frame_by_uri(&uri) {
            if let Ok(text) = mem.frame_text_by_id(frame.id) {
                fs::create_dir_all(workspace)?;
                fs::write(&path, text)?;
                paths.push(path.display().to_string());
            }
        }
    }
    if include_daily {
        use aether_core::types::SearchRequest;

        let daily_dir = workspace.join("memory");
        fs::create_dir_all(&daily_dir)?;

        // Use scoped search instead of O(n) linear scan over all frames.
        let request = SearchRequest {
            query: "track:aethervault.memory".to_string(),
            top_k: 500,
            snippet_chars: 0,
            uri: None,
            scope: Some("aethervault://memory/daily/".to_string()),
            cursor: None,
            temporal: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: true,
        };

        if let Ok(response) = mem.search(request) {
            for hit in &response.hits {
                let frame = match mem.frame_by_id(hit.frame_id) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                let Some(uri) = frame.uri.as_deref() else { continue };
                if !uri.starts_with("aethervault://memory/daily/") { continue; }
                if let Some(name) = uri.rsplit('/').next() {
                    let path = daily_dir.join(name);
                    if let Ok(text) = mem.frame_text_by_id(hit.frame_id) {
                        fs::write(&path, text)?;
                        paths.push(path.display().to_string());
                    }
                }
            }
        }
    }
    Ok(paths)
}

// ── OAuth ───────────────────────────────────────────────────────────────

pub(crate) fn oauth_env(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    env_optional(name)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("Missing {name}").into())
}

pub(crate) fn build_oauth_redirect(base: &str, provider: &str) -> String {
    format!("{base}/oauth/{provider}/callback")
}

pub(crate) fn build_google_auth_url(client_id: &str, redirect_uri: &str, scope: &str, state: &str) -> String {
    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id={}&redirect_uri={}&scope={}&access_type=offline&prompt=consent&state={}",
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(scope),
        urlencoding::encode(state)
    )
}

pub(crate) fn build_microsoft_auth_url(
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    state: &str,
) -> String {
    format!(
        "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}&response_mode=query&state={}",
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(scope),
        urlencoding::encode(state)
    )
}

pub(crate) fn exchange_oauth_code(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_write(Duration::from_millis(NO_TIMEOUT_MS))
        .build();
    let payload = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", client_id)
        .append_pair("client_secret", client_secret)
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", redirect_uri)
        .finish();
    let response = agent
        .post(token_url)
        .set("content-type", "application/x-www-form-urlencoded")
        .send_string(&payload);
    match response {
        Ok(resp) => Ok(resp.into_json()?),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(format!("token error {code}: {text}").into())
        }
        Err(err) => Err(format!("token request failed: {err}").into()),
    }
}

pub(crate) fn run_oauth_broker(
    mv2: PathBuf,
    provider: String,
    bind: String,
    port: u16,
    redirect_base: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let provider = provider.to_ascii_lowercase();
    let redirect_base = redirect_base.unwrap_or_else(|| format!("http://{}:{}", bind, port));
    let redirect_uri = build_oauth_redirect(&redirect_base, &provider);
    let state = "aethervault";

    let (client_id, client_secret, _scope, token_url, auth_url) = if provider == "google" {
        let client_id = oauth_env("GOOGLE_CLIENT_ID")?;
        let client_secret = oauth_env("GOOGLE_CLIENT_SECRET")?;
        let scope = env_optional("GOOGLE_SCOPES").unwrap_or_else(|| {
            "https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/calendar https://www.googleapis.com/auth/gmail.send"
                .to_string()
        });
        let auth_url = build_google_auth_url(&client_id, &redirect_uri, &scope, state);
        (
            client_id,
            client_secret,
            scope,
            "https://oauth2.googleapis.com/token".to_string(),
            auth_url,
        )
    } else if provider == "microsoft" {
        let client_id = oauth_env("MICROSOFT_CLIENT_ID")?;
        let client_secret = oauth_env("MICROSOFT_CLIENT_SECRET")?;
        let scope = env_optional("MICROSOFT_SCOPES").unwrap_or_else(|| {
            "offline_access https://graph.microsoft.com/Mail.Read https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite"
                .to_string()
        });
        let auth_url = build_microsoft_auth_url(&client_id, &redirect_uri, &scope, state);
        (
            client_id,
            client_secret,
            scope,
            "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string(),
            auth_url,
        )
    } else {
        return Err("provider must be google or microsoft".into());
    };

    println!("Open this URL to authorize:\n{auth_url}");
    let addr = format!("{bind}:{port}");
    let server = Server::http(&addr)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("server: {e}")))?;
    eprintln!("OAuth broker listening on http://{addr}");

    for request in server.incoming_requests() {
        let url = request.url().to_string();
        if !url.starts_with(&format!("/oauth/{provider}/callback")) {
            let response = Response::from_string("ok");
            let _ = request.respond(response);
            continue;
        }
        let query = url.splitn(2, '?').nth(1).unwrap_or("");
        let params: HashMap<String, String> = form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        let code = match params.get("code") {
            Some(c) => c.to_string(),
            None => {
                let response = Response::from_string("missing code");
                let _ = request.respond(response);
                continue;
            }
        };
        let token =
            exchange_oauth_code(&token_url, &client_id, &client_secret, &redirect_uri, &code)?;
        let key = format!("oauth.{provider}");
        // Primary: flat file config
        if let Some(ws) = flat_file_workspace() {
            save_config_to_file(&ws, &key, token.clone())?;
        } else {
            let payload = serde_json::to_vec_pretty(&token)?;
            let mut mem = open_or_create(&mv2)?;
            let _ = save_config_entry(&mut mem, &key, &payload)?;
        }
        let response = Response::from_string("Authorized. You can close this tab.");
        let _ = request.respond(response);
        println!("Stored token in config key: {key}");
        break;
    }
    Ok(())
}

// ── Config / Approvals ──────────────────────────────────────────────────

pub(crate) fn load_config_json(mem: &mut Vault, key: &str) -> Option<serde_json::Value> {
    let bytes = load_config_entry(mem, key)?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn approval_hash(tool: &str, args: &serde_json::Value) -> String {
    let payload = serde_json::json!({ "tool": tool, "args": args });
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    blake3_hash(&bytes).to_hex().to_string()
}

/// Resolve workspace path from AETHERVAULT_WORKSPACE env var (used by flat-file config helpers).
fn flat_file_workspace() -> Option<PathBuf> {
    env_optional("AETHERVAULT_WORKSPACE")
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
}

pub(crate) fn load_approvals(_mem: &mut Vault) -> Vec<ApprovalEntry> {
    // Primary: flat file config
    if let Some(ws) = flat_file_workspace() {
        let cfg_path = config_file_path(&ws);
        if cfg_path.exists() {
            let fc = load_file_config(&cfg_path);
            if !fc.approvals.is_empty() {
                return fc.approvals;
            }
        }
    }
    // Fallback: capsule
    load_config_json(_mem, "approvals")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub(crate) fn save_approvals(_mem: &mut Vault, approvals: &[ApprovalEntry]) -> Result<(), String> {
    // Primary: flat file config
    if let Some(ws) = flat_file_workspace() {
        let value = serde_json::to_value(approvals).map_err(|e| e.to_string())?;
        return save_config_to_file(&ws, "approvals", value).map_err(|e| e.to_string());
    }
    // Fallback: capsule
    let json = serde_json::to_value(approvals).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&json).map_err(|e| e.to_string())?;
    save_config_entry(_mem, "approvals", &bytes).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) enum ApprovalChatCommand {
    Approve(String),
    Reject(String),
}

pub(crate) fn parse_approval_chat_command(text: &str) -> Option<ApprovalChatCommand> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = trimmed.split_whitespace();
    let cmd = parts.next()?.to_ascii_lowercase();
    let id = parts.next()?.trim();
    if id.is_empty() {
        return None;
    }
    match cmd.as_str() {
        "approve" => Some(ApprovalChatCommand::Approve(id.to_string())),
        "reject" => Some(ApprovalChatCommand::Reject(id.to_string())),
        _ => None,
    }
}

pub(crate) fn approve_and_maybe_execute(mv2: &Path, id: &str, execute: bool) -> Result<String, String> {
    let mut mem = open_or_create(mv2).map_err(|e| e.to_string())?;
    let mut approvals = load_approvals(&mut mem);
    let mut entry: Option<ApprovalEntry> = None;
    for a in approvals.iter_mut() {
        if a.id == id {
            a.status = "approved".to_string();
            entry = Some(a.clone());
            break;
        }
    }
    if entry.is_none() {
        return Ok("Approval id not found.".to_string());
    }
    save_approvals(&mut mem, &approvals)?;
    mem.commit().map_err(|e| e.to_string())?;

    if !execute {
        return Ok("Approved.".to_string());
    }
    let entry = entry.unwrap();
    let result = execute_tool(&entry.tool, entry.args, mv2, false);
    match result {
        Ok(exec) => Ok(exec.output),
        Err(err) => Ok(format!("Execution error: {err}")),
    }
}

pub(crate) fn reject_approval(mv2: &Path, id: &str) -> Result<String, String> {
    let mut mem = open_or_create(mv2).map_err(|e| e.to_string())?;
    let mut approvals = load_approvals(&mut mem);
    let before = approvals.len();
    approvals.retain(|a| a.id != id);
    let updated = approvals.len() != before;
    if updated {
        save_approvals(&mut mem, &approvals)?;
        mem.commit().map_err(|e| e.to_string())?;
        Ok("Rejected.".to_string())
    } else {
        Ok("Approval id not found.".to_string())
    }
}

pub(crate) fn try_handle_approval_chat(mv2: &Path, text: &str) -> Option<String> {
    let cmd = parse_approval_chat_command(text)?;
    let result = match cmd {
        ApprovalChatCommand::Approve(id) => approve_and_maybe_execute(mv2, &id, true),
        ApprovalChatCommand::Reject(id) => reject_approval(mv2, &id),
    };
    Some(result.unwrap_or_else(|e| format!("Approval error: {e}")))
}

pub(crate) fn requires_approval(name: &str, args: &serde_json::Value) -> bool {
    // In bridge mode (env AETHERVAULT_BRIDGE_AUTO_APPROVE=1), auto-approve ALL tools.
    // The user explicitly opted in to full agency — no approval gates.
    let bridge_auto = std::env::var("AETHERVAULT_BRIDGE_AUTO_APPROVE")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    if bridge_auto {
        return false;
    }
    // Per-tool autonomy override via env (e.g. TOOL_AUTONOMY_EXEC=autonomous)
    match tool_autonomy_for(name) {
        ToolAutonomyLevel::Autonomous | ToolAutonomyLevel::Background => return false,
        ToolAutonomyLevel::SuggestOnly => return true,
        ToolAutonomyLevel::Confirm => {} // fall through to default logic
    }
    // All MCP tools require approval (external plugins)
    if name.starts_with("mcp__") {
        return true;
    }
    match name {
        "exec" | "email_send" | "email_archive" | "config_set" | "gmail_send" | "gcal_create"
        | "ms_calendar_create" | "trigger_add" | "trigger_remove" | "notify" | "signal_send"
        | "imessage_send" | "memory_export" | "fs_write" | "browser" | "excalidraw"
        | "self_upgrade" => true,
        "http_request" => {
            let method = args
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_ascii_uppercase();
            method != "GET"
        }
        "scale" => {
            args.get("action").and_then(|v| v.as_str()) == Some("resize")
        }
        _ => false,
    }
}

// ── Triggers ────────────────────────────────────────────────────────────

pub(crate) fn load_triggers(_mem: &mut Vault) -> Vec<TriggerEntry> {
    // Primary: flat file config
    if let Some(ws) = flat_file_workspace() {
        let cfg_path = config_file_path(&ws);
        if cfg_path.exists() {
            let fc = load_file_config(&cfg_path);
            if !fc.triggers.is_empty() {
                return fc.triggers;
            }
        }
    }
    // Fallback: capsule
    load_config_json(_mem, "triggers")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub(crate) fn save_triggers(_mem: &mut Vault, triggers: &[TriggerEntry]) -> Result<(), String> {
    // Primary: flat file config
    if let Some(ws) = flat_file_workspace() {
        let value = serde_json::to_value(triggers).map_err(|e| e.to_string())?;
        return save_config_to_file(&ws, "triggers", value).map_err(|e| e.to_string());
    }
    // Fallback: capsule
    let json = serde_json::to_value(triggers).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&json).map_err(|e| e.to_string())?;
    save_config_entry(_mem, "triggers", &bytes).map_err(|e| e.to_string())?;
    Ok(())
}

// ── Filesystem helpers ──────────────────────────────────────────────────

pub(crate) fn allowed_fs_roots(workspace_override: &Option<PathBuf>) -> Vec<PathBuf> {
    if let Some(raw) = env_optional("AETHERVAULT_FS_ROOTS") {
        let roots: Vec<PathBuf> = raw
            .split(':')
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .collect();
        if !roots.is_empty() {
            return roots;
        }
    }
    if let Some(ws) = workspace_override {
        return vec![ws.clone()];
    }
    vec![env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
}

pub(crate) fn resolve_fs_path(path: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let raw = PathBuf::from(path);
    let candidates: Vec<PathBuf> = if raw.is_absolute() {
        vec![raw.clone()]
    } else {
        roots.iter().map(|r| r.join(&raw)).collect()
    };
    for root in roots {
        let root_canon = fs::canonicalize(root).map_err(|e| e.to_string())?;
        for cand in &candidates {
            let cand_canon = if cand.exists() {
                fs::canonicalize(cand).map_err(|e| e.to_string())?
            } else if let Some(parent) = cand.parent() {
                let parent_canon = fs::canonicalize(parent).map_err(|e| e.to_string())?;
                parent_canon.join(cand.file_name().unwrap_or_default())
            } else {
                continue;
            };
            if cand_canon.starts_with(&root_canon) {
                return Ok(cand.clone());
            }
        }
    }
    Err("path outside allowed roots".into())
}

// ── OAuth Token Refresh ─────────────────────────────────────────────────

pub(crate) fn refresh_google_token(
    mv2: &Path,
    token: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let refresh_token = token
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or("missing refresh_token")?;
    let client_id = oauth_env("GOOGLE_CLIENT_ID")?;
    let client_secret = oauth_env("GOOGLE_CLIENT_SECRET")?;
    let payload = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &client_id)
        .append_pair("client_secret", &client_secret)
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", refresh_token)
        .finish();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_write(Duration::from_millis(NO_TIMEOUT_MS))
        .build();
    let resp = agent
        .post("https://oauth2.googleapis.com/token")
        .set("content-type", "application/x-www-form-urlencoded")
        .send_string(&payload);
    let refreshed = match resp {
        Ok(resp) => resp.into_json::<serde_json::Value>()?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            return Err(format!("refresh error {code}: {text}").into());
        }
        Err(err) => return Err(format!("refresh failed: {err}").into()),
    };
    let mut new_token = refreshed.clone();
    if refreshed.get("refresh_token").is_none() {
        if let Some(rt) = token.get("refresh_token") {
            new_token["refresh_token"] = rt.clone();
        }
    }
    // Primary: flat file config
    if let Some(ws) = flat_file_workspace() {
        save_config_to_file(&ws, "oauth.google", new_token.clone())?;
    } else {
        let mut mem = open_or_create(mv2)?;
        let bytes = serde_json::to_vec_pretty(&new_token)?;
        let _ = save_config_entry(&mut mem, "oauth.google", &bytes)?;
    }
    Ok(new_token)
}

pub(crate) fn refresh_microsoft_token(
    mv2: &Path,
    token: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let refresh_token = token
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or("missing refresh_token")?;
    let client_id = oauth_env("MICROSOFT_CLIENT_ID")?;
    let client_secret = oauth_env("MICROSOFT_CLIENT_SECRET")?;
    let payload = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &client_id)
        .append_pair("client_secret", &client_secret)
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", refresh_token)
        .append_pair("scope", "offline_access https://graph.microsoft.com/Mail.Read https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite")
        .finish();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_write(Duration::from_millis(NO_TIMEOUT_MS))
        .build();
    let resp = agent
        .post("https://login.microsoftonline.com/common/oauth2/v2.0/token")
        .set("content-type", "application/x-www-form-urlencoded")
        .send_string(&payload);
    let refreshed = match resp {
        Ok(resp) => resp.into_json::<serde_json::Value>()?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            return Err(format!("refresh error {code}: {text}").into());
        }
        Err(err) => return Err(format!("refresh failed: {err}").into()),
    };
    let mut new_token = refreshed.clone();
    if refreshed.get("refresh_token").is_none() {
        if let Some(rt) = token.get("refresh_token") {
            new_token["refresh_token"] = rt.clone();
        }
    }
    // Primary: flat file config
    if let Some(ws) = flat_file_workspace() {
        save_config_to_file(&ws, "oauth.microsoft", new_token.clone())?;
    } else {
        let mut mem = open_or_create(mv2)?;
        let bytes = serde_json::to_vec_pretty(&new_token)?;
        let _ = save_config_entry(&mut mem, "oauth.microsoft", &bytes)?;
    }
    Ok(new_token)
}

pub(crate) fn get_oauth_token(mv2: &Path, provider: &str) -> Result<String, Box<dyn std::error::Error>> {
    let key = format!("oauth.{provider}");
    // Primary: try flat file config
    let flat_token = flat_file_workspace().and_then(|ws| {
        let cfg_path = config_file_path(&ws);
        if !cfg_path.exists() { return None; }
        let fc = load_file_config(&cfg_path);
        match provider {
            "google" => fc.oauth_google,
            "microsoft" => fc.oauth_microsoft,
            _ => None,
        }
    });
    let token = if let Some(t) = flat_token {
        t
    } else {
        let mut mem = Vault::open_read_only(mv2)?;
        load_config_json(&mut mem, &key).ok_or("missing oauth token")?
    };
    let access = token.get("access_token").and_then(|v| v.as_str());
    if let Some(access) = access {
        return Ok(access.to_string());
    }
    if provider == "google" {
        let refreshed = refresh_google_token(mv2, &token)?;
        let access = refreshed
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or("missing access_token")?;
        return Ok(access.to_string());
    }
    let refreshed = refresh_microsoft_token(mv2, &token)?;
    let access = refreshed
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("missing access_token")?;
    Ok(access.to_string())
}

// === Knowledge Graph Auto-Injection ===

#[derive(Debug, Deserialize)]
pub(crate) struct KgGraph {
    pub(crate) nodes: Vec<KgNode>,
    #[serde(default)]
    pub(crate) edges: Vec<KgEdge>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct KgNode {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(rename = "type", default)]
    pub(crate) node_type: Option<String>,
    #[serde(default)]
    pub(crate) properties: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct KgEdge {
    pub(crate) source: String,
    pub(crate) target: String,
    #[serde(default)]
    pub(crate) relation: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) confidence: Option<f64>,
}

pub(crate) fn load_kg_graph(path: &std::path::Path) -> Option<KgGraph> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Tokenize text into lowercase word tokens for similarity matching.
pub(crate) fn tokenize_words(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| w.chars().count() >= 2)
        .map(|w| w.to_string())
        .collect()
}

/// Token containment coefficient: what fraction of b's tokens appear in a?
/// Better than Jaccard for asymmetric sets (long query vs short entity name).
pub(crate) fn token_containment(query_tokens: &HashSet<String>, entity_tokens: &HashSet<String>) -> f64 {
    if entity_tokens.is_empty() {
        return 0.0;
    }
    let intersection = query_tokens.intersection(entity_tokens).count() as f64;
    intersection / entity_tokens.len() as f64
}

/// Simple edit distance (Levenshtein) for fuzzy matching.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();
    if m == 0 { return n; }
    if n == 0 { return m; }
    let mut prev = (0..=n).collect::<Vec<_>>();
    let mut curr = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

/// Check if the character immediately before byte_pos is a word boundary (non-alphanumeric).
pub(crate) fn is_boundary_before(text: &str, byte_pos: usize) -> bool {
    if byte_pos == 0 || byte_pos >= text.len() {
        return true;
    }
    let ch = text[..byte_pos].chars().next_back();
    ch.map(|c| !c.is_alphanumeric()).unwrap_or(true)
}

/// Check if the character at byte_pos is a word boundary (non-alphanumeric).
pub(crate) fn is_boundary_after(text: &str, byte_pos: usize) -> bool {
    if byte_pos >= text.len() {
        return true;
    }
    let ch = text[byte_pos..].chars().next();
    ch.map(|c| !c.is_alphanumeric()).unwrap_or(true)
}

pub(crate) fn find_kg_entities(text: &str, graph: &KgGraph) -> Vec<String> {
    let text_lower = text.to_lowercase();
    let text_tokens = tokenize_words(text);
    let mut scored: Vec<(String, f64)> = Vec::new();

    for node in &graph.nodes {
        let name = node.name.as_deref().unwrap_or(&node.id);
        let char_count = name.chars().count();
        if char_count < 3 { continue; }
        let name_lower = name.to_lowercase();
        let mut score: f64 = 0.0;

        // 1. Exact substring match (highest confidence)
        // For short names (<=5 chars), require word boundaries; check ALL occurrences
        if char_count <= 5 {
            let mut search_start = 0;
            while let Some(pos) = text_lower[search_start..].find(&name_lower) {
                let abs_pos = search_start + pos;
                let after_pos = abs_pos + name_lower.len();
                let before_ok = is_boundary_before(&text_lower, abs_pos);
                let after_ok = is_boundary_after(&text_lower, after_pos);
                if before_ok && after_ok {
                    score = 1.0;
                    break;
                }
                // Move past this occurrence and try the next
                search_start = abs_pos + name_lower.len().max(1);
                if search_start >= text_lower.len() { break; }
            }
        } else if text_lower.contains(&name_lower) {
            score = 1.0;
        }

        // 2. Token containment — what fraction of entity's words appear in the query?
        // Uses containment coefficient instead of Jaccard to handle asymmetric set sizes
        if score < 1.0 {
            let name_tokens = tokenize_words(name);
            let containment = token_containment(&text_tokens, &name_tokens);
            if containment > 0.3 {
                score = score.max(containment * 0.9); // slight discount vs exact match
            }
        }

        // 3. Edit distance fuzzy match — catches typos (only for single-word names)
        if score < 0.5 && !name_lower.contains(' ') {
            let name_char_count = name_lower.chars().count();
            for word in &text_tokens {
                let word_char_count = word.chars().count();
                if word_char_count < 3 { continue; }
                // Skip if length difference is too large (can't meet 0.7 threshold)
                let len_diff = (word_char_count as isize - name_char_count as isize).unsigned_abs();
                let max_len = word_char_count.max(name_char_count);
                if max_len > 0 && (len_diff as f64 / max_len as f64) > 0.3 {
                    continue; // length filter: skip impossible matches
                }
                let dist = edit_distance(word, &name_lower);
                if max_len > 0 {
                    let similarity = 1.0 - (dist as f64 / max_len as f64);
                    if similarity >= 0.7 {
                        score = score.max(similarity * 0.8); // discount fuzzy matches
                    }
                }
            }
        }

        if score >= 0.3 {
            scored.push((name.to_string(), score));
        }
    }

    // Sort by score descending, cap at 5
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(5);
    scored.into_iter().map(|(name, _)| name).collect()
}

pub(crate) fn build_kg_context(entity_names: &[String], graph: &KgGraph) -> String {
    let mut ctx = String::new();
    for name in entity_names {
        let node = graph.nodes.iter().find(|n| {
            n.name.as_deref().unwrap_or(&n.id) == name
        });
        if let Some(node) = node {
            let node_type = node.node_type.as_deref().unwrap_or("unknown");
            ctx.push_str(&format!("## {} ({})\n", name, node_type));
            if let Some(ref props) = node.properties {
                if !props.is_empty() {
                    let props_str: Vec<String> = props.iter()
                        .filter(|(k, _)| *k != "name" && *k != "type")
                        .map(|(k, v)| format!("{}={}", k, v))
                        .collect();
                    if !props_str.is_empty() {
                        ctx.push_str(&format!("Properties: {}\n", props_str.join(", ")));
                    }
                }
            }
            for edge in &graph.edges {
                if edge.source == node.id {
                    let rel = edge.relation.as_deref().unwrap_or("related-to");
                    ctx.push_str(&format!("  -> {} -> {}\n", rel, edge.target));
                }
                if edge.target == node.id {
                    let rel = edge.relation.as_deref().unwrap_or("related-to");
                    ctx.push_str(&format!("  <- {} <- {}\n", rel, edge.source));
                }
            }
            ctx.push('\n');
        }
    }
    ctx
}

// ── Workspace ───────────────────────────────────────────────────────────

pub(crate) fn load_workspace_context(workspace: &Path) -> String {
    let mut sections = Vec::new();
    let soul = workspace.join("SOUL.md");
    let user = workspace.join("USER.md");
    let memory = workspace.join("MEMORY.md");
    if let Some(text) = read_optional_file(&soul) {
        sections.push(format!("# Soul\n{text}"));
    }
    if let Some(text) = read_optional_file(&user) {
        sections.push(format!("# User\n{text}"));
    }
    if let Some(text) = read_optional_file(&memory) {
        sections.push(format!("# Memory\n{text}"));
    }
    let daily = daily_memory_path(workspace);
    if let Some(text) = read_optional_file(&daily) {
        sections.push(format!("# Daily Log\n{text}"));
    }
    sections.join("\n\n")
}

pub(crate) fn bootstrap_workspace(
    mv2: &Path,
    workspace: &Path,
    timezone: Option<String>,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(workspace)?;
    fs::create_dir_all(workspace.join("memory"))?;

    let soul_path = workspace.join("SOUL.md");
    let user_path = workspace.join("USER.md");
    let memory_path = workspace.join("MEMORY.md");
    let daily_path = daily_memory_path(workspace);

    let create_file = |path: &Path, contents: &str| -> Result<(), Box<dyn std::error::Error>> {
        if path.exists() && !force {
            return Err(format!("File already exists: {}", path.display()).into());
        }
        fs::write(path, contents)?;
        Ok(())
    };

    let soul_template = "# Executive Assistant Soul\n\n- Act as a proactive executive assistant.\n- Be concise, direct, and high\u{2011}leverage.\n- Prefer action over explanation.\n- Ask for approval before external sends unless policy allows.\n";
    let user_template = "# User Profile\n\n- Name: Sunil Rao\n- Role: Executive\n- Preferences:\n  - Daily Overview at 8:30 AM\n  - Daily Recap at 3:30 PM\n  - Weekly Overview Monday 8:15 AM\n  - Weekly Recap Friday 3:15 PM\n";
    let memory_template =
        "# Long\u{2011}term Memory\n\n- Important contacts, preferences, and policies go here.\n";
    let daily_template = "# Daily Log\n\n- Created by bootstrap.\n";

    create_file(&soul_path, soul_template)?;
    create_file(&user_path, user_template)?;
    create_file(&memory_path, memory_template)?;
    create_file(&daily_path, daily_template)?;

    // Write config to flat file (primary) and capsule (fallback).
    let mut agent_cfg = AgentConfig::default();
    agent_cfg.workspace = Some(workspace.display().to_string());
    agent_cfg.onboarding_complete = Some(false);
    if timezone.is_some() {
        agent_cfg.timezone = timezone;
    }
    let mut config = CapsuleConfig::default();
    config.agent = Some(agent_cfg.clone());

    // Write to flat file
    let fc = crate::FileConfig {
        agent: agent_cfg,
        ..Default::default()
    };
    let cfg_path = config_file_path(workspace);
    crate::save_file_config(&cfg_path, &fc)?;

    // Also write to capsule for backwards compatibility
    let mut mem = open_or_create(mv2)?;
    let bytes = serde_json::to_vec_pretty(&config)?;
    let _ = save_config_entry(&mut mem, "index", &bytes)?;
    Ok(())
}

// ── Timezone / Scheduling ───────────────────────────────────────────────

pub(crate) fn parse_timezone_offset(value: &str) -> Result<chrono::FixedOffset, Box<dyn std::error::Error>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(chrono::FixedOffset::east_opt(0).unwrap());
    }
    let sign = if trimmed.starts_with('-') { -1 } else { 1 };
    let value = trimmed.trim_start_matches(['+', '-']);
    let mut parts = value.split(':');
    let hours: i32 = parts
        .next()
        .ok_or("timezone")?
        .parse()
        .map_err(|_| "timezone hours")?;
    let minutes: i32 = parts
        .next()
        .unwrap_or("0")
        .parse()
        .map_err(|_| "timezone minutes")?;
    let total = sign * (hours * 3600 + minutes * 60);
    chrono::FixedOffset::east_opt(total).ok_or_else(|| "timezone offset".into())
}

pub(crate) fn resolve_timezone(
    agent_cfg: &AgentConfig,
    override_value: Option<String>,
) -> chrono::FixedOffset {
    let raw = override_value.or_else(|| agent_cfg.timezone.clone());
    raw.and_then(|v| parse_timezone_offset(&v).ok())
        .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap())
}

pub(crate) fn should_run_daily(
    last: &mut Option<chrono::NaiveDate>,
    now: chrono::DateTime<chrono::FixedOffset>,
    hour: u32,
    minute: u32,
) -> bool {
    let date = now.date_naive();
    if now.time().hour() != hour || now.time().minute() != minute {
        return false;
    }
    if last.as_ref().is_some_and(|d| *d == date) {
        return false;
    }
    *last = Some(date);
    true
}

pub(crate) fn should_run_weekly(
    last: &mut Option<chrono::NaiveDate>,
    now: chrono::DateTime<chrono::FixedOffset>,
    weekday: chrono::Weekday,
    hour: u32,
    minute: u32,
) -> bool {
    if now.weekday() != weekday {
        return false;
    }
    should_run_daily(last, now, hour, minute)
}

pub(crate) fn schedule_prompt(kind: &str) -> String {
    match kind {
        "daily_overview" => "Generate the Daily Overview. Sweep inbox (email_list), identify conflicts, and list top priorities. Include \"Needs Your Action\" items.".to_string(),
        "daily_recap" => "Generate the Daily Recap. Summarize what changed in inbox and calendar, actions taken, and pending follow-ups.".to_string(),
        "weekly_overview" => "Generate the Weekly Overview. List top priorities and key meetings. Flag conflicts and follow-ups.".to_string(),
        "weekly_recap" => "Generate the Weekly Recap. Summarize meetings handled, logistics, and outstanding items.".to_string(),
        _ => "Generate an executive summary.".to_string(),
    }
}

pub(crate) fn run_schedule_loop(
    mv2: PathBuf,
    workspace: Option<PathBuf>,
    timezone: Option<String>,
    telegram_token: Option<String>,
    telegram_chat_id: Option<String>,
    model_hook: Option<String>,
    max_steps: usize,
    log: bool,
    log_commit_interval: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    // Try flat file config first, fall back to capsule.
    let ws_env = env_optional("AETHERVAULT_WORKSPACE").map(PathBuf::from);
    let config = if let Some(ref ws) = ws_env {
        let cfg_path = config_file_path(ws);
        if cfg_path.exists() {
            load_config_from_file(ws)
        } else {
            let mut mem_read = Some(Vault::open_read_only(&mv2)?);
            load_capsule_config(mem_read.as_mut().unwrap()).unwrap_or_default()
        }
    } else {
        let mut mem_read = Some(Vault::open_read_only(&mv2)?);
        load_capsule_config(mem_read.as_mut().unwrap()).unwrap_or_default()
    };
    let agent_cfg = config.agent.clone().unwrap_or_default();
    let tz = resolve_timezone(&agent_cfg, timezone);
    let workspace = resolve_workspace(workspace, &agent_cfg);
    let telegram_token = telegram_token
        .or(agent_cfg.telegram_token)
        .or_else(|| env_optional("TELEGRAM_BOT_TOKEN"));
    let telegram_chat_id = telegram_chat_id
        .or(agent_cfg.telegram_chat_id)
        .or_else(|| env_optional("AETHERVAULT_TELEGRAM_CHAT_ID"));

    let agent_config = build_bridge_agent_config(
        mv2.clone(),
        model_hook,
        None,
        false,
        None,
        8,
        12_000,
        max_steps,
        log,
        log_commit_interval,
    )?;

    let mut last_daily_overview = None;
    let mut last_daily_recap = None;
    let mut last_weekly_overview = None;
    let mut last_weekly_recap = None;

    loop {
        let now = chrono::Utc::now().with_timezone(&tz);
        let mut tasks = Vec::new();
        if should_run_daily(&mut last_daily_overview, now, 8, 30) {
            tasks.push("daily_overview");
        }
        if should_run_daily(&mut last_daily_recap, now, 15, 30) {
            tasks.push("daily_recap");
        }
        if should_run_weekly(&mut last_weekly_overview, now, chrono::Weekday::Mon, 8, 15) {
            tasks.push("weekly_overview");
        }
        if should_run_weekly(&mut last_weekly_recap, now, chrono::Weekday::Fri, 15, 15) {
            tasks.push("weekly_recap");
        }

        for task in tasks {
            let mut prompt = schedule_prompt(task);
            if let Some(ws) = &workspace {
                prompt.push_str(&format!("\n\nWorkspace: {}", ws.display()));
            }
            let session = format!("schedule:{task}");
            let result = run_agent_for_bridge(&agent_config, &prompt, session, None, None, None);
            if let Ok(output) = result {
                if let Some(text) = output.final_text {
                    if let (Some(token), Some(chat_id)) =
                        (telegram_token.as_ref(), telegram_chat_id.as_ref())
                    {
                        let agent = ureq::AgentBuilder::new()
                            .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
                            .timeout_write(Duration::from_millis(NO_TIMEOUT_MS))
                            .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                            .build();
                        let base_url = match std::env::var("TELEGRAM_API_BASE") {
    Ok(base) => format!("{base}/bot{token}"),
    Err(_) => format!("https://api.telegram.org/bot{token}"),
};
                        if let Ok(chat_id) = chat_id.parse::<i64>() {
                            let _ = telegram_send_message(&agent, &base_url, chat_id, &text);
                        }
                    }
                }
            }
        }

        thread::sleep(Duration::from_secs(30));
    }
}

pub(crate) fn run_watch_loop(
    mv2: PathBuf,
    workspace: Option<PathBuf>,
    timezone: Option<String>,
    model_hook: Option<String>,
    max_steps: usize,
    log: bool,
    log_commit_interval: usize,
    poll_seconds: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    // Try flat file config first, fall back to capsule.
    let ws_env2 = env_optional("AETHERVAULT_WORKSPACE").map(PathBuf::from);
    let config = if let Some(ref ws) = ws_env2 {
        let cfg_path = config_file_path(ws);
        if cfg_path.exists() {
            load_config_from_file(ws)
        } else {
            let mut mr = Some(Vault::open_read_only(&mv2)?);
            load_capsule_config(mr.as_mut().unwrap()).unwrap_or_default()
        }
    } else {
        let mut mr = Some(Vault::open_read_only(&mv2)?);
        load_capsule_config(mr.as_mut().unwrap()).unwrap_or_default()
    };
    let agent_cfg = config.agent.clone().unwrap_or_default();
    let tz = resolve_timezone(&agent_cfg, timezone);
    let workspace = resolve_workspace(workspace, &agent_cfg);
    let agent_config = build_bridge_agent_config(
        mv2.clone(),
        model_hook,
        None,
        false,
        None,
        8,
        12_000,
        max_steps,
        log,
        log_commit_interval,
    )?;

    loop {
        let now = chrono::Utc::now().with_timezone(&tz);
        let mut mem = open_or_create(&mv2)?;
        let mut triggers = load_triggers(&mut mem);
        let mut updated = false;

        for trigger in triggers.iter_mut() {
            if !trigger.enabled {
                continue;
            }
            match trigger.kind.as_str() {
                "email" => {
                    let query = match &trigger.query {
                        Some(q) if !q.trim().is_empty() => q.clone(),
                        _ => continue,
                    };
                    let token = match get_oauth_token(&mv2, "google") {
                        Ok(token) => token,
                        Err(_) => continue,
                    };
                    let agent = ureq::AgentBuilder::new()
                        .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
                        .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                        .build();
                    let mut url =
                        "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults=1"
                            .to_string();
                    url.push_str("&q=");
                    url.push_str(&urlencoding::encode(&query));
                    let resp = agent
                        .get(&url)
                        .set("authorization", &format!("Bearer {}", token))
                        .call();
                    let payload = match resp {
                        Ok(resp) => resp.into_json::<serde_json::Value>().unwrap_or_default(),
                        Err(_) => continue,
                    };
                    let id = payload
                        .get("messages")
                        .and_then(|m| m.as_array())
                        .and_then(|arr| arr.get(0))
                        .and_then(|m| m.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    if let Some(id) = id {
                        if trigger.last_seen.as_deref() != Some(&id) {
                            trigger.last_seen = Some(id.clone());
                            trigger.last_fired = Some(now.to_rfc3339());
                            updated = true;
                            let mut prompt = trigger.prompt.clone().unwrap_or_else(|| {
                                "New email received. Review and take action.".to_string()
                            });
                            prompt.push_str(&format!(
                                "\n\nQuery: {query}\nMessage ID: {id}\nUse gmail_read to inspect."
                            ));
                            if let Some(ws) = &workspace {
                                prompt.push_str(&format!("\nWorkspace: {}", ws.display()));
                            }
                            let session = format!("trigger:email:{}", trigger.id);
                            let _ =
                                run_agent_for_bridge(&agent_config, &prompt, session, None, None, None);
                        }
                    }
                }
                "calendar_free" => {
                    let start = match &trigger.start {
                        Some(s) => s.clone(),
                        None => continue,
                    };
                    let end = match &trigger.end {
                        Some(e) => e.clone(),
                        None => continue,
                    };
                    let token = match get_oauth_token(&mv2, "google") {
                        Ok(token) => token,
                        Err(_) => continue,
                    };
                    let agent = ureq::AgentBuilder::new()
                        .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
                        .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                        .build();
                    let url = format!(
                        "https://www.googleapis.com/calendar/v3/calendars/primary/events?timeMin={}&timeMax={}&maxResults=1&singleEvents=true",
                        urlencoding::encode(&start),
                        urlencoding::encode(&end)
                    );
                    let resp = agent
                        .get(&url)
                        .set("authorization", &format!("Bearer {}", token))
                        .call();
                    let payload = match resp {
                        Ok(resp) => resp.into_json::<serde_json::Value>().unwrap_or_default(),
                        Err(_) => continue,
                    };
                    let has_events = payload
                        .get("items")
                        .and_then(|v| v.as_array())
                        .map(|arr| !arr.is_empty())
                        .unwrap_or(false);
                    if !has_events {
                        let fired_today = trigger
                            .last_fired
                            .as_deref()
                            .and_then(|v| v.split('T').next())
                            .map(|d| d == now.date_naive().to_string())
                            .unwrap_or(false);
                        if !fired_today {
                            trigger.last_fired = Some(now.to_rfc3339());
                            updated = true;
                            let mut prompt = trigger.prompt.clone().unwrap_or_else(|| {
                                "Calendar is free in the requested window. Schedule task."
                                    .to_string()
                            });
                            prompt.push_str(&format!(
                                "\n\nWindow: {start} → {end}\nNo events detected."
                            ));
                            if let Some(ws) = &workspace {
                                prompt.push_str(&format!("\nWorkspace: {}", ws.display()));
                            }
                            let session = format!("trigger:calendar:{}", trigger.id);
                            let _ =
                                run_agent_for_bridge(&agent_config, &prompt, session, None, None, None);
                        }
                    }
                }
                "cron" => {
                    let cron_str = match &trigger.cron {
                        Some(c) if !c.trim().is_empty() => c.clone(),
                        _ => continue,
                    };
                    let cron_expr = match CronExpr::parse(&cron_str) {
                        Ok(expr) => expr,
                        Err(e) => {
                            eprintln!("[watch] trigger '{}' bad cron: {e}", trigger.id);
                            continue;
                        }
                    };
                    // chrono dow: Mon=1..Sun=7; cron: Sun=0..Sat=6
                    let dow = match now.weekday() {
                        chrono::Weekday::Sun => 0,
                        chrono::Weekday::Mon => 1,
                        chrono::Weekday::Tue => 2,
                        chrono::Weekday::Wed => 3,
                        chrono::Weekday::Thu => 4,
                        chrono::Weekday::Fri => 5,
                        chrono::Weekday::Sat => 6,
                    };
                    if cron_expr.matches(
                        now.minute(),
                        now.hour(),
                        now.day(),
                        now.month(),
                        dow,
                    ) {
                        // Don't fire more than once in the same minute
                        let current_minute = format!("{}-{:02}-{:02}T{:02}:{:02}",
                            now.year(), now.month(), now.day(), now.hour(), now.minute());
                        if trigger.last_fired.as_deref() == Some(&current_minute) {
                            continue;
                        }
                        trigger.last_fired = Some(current_minute);
                        updated = true;
                        let mut prompt = trigger.prompt.clone().unwrap_or_else(|| {
                            format!("Cron trigger '{}' fired.", trigger.name.as_deref().unwrap_or(&trigger.id))
                        });
                        if let Some(ws) = &workspace {
                            prompt.push_str(&format!("\nWorkspace: {}", ws.display()));
                        }
                        let session = format!("trigger:cron:{}", trigger.id);
                        if let Err(e) = run_agent_for_bridge(&agent_config, &prompt, session, None, None, None) {
                            eprintln!("[watch] trigger '{}' agent failed: {e}", trigger.id);
                        }
                    }
                }
                "webhook" => {
                    let url = match &trigger.webhook_url {
                        Some(u) if !u.trim().is_empty() => u.clone(),
                        _ => continue,
                    };
                    let method = trigger.webhook_method.as_deref().unwrap_or("GET").to_uppercase();
                    let agent = ureq::AgentBuilder::new()
                        .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
                        .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
                        .build();
                    let resp = match method.as_str() {
                        "POST" => agent.post(&url).call(),
                        _ => agent.get(&url).call(),
                    };
                    let payload = match resp {
                        Ok(resp) => resp.into_string().unwrap_or_default(),
                        Err(e) => {
                            eprintln!("[watch] trigger '{}' webhook error: {e}", trigger.id);
                            continue;
                        }
                    };
                    // Only fire if response changed since last check
                    let payload_hash = blake3::hash(payload.as_bytes()).to_hex().to_string();
                    if trigger.last_seen.as_deref() == Some(&payload_hash) {
                        continue;
                    }
                    // First poll: record baseline without firing
                    if trigger.last_seen.is_none() {
                        trigger.last_seen = Some(payload_hash);
                        updated = true;
                        continue;
                    }
                    trigger.last_seen = Some(payload_hash);
                    trigger.last_fired = Some(now.to_rfc3339());
                    updated = true;
                    let mut prompt = trigger.prompt.clone().unwrap_or_else(|| {
                        format!("Webhook trigger '{}' detected a change.", trigger.name.as_deref().unwrap_or(&trigger.id))
                    });
                    let preview_end = payload.char_indices()
                        .take_while(|&(i, _)| i < 500)
                        .last()
                        .map(|(i, c)| i + c.len_utf8())
                        .unwrap_or(0);
                    prompt.push_str(&format!("\n\nWebhook URL: {url}\nResponse preview: {}", &payload[..preview_end]));
                    if let Some(ws) = &workspace {
                        prompt.push_str(&format!("\nWorkspace: {}", ws.display()));
                    }
                    let session = format!("trigger:webhook:{}", trigger.id);
                    if let Err(e) = run_agent_for_bridge(&agent_config, &prompt, session, None, None, None) {
                        eprintln!("[watch] trigger '{}' agent failed: {e}", trigger.id);
                    }
                }
                _ => {}
            }
        }

        if updated {
            if let Err(e) = save_triggers(&mut mem, &triggers) {
                eprintln!("[watch] CRITICAL: failed to persist trigger state: {e}");
            }
        }
        thread::sleep(Duration::from_secs(poll_seconds));
    }
}

// ── Qdrant / Vector DB ──────────────────────────────────────────────────

#[cfg(feature = "vec")]
pub(crate) fn collect_active_frame_ids(mem: &Vault, scope: Option<&str>) -> Vec<u64> {
    let mut latest: HashMap<String, u64> = HashMap::new();
    let count = mem.frame_count() as u64;
    for frame_id in 0..count {
        let frame = match mem.frame_by_id(frame_id) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let Some(uri) = frame.uri.clone() else {
            continue;
        };
        if let Some(prefix) = scope {
            if !uri.starts_with(prefix) {
                continue;
            }
        }
        if frame.status == FrameStatus::Active {
            latest.insert(uri, frame.id);
        } else {
            latest.remove(&uri);
        }
    }
    let mut ids: Vec<u64> = latest.values().copied().collect();
    ids.sort();
    ids
}

#[cfg(feature = "vec")]
pub(crate) fn build_embed_config(
    model: Option<&str>,
    cache_capacity: usize,
    enable_cache: bool,
) -> TextEmbedConfig {
    let mut config = match model.map(|m| m.to_ascii_lowercase()) {
        Some(ref name) if name == "bge-small" || name == "bge-small-en-v1.5" => {
            TextEmbedConfig::bge_small()
        }
        Some(ref name) if name == "bge-base" || name == "bge-base-en-v1.5" => {
            TextEmbedConfig::bge_base()
        }
        Some(ref name) if name == "nomic" || name == "nomic-embed-text-v1.5" => {
            TextEmbedConfig::nomic()
        }
        Some(ref name) if name == "gte-large" => TextEmbedConfig::gte_large(),
        Some(name) => {
            let mut cfg = TextEmbedConfig::default();
            cfg.model_name = name;
            cfg
        }
        None => TextEmbedConfig::default(),
    };
    config.cache_capacity = cache_capacity;
    config.enable_cache = enable_cache;
    config
}

// === Qdrant External Vector DB Integration ===
// Provides REST-based integration with Qdrant for scalable vector search.
// Enabled when QDRANT_URL is set. Uses text-based search via Qdrant's built-in
// sparse/dense encoders or pre-indexed vectors.

/// Search Qdrant by text using the REST API.
/// Returns SearchHit results compatible with the existing fusion pipeline.
pub(crate) fn qdrant_search_text(
    base_url: &str,
    collection: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, String> {
    let url = format!("{}/collections/{}/points/query", base_url.trim_end_matches('/'), collection);
    let body = serde_json::json!({
        "query": query,
        "limit": limit,
        "with_payload": true
    });

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
        .build();

    let resp = agent.post(&url)
        .set("content-type", "application/json")
        .send_string(&serde_json::to_string(&body).map_err(|e| e.to_string())?)
        .map_err(|e| format!("qdrant request: {e}"))?;

    let result: serde_json::Value = resp.into_json().map_err(|e| format!("qdrant parse: {e}"))?;

    // Check for Qdrant error status before extracting points
    if let Some(status) = result.get("status").and_then(|s| s.as_str()) {
        if status == "error" {
            let msg = result.get("result").and_then(|r| r.get("description")).and_then(|m| m.as_str()).unwrap_or("unknown");
            return Err(format!("qdrant error: {msg}"));
        }
    }

    let points = result.get("result")
        .or_else(|| result.get("points"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut hits = Vec::new();
    for (rank, point) in points.iter().enumerate() {
        let score = point.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let payload = point.get("payload").cloned().unwrap_or_default();
        let uri = payload.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let title = payload.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
        let text = payload.get("text")
            .or_else(|| payload.get("snippet"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(300)
            .collect::<String>();
        let frame_id = point.get("id")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        hits.push(SearchHit {
            rank,
            frame_id,
            uri,
            title,
            range: (0, 0),
            text,
            matches: 0,
            chunk_range: None,
            chunk_text: None,
            score: Some(score),
            metadata: None,
        });
    }

    Ok(hits)
}


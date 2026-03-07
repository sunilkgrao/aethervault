use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use aether_core::Vault;
use serde::{Deserialize, Serialize};

use crate::{
    ToolExecution, blake3_hash, env_optional, execute_tool, load_config_json, open_or_create,
    save_config_entry,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ApprovalEntry {
    pub(crate) id: String,
    pub(crate) tool: String,
    pub(crate) args_hash: String,
    pub(crate) args: serde_json::Value,
    pub(crate) status: String,
    pub(crate) created_at: String,
}

enum ApprovalChatCommand {
    Approve(String),
    Reject(String),
}

pub(crate) fn approval_hash(tool: &str, args: &serde_json::Value) -> String {
    let payload = serde_json::json!({ "tool": tool, "args": args });
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    blake3_hash(&bytes).to_hex().to_string()
}

pub(crate) fn load_approvals(mem: &mut Vault) -> Vec<ApprovalEntry> {
    load_config_json(mem, "approvals")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub(crate) fn save_approvals(mem: &mut Vault, approvals: &[ApprovalEntry]) -> Result<(), String> {
    let json = serde_json::to_value(approvals).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&json).map_err(|e| e.to_string())?;
    save_config_entry(mem, "approvals", &bytes).map_err(|e| e.to_string())?;
    Ok(())
}

fn parse_approval_chat_command(text: &str) -> Option<ApprovalChatCommand> {
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

pub(crate) fn approve_and_maybe_execute(
    mv2: &Path,
    id: &str,
    execute: bool,
) -> Result<String, String> {
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
    let result: Result<ToolExecution, String> = execute_tool(&entry.tool, entry.args, mv2, false);
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
    match name {
        "exec" | "email_send" | "email_archive" | "config_set" | "gmail_send" | "gcal_create"
        | "ms_calendar_create" | "trigger_add" | "trigger_remove" | "notify" | "signal_send"
        | "imessage_send" | "memory_export" | "fs_write" | "browser_request" => true,
        "http_request" => {
            let method = args
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_ascii_uppercase();
            method != "GET"
        }
        _ => false,
    }
}

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

#[cfg(test)]
mod tests {
    use super::requires_approval;

    #[test]
    fn requires_approval_matches_high_risk_tools() {
        assert!(requires_approval(
            "notify",
            &serde_json::json!({"text":"hi"})
        ));
        assert!(requires_approval(
            "http_request",
            &serde_json::json!({"method":"POST","url":"https://example.com"})
        ));
        assert!(!requires_approval(
            "http_request",
            &serde_json::json!({"method":"GET","url":"https://example.com"})
        ));
        assert!(!requires_approval(
            "scale",
            &serde_json::json!({"action":"status"})
        ));
    }
}

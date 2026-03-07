use aether_core::{PutOptions, Vault};
use chrono::{NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::blake3_hash;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AgentLogEntry {
    pub(crate) session: Option<String>,
    pub(crate) role: String,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) meta: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) ts_utc: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CollectedAgentLog {
    pub(crate) frame_id: u64,
    pub(crate) uri: String,
    pub(crate) session: Option<String>,
    pub(crate) role: String,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) meta: Option<serde_json::Value>,
    pub(crate) ts_utc: i64,
}

pub(crate) fn append_agent_log(mem: &mut Vault, entry: &AgentLogEntry) -> Result<String, String> {
    append_agent_log_with_commit(mem, entry, true)
}

pub(crate) fn append_agent_log_uncommitted(
    mem: &mut Vault,
    entry: &AgentLogEntry,
) -> Result<String, String> {
    append_agent_log_with_commit(mem, entry, false)
}

fn append_agent_log_with_commit(
    mem: &mut Vault,
    entry: &AgentLogEntry,
    commit: bool,
) -> Result<String, String> {
    let ts = entry.ts_utc.unwrap_or_else(|| Utc::now().timestamp());
    let session = entry
        .session
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("default");
    let mut payload = serde_json::to_vec(entry).map_err(|e| e.to_string())?;
    if !payload.ends_with(b"\n") {
        payload.push(b'\n');
    }
    let uri = format!(
        "aethervault://agent-log/{session}/{ts}-{}",
        blake3_hash(&payload).to_hex()
    );
    let mut options = PutOptions::default();
    options.uri = Some(uri.clone());
    options.title = Some(format!("agent log {session} {ts}"));
    options.track = Some("aethervault.agent_log".to_string());
    options.kind = Some("application/json".to_string());
    options.search_text = Some(format!("{} {}", entry.role, entry.text));
    mem.put_bytes_with_options(&payload, options)
        .map_err(|e| e.to_string())?;
    if commit {
        mem.commit().map_err(|e| e.to_string())?;
    }
    Ok(uri)
}

fn parse_log_ts_from_uri(uri: &str) -> Option<i64> {
    let tail = uri.rsplit('/').next()?;
    let ts_str = tail.split('-').next()?;
    ts_str.parse::<i64>().ok()
}

pub(crate) fn collect_agent_logs(
    mem: &mut Vault,
    session_filter: Option<&str>,
    date_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<CollectedAgentLog>, String> {
    let target_date = match date_filter {
        Some(value) => Some(
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|e| format!("invalid date '{value}': {e}"))?,
        ),
        None => None,
    };
    let scope = session_filter
        .map(|session| format!("aethervault://agent-log/{session}/"))
        .unwrap_or_else(|| "aethervault://agent-log/".to_string());
    let mut entries = Vec::new();
    let total = mem.frame_count() as u64;
    for frame_id in 0..total {
        let frame = match mem.frame_by_id(frame_id) {
            Ok(frame) => frame,
            Err(_) => continue,
        };
        let Some(uri) = frame.uri.as_deref() else {
            continue;
        };
        if !uri.starts_with(&scope) {
            continue;
        }
        let text = match mem.frame_text_by_id(frame_id) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let entry = match serde_json::from_str::<AgentLogEntry>(&text) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let ts_utc = entry
            .ts_utc
            .or_else(|| parse_log_ts_from_uri(uri))
            .unwrap_or(frame.timestamp);
        if let Some(target_date) = target_date {
            let Some(ts) = Utc.timestamp_opt(ts_utc, 0).single() else {
                continue;
            };
            if ts.date_naive() != target_date {
                continue;
            }
        }
        entries.push(CollectedAgentLog {
            frame_id,
            uri: uri.to_string(),
            session: entry.session.clone(),
            role: entry.role,
            text: entry.text,
            meta: entry.meta,
            ts_utc,
        });
    }
    entries.sort_by(|a, b| b.ts_utc.cmp(&a.ts_utc).then_with(|| a.uri.cmp(&b.uri)));
    entries.truncate(limit);
    Ok(entries)
}

pub(crate) fn format_agent_logs(entries: &[CollectedAgentLog]) -> String {
    if entries.is_empty() {
        return "No agent logs found.".to_string();
    }
    let mut ordered = entries.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| {
        a.session
            .cmp(&b.session)
            .then_with(|| a.ts_utc.cmp(&b.ts_utc))
            .then_with(|| a.uri.cmp(&b.uri))
    });
    let mut lines = Vec::new();
    let mut current_session: Option<&str> = None;
    for entry in ordered {
        let session = entry.session.as_deref().unwrap_or("default");
        if current_session != Some(session) {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!("## {session}"));
            current_session = Some(session);
        }
        let stamp = Utc
            .timestamp_opt(entry.ts_utc, 0)
            .single()
            .map(|ts| ts.to_rfc3339())
            .unwrap_or_else(|| entry.ts_utc.to_string());
        let text = entry.text.replace('\n', " ").trim().to_string();
        lines.push(format!("[{stamp}] {}: {}", entry.role, text));
    }
    lines.join("\n")
}

use std::fs;
use std::path::{Path, PathBuf};

use aether_core::{PutOptions, Vault};
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};

const PRIMARY_STATE_MARKDOWN_FILE: &str = "STATE.md";
const PRIMARY_STATE_JSON_FILE: &str = "STATE.json";
const LEGACY_STATE_MARKDOWN_FILE: &str = "SSTATE.md";
const LEGACY_STATE_JSON_FILE: &str = "SSTATE.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutiveState {
    #[serde(default)]
    pub items: Vec<ExecutiveStateItem>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutiveStateItem {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub next_action: Option<String>,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub waiting_on: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    pub updated_at: String,
}

pub struct StateCaptureInput<'a> {
    pub id: Option<&'a str>,
    pub title: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub status: Option<&'a str>,
    pub next_action: Option<&'a str>,
    pub due: Option<&'a str>,
    pub waiting_on: Option<&'a str>,
    pub note: Option<&'a str>,
    pub source: Option<&'a str>,
    pub session: Option<&'a str>,
}

pub fn state_markdown_path(workspace: &Path) -> PathBuf {
    workspace.join(PRIMARY_STATE_MARKDOWN_FILE)
}

pub fn state_json_path(workspace: &Path) -> PathBuf {
    workspace.join(PRIMARY_STATE_JSON_FILE)
}

pub fn state_memory_uri() -> String {
    "aethervault://memory/state/state.md".to_string()
}

pub fn state_json_memory_uri() -> String {
    "aethervault://memory/state/state.json".to_string()
}

fn legacy_state_memory_uri() -> String {
    "aethervault://memory/state/sstate.md".to_string()
}

fn legacy_state_json_memory_uri() -> String {
    "aethervault://memory/state/sstate.json".to_string()
}

pub fn state_memory_uri_candidates() -> Vec<String> {
    vec![state_memory_uri(), legacy_state_memory_uri()]
}

pub fn state_json_memory_uri_candidates() -> Vec<String> {
    vec![state_json_memory_uri(), legacy_state_json_memory_uri()]
}

fn legacy_state_markdown_path(workspace: &Path) -> PathBuf {
    workspace.join(LEGACY_STATE_MARKDOWN_FILE)
}

fn legacy_state_json_path(workspace: &Path) -> PathBuf {
    workspace.join(LEGACY_STATE_JSON_FILE)
}

pub fn executive_state_files_exist(workspace: &Path) -> bool {
    state_markdown_path(workspace).exists()
        || state_json_path(workspace).exists()
        || legacy_state_markdown_path(workspace).exists()
        || legacy_state_json_path(workspace).exists()
}

pub fn state_markdown_source_path(workspace: &Path) -> Option<PathBuf> {
    let primary = state_markdown_path(workspace);
    if primary.exists() {
        return Some(primary);
    }
    let legacy = legacy_state_markdown_path(workspace);
    if legacy.exists() {
        return Some(legacy);
    }
    None
}

pub fn state_json_source_path(workspace: &Path) -> Option<PathBuf> {
    let primary = state_json_path(workspace);
    if primary.exists() {
        return Some(primary);
    }
    let legacy = legacy_state_json_path(workspace);
    if legacy.exists() {
        return Some(legacy);
    }
    None
}

pub fn normalize_state_kind(value: Option<&str>) -> String {
    match value.unwrap_or("task").trim().to_ascii_lowercase().as_str() {
        "priority" | "task" | "project" | "follow_up" | "follow-up" | "waiting_on"
        | "waiting-on" | "note" | "meeting" | "draft" => value
            .unwrap_or("task")
            .trim()
            .to_ascii_lowercase()
            .replace('-', "_"),
        _ => "task".to_string(),
    }
}

pub fn normalize_state_status(value: Option<&str>) -> String {
    match value
        .unwrap_or("active")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "active" | "pending" | "waiting" | "done" | "archived" => {
            value.unwrap_or("active").trim().to_ascii_lowercase()
        }
        _ => "active".to_string(),
    }
}

pub fn state_item_sort_key(item: &ExecutiveStateItem) -> (i32, String, String) {
    let status_rank = match item.status.as_str() {
        "active" => 0,
        "pending" => 1,
        "waiting" => 2,
        "done" => 3,
        "archived" => 4,
        _ => 5,
    };
    let due = item.due.clone().unwrap_or_else(|| "9999-99-99".to_string());
    (status_rank, due, item.updated_at.clone())
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn state_context_line(item: &ExecutiveStateItem) -> String {
    let mut parts = vec![format!(
        "[{}][{}] {}",
        item.status,
        item.kind,
        item.title.trim()
    )];
    if let Some(next_action) = item.next_action.as_deref() {
        if !next_action.trim().is_empty() {
            parts.push(format!("next: {}", next_action.trim()));
        }
    }
    if let Some(waiting_on) = item.waiting_on.as_deref() {
        if !waiting_on.trim().is_empty() {
            parts.push(format!("waiting on: {}", waiting_on.trim()));
        }
    }
    if let Some(due) = item.due.as_deref() {
        if !due.trim().is_empty() {
            parts.push(format!("due: {}", due.trim()));
        }
    }
    parts.join(" | ")
}

fn state_due_within_days(item: &ExecutiveStateItem, days: i64) -> bool {
    item.due
        .as_deref()
        .and_then(|due| NaiveDate::parse_from_str(due.trim(), "%Y-%m-%d").ok())
        .map(|due| {
            let today = Utc::now().date_naive();
            let delta = due.signed_duration_since(today).num_days();
            (0..=days).contains(&delta)
        })
        .unwrap_or(false)
}

pub fn render_executive_state_context(state: &ExecutiveState) -> String {
    render_executive_state_focus(state, 8, true)
}

pub fn render_executive_state_focus(
    state: &ExecutiveState,
    limit: usize,
    include_notes: bool,
) -> String {
    let mut items = state.items.clone();
    items.sort_by_key(state_item_sort_key);
    let open_items = items
        .iter()
        .filter(|item| !matches!(item.status.as_str(), "done" | "archived"))
        .collect::<Vec<_>>();

    let mut lines = Vec::new();
    if !open_items.is_empty() {
        lines.push("Top open loops:".to_string());
        for item in open_items.iter().take(limit.max(1)) {
            lines.push(format!("- {}", state_context_line(item)));
        }
    }

    let due_soon = open_items
        .iter()
        .copied()
        .filter(|item| state_due_within_days(item, 14))
        .collect::<Vec<_>>();
    if !due_soon.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("Upcoming deadlines:".to_string());
        for item in due_soon.iter().take(limit.min(5).max(1)) {
            lines.push(format!("- {}", state_context_line(item)));
        }
    }

    let waiting = open_items
        .iter()
        .copied()
        .filter(|item| {
            item.status == "waiting" || matches!(item.kind.as_str(), "waiting_on" | "follow_up")
        })
        .collect::<Vec<_>>();
    if !waiting.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("Waiting on / follow-ups:".to_string());
        for item in waiting.iter().take(limit.min(5).max(1)) {
            lines.push(format!("- {}", state_context_line(item)));
        }
    }

    if include_notes && !state.notes.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("Recent executive notes:".to_string());
        for note in state.notes.iter().rev().take(4).rev() {
            if !note.trim().is_empty() {
                lines.push(format!("- {}", note.trim()));
            }
        }
    }

    lines.join("\n")
}

pub fn render_executive_state_markdown(state: &ExecutiveState) -> String {
    let mut lines = vec![
        "# Executive State".to_string(),
        "".to_string(),
        "## Open Loops".to_string(),
    ];
    let mut items = state.items.clone();
    items.sort_by_key(state_item_sort_key);
    let mut wrote_open_loop = false;
    for item in items
        .iter()
        .filter(|item| !matches!(item.status.as_str(), "done" | "archived"))
    {
        wrote_open_loop = true;
        lines.push(format!(
            "- [{}][{}] {}",
            item.status,
            item.kind,
            item.title.trim()
        ));
        if let Some(next_action) = item.next_action.as_deref() {
            if !next_action.trim().is_empty() {
                lines.push(format!("  next: {}", next_action.trim()));
            }
        }
        if let Some(waiting_on) = item.waiting_on.as_deref() {
            if !waiting_on.trim().is_empty() {
                lines.push(format!("  waiting_on: {}", waiting_on.trim()));
            }
        }
        if let Some(due) = item.due.as_deref() {
            if !due.trim().is_empty() {
                lines.push(format!("  due: {}", due.trim()));
            }
        }
        if let Some(note) = item.notes.last() {
            if !note.trim().is_empty() {
                lines.push(format!("  note: {}", note.trim()));
            }
        }
    }
    if !wrote_open_loop {
        lines.push("- None currently tracked.".to_string());
    }
    if !state.notes.is_empty() {
        lines.push("".to_string());
        lines.push("## Notes".to_string());
        for note in state.notes.iter().rev().take(8).rev() {
            lines.push(format!("- {}", note.trim()));
        }
    }
    if let Some(updated_at) = state.updated_at.as_deref() {
        lines.push("".to_string());
        lines.push(format!("_Updated: {updated_at}_"));
    }
    lines.join("\n")
}

pub fn load_executive_state(workspace: &Path) -> ExecutiveState {
    state_json_source_path(workspace)
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<ExecutiveState>(&text).ok())
        .unwrap_or_default()
}

pub fn save_executive_state(
    workspace: &Path,
    state: &ExecutiveState,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(workspace)?;
    let json_path = state_json_path(workspace);
    let markdown_path = state_markdown_path(workspace);
    fs::write(&json_path, serde_json::to_string_pretty(state)?)?;
    fs::write(&markdown_path, render_executive_state_markdown(state))?;
    let _ = fs::remove_file(legacy_state_json_path(workspace));
    let _ = fs::remove_file(legacy_state_markdown_path(workspace));
    Ok(())
}

fn sync_file_with_kind(
    mem: &mut Vault,
    path: &Path,
    uri: String,
    title: &str,
    kind: &str,
    track: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut options = PutOptions::default();
    options.uri = Some(uri);
    options.title = Some(title.to_string());
    options.kind = Some(kind.to_string());
    options.track = Some(track.to_string());
    options.search_text = Some(text.clone());
    let id = mem.put_bytes_with_options(text.as_bytes(), options)?;
    mem.commit()?;
    Ok(id)
}

pub fn sync_executive_state_files(
    mem: &mut Vault,
    workspace: &Path,
) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    let mut ids = Vec::new();
    if let Some(json_path) = state_json_source_path(workspace) {
        ids.push(sync_file_with_kind(
            mem,
            &json_path,
            state_json_memory_uri(),
            "memory executive state json",
            "application/json",
            "aethervault.memory",
        )?);
    }
    if let Some(markdown_path) = state_markdown_source_path(workspace) {
        ids.push(sync_file_with_kind(
            mem,
            &markdown_path,
            state_memory_uri(),
            "memory executive state",
            "text/markdown",
            "aethervault.memory",
        )?);
    }
    Ok(ids)
}

impl ExecutiveState {
    pub fn list_items(
        &self,
        kind_filter: Option<&str>,
        status_filter: Option<&str>,
        include_closed: bool,
        limit: usize,
    ) -> Vec<ExecutiveStateItem> {
        let kind_filter = kind_filter.map(|value| normalize_state_kind(Some(value)));
        let status_filter = status_filter.map(|value| normalize_state_status(Some(value)));
        let mut items = self.items.clone();
        items.retain(|item| {
            if !include_closed && matches!(item.status.as_str(), "done" | "archived") {
                return false;
            }
            if let Some(kind) = kind_filter.as_deref() {
                if item.kind != kind {
                    return false;
                }
            }
            if let Some(status) = status_filter.as_deref() {
                if item.status != status {
                    return false;
                }
            }
            true
        });
        items.sort_by_key(state_item_sort_key);
        items.truncate(limit);
        items
    }

    pub fn capture_item(
        &mut self,
        input: StateCaptureInput<'_>,
    ) -> Result<ExecutiveStateItem, String> {
        let now = now_rfc3339();
        let item_id = input.id.map(str::to_string).unwrap_or_else(|| {
            format!("state-{}-{}", Utc::now().timestamp(), self.items.len() + 1)
        });

        let saved_item = if let Some(item) = self.items.iter_mut().find(|item| item.id == item_id) {
            if let Some(title) = input.title.map(str::trim).filter(|title| !title.is_empty()) {
                item.title = title.to_string();
            }
            if input.kind.is_some() {
                item.kind = normalize_state_kind(input.kind);
            }
            if input.status.is_some() {
                item.status = normalize_state_status(input.status);
            }
            if let Some(next_action) = input.next_action.map(str::trim) {
                item.next_action = Some(next_action.to_string());
            }
            if let Some(due) = input.due.map(str::trim) {
                item.due = Some(due.to_string());
            }
            if let Some(waiting_on) = input.waiting_on.map(str::trim) {
                item.waiting_on = Some(waiting_on.to_string());
            }
            if let Some(source) = input.source.map(str::trim) {
                item.source = Some(source.to_string());
            }
            if let Some(session) = input.session.map(str::trim) {
                item.session = Some(session.to_string());
            }
            if let Some(note) = input.note.map(str::trim).filter(|note| !note.is_empty()) {
                item.notes.push(note.to_string());
            }
            item.updated_at = now.clone();
            item.clone()
        } else {
            let title = input
                .title
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .ok_or_else(|| {
                    "state_capture requires title when creating a new item".to_string()
                })?;
            let mut notes = Vec::new();
            if let Some(note) = input.note.map(str::trim).filter(|note| !note.is_empty()) {
                notes.push(note.to_string());
            }
            let item = ExecutiveStateItem {
                id: item_id.clone(),
                title: title.to_string(),
                kind: normalize_state_kind(input.kind),
                status: normalize_state_status(input.status),
                next_action: input.next_action.map(str::trim).map(str::to_string),
                due: input.due.map(str::trim).map(str::to_string),
                waiting_on: input.waiting_on.map(str::trim).map(str::to_string),
                notes,
                source: input.source.map(str::trim).map(str::to_string),
                session: input.session.map(str::trim).map(str::to_string),
                updated_at: now.clone(),
            };
            self.items.push(item.clone());
            item
        };

        self.updated_at = Some(now);
        Ok(saved_item)
    }

    pub fn close_item(
        &mut self,
        id: &str,
        resolution: Option<&str>,
    ) -> Result<ExecutiveStateItem, String> {
        let now = now_rfc3339();
        let mut closed_item = None;
        for item in self.items.iter_mut() {
            if item.id == id {
                item.status = "done".to_string();
                item.updated_at = now.clone();
                if let Some(resolution) =
                    resolution.map(str::trim).filter(|value| !value.is_empty())
                {
                    item.notes.push(format!("resolution: {resolution}"));
                }
                closed_item = Some(item.clone());
                break;
            }
        }
        let Some(closed_item) = closed_item else {
            return Err(format!("state item not found: {id}"));
        };
        self.updated_at = Some(now);
        Ok(closed_item)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn unique_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aethervault-{name}-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn render_focus_surfaces_open_waiting_and_due_items() {
        let state = ExecutiveState {
            items: vec![
                ExecutiveStateItem {
                    id: "task-1".to_string(),
                    title: "Prep board memo".to_string(),
                    kind: "task".to_string(),
                    status: "active".to_string(),
                    next_action: Some("Draft the outline".to_string()),
                    due: Some(Utc::now().date_naive().to_string()),
                    waiting_on: None,
                    notes: vec![],
                    source: None,
                    session: None,
                    updated_at: now_rfc3339(),
                },
                ExecutiveStateItem {
                    id: "follow-1".to_string(),
                    title: "Waiting on legal redlines".to_string(),
                    kind: "follow_up".to_string(),
                    status: "waiting".to_string(),
                    next_action: Some("Ping on Tuesday".to_string()),
                    due: None,
                    waiting_on: Some("Legal".to_string()),
                    notes: vec![],
                    source: None,
                    session: None,
                    updated_at: now_rfc3339(),
                },
                ExecutiveStateItem {
                    id: "done-1".to_string(),
                    title: "Booked restaurant".to_string(),
                    kind: "task".to_string(),
                    status: "done".to_string(),
                    next_action: None,
                    due: None,
                    waiting_on: None,
                    notes: vec![],
                    source: None,
                    session: None,
                    updated_at: now_rfc3339(),
                },
            ],
            notes: vec!["Keep Friday afternoon light.".to_string()],
            updated_at: Some(now_rfc3339()),
        };

        let rendered = render_executive_state_focus(&state, 5, true);
        assert!(rendered.contains("Top open loops:"));
        assert!(rendered.contains("Upcoming deadlines:"));
        assert!(rendered.contains("Waiting on / follow-ups:"));
        assert!(rendered.contains("Prep board memo"));
        assert!(rendered.contains("Waiting on legal redlines"));
        assert!(rendered.contains("Keep Friday afternoon light."));
        assert!(!rendered.contains("Booked restaurant"));
    }

    #[test]
    fn load_state_falls_back_to_legacy_json() {
        let workspace = unique_temp_dir("legacy-state");
        let legacy = workspace.join("SSTATE.json");
        fs::write(
            &legacy,
            r#"{"items":[{"id":"x1","title":"Legacy task","kind":"task","status":"active","updated_at":"2026-03-01T00:00:00Z"}],"notes":[],"updated_at":"2026-03-01T00:00:00Z"}"#,
        )
        .expect("write legacy state");

        let state = load_executive_state(&workspace);
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].title, "Legacy task");

        fs::remove_dir_all(workspace).expect("cleanup temp dir");
    }
}

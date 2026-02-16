use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use chrono::Utc;
use crate::AgentLogEntry;

pub(crate) fn log_dir_path(workspace: &Path) -> PathBuf {
    workspace.join("logs")
}

pub(crate) fn append_log_jsonl(
    log_dir: &Path,
    entry: &AgentLogEntry,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(log_dir)?;
    let date_str = Utc::now().format("%Y-%m-%d");
    let filename = format!("agent-{}.jsonl", date_str);
    let path = log_dir.join(filename);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let json = serde_json::to_string(entry)?;
    writeln!(file, "{}", json)?;
    Ok(())
}

pub(crate) fn load_session_logs(
    log_dir: &Path,
    session: &str,
    limit: usize,
) -> Vec<AgentLogEntry> {
    let mut files: Vec<PathBuf> = match fs::read_dir(log_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("agent-") && n.ends_with(".jsonl"))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => return Vec::new(),
    };
    files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    files.truncate(7);

    let mut collected = Vec::new();
    for path in &files {
        let file = match fs::File::open(path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let entry: AgentLogEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.session.as_deref() == Some(session) {
                collected.push(entry);
                if collected.len() >= limit {
                    collected.reverse();
                    return collected;
                }
            }
        }
    }
    collected.reverse();
    collected
}

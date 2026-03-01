use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SwarmStatus {
    Queued,
    Running,
    PrOpen,
    Reviewing,
    Done,
    Failed,
}

impl SwarmStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            SwarmStatus::Queued => "queued",
            SwarmStatus::Running => "running",
            SwarmStatus::PrOpen => "pr_open",
            SwarmStatus::Reviewing => "reviewing",
            SwarmStatus::Done => "done",
            SwarmStatus::Failed => "failed",
        }
    }

    pub(crate) fn from_str(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(SwarmStatus::Queued),
            "running" => Some(SwarmStatus::Running),
            "pr_open" => Some(SwarmStatus::PrOpen),
            "reviewing" => Some(SwarmStatus::Reviewing),
            "done" => Some(SwarmStatus::Done),
            "failed" => Some(SwarmStatus::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SwarmTask {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) prompt: String,
    pub(crate) status: SwarmStatus,
    pub(crate) branch: Option<String>,
    pub(crate) worktree_path: Option<String>,
    pub(crate) pr_number: Option<i64>,
    pub(crate) pr_url: Option<String>,
    pub(crate) ci_status: Option<String>,
    pub(crate) review_status: Option<String>,
    pub(crate) retry_count: u32,
    pub(crate) max_retries: u32,
    pub(crate) error_context: Option<String>,
    pub(crate) agent_backend: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

pub(crate) fn open_swarm_db(workspace: &Path) -> Result<Connection, String> {
    let db_path = workspace.join("swarm.sqlite");
    let conn = Connection::open(&db_path).map_err(|e| format!("open swarm db: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS swarm_tasks (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            prompt TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'queued',
            branch TEXT,
            worktree_path TEXT,
            pr_number INTEGER,
            pr_url TEXT,
            ci_status TEXT,
            review_status TEXT,
            retry_count INTEGER NOT NULL DEFAULT 0,
            max_retries INTEGER NOT NULL DEFAULT 3,
            error_context TEXT,
            agent_backend TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
    )
    .map_err(|e| format!("create swarm_tasks table: {e}"))?;
    Ok(conn)
}

fn next_swarm_id(conn: &Connection) -> String {
    let max: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(CAST(REPLACE(id, 'swarm-', '') AS INTEGER)), 0) FROM swarm_tasks",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    format!("swarm-{}", max + 1)
}

pub(crate) fn swarm_create_task(
    conn: &Connection,
    name: &str,
    prompt: &str,
    max_retries: Option<u32>,
) -> Result<SwarmTask, String> {
    let id = next_swarm_id(conn);
    let now = Utc::now().to_rfc3339();
    let max_retries = max_retries.unwrap_or(3);
    conn.execute(
        "INSERT INTO swarm_tasks (id, name, prompt, status, retry_count, max_retries, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'queued', 0, ?4, ?5, ?6)",
        params![id, name, prompt, max_retries, now, now],
    )
    .map_err(|e| format!("insert swarm task: {e}"))?;
    Ok(SwarmTask {
        id,
        name: name.to_string(),
        prompt: prompt.to_string(),
        status: SwarmStatus::Queued,
        branch: None,
        worktree_path: None,
        pr_number: None,
        pr_url: None,
        ci_status: None,
        review_status: None,
        retry_count: 0,
        max_retries,
        error_context: None,
        agent_backend: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Partial update: only non-None fields are written.
pub(crate) fn swarm_update_task(
    conn: &Connection,
    id: &str,
    status: Option<&str>,
    branch: Option<&str>,
    worktree_path: Option<&str>,
    pr_number: Option<i64>,
    pr_url: Option<&str>,
    ci_status: Option<&str>,
    review_status: Option<&str>,
    error_context: Option<&str>,
    agent_backend: Option<&str>,
    retry_count: Option<u32>,
) -> Result<SwarmTask, String> {
    let now = Utc::now().to_rfc3339();
    let mut sets = vec!["updated_at = ?1".to_string()];
    let mut idx = 2u32;

    macro_rules! maybe_set {
        ($field:expr, $col:expr) => {
            if $field.is_some() {
                sets.push(format!("{} = ?{}", $col, idx));
                idx += 1;
            }
        };
    }
    maybe_set!(status, "status");
    maybe_set!(branch, "branch");
    maybe_set!(worktree_path, "worktree_path");
    maybe_set!(pr_number, "pr_number");
    maybe_set!(pr_url, "pr_url");
    maybe_set!(ci_status, "ci_status");
    maybe_set!(review_status, "review_status");
    maybe_set!(error_context, "error_context");
    maybe_set!(agent_backend, "agent_backend");
    maybe_set!(retry_count, "retry_count");

    let sql = format!(
        "UPDATE swarm_tasks SET {} WHERE id = ?{}",
        sets.join(", "),
        idx
    );

    // Build dynamic params using rusqlite's boxed trait objects
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    param_values.push(Box::new(now));
    if let Some(v) = status { param_values.push(Box::new(v.to_string())); }
    if let Some(v) = branch { param_values.push(Box::new(v.to_string())); }
    if let Some(v) = worktree_path { param_values.push(Box::new(v.to_string())); }
    if let Some(v) = pr_number { param_values.push(Box::new(v)); }
    if let Some(v) = pr_url { param_values.push(Box::new(v.to_string())); }
    if let Some(v) = ci_status { param_values.push(Box::new(v.to_string())); }
    if let Some(v) = review_status { param_values.push(Box::new(v.to_string())); }
    if let Some(v) = error_context { param_values.push(Box::new(v.to_string())); }
    if let Some(v) = agent_backend { param_values.push(Box::new(v.to_string())); }
    if let Some(v) = retry_count { param_values.push(Box::new(v as i64)); }
    param_values.push(Box::new(id.to_string()));

    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();
    conn.execute(&sql, params_ref.as_slice())
        .map_err(|e| format!("update swarm task: {e}"))?;

    swarm_task_by_id(conn, id)?.ok_or_else(|| format!("task {id} not found after update"))
}

pub(crate) fn swarm_list_tasks(
    conn: &Connection,
    status_filter: Option<&str>,
    limit: Option<usize>,
) -> Vec<SwarmTask> {
    let limit = limit.unwrap_or(50) as i64;
    let (sql, filter_val);
    if let Some(status) = status_filter {
        sql = "SELECT id, name, prompt, status, branch, worktree_path, pr_number, pr_url, \
               ci_status, review_status, retry_count, max_retries, error_context, agent_backend, \
               created_at, updated_at FROM swarm_tasks WHERE status = ?1 ORDER BY created_at DESC LIMIT ?2";
        filter_val = Some(status.to_string());
    } else {
        sql = "SELECT id, name, prompt, status, branch, worktree_path, pr_number, pr_url, \
               ci_status, review_status, retry_count, max_retries, error_context, agent_backend, \
               created_at, updated_at FROM swarm_tasks ORDER BY created_at DESC LIMIT ?2";
        filter_val = None;
    }

    let result = if let Some(ref fv) = filter_val {
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![fv, limit], |row| Ok(row_to_swarm_task(row)))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    } else {
        // no status filter — use a dummy param for ?1 position; actually restructure SQL
        let sql_no_filter = "SELECT id, name, prompt, status, branch, worktree_path, pr_number, pr_url, \
               ci_status, review_status, retry_count, max_retries, error_context, agent_backend, \
               created_at, updated_at FROM swarm_tasks ORDER BY created_at DESC LIMIT ?1";
        let mut stmt = match conn.prepare(sql_no_filter) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![limit], |row| Ok(row_to_swarm_task(row)))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    };
    result
}

pub(crate) fn swarm_task_by_id(conn: &Connection, id: &str) -> Result<Option<SwarmTask>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, prompt, status, branch, worktree_path, pr_number, pr_url, \
             ci_status, review_status, retry_count, max_retries, error_context, agent_backend, \
             created_at, updated_at FROM swarm_tasks WHERE id = ?1",
        )
        .map_err(|e| format!("prepare: {e}"))?;
    let task = stmt
        .query_row(params![id], |row| Ok(row_to_swarm_task(row)))
        .ok();
    Ok(task)
}

#[allow(dead_code)]
pub(crate) fn swarm_cleanup_done(conn: &Connection, older_than_days: u32) -> Result<usize, String> {
    let cutoff = Utc::now() - chrono::Duration::days(older_than_days as i64);
    let cutoff_str = cutoff.to_rfc3339();
    let count = conn
        .execute(
            "DELETE FROM swarm_tasks WHERE status IN ('done', 'failed') AND updated_at < ?1",
            params![cutoff_str],
        )
        .map_err(|e| format!("cleanup: {e}"))?;
    Ok(count)
}

fn row_to_swarm_task(row: &rusqlite::Row<'_>) -> SwarmTask {
    SwarmTask {
        id: row.get(0).unwrap_or_default(),
        name: row.get(1).unwrap_or_default(),
        prompt: row.get(2).unwrap_or_default(),
        status: SwarmStatus::from_str(&row.get::<_, String>(3).unwrap_or_default())
            .unwrap_or(SwarmStatus::Queued),
        branch: row.get(4).ok(),
        worktree_path: row.get(5).ok(),
        pr_number: row.get(6).ok(),
        pr_url: row.get(7).ok(),
        ci_status: row.get(8).ok(),
        review_status: row.get(9).ok(),
        retry_count: row.get::<_, i64>(10).unwrap_or(0) as u32,
        max_retries: row.get::<_, i64>(11).unwrap_or(3) as u32,
        error_context: row.get(12).ok(),
        agent_backend: row.get(13).ok(),
        created_at: row.get(14).unwrap_or_default(),
        updated_at: row.get(15).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Worktree helpers (Phase 2)
// ---------------------------------------------------------------------------

/// Base directory for swarm worktrees. On the droplet this is /root/aethervault-swarm/.
fn swarm_worktree_base() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join("aethervault-swarm")
}

/// Create a git worktree for an isolated coding agent.
/// Returns the absolute path to the worktree directory.
pub(crate) fn create_worktree(repo_path: &Path, branch_name: &str) -> Result<PathBuf, String> {
    let base = swarm_worktree_base();
    std::fs::create_dir_all(&base).map_err(|e| format!("create worktree base: {e}"))?;

    let wt_path = base.join(branch_name);

    // If worktree directory already exists and is valid, reuse it
    if wt_path.exists() && wt_path.join(".git").exists() {
        eprintln!("[swarm] reusing existing worktree at {}", wt_path.display());
        return Ok(wt_path);
    }

    let output = Command::new("git")
        .args(["worktree", "add", &wt_path.to_string_lossy(), "-b", branch_name])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git worktree add: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // If branch already exists, try without -b
        if stderr.contains("already exists") {
            // Clean up stale worktree entry if path doesn't exist
            if !wt_path.exists() {
                let _ = Command::new("git")
                    .args(["worktree", "prune"])
                    .current_dir(repo_path)
                    .output();
            }
            let output2 = Command::new("git")
                .args(["worktree", "add", &wt_path.to_string_lossy(), branch_name])
                .current_dir(repo_path)
                .output()
                .map_err(|e| format!("git worktree add (existing branch): {e}"))?;
            if !output2.status.success() {
                let stderr2 = String::from_utf8_lossy(&output2.stderr);
                // If worktree is already checked out, it's still usable
                if stderr2.contains("already checked out") && wt_path.exists() {
                    eprintln!("[swarm] worktree already checked out, reusing");
                    return Ok(wt_path);
                }
                return Err(format!(
                    "git worktree add failed: {}",
                    stderr2
                ));
            }
        } else {
            return Err(format!("git worktree add failed: {stderr}"));
        }
    }

    Ok(wt_path)
}

/// Remove a git worktree and optionally delete the branch if merged.
#[allow(dead_code)]
pub(crate) fn cleanup_worktree(repo_path: &Path, worktree_path: &Path) -> Result<(), String> {
    // Force-remove the worktree
    let output = Command::new("git")
        .args([
            "worktree",
            "remove",
            &worktree_path.to_string_lossy(),
            "--force",
        ])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git worktree remove: {e}"))?;

    if !output.status.success() {
        eprintln!(
            "[swarm] worktree remove warning: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Try to delete the branch (only succeeds if merged)
    if let Some(branch) = worktree_path.file_name().and_then(|f| f.to_str()) {
        let _ = Command::new("git")
            .args(["branch", "-d", branch])
            .current_dir(repo_path)
            .output();
    }

    Ok(())
}

/// Check CI and review status for all open swarm tasks using `gh` CLI.
/// Returns a summary string of updates made.
pub(crate) fn swarm_check_open_tasks(conn: &Connection) -> String {
    let tasks = swarm_list_tasks(conn, Some("pr_open"), Some(100));
    let reviewing_tasks = swarm_list_tasks(conn, Some("reviewing"), Some(100));
    let all_tasks: Vec<SwarmTask> = tasks.into_iter().chain(reviewing_tasks).collect();

    if all_tasks.is_empty() {
        return "No open PR tasks to check.".to_string();
    }

    let mut updates = Vec::new();
    for task in &all_tasks {
        let pr_num = match task.pr_number {
            Some(n) => n,
            None => continue,
        };

        // Check CI status via gh
        let ci_output = Command::new("gh")
            .args(["pr", "checks", &pr_num.to_string(), "--json", "state", "-q", ".[].state"])
            .output();

        if let Ok(output) = ci_output {
            if output.status.success() {
                let stdout_str = String::from_utf8_lossy(&output.stdout).to_string();
                let states: Vec<&str> = stdout_str
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .collect();

                let ci_status = if states.iter().all(|s| *s == "SUCCESS" || *s == "NEUTRAL" || *s == "SKIPPED") && !states.is_empty() {
                    "passing"
                } else if states.iter().any(|s| *s == "FAILURE" || *s == "ERROR") {
                    "failing"
                } else {
                    "pending"
                };

                let new_status = if ci_status == "passing" && task.status == SwarmStatus::PrOpen {
                    Some("reviewing")
                } else {
                    None
                };

                let _ = swarm_update_task(
                    conn, &task.id,
                    new_status,
                    None, None, None, None,
                    Some(ci_status),
                    None, None, None, None,
                );
                updates.push(format!("{} (PR #{}): CI {}", task.name, pr_num, ci_status));
            }
        }

        // Check review status via gh
        let review_output = Command::new("gh")
            .args(["pr", "view", &pr_num.to_string(), "--json", "reviewDecision", "-q", ".reviewDecision"])
            .output();

        if let Ok(output) = review_output {
            if output.status.success() {
                let decision = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let review_status = match decision.as_str() {
                    "APPROVED" => "approved",
                    "CHANGES_REQUESTED" => "changes_requested",
                    "REVIEW_REQUIRED" => "pending",
                    _ if decision.is_empty() => "pending",
                    _ => "pending",
                };

                let new_status = if review_status == "approved" {
                    Some("done")
                } else {
                    None
                };

                let _ = swarm_update_task(
                    conn, &task.id,
                    new_status,
                    None, None, None, None, None,
                    Some(review_status),
                    None, None, None,
                );
                if !decision.is_empty() {
                    updates.push(format!("{} (PR #{}): review {}", task.name, pr_num, review_status));
                }
            }
        }
    }

    if updates.is_empty() {
        "Checked open tasks — no status changes.".to_string()
    } else {
        format!("Swarm check results:\n{}", updates.join("\n"))
    }
}

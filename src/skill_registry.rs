use std::path::Path;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SkillRecord {
    pub(crate) name: String,
    pub(crate) trigger: Option<String>,
    pub(crate) steps: Vec<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) notes: Option<String>,
    pub(crate) success_rate: f64,
    pub(crate) times_used: u64,
    pub(crate) times_succeeded: u64,
    pub(crate) last_used: Option<String>,
    pub(crate) created_at: String,
    pub(crate) contexts: Vec<String>,
}

pub(crate) fn open_skill_db(path: &Path) -> Result<Connection, Box<dyn std::error::Error>> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS skills (
            name TEXT PRIMARY KEY,
            trigger TEXT,
            steps TEXT NOT NULL,
            tools TEXT NOT NULL,
            notes TEXT,
            success_rate REAL NOT NULL DEFAULT 0.0,
            times_used INTEGER NOT NULL DEFAULT 0,
            times_succeeded INTEGER NOT NULL DEFAULT 0,
            last_used TEXT,
            created_at TEXT NOT NULL,
            contexts TEXT NOT NULL DEFAULT '[]'
        )",
    )?;
    Ok(conn)
}

pub(crate) fn upsert_skill(
    conn: &Connection,
    skill: &SkillRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    let steps_json = serde_json::to_string(&skill.steps)?;
    let tools_json = serde_json::to_string(&skill.tools)?;
    let contexts_json = serde_json::to_string(&skill.contexts)?;
    conn.execute(
        "INSERT INTO skills (name, trigger, steps, tools, notes, success_rate, times_used, times_succeeded, last_used, created_at, contexts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(name) DO UPDATE SET
           trigger = excluded.trigger,
           steps = excluded.steps,
           tools = excluded.tools,
           notes = excluded.notes,
           contexts = excluded.contexts",
        params![
            skill.name,
            skill.trigger,
            steps_json,
            tools_json,
            skill.notes,
            skill.success_rate,
            skill.times_used as i64,
            skill.times_succeeded as i64,
            skill.last_used,
            skill.created_at,
            contexts_json,
        ],
    )?;
    Ok(())
}

pub(crate) fn search_skills(conn: &Connection, query: &str, limit: usize) -> Vec<SkillRecord> {
    let pattern = format!("%{query}%");
    let mut stmt = match conn.prepare(
        "SELECT name, trigger, steps, tools, notes, success_rate, times_used, times_succeeded, last_used, created_at, contexts
         FROM skills
         WHERE name LIKE ?1 OR trigger LIKE ?1 OR notes LIKE ?1
         ORDER BY success_rate DESC
         LIMIT ?2",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map(params![pattern, limit as i64], |row| {
        Ok(row_to_skill(row))
    }) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.filter_map(|r| r.ok()).collect()
}

pub(crate) fn record_skill_use(
    conn: &Connection,
    name: &str,
    succeeded: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = chrono::Utc::now().to_rfc3339();
    if succeeded {
        conn.execute(
            "UPDATE skills SET times_used = times_used + 1, times_succeeded = times_succeeded + 1, last_used = ?1 WHERE name = ?2",
            params![now, name],
        )?;
    } else {
        conn.execute(
            "UPDATE skills SET times_used = times_used + 1, last_used = ?1 WHERE name = ?2",
            params![now, name],
        )?;
    }
    conn.execute(
        "UPDATE skills SET success_rate = CAST(times_succeeded AS REAL) / CAST(times_used AS REAL) WHERE name = ?1 AND times_used > 0",
        params![name],
    )?;
    Ok(())
}

pub(crate) fn list_skills(conn: &Connection, limit: usize) -> Vec<SkillRecord> {
    let mut stmt = match conn.prepare(
        "SELECT name, trigger, steps, tools, notes, success_rate, times_used, times_succeeded, last_used, created_at, contexts
         FROM skills
         ORDER BY success_rate DESC, times_used DESC
         LIMIT ?1",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map(params![limit as i64], |row| Ok(row_to_skill(row))) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.filter_map(|r| r.ok()).collect()
}

fn row_to_skill(row: &rusqlite::Row<'_>) -> SkillRecord {
    let steps_json: String = row.get(2).unwrap_or_default();
    let tools_json: String = row.get(3).unwrap_or_default();
    let contexts_json: String = row.get(10).unwrap_or_default();
    SkillRecord {
        name: row.get(0).unwrap_or_default(),
        trigger: row.get(1).ok(),
        steps: serde_json::from_str(&steps_json).unwrap_or_default(),
        tools: serde_json::from_str(&tools_json).unwrap_or_default(),
        notes: row.get(4).ok(),
        success_rate: row.get(5).unwrap_or(0.0),
        times_used: row.get::<_, i64>(6).unwrap_or(0) as u64,
        times_succeeded: row.get::<_, i64>(7).unwrap_or(0) as u64,
        last_used: row.get(8).ok(),
        created_at: row.get(9).unwrap_or_default(),
        contexts: serde_json::from_str(&contexts_json).unwrap_or_default(),
    }
}

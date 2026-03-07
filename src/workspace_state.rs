use std::fs;
use std::path::{Path, PathBuf};

use aether_core::{PutOptions, Vault};
use chrono::Utc;
use walkdir::WalkDir;

use crate::executive_state::{
    ExecutiveState, executive_state_files_exist, load_executive_state,
    render_executive_state_context, state_json_memory_uri_candidates, state_json_path,
    state_markdown_path, state_markdown_source_path, state_memory_uri_candidates,
    sync_executive_state_files,
};
use crate::{load_capsule_config, open_or_create, save_config_entry};

fn read_optional_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().and_then(|text| {
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    })
}

fn daily_memory_path(workspace: &Path) -> PathBuf {
    let date = Utc::now().format("%Y-%m-%d").to_string();
    workspace.join("memory").join(format!("{date}.md"))
}

fn memory_uri(kind: &str) -> String {
    format!("aethervault://memory/{kind}.md")
}

fn memory_daily_uri(date: &str) -> String {
    format!("aethervault://memory/daily/{date}.md")
}

fn truncate_for_context(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut out = text.chars().take(max_chars).collect::<String>();
    out.push_str("\n\n[truncated]");
    out
}

fn tail_for_context(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let kept = text
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("[showing most recent excerpt]\n{kept}")
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

pub fn sync_memory_file(
    mem: &mut Vault,
    path: &Path,
    uri: String,
    title: &str,
    track: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    sync_file_with_kind(mem, path, uri, title, "text/markdown", track)
}

pub fn sync_workspace_memory(
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
    if executive_state_files_exist(workspace) {
        ids.extend(sync_executive_state_files(&mut mem, workspace)?);
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

pub fn export_capsule_memory(
    mv2: &Path,
    workspace: &Path,
    include_daily: bool,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut mem = Vault::open_read_only(mv2)?;
    let mut paths = Vec::new();
    let items = vec![
        (vec![memory_uri("soul")], workspace.join("SOUL.md")),
        (vec![memory_uri("user")], workspace.join("USER.md")),
        (vec![memory_uri("longterm")], workspace.join("MEMORY.md")),
        (
            state_memory_uri_candidates(),
            state_markdown_path(workspace),
        ),
        (
            state_json_memory_uri_candidates(),
            state_json_path(workspace),
        ),
    ];
    for (uris, path) in items {
        for uri in uris {
            if let Ok(frame) = mem.frame_by_uri(&uri) {
                if let Ok(text) = mem.frame_text_by_id(frame.id) {
                    fs::create_dir_all(workspace)?;
                    fs::write(&path, text)?;
                    paths.push(path.display().to_string());
                }
                break;
            }
        }
    }
    if include_daily {
        let daily_dir = workspace.join("memory");
        fs::create_dir_all(&daily_dir)?;
        let total = mem.frame_count() as u64;
        for frame_id in 0..total {
            let frame = match mem.frame_by_id(frame_id) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let Some(uri) = frame.uri.as_deref() else {
                continue;
            };
            if !uri.starts_with("aethervault://memory/daily/") {
                continue;
            }
            if let Some(name) = uri.rsplit('/').next() {
                let path = daily_dir.join(name);
                if let Ok(text) = mem.frame_text_by_id(frame_id) {
                    fs::write(&path, text)?;
                    paths.push(path.display().to_string());
                }
            }
        }
    }
    Ok(paths)
}

pub fn load_workspace_context(workspace: &Path) -> String {
    let mut sections = Vec::new();
    let soul = workspace.join("SOUL.md");
    let user = workspace.join("USER.md");
    if let Some(text) = read_optional_file(&soul) {
        sections.push(format!("# Soul\n{}", truncate_for_context(&text, 2_500)));
    }
    if let Some(text) = read_optional_file(&user) {
        sections.push(format!("# User\n{}", truncate_for_context(&text, 2_500)));
    }
    let state_context = render_executive_state_context(&load_executive_state(workspace));
    if !state_context.trim().is_empty() {
        sections.push(format!("# Executive State\n{state_context}"));
    } else {
        if let Some(state) = state_markdown_source_path(workspace) {
            if let Some(text) = read_optional_file(&state) {
                sections.push(format!(
                    "# Executive State\n{}",
                    truncate_for_context(&text, 2_000)
                ));
            }
        }
    }
    let daily = daily_memory_path(workspace);
    if let Some(text) = read_optional_file(&daily) {
        sections.push(format!("# Daily Log\n{}", tail_for_context(&text, 2_500)));
    }
    sections.join("\n\n")
}

pub fn bootstrap_workspace(
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
    let state_path = state_markdown_path(workspace);
    let state_json = state_json_path(workspace);
    let daily_path = daily_memory_path(workspace);

    let create_file = |path: &Path, contents: &str| -> Result<(), Box<dyn std::error::Error>> {
        if path.exists() && !force {
            return Err(format!("File already exists: {}", path.display()).into());
        }
        fs::write(path, contents)?;
        Ok(())
    };

    let soul_template = "# Executive Assistant Soul\n\n- Act as a proactive executive assistant.\n- Be concise, direct, and high‑leverage.\n- Prefer action over explanation.\n- Ask for approval before external sends unless policy allows.\n";
    let user_template = "# User Profile\n\n- Name: Sunil Rao\n- Role: Executive\n- Preferences:\n  - Daily Overview at 8:30 AM\n  - Daily Recap at 3:30 PM\n  - Weekly Overview Monday 8:15 AM\n  - Weekly Recap Friday 3:15 PM\n";
    let memory_template =
        "# Long‑term Memory\n\n- Important contacts, preferences, and policies go here.\n";
    let state_template = "# Executive State\n\n## Open Loops\n- None currently tracked.\n";
    let daily_template = "# Daily Log\n\n- Created by bootstrap.\n";

    create_file(&soul_path, soul_template)?;
    create_file(&user_path, user_template)?;
    create_file(&memory_path, memory_template)?;
    create_file(&state_path, state_template)?;
    create_file(
        &state_json,
        &serde_json::to_string_pretty(&ExecutiveState::default())?,
    )?;
    create_file(&daily_path, daily_template)?;

    let mut mem = open_or_create(mv2)?;
    let mut config = load_capsule_config(&mut mem).unwrap_or_default();
    let mut agent_cfg = config.agent.unwrap_or_default();
    agent_cfg.workspace = Some(workspace.display().to_string());
    agent_cfg.onboarding_complete = Some(false);
    if timezone.is_some() {
        agent_cfg.timezone = timezone;
    }
    config.agent = Some(agent_cfg);
    let bytes = serde_json::to_vec_pretty(&config)?;
    let _ = save_config_entry(&mut mem, "index", &bytes)?;
    Ok(())
}

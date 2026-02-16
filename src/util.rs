use std::env;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use aether_core::Vault;
use blake3::Hash;
use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};

use std::time::{SystemTime, UNIX_EPOCH};

use super::{AgentConfig, DEFAULT_WORKSPACE_DIR};

pub(crate) fn normalize_collection(name: &str) -> String {
    name.trim().trim_matches('/').to_string()
}

pub(crate) fn scope_prefix(collection: &str) -> String {
    format!("aethervault://{}/", normalize_collection(collection))
}

pub(crate) fn uri_for_path(collection: &str, relative: &Path) -> String {
    let rel = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    format!("aethervault://{}/{rel}", normalize_collection(collection))
}

pub(crate) fn infer_title(path: &Path, bytes: &[u8]) -> String {
    let fallback = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("untitled")
        .to_string();

    let Ok(text) = std::str::from_utf8(bytes) else {
        return fallback;
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return title.to_string();
            }
        }
    }

    fallback
}

pub(crate) fn blake3_hash(bytes: &[u8]) -> Hash {
    blake3::hash(bytes)
}

pub(crate) fn open_or_create(mv2: &Path) -> aether_core::Result<Vault> {
    if mv2.exists() {
        Vault::open(mv2)
    } else {
        Vault::create(mv2)
    }
}

pub(crate) fn is_extension_allowed(path: &Path, exts: &[String]) -> bool {
    let ext = path.extension().and_then(OsStr::to_str).unwrap_or("");
    if exts.is_empty() {
        return ext.eq_ignore_ascii_case("md");
    }
    exts.iter().any(|allowed| ext.eq_ignore_ascii_case(allowed))
}

#[derive(Default, Debug)]
pub(crate) struct ParsedMarkup {
    pub(crate) collection: Option<String>,
    pub(crate) asof_ts: Option<i64>,
    pub(crate) before_ts: Option<i64>,
    pub(crate) after_ts: Option<i64>,
}

pub(crate) fn parse_date_to_ts(value: &str) -> Option<i64> {
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M") {
        return Some(Utc.from_utc_datetime(&dt).timestamp());
    }
    if let Ok(d) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let dt = d.and_hms_opt(0, 0, 0)?;
        return Some(Utc.from_utc_datetime(&dt).timestamp());
    }
    None
}

pub(crate) fn parse_query_markup(raw: &str) -> (String, ParsedMarkup) {
    let mut parsed = ParsedMarkup::default();
    let mut kept = Vec::new();

    for token in raw.split_whitespace() {
        let Some((key, value)) = token.split_once(':') else {
            kept.push(token);
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "in" | "collection" => {
                if !value.trim().is_empty() {
                    parsed.collection = Some(value.trim().to_string());
                }
            }
            "asof" => {
                parsed.asof_ts = parse_date_to_ts(value);
            }
            "before" => {
                parsed.before_ts = parse_date_to_ts(value);
            }
            "after" => {
                parsed.after_ts = parse_date_to_ts(value);
            }
            _ => kept.push(token),
        }
    }

    let cleaned = kept.join(" ").trim().to_string();
    (cleaned, parsed)
}

pub(crate) fn is_stopword(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "but"
            | "by"
            | "for"
            | "from"
            | "has"
            | "have"
            | "if"
            | "in"
            | "into"
            | "is"
            | "it"
            | "its"
            | "of"
            | "on"
            | "or"
            | "that"
            | "the"
            | "their"
            | "then"
            | "there"
            | "these"
            | "they"
            | "this"
            | "to"
            | "was"
            | "were"
            | "with"
            | "you"
            | "your"
    )
}

pub(crate) fn tokenize(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

pub(crate) fn dedup_keep_order(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for v in values {
        if seen.insert(v.clone()) {
            out.push(v);
        }
    }
    out
}

pub(crate) fn build_expansions(base: &str, max: usize) -> Vec<String> {
    let tokens = tokenize(base);
    if tokens.len() <= 1 || max == 0 {
        return vec![base.to_string()];
    }

    let mut expansions = vec![base.trim().to_string()];

    let reduced_tokens: Vec<String> = tokens.iter().filter(|t| !is_stopword(t)).cloned().collect();
    let reduced = reduced_tokens.join(" ");
    if !reduced.is_empty() && reduced != base {
        expansions.push(reduced);
    }

    if !base.trim().starts_with('"') && !base.trim().ends_with('"') {
        expansions.push(format!("\"{}\"", base.trim()));
    }

    let expansions = dedup_keep_order(expansions);
    expansions.into_iter().take(max.max(1)).collect()
}

pub(crate) fn env_required(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value = env::var(name).unwrap_or_default();
    if value.trim().is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("Missing {name}")).into());
    }
    Ok(value)
}

pub(crate) fn env_optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

pub(crate) fn env_u64(name: &str, default: u64) -> Result<u64, Box<dyn std::error::Error>> {
    match env_optional(name) {
        Some(value) => Ok(value
            .parse::<u64>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid {name}")))?),
        None => Ok(default),
    }
}

pub(crate) fn env_usize(name: &str, default: usize) -> Result<usize, Box<dyn std::error::Error>> {
    match env_optional(name) {
        Some(value) => Ok(value
            .parse::<usize>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid {name}")))?),
        None => Ok(default),
    }
}

pub(crate) fn env_f64(name: &str, default: f64) -> Result<f64, Box<dyn std::error::Error>> {
    match env_optional(name) {
        Some(value) => Ok(value
            .parse::<f64>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid {name}")))?),
        None => Ok(default),
    }
}

pub(crate) fn env_bool(name: &str, default: bool) -> bool {
    match env_optional(name) {
        Some(value) => {
            let v = value.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "y" | "on")
        }
        None => default,
    }
}

pub(crate) fn jitter_ratio() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1000) as f64 / 1000.0
}

pub(crate) fn parse_retry_after(resp: &ureq::Response) -> Option<f64> {
    resp.header("retry-after")
        .and_then(|v| v.trim().parse::<f64>().ok())
}

pub(crate) fn command_wrapper() -> Option<Vec<String>> {
    env_optional("AETHERVAULT_COMMAND_WRAPPER").map(|raw| {
        raw.split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    })
}

pub(crate) fn build_external_command(program: &str, args: &[String]) -> ProcessCommand {
    let mut cmd = if let Some(wrapper) = command_wrapper() {
        let mut c = ProcessCommand::new(&wrapper[0]);
        c.args(&wrapper[1..]).arg(program).args(args);
        c
    } else {
        let mut c = ProcessCommand::new(program);
        c.args(args);
        c
    };

    // Process group isolation: the child becomes its own process group leader
    // so we can kill the entire tree without affecting the parent.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd
}

/// Kill a child process and its entire process group.
/// On Unix, sends SIGTERM first for graceful shutdown, then SIGKILL after 2 seconds.
#[cfg(unix)]
pub(crate) fn kill_process_tree(child: &mut std::process::Child) {
    let pid = child.id() as i32;
    // SIGTERM the group first (graceful)
    unsafe { libc::kill(-pid, libc::SIGTERM); }
    // Give 2 seconds for graceful shutdown
    std::thread::sleep(std::time::Duration::from_secs(2));
    // SIGKILL if still running
    match child.try_wait() {
        Ok(Some(_)) => {}
        _ => { unsafe { libc::killpg(pid, libc::SIGKILL); } }
    }
    let _ = child.wait();
}

#[cfg(not(unix))]
pub(crate) fn kill_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Build a descriptive exit code value for subprocess results.
/// On Unix, reports the signal name when a process is killed by a signal.
pub(crate) fn subprocess_exit_info(status: &std::process::ExitStatus) -> serde_json::Value {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(code) = status.code() {
            serde_json::json!(code)
        } else if let Some(sig) = status.signal() {
            serde_json::json!(format!("signal {sig}"))
        } else {
            serde_json::json!("unknown")
        }
    }
    #[cfg(not(unix))]
    {
        serde_json::json!(status.code())
    }
}

/// Build primary output text for subprocess results, surfacing stderr when relevant.
pub(crate) fn subprocess_output_text(stdout: &str, stderr: &str, is_error: bool) -> String {
    if is_error {
        // On failure, combine stdout and stderr so the LLM sees the full picture
        let mut out = String::new();
        if !stdout.is_empty() {
            out.push_str(stdout);
        }
        if !stderr.is_empty() {
            if !out.is_empty() {
                out.push_str("\n--- stderr ---\n");
            }
            out.push_str(stderr);
        }
        if out.is_empty() {
            "Command failed with no output.".to_string()
        } else {
            out
        }
    } else if stdout.is_empty() && !stderr.is_empty() {
        // Some tools write informational output to stderr even on success
        stderr.to_string()
    } else if stdout.is_empty() {
        "Command executed.".to_string()
    } else {
        stdout.to_string()
    }
}

pub(crate) fn resolve_workspace(cli: Option<PathBuf>, agent_cfg: &AgentConfig) -> Option<PathBuf> {
    if let Some(path) = cli {
        return Some(path);
    }
    if let Some(value) = env_optional("AETHERVAULT_WORKSPACE") {
        if !value.trim().is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    if let Some(value) = &agent_cfg.workspace {
        if !value.trim().is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    Some(PathBuf::from(DEFAULT_WORKSPACE_DIR))
}


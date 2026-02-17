use std::collections::HashMap;
use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use aether_core::{PutOptions, Vault};

use std::collections::HashSet;
use std::thread;
use std::time::Instant;

use std::path::Path;

use super::{
    build_external_command, config_file_path, dedup_keep_order, load_file_config,
    save_file_config, CapsuleConfig, CommandSpec, ConfigEntry, ExpansionHookInput,
    ExpansionHookOutput, FileConfig, HookSpec, RerankHookInput, RerankHookOutput,
};

const NO_DEADLINE_TIMEOUT_MS: u64 = u64::MAX;

pub(crate) fn config_key_to_uri(key: &str) -> String {
    let mut key = key.trim().to_string();
    if key.is_empty() {
        key = "index".to_string();
    }
    if !key.ends_with(".json") {
        key.push_str(".json");
    }
    format!("aethervault://config/{key}")
}

pub(crate) fn config_uri_to_key(uri: &str) -> Option<String> {
    let prefix = "aethervault://config/";
    if !uri.starts_with(prefix) {
        return None;
    }
    let mut key = uri.trim_start_matches(prefix).to_string();
    if key.ends_with(".json") {
        key.truncate(key.len().saturating_sub(5));
    }
    if key.is_empty() { None } else { Some(key) }
}

pub(crate) fn load_config_entry(mem: &mut Vault, key: &str) -> Option<Vec<u8>> {
    let uri = config_key_to_uri(key);
    let frame = mem.frame_by_uri(&uri).ok()?;
    mem.frame_canonical_payload(frame.id).ok()
}

pub(crate) fn load_capsule_config(mem: &mut Vault) -> Option<CapsuleConfig> {
    let bytes = load_config_entry(mem, "index")?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn save_config_entry(
    mem: &mut Vault,
    key: &str,
    bytes: &[u8],
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut options = PutOptions::default();
    options.uri = Some(config_key_to_uri(key));
    options.title = Some(format!("config:{key}"));
    options.kind = Some("application/json".to_string());
    options.track = Some("aethervault.config".to_string());
    options.search_text = Some(format!("config {key}"));
    options.auto_tag = false;
    options.extract_dates = false;
    options.extract_triplets = false;
    options.instant_index = true;
    let id = mem.put_bytes_with_options(bytes, options)?;
    mem.commit()?;
    Ok(id)
}

/// Load CapsuleConfig from flat file (config.json in workspace).
/// Maps FileConfig fields into CapsuleConfig structure.
pub(crate) fn load_config_from_file(workspace: &Path) -> CapsuleConfig {
    let path = config_file_path(workspace);
    let fc = load_file_config(&path);
    file_config_to_capsule_config(&fc)
}

/// Save a config key/value to the flat file (config.json in workspace).
/// Loads existing FileConfig, merges the key, and writes back atomically.
pub(crate) fn save_config_to_file(
    workspace: &Path,
    key: &str,
    value: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = config_file_path(workspace);
    let mut fc = load_file_config(&path);
    match key {
        "index" => {
            // The "index" key contains a full CapsuleConfig JSON.
            // Extract the agent field and merge it into FileConfig.
            if let Ok(cc) = serde_json::from_value::<CapsuleConfig>(value) {
                if let Some(agent) = cc.agent {
                    fc.agent = agent;
                }
            }
        }
        "approvals" => {
            if let Ok(approvals) = serde_json::from_value(value) {
                fc.approvals = approvals;
            }
        }
        "triggers" => {
            if let Ok(triggers) = serde_json::from_value(value) {
                fc.triggers = triggers;
            }
        }
        "oauth.google" => {
            fc.oauth_google = Some(value);
        }
        "oauth.microsoft" => {
            fc.oauth_microsoft = Some(value);
        }
        _ => {
            // For arbitrary keys, store in agent config or log a warning.
            eprintln!("[config] unknown config key '{key}', storing in index");
        }
    }
    save_file_config(&path, &fc)?;
    Ok(())
}

/// Convert a FileConfig into a CapsuleConfig for code that expects the old type.
fn file_config_to_capsule_config(fc: &FileConfig) -> CapsuleConfig {
    CapsuleConfig {
        context: None,
        collections: HashMap::new(),
        hooks: fc.hooks.clone(),
        agent: Some(fc.agent.clone()),
        extra: HashMap::new(),
    }
}

pub(crate) fn list_config_entries(mem: &mut Vault) -> Vec<ConfigEntry> {
    use aether_core::types::SearchRequest;

    let mut seen = HashSet::new();
    let mut entries = Vec::new();

    // Use scoped search instead of O(n) linear scan over all frames.
    let request = SearchRequest {
        query: "track:aethervault.config".to_string(),
        top_k: 200,
        snippet_chars: 0,
        uri: None,
        scope: Some("aethervault://config/".to_string()),
        cursor: None,
        temporal: None,
        as_of_frame: None,
        as_of_ts: None,
        no_sketch: true,
    };

    if let Ok(response) = mem.search(request) {
        // Hits are scored by relevance; we need latest-first dedup by key.
        // Collect all hits with their frame metadata, sort by frame_id desc (latest first).
        let mut hits: Vec<_> = response.hits.iter().filter_map(|hit| {
            let frame = mem.frame_by_id(hit.frame_id).ok()?;
            let uri = frame.uri.as_deref()?;
            let key = config_uri_to_key(uri)?;
            Some((key, frame.id, frame.timestamp))
        }).collect();
        hits.sort_by(|a, b| b.1.cmp(&a.1)); // latest frame_id first

        for (key, frame_id, timestamp) in hits {
            if seen.insert(key.clone()) {
                entries.push(ConfigEntry { key, frame_id, timestamp });
            }
        }
    }

    entries
}

pub(crate) fn command_spec_to_vec(spec: &CommandSpec) -> Vec<String> {
    match spec {
        CommandSpec::Array(items) => items.clone(),
        CommandSpec::String(cmd) => {
            if cfg!(windows) {
                vec!["cmd".to_string(), "/C".to_string(), cmd.clone()]
            } else {
                vec!["sh".to_string(), "-c".to_string(), cmd.clone()]
            }
        }
    }
}

pub(crate) fn run_hook_command(
    command: &[String],
    input: &serde_json::Value,
    _timeout_ms: u64, // Unused — no hard timeouts. Zombie detection handles stuck processes.
    kind: &str,
) -> Result<String, String> {
    if command.is_empty() {
        return Err("hook command is empty".into());
    }
    let mut cmd = build_external_command(&command[0], &command[1..]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("KAIROS_HOOK", kind);

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;

    // Write stdin with broken-pipe resilience: if the child dies before reading,
    // capture the error but still collect stdout/stderr for diagnostics.
    let stdin_err = if let Some(mut stdin) = child.stdin.take() {
        let payload = serde_json::to_vec(input).map_err(|e| format!("encode input: {e}"))?;
        match stdin.write_all(&payload).and_then(|_| stdin.flush()) {
            Ok(()) => None,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                eprintln!("[hook:{kind}] broken pipe writing stdin, collecting output...");
                Some(e)
            }
            Err(e) => return Err(format!("write stdin: {e}")),
        }
    } else {
        None
    };

    // No wall-clock timeout — hooks (especially Codex) can run for hours.
    // Instead, detect zombie/stuck processes by checking /proc/<pid>/stat
    // for zombie state (Z). Only kill if the process is truly dead.
    let start = Instant::now();
    let pid = child.id();
    let mut last_log = Instant::now();
    let zombie_check_interval = Duration::from_secs(30);

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if last_log.elapsed() >= zombie_check_interval {
                    let elapsed_secs = start.elapsed().as_secs();
                    // Check for zombie state on Linux via /proc
                    let is_zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                        .map(|s| {
                            // /proc/<pid>/stat format: pid (comm) state ...
                            // state is the character after the last ')'
                            s.rfind(')')
                                .and_then(|i| s.get(i + 1..))
                                .map(|rest| rest.trim_start().starts_with('Z'))
                                .unwrap_or(false)
                        })
                        .unwrap_or(false); // non-Linux or /proc unavailable = not zombie

                    if is_zombie {
                        eprintln!("[hook:{kind}] pid={pid} is ZOMBIE after {elapsed_secs}s, killing process tree");
                        crate::kill_process_tree(&mut child);
                        return Err(format!("hook '{kind}' pid={pid} became zombie after {elapsed_secs}s"));
                    }

                    // Log at escalating intervals: every 30s for first 5m, then every 5m
                    let log_interval = if elapsed_secs < 300 { 30 } else { 300 };
                    if last_log.elapsed() >= Duration::from_secs(log_interval) {
                        let h = elapsed_secs / 3600;
                        let m = (elapsed_secs % 3600) / 60;
                        let s = elapsed_secs % 60;
                        if h > 0 {
                            eprintln!("[hook:{kind}] pid={pid} running {h}h {m}m {s}s");
                        } else {
                            eprintln!("[hook:{kind}] pid={pid} running {m}m {s}s");
                        }
                        last_log = Instant::now();
                    }
                }
                thread::sleep(Duration::from_millis(200));
            }
            Err(e) => {
                crate::kill_process_tree(&mut child);
                return Err(format!("hook wait failed: {e}"));
            }
        }
    }

    // Collect output — recover valid JSON from non-zero exit codes.
    // codex-hook.sh emits valid JSON even when killed by signal (exit > 128).
    let output = child
        .wait_with_output()
        .map_err(|e| format!("hook output failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // If stdout has valid JSON, use it despite non-zero exit
        if !stdout.is_empty() {
            if serde_json::from_str::<serde_json::Value>(&stdout).is_ok() {
                eprintln!("[hook:{kind}] non-zero exit but stdout has valid JSON, using it");
                return Ok(stdout);
            }
        }
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        let mut msg = format!("hook exited {code}");
        if !stderr.is_empty() {
            msg.push_str(&format!(": {}", &stderr[..stderr.len().min(500)]));
        }
        if let Some(ref e) = stdin_err {
            msg.push_str(&format!(" (broken pipe: {e})"));
        }
        return Err(msg);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err(format!(
            "hook returned empty output{}",
            stdin_err
                .map(|e| format!(" (broken pipe: {e})"))
                .unwrap_or_default()
        ));
    }
    Ok(stdout)
}

pub(crate) fn resolve_hook_spec(
    cli_command: Option<String>,
    cli_timeout_ms: u64,
    config_spec: Option<HookSpec>,
    force_full_text: Option<bool>,
) -> Option<HookSpec> {
    if let Some(cmd) = cli_command {
        return Some(HookSpec {
            command: CommandSpec::String(cmd),
            timeout_ms: Some(cli_timeout_ms),
            full_text: force_full_text,
        });
    }
    config_spec.map(|mut spec| {
        if spec.timeout_ms.is_none() {
            spec.timeout_ms = Some(cli_timeout_ms);
        }
        if force_full_text.is_some() {
            spec.full_text = force_full_text;
        }
        spec
    })
}

pub(crate) fn run_expansion_hook(
    hook: &HookSpec,
    input: &ExpansionHookInput,
) -> Result<ExpansionHookOutput, String> {
    let cmd = command_spec_to_vec(&hook.command);
    let timeout = hook.timeout_ms.unwrap_or(NO_DEADLINE_TIMEOUT_MS);
    let value = serde_json::to_value(input).map_err(|e| format!("hook input: {e}"))?;
    let raw = run_hook_command(&cmd, &value, timeout, "expansion")?;
    let mut output: ExpansionHookOutput =
        serde_json::from_str(&raw).map_err(|e| format!("hook output: {e}"))?;
    output.lex = dedup_keep_order(output.lex);
    output.vec = dedup_keep_order(output.vec);
    Ok(output)
}

pub(crate) fn run_rerank_hook(hook: &HookSpec, input: &RerankHookInput) -> Result<RerankHookOutput, String> {
    let cmd = command_spec_to_vec(&hook.command);
    let timeout = hook.timeout_ms.unwrap_or(NO_DEADLINE_TIMEOUT_MS);
    let value = serde_json::to_value(input).map_err(|e| format!("hook input: {e}"))?;
    let raw = run_hook_command(&cmd, &value, timeout, "rerank")?;
    let mut output: RerankHookOutput =
        serde_json::from_str(&raw).map_err(|e| format!("hook output: {e}"))?;
    for item in output.items.drain(..) {
        output.scores.insert(item.key.clone(), item.score);
        if let Some(snippet) = item.snippet {
            output.snippets.insert(item.key, snippet);
        }
    }
    Ok(output)
}

pub(crate) fn checksum_hex(checksum: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in checksum {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_key_to_uri_basic() {
        assert_eq!(config_key_to_uri("index"), "aethervault://config/index.json");
    }

    #[test]
    fn config_key_to_uri_already_has_extension() {
        assert_eq!(
            config_key_to_uri("my_config.json"),
            "aethervault://config/my_config.json"
        );
    }

    #[test]
    fn config_key_to_uri_empty_defaults_to_index() {
        assert_eq!(config_key_to_uri(""), "aethervault://config/index.json");
    }

    #[test]
    fn config_key_to_uri_with_dots() {
        assert_eq!(
            config_key_to_uri("oauth.google"),
            "aethervault://config/oauth.google.json"
        );
    }

    #[test]
    fn config_uri_to_key_basic() {
        assert_eq!(
            config_uri_to_key("aethervault://config/index.json"),
            Some("index".to_string())
        );
    }

    #[test]
    fn config_uri_to_key_no_extension() {
        assert_eq!(
            config_uri_to_key("aethervault://config/mykey"),
            Some("mykey".to_string())
        );
    }

    #[test]
    fn config_uri_to_key_wrong_prefix() {
        assert_eq!(config_uri_to_key("other://config/index.json"), None);
    }

    #[test]
    fn config_uri_to_key_empty_key() {
        // "aethervault://config/.json" -> key after stripping is empty
        assert_eq!(config_uri_to_key("aethervault://config/.json"), None);
    }

    #[test]
    fn config_key_roundtrip() {
        let key = "oauth.google";
        let uri = config_key_to_uri(key);
        let recovered = config_uri_to_key(&uri);
        assert_eq!(recovered, Some(key.to_string()));
    }

    #[test]
    fn checksum_hex_all_zeros() {
        let checksum = [0u8; 32];
        let hex = checksum_hex(&checksum);
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c == '0'));
    }

    #[test]
    fn checksum_hex_known_value() {
        let mut checksum = [0u8; 32];
        checksum[0] = 0xff;
        checksum[31] = 0xab;
        let hex = checksum_hex(&checksum);
        assert!(hex.starts_with("ff"));
        assert!(hex.ends_with("ab"));
        assert_eq!(hex.len(), 64);
    }
}

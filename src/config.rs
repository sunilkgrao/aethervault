use std::collections::HashMap;
use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use aether_core::{PutOptions, Vault};

use std::collections::HashSet;
use std::thread;
use std::time::Instant;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    let total = mem.frame_count() as i64;
    for idx in (0..total).rev() {
        let frame_id = idx as u64;
        let frame = match mem.frame_by_id(frame_id) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let uri = match frame.uri.as_deref() {
            Some(u) => u,
            None => continue,
        };
        let key = match config_uri_to_key(uri) {
            Some(k) => k,
            None => continue,
        };
        if seen.insert(key.clone()) {
            entries.push(ConfigEntry {
                key,
                frame_id: frame.id,
                timestamp: frame.timestamp,
            });
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
    timeout_ms: u64,
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
    if let Some(mut stdin) = child.stdin.take() {
        let payload = serde_json::to_vec(input).map_err(|e| format!("encode input: {e}"))?;
        stdin
            .write_all(&payload)
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("write stdin: {e}"))?;
    }

    let timeout = Duration::from_millis(timeout_ms.max(1));
    let timeout_ms = timeout.as_millis() as u64;
    let cancel_token = Arc::new(AtomicBool::new(false));
    let pid = child.id().to_string();
    let mut last_update = Instant::now();
    loop {
        if cancel_token.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("hook '{kind}' canceled after {timeout_ms}ms"));
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if last_update.elapsed() >= Duration::from_secs(5) {
                    eprintln!(
                        "[hook:{kind}] pid={pid} still running (no deadline, configured timeout {timeout_ms}ms)"
                    );
                    last_update = Instant::now();
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(format!("hook wait failed: {e}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("hook output failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            return Err("hook exited with error".into());
        }
        return Err(format!("hook error: {stderr}"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err("hook returned empty output".into());
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

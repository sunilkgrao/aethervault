#[allow(unused_imports)]
use std::collections::HashMap;
use std::io::Read;
use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use aether_core::{PutOptions, Vault};

use std::collections::HashSet;
use std::thread;
use std::time::Instant;

use super::{
    build_external_command, dedup_keep_order, CapsuleConfig, CommandSpec, ConfigEntry,
    ExpansionHookInput, ExpansionHookOutput, HookSpec, RerankHookInput, RerankHookOutput,
};

const NO_DEADLINE_TIMEOUT_MS: u64 = u64::MAX;
const HOOK_STREAM_CAP_BYTES: usize = 64 * 1024;
const HOOK_STREAM_READ_SLEEP_MS: u64 = 10;

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

    let effective_timeout_ms = timeout_ms.max(1);
    let timeout = if timeout_ms == NO_DEADLINE_TIMEOUT_MS {
        None
    } else {
        Some(Duration::from_millis(effective_timeout_ms))
    };
    let start = Instant::now();
    let mut stdout_handle = child.stdout.take().map(|stdout| {
        thread::spawn(move || {
            let mut captured: Vec<u8> = Vec::new();
            let mut truncated = false;
            let mut buffer = [0_u8; 4096];
            let mut reader = stdout;
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let remaining = HOOK_STREAM_CAP_BYTES.saturating_sub(captured.len());
                        if remaining > 0 {
                            let take = remaining.min(n);
                            captured.extend_from_slice(&buffer[..take]);
                            if n > take {
                                truncated = true;
                            }
                        } else {
                            truncated = true;
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            (captured, truncated)
        })
    });
    let mut stderr_handle = child.stderr.take().map(|stderr| {
        thread::spawn(move || {
            let mut captured: Vec<u8> = Vec::new();
            let mut truncated = false;
            let mut buffer = [0_u8; 4096];
            let mut reader = stderr;
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let remaining = HOOK_STREAM_CAP_BYTES.saturating_sub(captured.len());
                        if remaining > 0 {
                            let take = remaining.min(n);
                            captured.extend_from_slice(&buffer[..take]);
                            if n > take {
                                truncated = true;
                            }
                        } else {
                            truncated = true;
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            (captured, truncated)
        })
    });

    let mut timed_out = false;
    let status = loop {
        if let Some(timeout) = timeout {
            if start.elapsed() >= timeout {
                timed_out = true;
                let _ = child.kill();
                break child.wait().map_err(|e| format!("hook wait failed: {e}"));
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                thread::sleep(Duration::from_millis(HOOK_STREAM_READ_SLEEP_MS));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("hook wait failed: {e}"));
            }
        }
    };

    let collect =
        |handle: &mut Option<thread::JoinHandle<(Vec<u8>, bool)>>| -> (Vec<u8>, bool) {
            handle
                .take()
                .and_then(|join| join.join().ok())
                .unwrap_or_else(|| (Vec::new(), false))
        };
    let (stdout, stdout_truncated) = collect(&mut stdout_handle);
    let (stderr, stderr_truncated) = collect(&mut stderr_handle);

    if timed_out {
        return Err(format!("hook '{kind}' timed out after {effective_timeout_ms}ms"));
    }

    let status = status?;
    if !status.success() {
        let mut stderr = String::from_utf8_lossy(&stderr).trim().to_string();
        if stderr.is_empty() {
            if stderr_truncated {
                return Err("hook error: stderr output exceeded capture limit".into());
            }
            return Err("hook exited with error".into());
        }
        if stderr_truncated {
            stderr.push_str(" (stderr output truncated)");
        }
        return Err(format!("hook error: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&stdout).trim().to_string();
    if stdout.is_empty() {
        if stdout_truncated {
            return Err("hook output exceeded capture limit".into());
        }
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

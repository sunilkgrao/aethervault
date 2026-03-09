use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use walkdir::WalkDir;

use crate::policy::{allowed_fs_roots, resolve_fs_path};
use crate::{
    ToolBrowserRequestArgs, ToolExecArgs, ToolExecution, ToolFsListArgs, ToolFsReadArgs,
    ToolFsWriteArgs, ToolHttpRequestArgs, ToolIMessageSendArgs, ToolNotifyArgs, ToolScaleArgs,
    ToolSignalSendArgs, build_external_command, env_optional, env_optional_alias,
};

pub(crate) fn handle_exec(args: serde_json::Value) -> Result<ToolExecution, String> {
    let parsed: ToolExecArgs = serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let timeout_ms = parsed.timeout_ms.unwrap_or(60_000).max(1);
    let command = if cfg!(windows) {
        vec!["cmd".to_string(), "/C".to_string(), parsed.command]
    } else {
        vec!["sh".to_string(), "-c".to_string(), parsed.command]
    };
    let mut cmd = build_external_command(&command[0], &command[1..]);
    if let Some(cwd) = parsed.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("exec spawn: {e}"))?;
    let timeout = Duration::from_millis(timeout_ms);
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return Err(format!("exec timed out after {timeout_ms}ms"));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(format!("exec wait failed: {err}")),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("exec output: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(ToolExecution {
        output: if output.status.success() {
            "Command executed.".to_string()
        } else {
            "Command failed.".to_string()
        },
        details: serde_json::json!({
            "status": output.status.code(),
            "stdout": stdout,
            "stderr": stderr
        }),
        is_error: !output.status.success(),
    })
}

pub(crate) fn handle_notify(args: serde_json::Value) -> Result<ToolExecution, String> {
    let parsed: ToolNotifyArgs = serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let channel = parsed
        .channel
        .unwrap_or_else(|| "slack".to_string())
        .to_ascii_lowercase();
    let webhook = parsed.webhook.or_else(|| match channel.as_str() {
        "discord" => env_optional("DISCORD_WEBHOOK_URL"),
        "teams" => env_optional("TEAMS_WEBHOOK_URL"),
        _ => env_optional("SLACK_WEBHOOK_URL"),
    });
    let Some(webhook) = webhook else {
        return Err("notify requires webhook url".into());
    };
    let payload = match channel.as_str() {
        "discord" => serde_json::json!({ "content": parsed.text }),
        "teams" => serde_json::json!({ "text": parsed.text }),
        _ => serde_json::json!({ "text": parsed.text }),
    };
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .timeout_write(Duration::from_secs(10))
        .build();
    agent
        .post(&webhook)
        .set("content-type", "application/json")
        .send_json(payload)
        .map_err(|e| format!("notify error: {e}"))?;
    Ok(ToolExecution {
        output: "Notification sent.".to_string(),
        details: serde_json::json!({ "channel": channel }),
        is_error: false,
    })
}

pub(crate) fn handle_signal_send(args: serde_json::Value) -> Result<ToolExecution, String> {
    let parsed: ToolSignalSendArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let sender = parsed.sender.or_else(|| env_optional("SIGNAL_SENDER"));
    let Some(sender) = sender else {
        return Err("signal_send requires sender".into());
    };
    let mut cmd = build_external_command("signal-cli", &[]);
    cmd.arg("-u")
        .arg(sender)
        .arg("send")
        .arg("-m")
        .arg(parsed.text)
        .arg(parsed.to);
    let output = cmd.output().map_err(|e| format!("signal-cli: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("signal-cli error: {stderr}"));
    }
    Ok(ToolExecution {
        output: "Signal message sent.".to_string(),
        details: serde_json::json!({ "status": "sent" }),
        is_error: false,
    })
}

pub(crate) fn handle_imessage_send(args: serde_json::Value) -> Result<ToolExecution, String> {
    let parsed: ToolIMessageSendArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    if !cfg!(target_os = "macos") {
        return Err("imessage_send requires macOS".into());
    }
    let script = format!(
        "tell application \"Messages\" to send \"{}\" to buddy \"{}\"",
        parsed.text.replace('"', "\\\""),
        parsed.to.replace('"', "\\\"")
    );
    let mut cmd = build_external_command("osascript", &[]);
    cmd.arg("-e").arg(script);
    let output = cmd.output().map_err(|e| format!("osascript: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("osascript error: {stderr}"));
    }
    Ok(ToolExecution {
        output: "iMessage sent.".to_string(),
        details: serde_json::json!({ "status": "sent" }),
        is_error: false,
    })
}

pub(crate) fn handle_http_request(args: serde_json::Value) -> Result<ToolExecution, String> {
    let parsed: ToolHttpRequestArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let method = parsed
        .method
        .unwrap_or_else(|| "GET".to_string())
        .to_ascii_uppercase();
    let timeout = parsed.timeout_ms.unwrap_or(20_000);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(timeout))
        .timeout_write(Duration::from_millis(timeout))
        .timeout_read(Duration::from_millis(timeout))
        .build();
    let mut req = match method.as_str() {
        "GET" => agent.get(&parsed.url),
        "POST" => agent.post(&parsed.url),
        "PUT" => agent.put(&parsed.url),
        "PATCH" => agent.patch(&parsed.url),
        "DELETE" => agent.delete(&parsed.url),
        _ => return Err(format!("unsupported method: {method}")),
    };
    if let Some(headers) = parsed.headers {
        for (k, v) in headers {
            req = req.set(&k, &v);
        }
    }
    let resp = if let Some(body) = parsed.body {
        if parsed.json.unwrap_or(false) {
            req.set("content-type", "application/json")
                .send_string(&body)
        } else {
            req.send_string(&body)
        }
    } else {
        req.call()
    };
    let (status, text) = match resp {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.into_string().unwrap_or_default();
            (status, text)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            (code, text)
        }
        Err(err) => return Err(format!("http_request failed: {err}")),
    };
    let truncated = if text.len() > 20_000 {
        format!("{}...[truncated]", &text[..20_000])
    } else {
        text
    };
    Ok(ToolExecution {
        output: format!("http_request {method} {} -> {status}", parsed.url),
        details: serde_json::json!({
            "status": status,
            "body": truncated
        }),
        is_error: status >= 400,
    })
}

pub(crate) fn handle_browser_request(args: serde_json::Value) -> Result<ToolExecution, String> {
    let parsed: ToolBrowserRequestArgs =
        serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let endpoint = env_optional_alias(&["OPENCLAW_BROWSER_ENDPOINT", "AETHERVAULT_BROWSER_ENDPOINT"])
        .unwrap_or_else(|| "http://127.0.0.1:4040".to_string());
    let payload = serde_json::json!({
        "action": parsed.action,
        "url": parsed.url,
        "selector": parsed.selector,
        "text": parsed.text,
        "data": parsed.data,
    });
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(30))
        .build();
    let resp = agent
        .post(&endpoint)
        .set("content-type", "application/json")
        .send_json(payload);
    match resp {
        Ok(resp) => Ok(ToolExecution {
            output: "browser_request completed.".to_string(),
            details: resp
                .into_json::<serde_json::Value>()
                .map_err(|e| e.to_string())?,
            is_error: false,
        }),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(format!("browser_request error {code}: {text}"))
        }
        Err(err) => Err(format!("browser_request failed: {err}")),
    }
}

pub(crate) fn handle_fs_list(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
) -> Result<ToolExecution, String> {
    let parsed: ToolFsListArgs = serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let roots = allowed_fs_roots(workspace_override);
    let resolved = resolve_fs_path(&parsed.path, &roots)?;
    let mut items = Vec::new();
    let max_entries = parsed.max_entries.unwrap_or(200);
    if parsed.recursive.unwrap_or(false) {
        for entry in WalkDir::new(&resolved).max_depth(6) {
            let entry = entry.map_err(|e| e.to_string())?;
            if items.len() >= max_entries {
                break;
            }
            items.push(entry.path().display().to_string());
        }
    } else if resolved.is_dir() {
        for entry in fs::read_dir(&resolved).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            items.push(entry.path().display().to_string());
            if items.len() >= max_entries {
                break;
            }
        }
    } else if resolved.exists() {
        items.push(resolved.display().to_string());
    }
    Ok(ToolExecution {
        output: format!("Listed {} entries.", items.len()),
        details: serde_json::json!({ "entries": items }),
        is_error: false,
    })
}

pub(crate) fn handle_fs_read(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
) -> Result<ToolExecution, String> {
    let parsed: ToolFsReadArgs = serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let roots = allowed_fs_roots(workspace_override);
    let resolved = resolve_fs_path(&parsed.path, &roots)?;
    let max_bytes = parsed.max_bytes.unwrap_or(200_000);
    let file = fs::File::open(&resolved).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    file.take(max_bytes as u64)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf).to_string();
    Ok(ToolExecution {
        output: format!("Read {} bytes.", buf.len()),
        details: serde_json::json!({
            "path": resolved.display().to_string(),
            "text": text
        }),
        is_error: false,
    })
}

pub(crate) fn handle_fs_write(
    args: serde_json::Value,
    workspace_override: &Option<PathBuf>,
) -> Result<ToolExecution, String> {
    let parsed: ToolFsWriteArgs = serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    let roots = allowed_fs_roots(workspace_override);
    let resolved = resolve_fs_path(&parsed.path, &roots)?;
    if parsed.append.unwrap_or(false) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved)
            .map_err(|e| e.to_string())?;
        file.write_all(parsed.text.as_bytes())
            .map_err(|e| e.to_string())?;
    } else {
        fs::write(&resolved, parsed.text.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(ToolExecution {
        output: "File written.".to_string(),
        details: serde_json::json!({ "path": resolved.display().to_string() }),
        is_error: false,
    })
}

pub(crate) fn handle_scale(args: serde_json::Value) -> Result<ToolExecution, String> {
    let parsed: ToolScaleArgs = serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
    if parsed.action != "status" {
        return Err(format!(
            "unknown scale action: {} (use status)",
            parsed.action
        ));
    }
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let (load_1m, load_5m) = load_average();
    let (mem_total_mb, mem_avail_mb) = memory_stats_mb();
    let mem_used_pct = if mem_total_mb > 0 {
        ((mem_total_mb - mem_avail_mb) as f64 / mem_total_mb as f64 * 100.0).round()
    } else {
        0.0
    };
    let (disk_total_gb, disk_used_gb, disk_used_pct) = disk_stats_gb();
    Ok(ToolExecution {
        output: format!(
            "CPU: {} cores, load {:.1}/{:.1} | RAM: {}MB/{} MB ({:.0}% used) | Disk: {:.0}G/{:.0}G ({:.0}% used)",
            cpu_count,
            load_1m,
            load_5m,
            mem_total_mb.saturating_sub(mem_avail_mb),
            mem_total_mb,
            mem_used_pct,
            disk_used_gb,
            disk_total_gb,
            disk_used_pct,
        ),
        details: serde_json::json!({
            "cpu_count": cpu_count,
            "load_1m": load_1m,
            "load_5m": load_5m,
            "mem_total_mb": mem_total_mb,
            "mem_avail_mb": mem_avail_mb,
            "mem_used_pct": mem_used_pct,
            "disk_total_gb": disk_total_gb,
            "disk_used_gb": disk_used_gb,
            "disk_used_pct": disk_used_pct,
        }),
        is_error: false,
    })
}

fn load_average() -> (f64, f64) {
    if let Ok(text) = fs::read_to_string("/proc/loadavg") {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() >= 2 {
            return (
                parts[0].parse::<f64>().unwrap_or(0.0),
                parts[1].parse::<f64>().unwrap_or(0.0),
            );
        }
    }
    if let Ok(output) = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let cleaned = text.replace(['{', '}'], "");
        let parts: Vec<&str> = cleaned.split_whitespace().collect();
        if parts.len() >= 2 {
            return (
                parts[0].parse::<f64>().unwrap_or(0.0),
                parts[1].parse::<f64>().unwrap_or(0.0),
            );
        }
    }
    (0.0, 0.0)
}

fn memory_stats_mb() -> (u64, u64) {
    if let Ok(text) = fs::read_to_string("/proc/meminfo") {
        let mut total = 0_u64;
        let mut available = 0_u64;
        for line in text.lines() {
            if line.starts_with("MemTotal:") {
                total = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
                    / 1024;
            } else if line.starts_with("MemAvailable:") {
                available = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
                    / 1024;
            }
        }
        return (total, available);
    }
    #[cfg(target_os = "macos")]
    {
        let total = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|text| text.trim().parse::<u64>().ok())
            .map(|bytes| bytes / 1024 / 1024)
            .unwrap_or(0);
        let page_size = std::process::Command::new("pagesize")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(4096);
        let available_pages = std::process::Command::new("vm_stat")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|text| {
                text.lines()
                    .filter_map(|line| line.split(':').collect::<Vec<_>>().get(1).copied())
                    .enumerate()
                    .fold(0_u64, |acc, (idx, value)| {
                        let pages = value
                            .trim()
                            .trim_end_matches('.')
                            .replace('.', "")
                            .parse::<u64>()
                            .unwrap_or(0);
                        if idx <= 3 { acc + pages } else { acc }
                    })
            })
            .unwrap_or(0);
        let available = available_pages.saturating_mul(page_size) / 1024 / 1024;
        return (total, available.min(total));
    }
    #[allow(unreachable_code)]
    (0, 0)
}

fn disk_stats_gb() -> (f64, f64, f64) {
    let output = std::process::Command::new("df")
        .args(["-k", "/"])
        .output()
        .ok();
    let Some(output) = output else {
        return (0.0, 0.0, 0.0);
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(line) = text.lines().nth(1) else {
        return (0.0, 0.0, 0.0);
    };
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 5 {
        return (0.0, 0.0, 0.0);
    }
    let total_kb = parts[1].parse::<f64>().unwrap_or(0.0);
    let used_kb = parts[2].parse::<f64>().unwrap_or(0.0);
    let pct = parts[4].trim_end_matches('%').parse::<f64>().unwrap_or(0.0);
    (total_kb / 1_048_576.0, used_kb / 1_048_576.0, pct)
}

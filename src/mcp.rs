#[allow(unused_imports)]
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::thread;
use std::time::Duration;

use aether_core::{DoctorReport, Vault};

// === Generic MCP Client Registry ===
// Manages long-lived MCP server sidecars. Each server is spawned once, handshaked,
// tools discovered via tools/list, and kept alive for the agent session. Tool calls
// are routed to the correct server via a name->server routing map.

pub(crate) struct McpServerHandle {
    name: String,
    stdin: std::process::ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
    child: std::process::Child,
    next_id: i64,
    timeout_secs: u64,
    /// Tools discovered from this server (original names)
    tools: Vec<serde_json::Value>,
}

pub(crate) struct McpRegistry {
    servers: Vec<McpServerHandle>,
    /// Maps prefixed tool name (mcp__{server}__{tool}) -> (server_index, original_tool_name)
    pub(crate) route_map: HashMap<String, (usize, String)>,
}

impl McpRegistry {
    /// Spawn and initialize all configured MCP servers.
    pub(crate) fn start(configs: &[super::McpServerConfig]) -> Result<Self, String> {
        let mut servers = Vec::new();
        let mut route_map = HashMap::new();

        for cfg in configs {
            match Self::spawn_server(cfg) {
                Ok(handle) => {
                    let server_idx = servers.len();
                    // Build route map from discovered tools
                    for tool in &handle.tools {
                        if let Some(tool_name) = tool.get("name").and_then(|v| v.as_str()) {
                            let prefixed = format!("mcp__{}__{}", cfg.name, tool_name);
                            route_map.insert(prefixed, (server_idx, tool_name.to_string()));
                        }
                    }
                    servers.push(handle);
                }
                Err(e) => {
                    eprintln!("[mcp-registry] failed to start '{}': {e}", cfg.name);
                    // Non-fatal: skip this server, continue with others
                }
            }
        }

        Ok(McpRegistry { servers, route_map })
    }

    pub(crate) fn spawn_server(cfg: &super::McpServerConfig) -> Result<McpServerHandle, String> {
        // Validate server name for safe use in tool prefixes
        if cfg.name.is_empty() {
            return Err("mcp server name cannot be empty".to_string());
        }
        if !cfg.name.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return Err(format!(
                "mcp server name '{}' must be alphanumeric or hyphenated (no underscores)",
                cfg.name
            ));
        }

        let cmd_parts = shlex::split(&cfg.command)
            .ok_or_else(|| format!("mcp '{}': malformed command", cfg.name))?;
        if cmd_parts.is_empty() {
            return Err(format!("mcp '{}': empty command", cfg.name));
        }

        let mut cmd = super::build_external_command(&cmd_parts[0], &cmd_parts[1..]);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| format!("mcp '{}' spawn: {e}", cfg.name))?;
        let stdin = child.stdin.take().ok_or_else(|| format!("mcp '{}': no stdin", cfg.name))?;
        let stdout = child.stdout.take().ok_or_else(|| format!("mcp '{}': no stdout", cfg.name))?;
        let reader = BufReader::new(stdout);
        let timeout_secs = cfg.timeout_secs.unwrap_or(30);

        // Drain stderr in background to prevent pipe buffer deadlock and capture diagnostics
        if let Some(stderr) = child.stderr.take() {
            let name = cfg.name.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().flatten() {
                    eprintln!("[mcp:{name}:stderr] {line}");
                }
            });
        }

        let mut handle = McpServerHandle {
            name: cfg.name.clone(),
            stdin,
            reader,
            child,
            next_id: 1,
            timeout_secs,
            tools: Vec::new(),
        };

        // Initialize handshake
        handle.send_msg(&serde_json::json!({
            "jsonrpc": "2.0", "id": handle.next_id, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "aethervault", "version": "0.1" }
            }
        }))?;
        handle.next_id += 1;

        let init_resp = handle.read_msg()?;
        if let Some(err) = init_resp.get("error") {
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
            let _ = handle.child.kill();
            let _ = handle.child.wait();
            return Err(format!("mcp '{}': initialize failed: {msg}", cfg.name));
        }

        // Send initialized notification
        handle.send_msg(&serde_json::json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        }))?;

        // Discover tools
        handle.send_msg(&serde_json::json!({
            "jsonrpc": "2.0", "id": handle.next_id, "method": "tools/list"
        }))?;
        handle.next_id += 1;

        let list_resp = handle.read_msg()?;
        if let Some(err) = list_resp.get("error") {
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
            eprintln!("[mcp-registry] '{}': tools/list failed: {msg}", cfg.name);
        } else if let Some(tools_arr) = list_resp.get("result")
            .and_then(|r| r.get("tools"))
            .and_then(|t| t.as_array())
        {
            handle.tools = tools_arr.clone();
            eprintln!("[mcp-registry] '{}': discovered {} tools", cfg.name, handle.tools.len());
        }

        Ok(handle)
    }

    /// Get merged tool definitions with prefixed names for the agent catalog
    pub(crate) fn tool_definitions(&self) -> Vec<serde_json::Value> {
        let mut defs = Vec::new();
        for handle in &self.servers {
            for tool in &handle.tools {
                let original_name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                let prefixed_name = format!("mcp__{}__{}", handle.name, original_name);
                let description = tool.get("description").and_then(|v| v.as_str()).unwrap_or("");
                let input_schema = tool.get("inputSchema").cloned()
                    .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));

                defs.push(serde_json::json!({
                    "name": prefixed_name,
                    "description": format!("[MCP:{}] {}", handle.name, description),
                    "inputSchema": input_schema
                }));
            }
        }
        defs
    }

    /// Call a tool on the appropriate server
    pub(crate) fn call_tool(&mut self, prefixed_name: &str, args: serde_json::Value) -> Result<super::ToolExecution, String> {
        let (server_idx, original_name) = self.route_map.get(prefixed_name)
            .ok_or_else(|| format!("mcp: unknown tool '{prefixed_name}'"))?
            .clone();
        let handle = &mut self.servers[server_idx];
        let timeout = handle.timeout_secs;

        // Send tools/call
        let call_id = handle.next_id;
        handle.send_msg(&serde_json::json!({
            "jsonrpc": "2.0", "id": call_id, "method": "tools/call",
            "params": { "name": original_name, "arguments": args }
        }))?;
        handle.next_id += 1;

        // Read response, skipping any asynchronous notifications (messages without id)
        let resp = loop {
            let msg = handle.read_msg_timeout(timeout)?;
            if msg.get("id").is_none() {
                // This is a notification (no id field) -- skip it
                let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("unknown");
                eprintln!("[mcp:{}] skipping notification: {method}", handle.name);
                continue;
            }
            // Validate response ID matches our request
            if let Some(resp_id) = msg.get("id").and_then(|v| v.as_i64()) {
                if resp_id != call_id {
                    return Err(format!(
                        "mcp '{}': response id mismatch (expected {call_id}, got {resp_id})",
                        handle.name
                    ));
                }
            }
            break msg;
        };

        // Check for JSON-RPC error
        if let Some(err) = resp.get("error") {
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
            let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            return Err(format!("mcp '{}' error {code}: {msg}", handle.name));
        }
        let result = resp.get("result").cloned()
            .ok_or_else(|| format!("mcp '{}': response missing 'result'", handle.name))?;
        // Extract all text parts from content array (MCP responses can have multiple items)
        let content_text = match result.get("content").and_then(|c| c.as_array()) {
            Some(arr) => {
                let text_parts: Vec<&str> = arr.iter()
                    .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                    .collect();
                if text_parts.is_empty() {
                    // Content exists but no text parts -- show raw JSON so agent sees something
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                } else {
                    text_parts.join("\n")
                }
            }
            None => serde_json::to_string_pretty(&result).unwrap_or_default(),
        };
        let is_error = result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);

        Ok(super::ToolExecution {
            output: content_text.to_string(),
            details: result,
            is_error,
        })
    }

    /// Shutdown all servers
    pub(crate) fn shutdown(&mut self) {
        for handle in &mut self.servers {
            // Try graceful shutdown
            let _ = handle.send_msg(&serde_json::json!({
                "jsonrpc": "2.0", "id": handle.next_id, "method": "shutdown"
            }));
            // Brief pause for graceful exit, then kill
            thread::sleep(Duration::from_millis(500));
            let _ = handle.child.kill();
            let _ = handle.child.wait();
        }
        self.servers.clear();
        self.route_map.clear();
    }
}

impl McpServerHandle {
    pub(crate) fn send_msg(&mut self, msg: &serde_json::Value) -> Result<(), String> {
        let body = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body)
            .map_err(|e| format!("mcp '{}' write: {e}", self.name))?;
        self.stdin.flush().map_err(|e| format!("mcp '{}' flush: {e}", self.name))?;
        Ok(())
    }

    pub(crate) fn read_msg(&mut self) -> Result<serde_json::Value, String> {
        self.read_msg_inner()
    }

    pub(crate) fn read_msg_timeout(&mut self, _timeout_secs: u64) -> Result<serde_json::Value, String> {
        // For long-lived sidecars, we rely on the server responding within its own timeout.
        // The thread-based timeout pattern is used for spawn-per-call (legacy excalidraw).
        // TODO: Could add thread-based timeout wrapper here for extra safety.
        self.read_msg_inner()
    }

    pub(crate) fn read_msg_inner(&mut self) -> Result<serde_json::Value, String> {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let bytes_read = self.reader.read_line(&mut line)
                .map_err(|e| format!("mcp '{}' read: {e}", self.name))?;
            if bytes_read == 0 {
                return Err(format!(
                    "mcp '{}': server closed connection (process likely crashed)",
                    self.name
                ));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if content_length.is_some() { break; }
                continue;
            }
            if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(len_str.trim().parse()
                    .map_err(|e| format!("mcp '{}' bad content-length: {e}", self.name))?);
            }
        }
        let len = content_length.ok_or_else(|| format!("mcp '{}': missing Content-Length", self.name))?;
        if len > 10 * 1024 * 1024 {
            return Err(format!("mcp '{}': response too large ({len} bytes)", self.name));
        }
        let mut body = vec![0u8; len];
        io::Read::read_exact(&mut self.reader, &mut body)
            .map_err(|e| format!("mcp '{}' read body: {e}", self.name))?;
        serde_json::from_slice(&body).map_err(|e| format!("mcp '{}' parse: {e}", self.name))
    }
}

impl Drop for McpRegistry {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn read_mcp_message(reader: &mut BufReader<impl Read>) -> io::Result<Option<serde_json::Value>> {
    let mut first_line = String::new();
    if reader.read_line(&mut first_line)? == 0 {
        return Ok(None);
    }
    if first_line.trim().is_empty() {
        return Ok(None);
    }

    if first_line
        .to_ascii_lowercase()
        .starts_with("content-length:")
    {
        let mut content_length = first_line
            .split(':')
            .nth(1)
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);

        // Read remaining headers
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line == "\r\n" || line == "\n" || line.is_empty() {
                break;
            }
            if line.to_ascii_lowercase().starts_with("content-length:") {
                content_length = line
                    .split(':')
                    .nth(1)
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(content_length);
            }
        }

        if content_length == 0 {
            return Ok(None);
        }
        let mut buffer = vec![0u8; content_length];
        reader.read_exact(&mut buffer)?;
        let value = serde_json::from_slice(&buffer).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {e}"))
        })?;
        Ok(Some(value))
    } else {
        let value = serde_json::from_str(first_line.trim()).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {e}"))
        })?;
        Ok(Some(value))
    }
}

pub(crate) fn print_doctor_report(report: &DoctorReport) {
    println!("status: {:?}", report.status);
    println!(
        "actions: executed={} skipped={}",
        report.metrics.actions_completed, report.metrics.actions_skipped
    );
    println!("duration_ms: {}", report.metrics.total_duration_ms);
    if let Some(verification) = &report.verification {
        println!("verification: {:?}", verification.overall_status);
    }
    if report.findings.is_empty() {
        println!("findings: none");
    } else {
        println!("findings:");
        for finding in &report.findings {
            println!(
                "- {:?} {:?}: {}",
                finding.severity, finding.code, finding.message
            );
        }
    }
}

pub(crate) fn write_mcp_response(writer: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    let payload = serde_json::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e}")))?;
    write!(writer, "Content-Length: {}\r\n\r\n", payload.len())?;
    writer.write_all(&payload)?;
    writer.flush()
}

pub(crate) fn run_mcp_server(mv2: PathBuf, read_only: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = BufReader::new(io::stdin());
    let mut writer = io::stdout();
    let tools = super::tool_definitions_json();
    let mut mem_read: Option<Vault> = None;
    let mut mem_write: Option<Vault> = None;

    loop {
        let Some(msg) = read_mcp_message(&mut reader)? else {
            break;
        };
        let id = msg.get("id").cloned();
        let has_id = id.as_ref().is_some_and(|v| !v.is_null());
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg
            .get("params")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let response = match method {
            "initialize" => {
                let protocol = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0.1");
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": protocol,
                        "capabilities": {
                            "tools": {
                                "list": true,
                                "call": true
                            }
                        },
                        "serverInfo": {
                            "name": "kairos-vault",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                })
            }
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tools }
            }),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                match super::execute_tool_with_handles(
                    name,
                    arguments,
                    &mv2,
                    read_only,
                    &mut mem_read,
                    &mut mem_write,
                ) {
                    Ok(result) => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [
                                { "type": "text", "text": result.output }
                            ],
                            "details": result.details,
                            "isError": false
                        }
                    }),
                    Err(err) => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32000, "message": err }
                    }),
                }
            }
            "shutdown" => {
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": null
                });
                write_mcp_response(&mut writer, &response)?;
                break;
            }
            _ => {
                if !has_id {
                    continue;
                }
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "method not found" }
                })
            }
        };

        if has_id || method == "initialize" || method == "tools/list" || method == "tools/call" {
            write_mcp_response(&mut writer, &response)?;
        }
    }

    Ok(())
}

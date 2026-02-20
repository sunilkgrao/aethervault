use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn is_recoverable_mcp_error(msg: &str) -> bool {
    let msg = msg.to_ascii_lowercase();
    msg.contains("server closed connection")
        || msg.contains("reader disconnected")
        || msg.contains("reader reached eof")
}

enum ReaderEvent {
    Message(serde_json::Value),
    StdioClosed,
    Error(String),
}

fn spawn_reader_thread(
    name: String,
    stdout: std::process::ChildStdout,
) -> mpsc::Receiver<ReaderEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_mcp_message_strict(&mut reader) {
                Ok(msg) => {
                    if tx.send(ReaderEvent::Message(msg)).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    if err.kind() == io::ErrorKind::UnexpectedEof {
                        let _ = tx.send(ReaderEvent::StdioClosed);
                    } else {
                        let _ = tx.send(ReaderEvent::Error(format!("mcp '{name}': {err}")));
                    }
                    break;
                }
            }
        }
    });
    rx
}

// === Generic MCP Client Registry ===
// Manages long-lived MCP server sidecars. Each server is spawned once, handshaked,
// tools discovered via tools/list, and kept alive for the agent session. Tool calls
// are routed to the correct server via a name->server routing map.

pub(crate) struct McpServerHandle {
    name: String,
    config: super::McpServerConfig,
    stdin: std::process::ChildStdin,
    msg_rx: mpsc::Receiver<ReaderEvent>,
    child: std::process::Child,
    next_id: i64,
    dead: bool,
    /// Tools discovered from this server (original names)
    tools: Vec<serde_json::Value>,
}

const MCP_POLL_INTERVAL_MS: u64 = 250;

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
        McpServerHandle::start(cfg)
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

        let mut retries = 0u8;

        loop {
            if self.servers[server_idx].is_dead() {
                if retries >= 1 {
                    return Err(format!(
                        "mcp '{}': server is unavailable after reconnect attempt",
                        self.servers[server_idx].name
                    ));
                }
                eprintln!("[mcp:{}] server was marked dead, attempting reconnect", self.servers[server_idx].name);
                self.servers[server_idx].restart()?;
                retries += 1;
                continue;
            }

            let call_id = {
                let handle = &mut self.servers[server_idx];
                let call_id = handle.next_id;
                match handle.send_msg(&serde_json::json!({
                    "jsonrpc": "2.0", "id": call_id, "method": "tools/call",
                    "params": { "name": original_name, "arguments": args.clone() }
                })) {
                    Ok(_) => {
                        handle.next_id += 1;
                        Ok(call_id)
                    }
                    Err(err) => {
                        handle.mark_dead();
                        Err(err)
                    }
                }
            };

            let call_id = match call_id {
                Ok(id) => id,
                Err(err) => {
                    if retries >= 1 {
                        return Err(err);
                    }
                    eprintln!(
                        "[mcp:{}] failed to send '{}' call: {err}",
                        self.servers[server_idx].name, prefixed_name
                    );
                    retries += 1;
                    self.servers[server_idx].restart()?;
                    continue;
                }
            };

            let (response, should_retry) = {
                let handle = &mut self.servers[server_idx];
                let mut response: Option<serde_json::Value> = None;
                let mut should_retry = false;

                loop {
                    let msg = match handle.read_msg_timeout(Duration::from_millis(MCP_POLL_INTERVAL_MS)) {
                        Ok(msg) => msg,
                        Err(err) => {
                            if is_recoverable_mcp_error(&err) && retries < 1 {
                                eprintln!(
                                    "[mcp:{}] recoverable error while calling '{}': {err}",
                                    handle.name, prefixed_name
                                );
                                should_retry = true;
                                break;
                            }
                            return Err(err);
                        }
                    };

                    if msg.get("id").is_none() {
                        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("unknown");
                        eprintln!("[mcp:{}] skipping notification: {method}", handle.name);
                        continue;
                    }
                    if let Some(resp_id) = msg.get("id").and_then(|v| v.as_i64()) {
                        if resp_id != call_id {
                            return Err(format!(
                                "mcp '{}': response id mismatch (expected {call_id}, got {resp_id})",
                                handle.name
                            ));
                        }
                    }
                    response = Some(msg);
                    break;
                }

                (response, should_retry)
            };

            if should_retry {
                retries += 1;
                self.servers[server_idx].restart()?;
                continue;
            }

            let resp = response.ok_or_else(|| format!(
                "mcp '{}': missing response while calling '{}'",
                self.servers[server_idx].name, prefixed_name
            ))?;

            // Check for JSON-RPC error
            if let Some(err) = resp.get("error") {
                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
                let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                return Err(format!("mcp '{}' error {code}: {msg}", self.servers[server_idx].name));
            }
            let result = resp.get("result").cloned()
                .ok_or_else(|| format!("mcp '{}': response missing 'result'", self.servers[server_idx].name))?;
            // Extract all text parts from content array (MCP responses can have multiple items)
            let content_text = match result.get("content").and_then(|c| c.as_array()) {
                Some(arr) => {
                    let text_parts: Vec<&str> = arr.iter()
                        .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                        .collect();
                    if text_parts.is_empty() {
                        serde_json::to_string_pretty(&result).unwrap_or_default()
                    } else {
                        text_parts.join("\n")
                    }
                }
                None => serde_json::to_string_pretty(&result).unwrap_or_default(),
            };
            let is_error = result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);

            return Ok(super::ToolExecution {
                output: content_text.to_string(),
                details: result,
                is_error,
            });
        }
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
    fn start(cfg: &super::McpServerConfig) -> Result<McpServerHandle, String> {
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
        let msg_rx = spawn_reader_thread(cfg.name.clone(), stdout);

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
            config: cfg.clone(),
            name: cfg.name.clone(),
            stdin,
            child,
            msg_rx,
            next_id: 1,
            dead: false,
            tools: Vec::new(),
        };

        handle.bootstrap()?;
        Ok(handle)
    }

    fn bootstrap(&mut self) -> Result<(), String> {
        // Initialize handshake
        self.send_msg(&serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "aethervault", "version": "0.1" }
            }
        }))?;
        self.next_id += 1;

        let init_resp = self.read_msg()?;
        if let Some(err) = init_resp.get("error") {
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
            let _ = self.child.kill();
            let _ = self.child.wait();
            return Err(format!("mcp '{}': initialize failed: {msg}", self.name));
        }

        // Send initialized notification
        self.send_msg(&serde_json::json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        }))?;

        // Discover tools
        self.send_msg(&serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id, "method": "tools/list"
        }))?;
        self.next_id += 1;

        let list_resp = self.read_msg()?;
        if let Some(err) = list_resp.get("error") {
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
            eprintln!("[mcp-registry] '{}': tools/list failed: {msg}", self.name);
        } else if let Some(tools_arr) = list_resp.get("result")
            .and_then(|r| r.get("tools"))
            .and_then(|t| t.as_array())
        {
            self.tools = tools_arr.clone();
            eprintln!("[mcp-registry] '{}': discovered {} tools", self.name, self.tools.len());
        }

        Ok(())
    }

    fn restart(&mut self) -> Result<(), String> {
        let config = self.config.clone();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let replacement = Self::start(&config)?;
        *self = replacement;
        Ok(())
    }

    pub(crate) fn send_msg(&mut self, msg: &serde_json::Value) -> Result<(), String> {
        let body = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body)
            .map_err(|e| format!("mcp '{}' write: {e}", self.name))?;
        self.stdin.flush().map_err(|e| format!("mcp '{}' flush: {e}", self.name))?;
        Ok(())
    }

    pub(crate) fn read_msg(&mut self) -> Result<serde_json::Value, String> {
        self.read_msg_timeout(Duration::from_millis(MCP_POLL_INTERVAL_MS))
    }

    pub(crate) fn read_msg_timeout(&mut self, timeout: Duration) -> Result<serde_json::Value, String> {
        let mut last_update = Instant::now();
        let cancel_token = Arc::new(AtomicBool::new(false));
        let timeout = timeout.max(Duration::from_millis(1));
        loop {
            if cancel_token.load(Ordering::Acquire) {
                self.dead = true;
                return Err(format!("mcp '{}' response wait canceled", self.name));
            }
            match self.msg_rx.recv_timeout(timeout) {
                Ok(ReaderEvent::Message(msg)) => return Ok(msg),
                Ok(ReaderEvent::StdioClosed) => {
                    self.dead = true;
                    return Err(format!("mcp '{}' reader reached EOF", self.name));
                }
                Ok(ReaderEvent::Error(msg)) => return Err(msg),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if last_update.elapsed() >= Duration::from_secs(5) {
                        eprintln!("[mcp:{}] polling for message (no deadline)", self.name);
                        last_update = Instant::now();
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.dead = true;
                    return Err(format!("mcp '{}' reader disconnected", self.name));
                }
            }
        }
    }

    fn is_dead(&self) -> bool {
        self.dead
    }

    fn mark_dead(&mut self) {
        self.dead = true;
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

fn read_mcp_message_strict(reader: &mut BufReader<impl Read>) -> io::Result<serde_json::Value> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "server closed connection (process likely crashed)",
            ));
        }
        if !line.trim().is_empty() {
            break;
        }
    }

    if line
        .to_ascii_lowercase()
        .starts_with("content-length:")
    {
        let mut content_length = line
            .split(':')
            .nth(1)
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);

        loop {
            let mut hdr = String::new();
            reader.read_line(&mut hdr)?;
            if hdr == "\r\n" || hdr == "\n" || hdr.is_empty() {
                break;
            }
            if hdr.to_ascii_lowercase().starts_with("content-length:") {
                content_length = hdr
                    .split(':')
                    .nth(1)
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(content_length);
            }
        }

        if content_length == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid zero-length MCP message",
            ));
        }
        if content_length > 10 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("response too large ({content_length} bytes)"),
            ));
        }

        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
        serde_json::from_slice(&body).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {e}"))
        })
    } else {
        serde_json::from_str(line.trim()).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {e}"))
        })
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
    let db = super::open_or_create_db(&mv2)?;

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
                match super::execute_tool(
                    name,
                    arguments,
                    &mv2,
                    &db,
                    read_only,
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

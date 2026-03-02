use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use rmcp::{
    model::{CallToolRequestParams, JsonObject, Tool},
    service::{RunningService, RoleClient, ServiceExt},
    transport::TokioChildProcess,
};
use tokio::runtime::Builder;
use tokio::{process::Command, runtime::Runtime, time::timeout};

const MCP_TOOL_CALL_TIMEOUT_SECS: u64 = 30;

fn is_recoverable_mcp_error(msg: &str) -> bool {
    let msg = msg.to_ascii_lowercase();
    msg.contains("server closed connection")
        || msg.contains("reader disconnected")
        || msg.contains("reader reached eof")
        || msg.contains("connection closed")
        || msg.contains("transport closed")
}

fn normalize_mcp_id(id: &serde_json::Value) -> serde_json::Value {
    match id {
        serde_json::Value::String(text) => text
            .parse::<i64>()
            .map(|v| serde_json::json!(v))
            .unwrap_or_else(|_| serde_json::Value::String(text.clone())),
        serde_json::Value::Number(num) => {
            if let Some(v) = num.as_i64() {
                serde_json::json!(v)
            } else if let Some(v) = num.as_u64() {
                if v <= i64::MAX as u64 {
                    serde_json::json!(v as i64)
                } else {
                    serde_json::Value::Number(num.clone())
                }
            } else if let Some(v) = num.as_f64() {
                if v.fract() == 0.0 {
                    serde_json::json!(v as i64)
                } else {
                    serde_json::Value::Number(num.clone())
                }
            } else {
                serde_json::Value::Number(num.clone())
            }
        }
        _ => id.clone(),
    }
}

fn mcp_id_matches(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    normalize_mcp_id(a) == normalize_mcp_id(b)
}

pub(crate) struct McpServerHandle {
    name: String,
    config: super::McpServerConfig,
    timeout_secs: u64,
    dead: bool,
    tools: Vec<serde_json::Value>,
    child_pid: Option<u32>,
    runtime: Runtime,
    service: Option<RunningService<RoleClient, ()>>,
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
                let input_schema = tool
                    .get("inputSchema")
                    .cloned()
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
        let (server_idx, original_name) = self
            .route_map
            .get(prefixed_name)
            .ok_or_else(|| format!("mcp: unknown tool '{prefixed_name}'"))?
            .clone();
        let timeout_secs = self.servers[server_idx].timeout_secs;
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);

        let mut retries = 0u8;

        loop {
            if Instant::now() > deadline {
                return Err(format!(
                    "mcp '{}': timed out after {timeout_secs}s while calling '{}'",
                    self.servers[server_idx].name, prefixed_name
                ));
            }

            if self.servers[server_idx].is_dead() {
                if retries >= 1 {
                    return Err(format!(
                        "mcp '{}': server is unavailable after reconnect attempt",
                        self.servers[server_idx].name
                    ));
                }
                eprintln!(
                    "[mcp:{}] server was marked dead, attempting reconnect",
                    self.servers[server_idx].name
                );
                self.servers[server_idx].restart()?;
                retries += 1;
                continue;
            }

            let call_result = {
                let handle = &mut self.servers[server_idx];
                handle.call_tool(&original_name, args.clone(), deadline)
            };

            match call_result {
                Ok(result) => {
                    return Ok(Self::tool_execution_from_rmcp_result(result));
                }
                Err(err) => {
                    if retries < 1 && is_recoverable_mcp_error(&err) {
                        eprintln!(
                            "[mcp:{}] recoverable error while calling '{}': {err}",
                            self.servers[server_idx].name, prefixed_name
                        );
                        self.servers[server_idx].mark_dead();
                        self.servers[server_idx].restart()?;
                        retries += 1;
                        continue;
                    }
                    return Err(err);
                }
            }
        }
    }

    fn tool_execution_from_rmcp_result(result: rmcp::model::CallToolResult) -> super::ToolExecution {
        let text_parts: Vec<&str> = result
            .content
            .iter()
            .filter_map(|item| item.raw.as_text())
            .map(|text| text.text.as_str())
            .collect();

        let output = if text_parts.is_empty() {
            serde_json::to_string_pretty(&result).unwrap_or_default()
        } else {
            text_parts.join("\n")
        };

        super::ToolExecution {
            output,
            details: serde_json::to_value(&result)
                .unwrap_or_else(|_| serde_json::json!({"details": "unserializable"})),
            is_error: result.is_error.unwrap_or(false),
        }
    }

    /// Shutdown all servers
    pub(crate) fn shutdown(&mut self) {
        for handle in &mut self.servers {
            let _ = handle.shutdown();
        }
        self.servers.clear();
        self.route_map.clear();
    }
}

impl McpServerHandle {
    fn build_command(cfg: &super::McpServerConfig) -> Result<Command, String> {
        let cmd_parts = shlex::split(&cfg.command)
            .ok_or_else(|| format!("mcp '{}': malformed command", cfg.name))?;
        if cmd_parts.is_empty() {
            return Err(format!("mcp '{}': empty command", cfg.name));
        }

        let mut command = if let Some(wrapper) = super::command_wrapper() {
            let mut cmd = Command::new(&wrapper[0]);
            if wrapper.len() > 1 {
                cmd.args(&wrapper[1..]);
            }
            cmd.arg(&cmd_parts[0]).args(&cmd_parts[1..]);
            cmd
        } else {
            let mut cmd = Command::new(&cmd_parts[0]);
            cmd.args(&cmd_parts[1..]);
            cmd
        };

        for (k, v) in &cfg.env {
            command.env(k, v);
        }
        Ok(command)
    }

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

        let command = Self::build_command(cfg)?;
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("mcp '{}' failed to build runtime: {e}", cfg.name))?;

        let transport = TokioChildProcess::new(command)
            .map_err(|e| format!("mcp '{}' spawn transport: {e}", cfg.name))?;
        let child_pid = transport.id();
        let service = runtime
            .block_on(async {
                let service = ().serve(transport).await;
                let service = match service {
                    Ok(service) => service,
                    Err(err) => return Err(format!("mcp '{}' serve_client: {err}", cfg.name)),
                };
                Ok::<RunningService<RoleClient, ()>, String>(service)
            })?;

        let mut handle = McpServerHandle {
            config: cfg.clone(),
            name: cfg.name.clone(),
            timeout_secs: cfg.timeout_secs.unwrap_or(MCP_TOOL_CALL_TIMEOUT_SECS),
            dead: false,
            tools: Vec::new(),
            child_pid,
            runtime,
            service: Some(service),
        };

        handle.refresh_tools()?;
        Ok(handle)
    }

    fn refresh_tools(&mut self) -> Result<(), String> {
        let service = self
            .service
            .as_ref()
            .ok_or_else(|| format!("mcp '{}' service unavailable", self.name))?;

        let tools = match self.runtime.block_on(service.list_all_tools()) {
            Ok(tools) => tools,
            Err(err) => {
                eprintln!("[mcp] refresh_tools failed for {}: {err}", self.name);
                return Ok(());
            }
        };

        self.tools = Self::normalize_tool_list(tools);
        eprintln!(
            "[mcp-registry] '{}': discovered {} tools",
            self.name,
            self.tools.len()
        );
        Ok(())
    }

    fn normalize_tool_list(tools: Vec<Tool>) -> Vec<serde_json::Value> {
        tools
            .into_iter()
            .map(|tool| {
                let input_schema = serde_json::to_value(tool.input_schema.as_ref())
                    .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));
                let description = tool.description.unwrap_or_default().into_owned();

                serde_json::json!({
                    "name": tool.name.to_string(),
                    "description": description,
                    "inputSchema": input_schema,
                })
            })
            .collect()
    }

    fn restart(&mut self) -> Result<(), String> {
        let config = self.config.clone();
        let _ = self.shutdown();
        let replacement = Self::start(&config)?;
        *self = replacement;
        Ok(())
    }

    fn call_tool(
        &mut self,
        name: &str,
        args: serde_json::Value,
        deadline: Instant,
    ) -> Result<rmcp::model::CallToolResult, String> {
        if self.service.is_none() {
            self.mark_dead();
            return Err(format!("mcp '{}' service unavailable", self.name));
        }

        let remaining = deadline.checked_duration_since(Instant::now()).unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            return Err(format!(
                "mcp '{}' timed out before calling '{}', request exceeded deadline",
                self.name, name
            ));
        }

        let service = self
            .service
            .as_ref()
            .ok_or_else(|| format!("mcp '{}' service unavailable", self.name))?;

        if service.is_closed() {
            self.dead = true;
            return Err(format!("mcp '{}' service closed", self.name));
        }

        let arguments = if args.is_null() {
            None
        } else {
            Some(
                serde_json::from_value::<JsonObject>(args).map_err(|_| {
                    format!(
                        "mcp '{}' invalid call arguments for '{name}' (expected object)",
                        self.name
                    )
                })?,
            )
        };

        let req = CallToolRequestParams {
            name: name.to_owned().into(),
            arguments,
            meta: None,
            task: None,
        };

        let result = self
            .runtime
            .block_on(timeout(remaining, service.call_tool(req)))
            .map_err(|_| format!("mcp '{}' timed out while calling '{name}'", self.name))?;

        match result {
            Ok(result) => {
                self.dead = false;
                Ok(result)
            }
            Err(err) => {
                let msg = format!("{err}");
                if is_recoverable_mcp_error(&msg) {
                    self.dead = true;
                }
                Err(format!("mcp '{}' call_tool failed: {msg}", self.name))
            }
        }
    }

pub(crate) fn shutdown(&mut self) -> Result<(), String> {
        let service = self.service.take();
        let mut shutdown_error: Option<String> = None;

        if let Some(service) = service {
            let mut service = service;
            match self
                .runtime
                .block_on(async { service.close_with_timeout(Duration::from_millis(500)).await })
            {
                Ok(_) => {}
                Err(err) => {
                    shutdown_error = Some(format!("mcp '{}' shutdown: {err}", self.name));
                }
            }
        }

        if let Some(pid) = self.child_pid {
            thread::sleep(Duration::from_millis(500));
            if Self::force_kill_child(pid) {
                eprintln!("[mcp] force-killed stubborn MCP server: {}", self.name);
            }
        }

        if let Some(err) = shutdown_error {
            return Err(err);
        }

        Ok(())
    }

    #[cfg(unix)]
    fn force_kill_child(pid: u32) -> bool {
        let alive = {
            let check = unsafe { libc::kill(pid as libc::pid_t, 0) };
            if check == 0 {
                true
            } else {
                !matches!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH))
            }
        };
        if !alive {
            return false;
        }

        let kill_result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        kill_result == 0
    }

    #[cfg(not(unix))]
    fn force_kill_child(_pid: u32) -> bool {
        false
    }

    fn is_dead(&self) -> bool {
        self.dead || self.service.as_ref().is_none_or(RunningService::is_closed)
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

pub(crate) fn write_mcp_response(writer: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    let payload = serde_json::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e}")))?;
    write!(writer, "Content-Length: {}\r\n\r\n", payload.len())?;
    writer.write_all(&payload)?;
    writer.flush()
}

pub(crate) fn run_mcp_server(
    mv2: PathBuf,
    read_only: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = BufReader::new(io::stdin());
    let mut writer = io::stdout();
    let tools = super::tool_definitions_json();
    let db = super::open_or_create_db(&mv2)?;

    loop {
        let Some(msg) = read_mcp_message(&mut reader)? else {
            break;
        };
        let id = msg.get("id").cloned();
        let has_id = id
            .as_ref()
            .is_some_and(|value| !value.is_null() && mcp_id_matches(value, value));
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
                    None,
                    None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_id_normalization_matches_integral_variants() {
        let id_number = serde_json::json!(1);
        let id_float = serde_json::json!(1.0);
        let id_string = serde_json::json!("1");

        assert!(mcp_id_matches(&id_number, &id_float));
        assert!(mcp_id_matches(&id_number, &id_string));
        assert!(mcp_id_matches(&id_float, &id_string));
    }

    #[test]
    fn test_mcp_id_normalization_distinguishes_non_integral_numbers() {
        let id_int = serde_json::json!(1);
        let id_float = serde_json::json!(1.1);

        assert!(!mcp_id_matches(&id_int, &id_float));
    }
}

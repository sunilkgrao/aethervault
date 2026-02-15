use std::io::{self, Read};
use std::thread;
use std::time::Duration;

use serde_json;

use crate::{
    command_spec_to_vec, env_bool, env_f64, env_optional, env_required, env_u64, env_usize,
    jitter_ratio, parse_retry_after, run_hook_command, AgentHookRequest, AgentHookResponse,
    AgentMessage, AgentToolCall, CommandSpec, HookSpec,
};

const CRITIC_SYSTEM_PROMPT: &str = "\
You are a silent quality auditor for an AI agent. Your job is to evaluate whether \
the agent's recent reasoning is grounded in evidence or drifting into overconfidence.\n\n\
Evaluate the conversation and return JSON:\n\
{\"grounded\": true/false, \"issues\": [...], \"correction\": \"...\"}\n\n\
Flag as NOT grounded if the agent:\n\
- Claims something is \"done\" or \"working\" without running a verification step\n\
- Makes optimistic assumptions without checking (e.g., \"this should work\" without testing)\n\
- Ignores or minimizes errors/failures in tool output\n\
- Over-engineers or adds scope beyond what was asked\n\
- Reports success on aspirational claims (deployed, optimized, complete) without evidence\n\
- Takes destructive or irreversible actions without confirming with the user\n\
- Spins on the same approach after repeated failures instead of changing strategy\n\n\
If grounded, return {\"grounded\": true}. If not, write a terse correction (1-2 sentences) \
that steers back to reality. Do NOT reveal you are a separate critic — write as a \
system reminder.";

pub(crate) fn collect_system_blocks(messages: &[AgentMessage]) -> Vec<String> {
    let mut blocks = Vec::new();
    for msg in messages {
        if msg.role == "system" {
            if let Some(content) = &msg.content {
                if !content.trim().is_empty() {
                    blocks.push(content.trim().to_string());
                }
            }
        }
    }
    blocks
}

pub(crate) fn to_anthropic_messages(messages: &[AgentMessage]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for msg in messages {
        match msg.role.as_str() {
            "system" => continue,
            "user" => {
                let content = msg.content.clone().unwrap_or_default();
                // Check for embedded image markers: [AV_IMAGE:media_type:base64data]
                if content.contains("[AV_IMAGE:") {
                    let mut blocks: Vec<serde_json::Value> = Vec::new();
                    let mut remaining = content.as_str();
                    while let Some(start) = remaining.find("[AV_IMAGE:") {
                        // Text before the marker
                        let before = &remaining[..start];
                        if !before.trim().is_empty() {
                            blocks.push(serde_json::json!({"type": "text", "text": before.trim()}));
                        }
                        let after_prefix = &remaining[start + 10..]; // skip "[AV_IMAGE:"
                        if let Some(end) = after_prefix.find(']') {
                            let marker_content = &after_prefix[..end];
                            // marker_content = "media_type:base64data"
                            if let Some(colon) = marker_content.find(':') {
                                let media_type = &marker_content[..colon];
                                let b64_data = &marker_content[colon + 1..];
                                blocks.push(serde_json::json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
                                        "data": b64_data
                                    }
                                }));
                            }
                            remaining = &after_prefix[end + 1..];
                        } else {
                            remaining = after_prefix;
                            break;
                        }
                    }
                    if !remaining.trim().is_empty() {
                        blocks.push(serde_json::json!({"type": "text", "text": remaining.trim()}));
                    }
                    if blocks.is_empty() {
                        blocks.push(serde_json::json!({"type": "text", "text": ""}));
                    }
                    out.push(serde_json::json!({"role": "user", "content": blocks}));
                } else {
                    out.push(serde_json::json!({
                        "role": "user",
                        "content": [{"type": "text", "text": content}]
                    }));
                }
            }
            "assistant" => {
                let mut blocks = Vec::new();
                if let Some(content) = &msg.content {
                    if !content.is_empty() {
                        blocks.push(serde_json::json!({"type": "text", "text": content}));
                    }
                }
                for call in &msg.tool_calls {
                    blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": call.id.clone(),
                        "name": call.name.clone(),
                        "input": call.args.clone()
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(serde_json::json!({"type": "text", "text": ""}));
                }
                out.push(serde_json::json!({"role": "assistant", "content": blocks}));
            }
            "tool" => {
                let Some(tool_id) = msg.tool_call_id.clone() else {
                    continue;
                };
                let mut block = serde_json::Map::new();
                block.insert("type".to_string(), serde_json::json!("tool_result"));
                block.insert("tool_use_id".to_string(), serde_json::json!(tool_id));
                block.insert(
                    "content".to_string(),
                    serde_json::json!(msg.content.clone().unwrap_or_default()),
                );
                if msg.is_error.unwrap_or(false) {
                    block.insert("is_error".to_string(), serde_json::json!(true));
                }
                out.push(serde_json::json!({
                    "role": "user",
                    "content": [serde_json::Value::Object(block)]
                }));
            }
            _ => {}
        }
    }
    out
}

pub(crate) fn to_anthropic_tools(
    tools: &[serde_json::Value],
    cache_control: Option<serde_json::Value>,
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for tool in tools {
        let Some(obj) = tool.as_object() else {
            continue;
        };
        let Some(name) = obj.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let mut entry = serde_json::Map::new();
        entry.insert("name".to_string(), serde_json::json!(name));
        if let Some(desc) = obj.get("description").and_then(|v| v.as_str()) {
            entry.insert("description".to_string(), serde_json::json!(desc));
        }
        if let Some(schema) = obj.get("inputSchema").or_else(|| obj.get("input_schema")) {
            entry.insert("input_schema".to_string(), schema.clone());
        }
        if let Some(cache) = cache_control.clone() {
            entry.insert("cache_control".to_string(), cache);
        }
        out.push(serde_json::Value::Object(entry));
    }
    out
}

pub(crate) fn parse_claude_response(
    payload: &serde_json::Value,
) -> Result<AgentHookResponse, Box<dyn std::error::Error>> {
    let content = payload
        .get("content")
        .and_then(|v| v.as_array())
        .ok_or("Claude response missing content")?;
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for block in content {
        let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match btype {
            "text" => {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        text_parts.push(text.to_string());
                    }
                }
            }
            "tool_use" => {
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = block
                    .get("input")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                tool_calls.push(AgentToolCall { id, name, args });
            }
            _ => {}
        }
    }

    let content_text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    };

    Ok(AgentHookResponse {
        message: AgentMessage {
            role: "assistant".to_string(),
            content: content_text,
            tool_calls,
            name: None,
            tool_call_id: None,
            is_error: None,
        },
    })
}

pub(crate) fn call_claude(
    request: &AgentHookRequest,
) -> Result<AgentHookResponse, Box<dyn std::error::Error>> {
    let api_key = env_required("ANTHROPIC_API_KEY")?;
    let model = env_required("ANTHROPIC_MODEL")?;
    let base_url = env_optional("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string());
    let max_tokens = env_u64("ANTHROPIC_MAX_TOKENS", 8192)?;
    let temperature = env_optional("ANTHROPIC_TEMPERATURE")
        .map(|v| v.parse::<f64>())
        .transpose()
        .map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid ANTHROPIC_TEMPERATURE")
        })?;
    let top_p = env_optional("ANTHROPIC_TOP_P")
        .map(|v| v.parse::<f64>())
        .transpose()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid ANTHROPIC_TOP_P"))?;
    let timeout = env_u64("ANTHROPIC_TIMEOUT", u64::MAX)?;
    let max_retries = env_usize("ANTHROPIC_MAX_RETRIES", 2)?;
    let retry_base = env_f64("ANTHROPIC_RETRY_BASE", 0.5)?;
    let retry_max = env_f64("ANTHROPIC_RETRY_MAX", 4.0)?;
    let version = env_optional("ANTHROPIC_VERSION").unwrap_or_else(|| "2023-06-01".to_string());
    let beta = env_optional("ANTHROPIC_BETA");
    let token_efficient = env_bool("ANTHROPIC_TOKEN_EFFICIENT", false);
    let mut beta_values: Vec<String> = Vec::new();
    if let Some(b) = beta {
        for item in b.split(',') {
            let trimmed = item.trim();
            if !trimmed.is_empty() {
                beta_values.push(trimmed.to_string());
            }
        }
    }
    if token_efficient {
        beta_values.push("token-efficient-tools-2025-02-19".to_string());
    }

    let system_blocks = collect_system_blocks(&request.messages);
    let use_prompt_cache = env_bool("ANTHROPIC_PROMPT_CACHE", false);
    let cache_ttl = env_optional("ANTHROPIC_PROMPT_CACHE_TTL");
    let cache_control = if use_prompt_cache {
        let mut obj = serde_json::Map::new();
        obj.insert("type".to_string(), serde_json::json!("ephemeral"));
        if let Some(ttl) = cache_ttl {
            if !ttl.trim().is_empty() {
                obj.insert("ttl".to_string(), serde_json::json!(ttl));
            }
        }
        Some(serde_json::Value::Object(obj))
    } else {
        None
    };
    let mut payload = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": to_anthropic_messages(&request.messages),
    });
    if !system_blocks.is_empty() {
        if let Some(cache) = cache_control.clone() {
            let blocks: Vec<serde_json::Value> = system_blocks.iter().enumerate().map(|(i, text)| {
                let mut block = serde_json::json!({"type": "text", "text": text});
                if i == 0 {
                    block.as_object_mut().unwrap().insert("cache_control".to_string(), cache.clone());
                }
                block
            }).collect();
            payload["system"] = serde_json::json!(blocks);
        } else {
            payload["system"] = serde_json::json!(system_blocks.join("\n\n"));
        }
    }
    let tools = to_anthropic_tools(&request.tools, cache_control.clone());
    if !tools.is_empty() {
        payload["tools"] = serde_json::json!(tools);
    }
    if let Some(temp) = temperature {
        payload["temperature"] = serde_json::json!(temp);
    }
    if let Some(p) = top_p {
        payload["top_p"] = serde_json::json!(p);
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(timeout))
        .timeout_read(Duration::from_secs(timeout))
        .timeout_write(Duration::from_secs(timeout))
        .build();

    let retryable = |status: u16| matches!(status, 429 | 500 | 502 | 503 | 504 | 529);
    let mut body = None;

    for attempt in 0..=max_retries {
        let mut request = agent
            .post(&base_url)
            .set("content-type", "application/json")
            .set("x-api-key", &api_key)
            .set("anthropic-version", &version);
        if !beta_values.is_empty() {
            request = request.set("anthropic-beta", &beta_values.join(","));
        }

        let response = request.send_json(payload.clone());
        match response {
            Ok(resp) => {
                body = Some(resp.into_string()?);
                break;
            }
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = parse_retry_after(&resp);
                let text = resp.into_string().unwrap_or_default();
                if attempt < max_retries && retryable(code) {
                    let mut delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                    if let Some(retry_after) = retry_after {
                        delay = delay.max(retry_after);
                    }
                    let jitter = jitter_ratio() * 0.2;
                    delay *= 1.0 + jitter;
                    thread::sleep(Duration::from_secs_f64(delay));
                    continue;
                }
                eprintln!("[call_claude] primary API failed after {} retries: {code} {text}", max_retries);
                break; // fall through to fallback/Vertex
            }
            Err(ureq::Error::Transport(err)) => {
                if attempt < max_retries {
                    let mut delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                    let jitter = jitter_ratio() * 0.2;
                    delay *= 1.0 + jitter;
                    thread::sleep(Duration::from_secs_f64(delay));
                    continue;
                }
                eprintln!("[call_claude] primary API transport error after {} retries: {err}", max_retries);
                break; // fall through to fallback/Vertex
            }
        }
    }

    // If primary model failed, try fallback model
    if body.is_none() {
        if let Ok(fallback_model) = std::env::var("ANTHROPIC_FALLBACK_MODEL") {
            eprintln!("Primary model failed, trying fallback: {fallback_model}");
            payload["model"] = serde_json::json!(fallback_model);
            for attempt in 0..=1 {
                let mut request = agent
                    .post(&base_url)
                    .set("content-type", "application/json")
                    .set("x-api-key", &api_key)
                    .set("anthropic-version", &version);
                if !beta_values.is_empty() {
                    request = request.set("anthropic-beta", &beta_values.join(","));
                }
                match request.send_json(payload.clone()) {
                    Ok(resp) => {
                        body = Some(resp.into_string()?);
                        break;
                    }
                    Err(ureq::Error::Status(code, resp)) => {
                        let text = resp.into_string().unwrap_or_default();
                        if attempt == 1 {
                            return Err(format!("Fallback model also failed: {code} {text}").into());
                        }
                        thread::sleep(Duration::from_secs(1));
                    }
                    Err(ureq::Error::Transport(err)) => {
                        if attempt == 1 {
                            return Err(format!("Fallback model transport error: {err}").into());
                        }
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        }
    }

    // If both primary and fallback model failed, try Vertex proxy as last resort.
    if body.is_none() {
        let vertex_url = env_optional("VERTEX_FALLBACK_URL")
            .unwrap_or_else(|| "http://localhost:11436/v1/messages".to_string());
        let vertex_enabled = env_optional("VERTEX_FALLBACK").unwrap_or_else(|| "1".to_string()) == "1";
        if vertex_enabled {
            eprintln!("Anthropic direct failed, falling back to Vertex proxy at {vertex_url}");
            payload["model"] = serde_json::json!(model);
            let vertex_key = env_optional("VERTEX_API_KEY").unwrap_or_else(|| api_key.clone());
            for attempt in 0..=max_retries {
                let mut request = agent
                    .post(&vertex_url)
                    .set("content-type", "application/json")
                    .set("x-api-key", &vertex_key)
                    .set("anthropic-version", &version);
                if !beta_values.is_empty() {
                    request = request.set("anthropic-beta", &beta_values.join(","));
                }
                match request.send_json(payload.clone()) {
                    Ok(resp) => {
                        body = Some(resp.into_string()?);
                        break;
                    }
                    Err(ureq::Error::Status(code, resp)) => {
                        let text = resp.into_string().unwrap_or_default();
                        if attempt == max_retries {
                            return Err(format!("Vertex fallback also failed: {code} {text}").into());
                        }
                        let delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                        thread::sleep(Duration::from_secs_f64(delay));
                    }
                    Err(ureq::Error::Transport(err)) => {
                        if attempt == max_retries {
                            return Err(format!("Vertex fallback transport error: {err}").into());
                        }
                        let delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                        thread::sleep(Duration::from_secs_f64(delay));
                    }
                }
            }
        }
    }

    let body = body.ok_or("All API endpoints failed (Anthropic direct + Vertex fallback)")?;
    let payload: serde_json::Value = serde_json::from_str(&body)?;
    parse_claude_response(&payload)
}

pub(crate) fn run_claude_hook() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    if input.trim().is_empty() {
        return Err("Claude hook received empty input".into());
    }
    let req: AgentHookRequest = serde_json::from_str(&input)?;
    let response = call_claude(&req)?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

/// Silent critic: evaluates agent reasoning via a separate Opus API call.
/// Returns a correction string if the agent is not grounded, or None if grounded/error.
/// Never blocks or crashes the agent — all errors are silently swallowed.
pub(crate) fn call_critic(
    original_prompt: &str,
    messages: &[AgentMessage],
    step: usize,
    max_steps: usize,
) -> Option<String> {
    if !env_bool("CRITIC_ENABLED", true) {
        return None;
    }

    let api_key = env_optional("CRITIC_API_KEY")
        .or_else(|| env_optional("ANTHROPIC_API_KEY"))?;
    let model = env_optional("CRITIC_MODEL")
        .unwrap_or_else(|| "claude-opus-4-6".to_string());
    let base_url = env_optional("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string());
    let timeout_ms: u64 = env_optional("CRITIC_TIMEOUT")
        .and_then(|v| v.parse().ok())
        .unwrap_or(15_000);
    let max_tokens: u64 = env_optional("CRITIC_MAX_TOKENS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    let context_turns: usize = env_optional("CRITIC_CONTEXT_TURNS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let version = env_optional("ANTHROPIC_VERSION")
        .unwrap_or_else(|| "2023-06-01".to_string());

    // Collect recent non-system messages
    let recent: Vec<&AgentMessage> = messages
        .iter()
        .filter(|m| m.role != "system")
        .collect();
    let recent_slice = if recent.len() > context_turns {
        &recent[recent.len() - context_turns..]
    } else {
        &recent[..]
    };

    // Count tool successes and failures
    let tool_ok = messages
        .iter()
        .filter(|m| m.role == "tool" && !m.is_error.unwrap_or(false))
        .count();
    let tool_fail = messages
        .iter()
        .filter(|m| m.role == "tool" && m.is_error.unwrap_or(false))
        .count();

    // Build user prompt for the critic
    let mut context_text = format!(
        "## Original User Request\n{}\n\n## Agent Progress\nStep {}/{}\nTool calls: {} succeeded, {} failed\n\n## Recent Conversation\n",
        original_prompt,
        step + 1,
        max_steps,
        tool_ok,
        tool_fail,
    );
    for msg in recent_slice {
        let role = &msg.role;
        let content = msg.content.as_deref().unwrap_or("");
        let preview: String = content.chars().take(500).collect();
        context_text.push_str(&format!("[{role}] {preview}\n\n"));
    }

    let payload = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "temperature": 0.0,
        "system": CRITIC_SYSTEM_PROMPT,
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": context_text}]
        }]
    });

    let timeout_secs = (timeout_ms as f64 / 1000.0).max(1.0);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs_f64(timeout_secs))
        .timeout_read(Duration::from_secs_f64(timeout_secs))
        .timeout_write(Duration::from_secs_f64(timeout_secs))
        .build();

    let response = agent
        .post(&base_url)
        .set("content-type", "application/json")
        .set("x-api-key", &api_key)
        .set("anthropic-version", &version)
        .send_json(payload);

    let body = match response {
        Ok(resp) => match resp.into_string() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[critic] response read error: {e}");
                return None;
            }
        },
        Err(e) => {
            eprintln!("[critic] API error: {e}");
            return None;
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[critic] JSON parse error: {e}");
            return None;
        }
    };

    // Extract text from the Anthropic response
    let content = parsed.get("content")?.as_array()?;
    let text = content
        .iter()
        .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .and_then(|b| b.get("text").and_then(|t| t.as_str()))?;

    // Parse the critic's JSON verdict (strip markdown fences if present)
    let clean_text = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let verdict: serde_json::Value = match serde_json::from_str(clean_text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[critic] verdict parse error: {e}");
            return None;
        }
    };

    let grounded = verdict
        .get("grounded")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    if grounded {
        eprintln!("[critic] grounded=true");
        return None;
    }

    let issues = verdict
        .get("issues")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    eprintln!("[critic] grounded=false issues=[{issues}]");

    verdict
        .get("correction")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

pub(crate) fn call_agent_hook(hook: &HookSpec, request: &AgentHookRequest) -> Result<AgentMessage, String> {
    let is_builtin = match &hook.command {
        CommandSpec::String(cmd) => {
            let cmd = cmd.trim().to_ascii_lowercase();
            cmd == "builtin:claude" || cmd == "claude"
        }
        CommandSpec::Array(items) => items
            .first()
            .map(|cmd| cmd.trim().to_ascii_lowercase())
            .map(|cmd| cmd == "builtin:claude" || cmd == "claude")
            .unwrap_or(false),
    };
    if is_builtin {
        // Retry once at this level for transient failures (covers the case where
        // all fallback endpoints failed due to a temporary network blip)
        let result = call_claude(request);
        match result {
            Ok(resp) => return Ok(resp.message),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("transport") || err_str.contains("timed out") || err_str.contains("Network") {
                    eprintln!("[call_agent_hook] first attempt failed ({err_str}), retrying in 3s...");
                    thread::sleep(Duration::from_secs(3));
                    return call_claude(request)
                        .map(|resp| resp.message)
                        .map_err(|e| {
                            format!("I hit an API error and couldn't recover after retrying. The error was: {e}")
                        });
                }
                return Err(format!("API error: {e}"));
            }
        }
    }

    let cmd = command_spec_to_vec(&hook.command);
    let timeout = hook.timeout_ms.unwrap_or(u64::MAX);
    let value = serde_json::to_value(request).map_err(|e| format!("hook input: {e}"))?;
    let raw = run_hook_command(&cmd, &value, timeout, "agent")?;
    let response: AgentHookResponse =
        serde_json::from_str(&raw).map_err(|e| format!("hook output: {e}"))?;
    Ok(response.message)
}

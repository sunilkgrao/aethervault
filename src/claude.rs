use std::io::{self, Read};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use serde_json;

use crate::{
    command_spec_to_vec, env_bool, env_f64, env_optional, env_required, env_u64, env_usize,
    jitter_ratio, parse_retry_after, run_hook_command, AgentHookRequest, AgentHookResponse,
    AgentMessage, AgentToolCall, CommandSpec, HookSpec,
};

const CRITIC_SYSTEM_PROMPT: &str = "\
You are a silent quality monitor embedded in an AI agent's runtime. Your job is to verify \
the agent's claims against actual evidence in the conversation.\n\n\
EVALUATION CRITERIA (check ALL):\n\
1. FABRICATION: Does the agent claim specific details (file paths, config values, error messages, \
identifiers, version numbers, boot sequences) that do NOT appear in any tool output?\n\
2. OVERCLAIMING: Does the agent say tools succeeded when they actually failed or returned errors?\n\
3. UNACKNOWLEDGED FAILURES: Did a tool call fail (non-zero exit, error text) and the agent \
did not address it?\n\
4. SCOPE CREEP: Is the agent doing work far beyond what the user requested?\n\n\
IMPORTANT — SUBAGENT AWARENESS:\n\
This agent can invoke subagents via subagent_invoke and subagent_batch tools. When a subagent \
is invoked, its tool output contains the subagent's results. The agent is EXPECTED to \
report these results. This is NOT a phantom capability — it is a legitimate tool call. \
Only flag subagent claims as ungrounded if the subagent tool output is empty, shows errors, \
or the agent claims results that differ from what the subagent actually returned.\n\n\
IMPORTANT — ACTIVE SELF-CORRECTION:\n\
If the agent acknowledges a previous error and is actively correcting it (e.g., re-running \
a failed query with corrected parameters), this should be treated as GROUNDED behavior, not \
a new violation. Only flag if the agent claims the corrected action succeeded without evidence.\n\n\
RESPONSE FORMAT — return ONLY this JSON:\n\
{\"grounded\": true/false, \"issues\": [\"specific issue with evidence quote\"], \
\"agent_claim\": \"what the agent claimed (quote)\", \
\"evidence_shows\": \"what the tool output actually says (quote)\", \
\"correction\": \"specific behavioral instruction\"}\n\n\
If grounded=true, issues/agent_claim/evidence_shows/correction can be empty arrays/strings.\n\
If grounded=false, you MUST include at least one issue with specific quotes from the conversation.\n\
Do NOT return anything outside this JSON structure.";

// Critic circuit breaker: after N consecutive failures, skip critic for rest of session
static CRITIC_CONSECUTIVE_FAILURES: AtomicUsize = AtomicUsize::new(0);
const CRITIC_MAX_CONSECUTIVE_FAILURES: usize = 3;

// ---------------------------------------------------------------------------
// Image validation
// ---------------------------------------------------------------------------

fn validate_image_base64(media_type: &str, b64_data: &str) -> Result<(), String> {
    // 1. Check media type is supported
    let valid_types = ["image/png", "image/jpeg", "image/gif", "image/webp"];
    if !valid_types.contains(&media_type) {
        return Err(format!("unsupported media type: {media_type}"));
    }

    // 2. Check base64 is not empty and has reasonable length
    if b64_data.len() < 20 {
        return Err("base64 data too short".into());
    }

    // 3. Check decoded size (base64 is ~4/3 of raw, so 5MB raw = ~6.7MB base64)
    if b64_data.len() > 7_000_000 {
        return Err("image exceeds 5MB limit".into());
    }

    // 4. Try to decode first 16 bytes and check magic bytes
    use base64::Engine;
    let prefix = &b64_data[..b64_data.len().min(24)]; // 24 b64 chars = 18 raw bytes
    match base64::engine::general_purpose::STANDARD.decode(prefix) {
        Ok(bytes) if bytes.len() >= 4 => {
            let valid_magic = match media_type {
                "image/png" => bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
                "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
                "image/gif" => bytes.starts_with(b"GIF8"),
                "image/webp" => {
                    bytes.len() >= 12
                        && &bytes[0..4] == b"RIFF"
                        && &bytes[8..12] == b"WEBP"
                }
                _ => false,
            };
            if !valid_magic {
                return Err(format!("magic bytes don't match {media_type}"));
            }
            Ok(())
        }
        Ok(_) => Err("decoded image too small".into()),
        Err(e) => Err(format!("invalid base64: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Request repair for 400 errors (strips problematic image blocks)
// ---------------------------------------------------------------------------

fn repair_request_for_400(messages: &mut Vec<serde_json::Value>, error_body: &str) -> bool {
    let lower = error_body.to_lowercase();
    if lower.contains("base64")
        || lower.contains("could not process image")
        || lower.contains("image")
    {
        // Strip all image content blocks from messages
        let mut stripped = false;
        for msg in messages.iter_mut() {
            if let Some(content) = msg.get_mut("content") {
                if let Some(arr) = content.as_array_mut() {
                    let before = arr.len();
                    arr.retain(|block| {
                        block.get("type").and_then(|t| t.as_str()) != Some("image")
                    });
                    if arr.len() < before {
                        stripped = true;
                        // Add a text block noting images were removed
                        arr.push(serde_json::json!({"type": "text", "text": "[Images removed due to processing error]"}));
                    }
                    // If all content was images and was stripped, ensure at least one text block
                    if arr.is_empty() {
                        arr.push(serde_json::json!({"type": "text", "text": "[Images removed due to processing error]"}));
                        stripped = true;
                    }
                }
            }
        }
        return stripped;
    }
    false
}

// ---------------------------------------------------------------------------
// Lenient JSON extraction for critic verdicts
// ---------------------------------------------------------------------------

fn extract_critic_json(text: &str) -> Option<serde_json::Value> {
    let clean = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(clean) {
        return Some(v);
    }
    // Try to find JSON object within text
    if let Some(start) = clean.find('{') {
        if let Some(end) = clean.rfind('}') {
            if start < end {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&clean[start..=end]) {
                    return Some(v);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Message conversion helpers
// ---------------------------------------------------------------------------

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
                                // Validate image before creating block
                                match validate_image_base64(media_type, b64_data) {
                                    Ok(()) => {
                                        blocks.push(serde_json::json!({
                                            "type": "image",
                                            "source": {
                                                "type": "base64",
                                                "media_type": media_type,
                                                "data": b64_data
                                            }
                                        }));
                                    }
                                    Err(reason) => {
                                        eprintln!("[to_anthropic_messages] image validation failed: {reason}");
                                        blocks.push(serde_json::json!({
                                            "type": "text",
                                            "text": format!("[Image could not be included: {reason}]")
                                        }));
                                    }
                                }
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
                // Thinking blocks must come first in assistant content
                for tb in &msg.thinking_blocks {
                    blocks.push(tb.clone());
                }
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
    let mut thinking_blocks = Vec::new();

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
            "thinking" | "redacted_thinking" => {
                // Preserve thinking blocks for multi-turn tool-use conversations
                thinking_blocks.push(block.clone());
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
            thinking_blocks,
        },
    })
}

pub(crate) fn call_claude(
    request: &AgentHookRequest,
) -> Result<AgentHookResponse, Box<dyn std::error::Error>> {
    call_claude_with_model(request, None)
}

pub(crate) fn call_claude_with_model(
    request: &AgentHookRequest,
    model_override: Option<&str>,
) -> Result<AgentHookResponse, Box<dyn std::error::Error>> {
    let api_key = env_required("ANTHROPIC_API_KEY")?;
    let model = if let Some(m) = model_override {
        m.to_string()
    } else {
        env_required("ANTHROPIC_MODEL")?
    };
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
    // Extended thinking: ANTHROPIC_THINKING controls thinking mode.
    //   "adaptive" (recommended for Opus 4.6) — Claude decides when/how much to think.
    //   "off" or unset — no thinking.
    // ANTHROPIC_THINKING_EFFORT controls depth: "max", "high" (default), "medium", "low".
    //   "max" is Opus 4.6 only — highest quality, no constraints on thinking depth.
    let thinking_mode = env_optional("ANTHROPIC_THINKING")
        .unwrap_or_default();
    let thinking_enabled = thinking_mode == "adaptive";
    let thinking_effort = env_optional("ANTHROPIC_THINKING_EFFORT")
        .unwrap_or_else(|| "high".to_string());

    let effective_max_tokens = if thinking_enabled {
        // With thinking, max_tokens must cover thinking + response
        max_tokens.max(16384)
    } else {
        max_tokens
    };

    let mut payload = serde_json::json!({
        "model": model,
        "max_tokens": effective_max_tokens,
        "messages": to_anthropic_messages(&request.messages),
    });

    if thinking_enabled {
        payload["thinking"] = serde_json::json!({
            "type": "adaptive",
        });
        payload["output_config"] = serde_json::json!({
            "effort": thinking_effort,
        });
    }

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
    // Temperature is incompatible with extended thinking
    if !thinking_enabled {
        if let Some(temp) = temperature {
            payload["temperature"] = serde_json::json!(temp);
        }
        if let Some(p) = top_p {
            payload["top_p"] = serde_json::json!(p);
        }
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(timeout))
        .timeout_read(Duration::from_secs(timeout))
        .timeout_write(Duration::from_secs(timeout))
        .build();

    let retryable = |status: u16| matches!(status, 429 | 500 | 502 | 503 | 504 | 529);
    let mut body = None;
    // Track 400 error body for potential repair
    let mut last_400_body: Option<String> = None;

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
                if code == 400 {
                    eprintln!("[call_claude] got 400 from primary: {text}");
                    last_400_body = Some(text);
                    break; // don't retry 400s in the normal loop — handled below via repair
                }
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

    // REPAIR on 400: try to fix the request and retry primary once
    if body.is_none() {
        if let Some(ref error_text) = last_400_body {
            // Clone the messages array from the payload for repair
            let mut repaired_messages: Vec<serde_json::Value> = payload
                .get("messages")
                .and_then(|m| m.as_array())
                .cloned()
                .unwrap_or_default();

            if repair_request_for_400(&mut repaired_messages, error_text) {
                eprintln!("[call_claude] repaired request (stripped images), retrying primary once");
                let mut repaired_payload = payload.clone();
                repaired_payload["messages"] = serde_json::json!(repaired_messages);

                let mut req = agent
                    .post(&base_url)
                    .set("content-type", "application/json")
                    .set("x-api-key", &api_key)
                    .set("anthropic-version", &version);
                if !beta_values.is_empty() {
                    req = req.set("anthropic-beta", &beta_values.join(","));
                }
                match req.send_json(repaired_payload.clone()) {
                    Ok(resp) => {
                        body = Some(resp.into_string()?);
                        // Update payload for downstream fallbacks if needed
                        payload = repaired_payload;
                    }
                    Err(ureq::Error::Status(code, resp)) => {
                        let text = resp.into_string().unwrap_or_default();
                        eprintln!("[call_claude] repaired request also failed: {code} {text}");
                        // Update payload so Vertex/Sonnet use the repaired version
                        payload = repaired_payload;
                    }
                    Err(ureq::Error::Transport(err)) => {
                        eprintln!("[call_claude] repaired request transport error: {err}");
                        payload = repaired_payload;
                    }
                }
            }
        }
    }

    // Vertex proxy — same model, different endpoint (tried BEFORE Sonnet fallback)
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
                            eprintln!("[call_claude] Vertex fallback failed: {code} {text}");
                        } else {
                            let delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                            thread::sleep(Duration::from_secs_f64(delay));
                        }
                    }
                    Err(ureq::Error::Transport(err)) => {
                        if attempt == max_retries {
                            eprintln!("[call_claude] Vertex fallback transport error: {err}");
                        } else {
                            let delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                            thread::sleep(Duration::from_secs_f64(delay));
                        }
                    }
                }
            }
        }
    }

    // Sonnet fallback — last resort, different (cheaper/faster) model
    if body.is_none() {
        if let Ok(fallback_model) = std::env::var("ANTHROPIC_FALLBACK_MODEL") {
            eprintln!("All primary endpoints failed, trying Sonnet fallback: {fallback_model}");
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

    let body = body.ok_or("All API endpoints failed (Anthropic direct + Vertex + Sonnet fallback)")?;
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

    // Circuit breaker: skip critic after too many consecutive failures
    if CRITIC_CONSECUTIVE_FAILURES.load(Ordering::Relaxed) >= CRITIC_MAX_CONSECUTIVE_FAILURES {
        eprintln!("[critic] circuit breaker open — skipping for rest of session");
        return None;
    }

    let api_key = env_optional("CRITIC_API_KEY")
        .or_else(|| env_optional("ANTHROPIC_API_KEY"))?;
    let model = env_optional("CRITIC_MODEL")
        .unwrap_or_else(|| env_optional("SONNET_MODEL")
            .unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string()));
    let base_url = env_optional("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string());
    let timeout_ms: u64 = env_optional("CRITIC_TIMEOUT")
        .and_then(|v| v.parse().ok())
        .unwrap_or(15_000);
    let max_tokens: u64 = env_optional("CRITIC_MAX_TOKENS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);
    let context_turns: usize = env_optional("CRITIC_CONTEXT_TURNS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
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
        let preview: String = content.chars().take(2000).collect();
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
                CRITIC_CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        },
        Err(e) => {
            eprintln!("[critic] API error: {e}");
            CRITIC_CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[critic] JSON parse error: {e}");
            CRITIC_CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };

    // Extract text from the Anthropic response
    let content = parsed.get("content")?.as_array()?;
    let text = content
        .iter()
        .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .and_then(|b| b.get("text").and_then(|t| t.as_str()))?;

    // Parse the critic's JSON verdict using lenient extractor
    let verdict = match extract_critic_json(text) {
        Some(v) => v,
        None => {
            eprintln!("[critic] verdict parse error: could not extract JSON from response");
            CRITIC_CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };

    // Success — reset circuit breaker
    CRITIC_CONSECUTIVE_FAILURES.store(0, Ordering::Relaxed);

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

    let agent_claim = verdict
        .get("agent_claim")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let evidence_shows = verdict
        .get("evidence_shows")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    eprintln!(
        "[critic] grounded=false issues=[{issues}] claim=[{agent_claim}] evidence=[{evidence_shows}]"
    );

    verdict
        .get("correction")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

pub(crate) fn call_agent_hook(hook: &HookSpec, request: &AgentHookRequest) -> Result<AgentMessage, String> {
    let hook_cmd = match &hook.command {
        CommandSpec::String(cmd) => cmd.trim().to_ascii_lowercase(),
        CommandSpec::Array(items) => items
            .first()
            .map(|cmd| cmd.trim().to_ascii_lowercase())
            .unwrap_or_default(),
    };
    let is_builtin_claude = hook_cmd == "builtin:claude" || hook_cmd == "claude";
    let is_builtin_sonnet = hook_cmd == "builtin:sonnet" || hook_cmd == "sonnet";

    if is_builtin_claude || is_builtin_sonnet {
        // For builtin:sonnet, override model to Sonnet via env or hardcoded default
        let model_override = if is_builtin_sonnet {
            Some(std::env::var("SONNET_MODEL")
                .unwrap_or_else(|_| "claude-sonnet-4-5-20250929".to_string()))
        } else {
            None
        };

        // Retry once at this level for transient failures (covers the case where
        // all fallback endpoints failed due to a temporary network blip)
        let result = call_claude_with_model(request, model_override.as_deref());
        match result {
            Ok(resp) => return Ok(resp.message),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("transport") || err_str.contains("timed out") || err_str.contains("Network") {
                    eprintln!("[call_agent_hook] first attempt failed ({err_str}), retrying in 3s...");
                    thread::sleep(Duration::from_secs(3));
                    return call_claude_with_model(request, model_override.as_deref())
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
    let timeout = hook.timeout_ms.unwrap_or(u64::MAX); // No timeout — zombie detection handles stuck processes
    let value = serde_json::to_value(request).map_err(|e| format!("hook input: {e}"))?;

    let max_retries: usize = std::env::var("HOOK_MAX_RETRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);

    let mut last_err = String::new();
    for attempt in 0..=max_retries {
        if attempt > 0 {
            let delay = Duration::from_secs(3u64.pow(attempt as u32).min(30));
            eprintln!(
                "[call_agent_hook] attempt {}/{} failed ({last_err}), retrying in {delay:?}...",
                attempt,
                max_retries + 1
            );
            thread::sleep(delay);
        }
        match run_hook_command(&cmd, &value, timeout, "agent") {
            Ok(raw) => {
                match serde_json::from_str::<AgentHookResponse>(&raw) {
                    Ok(response) => return Ok(response.message),
                    Err(e) => {
                        // JSON parse failure = NOT retryable (hook ran but returned garbage)
                        return Err(format!(
                            "hook output parse error: {e}\nraw: {}",
                            &raw[..raw.len().min(200)]
                        ));
                    }
                }
            }
            Err(e) => {
                last_err = e.clone();
                if !is_hook_error_retryable(&e) {
                    return Err(format!("hook fatal error: {e}"));
                }
            }
        }
    }
    Err(format!(
        "External hook failed after {} attempts. Last error: {last_err}",
        max_retries + 1
    ))
}

fn is_hook_error_retryable(err: &str) -> bool {
    [
        "write stdin",
        "zombie",
        "hook exited",
        "spawn failed",
        "hook wait failed",
        "hook returned empty",
    ]
    .iter()
    .any(|p| err.contains(p))
}

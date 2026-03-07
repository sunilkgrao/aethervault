use std::io::{self, BufRead, BufReader, Read};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json;

use crate::{
    command_spec_to_vec, env_bool, env_f64, env_optional, env_required, env_u64, env_usize,
    extract_prompt_from_request, jitter_ratio, parse_retry_after, run_hook_command,
    run_claude_code_native, run_codex_native, run_pool_routed, should_hook_be_read_only,
    AgentHookRequest, AgentHookResponse, AgentMessage, AgentToolCall, ClaudeStreamEvent, CommandSpec, HookSpec,
};

const CRITIC_SYSTEM_PROMPT: &str = "\
You are a silent quality monitor inside an AI agent runtime. Check agent claims against conversation evidence.\n\n\
EVALUATION CRITERIA (check ALL):\n\
1. FABRICATION: Does the agent claim details (paths, config values, errors, ids, versions, boot steps) absent from any tool output?\n\
2. OVERCLAIMING: Does the agent say tools succeeded when they failed or returned errors?\n\
3. UNACKNOWLEDGED FAILURES: Did a tool fail (non-zero exit or error text) without the agent addressing it?\n\
4. SCOPE CREEP: Is the agent doing materially more than the user asked?\n\n\
SUBAGENT AWARENESS:\n\
subagent_invoke and subagent_batch are legitimate tools. Their output is evidence, and the agent is expected to report it. \
Only flag subagent claims if that output is empty, errored, or the claim differs from what the subagent returned.\n\n\
ACTIVE SELF-CORRECTION:\n\
If the agent admits an earlier error and is actively fixing it (e.g., rerunning a failed query with corrected parameters), \
treat that as GROUNDED, not a new violation. Only flag if it claims the retry succeeded without evidence.\n\n\
RESPONSE FORMAT — return ONLY this JSON:\n\
{\"grounded\": true/false, \"issues\": [\"specific issue with evidence quote\"], \
\"agent_claim\": \"what the agent claimed (quote)\", \
\"evidence_shows\": \"what the tool output actually says (quote)\", \
\"correction\": \"specific behavioral instruction\"}\n\n\
If grounded=true, issues/agent_claim/evidence_shows/correction may be empty arrays/strings.\n\
If grounded=false, you MUST include at least one issue with specific quotes.\n\
Return nothing outside this JSON.\n\n\
BROWSER TOOL AWARENESS:\n\
- navigate returning a title+URL is evidence the page loaded.\n\
- click/fill/type/select auto-snapshot: output includes action result + [AUTO-SNAPSHOT] accessibility tree. If present, agent may claim page state from it.\n\
- If auto-snapshot failed (HINT only), agent MUST call browser snapshot before claiming outcomes.\n\
- snapshot returns the full accessibility tree — reliable page-state evidence.\n\
- Do NOT flag browser calls. DO flag claims contradicting the snapshot.\n\
- Quoting snapshot elements is grounded.\n\n\
ENFORCEMENT:\n\
Subagent violations:\n\
- Correction MUST include: \"RETRACT your previous claim about subagent results.\"\n\
- Correction MUST instruct: \"Call session_status or check actual tool output before claiming subagent results.\"\n\
- If repeated: \"This is a REPEATED violation. You must NOT report subagent outcomes without a status-checking tool.\"\n\n\
Browser violations:\n\
- If auto-snapshot present and claim matches, that is GROUNDED.\n\
- If absent and agent claimed outcomes without manual snapshot: \"Call `browser snapshot` to verify page state before claiming.\"\n\
- Do NOT count navigate/snapshot as violations; only claims contradicting available evidence.";

// Critic circuit breaker: after N consecutive failures, skip critic for rest of session.
// Set high enough that long sessions (64+ steps) don't prematurely disable the critic.
static CRITIC_CONSECUTIVE_FAILURES: AtomicUsize = AtomicUsize::new(0);
const CRITIC_MAX_CONSECUTIVE_FAILURES: usize = 8;

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

pub(crate) fn extract_critic_json(text: &str) -> Option<serde_json::Value> {
    let clean = text.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(clean) {
        return Some(v);
    }
    if let Ok(object_re) = regex::Regex::new(r"\{[\s\S]*?\}") {
        for m in object_re.find_iter(clean) {
            let candidate = m.as_str();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(candidate) {
                return Some(v);
            }
            let fixed = candidate.replace(",}", "}").replace(",]", "]");
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&fixed) {
                eprintln!("[critic] JSON extracted via trailing comma fix");
                return Some(v);
            }
        }
    }
    if let Ok(verdict_re) = regex::Regex::new(r#""verdict"\s*:\s*"(pass|fail)""#) {
        if let Some(cap) = verdict_re.captures(clean) {
            let grounded = cap.get(1).is_some_and(|m| m.as_str() == "pass");
            eprintln!("[critic] verdict extracted via regex fallback (grounded={grounded})");
            return Some(serde_json::json!({"grounded": grounded, "issues": [], "agent_claim": "", "evidence_shows": "", "correction": ""}));
        }
    }

    // Regex fallback: extract grounded boolean from natural language
    let lower = clean.to_ascii_lowercase();
    let grounded_val = if lower.contains("\"grounded\": true") || lower.contains("\"grounded\":true") {
        Some(true)
    } else if lower.contains("\"grounded\": false") || lower.contains("\"grounded\":false") {
        Some(false)
    } else if lower.contains("is grounded") || lower.contains("appears grounded") {
        Some(true)
    } else if lower.contains("not grounded") {
        Some(false)
    } else {
        None
    };
    if let Some(g) = grounded_val {
        eprintln!("[critic] verdict extracted via regex fallback (grounded={g})");
        return Some(serde_json::json!({"grounded": g, "issues": [], "agent_claim": "", "evidence_shows": "", "correction": ""}));
    }
    let parse_score = |line: &str| line.split_once(':').and_then(|(_, raw)| raw.split('/').next()).and_then(|v| v.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<String>().parse::<f64>().ok());
    let (mut score, mut issues, mut suggestions, mut summary, mut section) = (None, Vec::<String>::new(), Vec::<String>::new(), String::new(), 0u8);
    for raw in clean.lines() {
        let line = raw.trim();
        if line.is_empty() { section = 0; continue; }
        let ll = line.to_ascii_lowercase();
        if ll.starts_with("score") || ll.starts_with("rating") { score = score.or_else(|| parse_score(line)); section = 0; continue; }
        if ll.starts_with("issues:") || ll.starts_with("problems:") { if let Some((_,rest))=line.split_once(':').filter(|(_,rest)| !rest.trim().is_empty()) {issues.push(rest.trim().to_string());}; section=1; continue; }
        if ll.starts_with("suggestions:") || ll.starts_with("recommendations:") { if let Some((_,rest))=line.split_once(':').filter(|(_,rest)| !rest.trim().is_empty()) {suggestions.push(rest.trim().to_string());}; section=2; continue; }
        if ll.starts_with("summary:") || ll.starts_with("overall:") { summary = line.split_once(':').map(|(_, rest)| rest.trim().to_string()).unwrap_or_default(); section = 3; continue; }
        let item = line.trim_start_matches(&['-', '*', '+', '•'][..]).trim();
        if section == 1 && !item.is_empty() { issues.push(item.to_string()); continue; }
        if section == 2 && !item.is_empty() { suggestions.push(item.to_string()); continue; }
        if section == 3 && !item.is_empty() {
            if !summary.is_empty() { summary.push(' '); }
            summary.push_str(item);
        } else { section = 0; }
    }
    if score.is_none() && issues.is_empty() && suggestions.is_empty() && summary.is_empty() {
        eprintln!("[critic] verdict parse error: could not extract JSON from response; defaulting to pass; raw response: {:?}", text);
        Some(serde_json::json!({"grounded": true, "issues": [], "agent_claim": "", "evidence_shows": "", "correction": ""}))
    } else {
        Some(serde_json::json!({"grounded": true, "issues": issues, "agent_claim": "", "evidence_shows": "", "correction": "", "score": score, "suggestions": suggestions, "summary": summary}))
    }
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
                    let mut cleaned = tb.clone();
                    if let Some(obj) = cleaned.as_object_mut() {
                        obj.remove("cache_control");
                    }
                    blocks.push(cleaned);
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
        out.push(serde_json::Value::Object(entry));
    }
    // Anthropic allows max 4 cache_control blocks; apply only to the last tool
    if let Some(cache) = cache_control {
        if let Some(last) = out.last_mut().and_then(|v| v.as_object_mut()) {
            last.insert("cache_control".to_string(), cache);
        }
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
                let mut cleaned = block.clone();
                if let Some(obj) = cleaned.as_object_mut() {
                    obj.remove("cache_control");
                }
                thinking_blocks.push(cleaned);
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
    let timeout = env_u64("ANTHROPIC_TIMEOUT", 180)?; // 3 min default, was infinite
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
    // Disable thinking when using a model override (e.g. Sonnet for compaction) —
    // adaptive thinking is Opus-only and will cause 400 errors on other models.
    let thinking_enabled = thinking_mode == "adaptive" && model_override.is_none();
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
                    if let Some(obj) = block.as_object_mut() {
                        obj.insert("cache_control".to_string(), cache.clone());
                    }
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
                        eprintln!("[call_claude] Vertex fallback failed: {code} {text}");
                        if code == 400 {
                            break; // 400 = bad request (prompt too long, etc.) — don't retry
                        }
                        if attempt < max_retries {
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

            // Strip thinking/output_config — Sonnet doesn't support adaptive thinking
            if let Some(obj) = payload.as_object_mut() {
                obj.remove("thinking");
                obj.remove("output_config");
            }
            // Re-add temperature/top_p now that thinking is disabled
            if let Some(temp) = temperature {
                payload["temperature"] = serde_json::json!(temp);
            }
            if let Some(p) = top_p {
                payload["top_p"] = serde_json::json!(p);
            }

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

    // NOTE: Critic was previously suppressed in autonomous sessions via
    // CRITIC_SUPPRESS_AUTONOMOUS env var. This was removed because autonomous
    // sessions (idle work, proactive tasks) are where the agent struggles most —
    // the critic needs to be active there to catch grounding violations,
    // brute-force patterns, and unacknowledged tool failures. The ~$0.01/call
    // cost is negligible compared to the wasted compute from unmonitored agents.

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
            eprintln!("[critic] JSON parse error: {e}; raw response: {:?}", body);
            CRITIC_CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };

    // Extract text from the Anthropic response
    let content = match parsed.get("content").and_then(|v| v.as_array()) {
        Some(v) => v,
        None => {
            eprintln!("[critic] response parse failed: missing/invalid content array in critic response: {:?}", body);
            CRITIC_CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    let text = match content
        .iter()
        .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .and_then(|b| b.get("text").and_then(|t| t.as_str()))
    {
        Some(v) => v,
        None => {
            eprintln!("[critic] response parse failed: missing text block in critic response: {:?}", body);
            CRITIC_CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };

    // Parse the critic's JSON verdict using lenient extractor
    let verdict = match extract_critic_json(text) {
        Some(v) => v,
        None => {
            eprintln!("[critic] verdict parse error: could not extract JSON from response; defaulting to pass; raw response: {:?}", text);
            serde_json::json!({"grounded": true, "issues": [], "agent_claim": "", "evidence_shows": "", "correction": ""})
        }
    };

    // Success — reset circuit breaker
    CRITIC_CONSECUTIVE_FAILURES.store(0, Ordering::Relaxed);

    let grounded = verdict
        .get("grounded")
        .and_then(|v| v.as_bool())
        .or_else(|| {
            verdict
                .get("verdict")
                .and_then(|v| v.as_str())
                .and_then(|v| Some(v.eq_ignore_ascii_case("pass")))
        })
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

    // Native pool routing: builtin:pool, builtin:codex, builtin:claude-code
    let is_builtin_pool = hook_cmd == "builtin:pool" || hook_cmd == "pool";
    let is_builtin_codex = hook_cmd == "builtin:codex";
    let is_builtin_claude_code = hook_cmd == "builtin:claude-code";

    if is_builtin_pool {
        let read_only = should_hook_be_read_only(request);
        return run_pool_routed(request, read_only);
    }
    if is_builtin_codex {
        let read_only = should_hook_be_read_only(request);
        return run_codex_native(&extract_prompt_from_request(request), read_only);
    }
    if is_builtin_claude_code {
        let read_only = should_hook_be_read_only(request);
        return run_claude_code_native(&extract_prompt_from_request(request), read_only);
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

// === Streaming API ===

/// Send a streaming request to the Claude API and return a receiver of stream events.
/// Fails fast — no retries. Caller should fall back to blocking `call_claude` on error.
pub(crate) fn call_claude_streaming(
    request: &AgentHookRequest,
) -> Result<mpsc::Receiver<ClaudeStreamEvent>, String> {
    let api_key = env_required("ANTHROPIC_API_KEY").map_err(|e| e.to_string())?;
    let model = env_required("ANTHROPIC_MODEL").map_err(|e| e.to_string())?;
    let base_url = env_optional("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string());
    let max_tokens = env_u64("ANTHROPIC_MAX_TOKENS", 8192).map_err(|e| e.to_string())?;
    let timeout = env_u64("ANTHROPIC_TIMEOUT", 180).map_err(|e| e.to_string())?;
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

    let thinking_mode = env_optional("ANTHROPIC_THINKING").unwrap_or_default();
    let thinking_enabled = thinking_mode == "adaptive";
    let thinking_effort = env_optional("ANTHROPIC_THINKING_EFFORT")
        .unwrap_or_else(|| "high".to_string());

    let effective_max_tokens = if thinking_enabled {
        max_tokens.max(16384)
    } else {
        max_tokens
    };

    let mut payload = serde_json::json!({
        "model": model,
        "max_tokens": effective_max_tokens,
        "messages": to_anthropic_messages(&request.messages),
        "stream": true,
    });

    if thinking_enabled {
        payload["thinking"] = serde_json::json!({ "type": "adaptive" });
        payload["output_config"] = serde_json::json!({ "effort": thinking_effort });
    }

    if !system_blocks.is_empty() {
        if let Some(cache) = cache_control.clone() {
            let blocks: Vec<serde_json::Value> = system_blocks.iter().enumerate().map(|(i, text)| {
                let mut block = serde_json::json!({"type": "text", "text": text});
                if i == 0 {
                    if let Some(obj) = block.as_object_mut() {
                        obj.insert("cache_control".to_string(), cache.clone());
                    }
                }
                block
            }).collect();
            payload["system"] = serde_json::json!(blocks);
        } else {
            payload["system"] = serde_json::json!(system_blocks.join("\n\n"));
        }
    }
    let tools = to_anthropic_tools(&request.tools, cache_control);
    if !tools.is_empty() {
        payload["tools"] = serde_json::json!(tools);
    }

    // Streaming reads need a generous but finite timeout. Using 0 (infinite)
    // caused API calls to hang forever when the server stalled mid-stream.
    // We use the configured timeout (min 300s) so long-running responses can
    // complete, but a genuinely stalled connection will eventually time out.
    let stream_read_timeout = std::cmp::max(timeout, 300);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(timeout))
        .timeout_read(Duration::from_secs(stream_read_timeout))
        .timeout_write(Duration::from_secs(timeout))
        .build();

    let mut req = agent
        .post(&base_url)
        .set("content-type", "application/json")
        .set("x-api-key", &api_key)
        .set("anthropic-version", &version);
    if !beta_values.is_empty() {
        req = req.set("anthropic-beta", &beta_values.join(","));
    }

    let response = req.send_json(payload).map_err(|e| format!("streaming request failed: {e}"))?;

    let reader = response.into_reader();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        parse_sse_stream(reader, tx);
    });

    Ok(rx)
}

/// Parse an SSE stream from the Claude API and send events over the channel.
fn parse_sse_stream(reader: Box<dyn Read + Send>, tx: mpsc::Sender<ClaudeStreamEvent>) {
    let buf_reader = BufReader::new(reader);
    let mut current_event_type = String::new();
    let mut data_buf = String::new();

    for line_result in buf_reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                let _ = tx.send(ClaudeStreamEvent::Error(format!("read error: {e}")));
                return;
            }
        };

        if line.is_empty() {
            // End of event — process accumulated data
            if !data_buf.is_empty() && !current_event_type.is_empty() {
                if let Some(evt) = parse_sse_event(&current_event_type, &data_buf) {
                    let is_stop = matches!(evt, ClaudeStreamEvent::MessageStop);
                    if tx.send(evt).is_err() {
                        return; // receiver dropped
                    }
                    if is_stop {
                        return;
                    }
                }
            }
            current_event_type.clear();
            data_buf.clear();
            continue;
        }

        if let Some(event_type) = line.strip_prefix("event: ") {
            current_event_type = event_type.to_string();
        } else if let Some(data) = line.strip_prefix("data: ") {
            if !data_buf.is_empty() {
                data_buf.push('\n');
            }
            data_buf.push_str(data);
        }
    }
    // Stream ended without MessageStop — send it anyway
    let _ = tx.send(ClaudeStreamEvent::MessageStop);
}

/// Map an SSE event type + JSON data to a ClaudeStreamEvent.
fn parse_sse_event(event_type: &str, data: &str) -> Option<ClaudeStreamEvent> {
    let json: serde_json::Value = serde_json::from_str(data).ok()?;

    match event_type {
        "content_block_start" => {
            let index = json.get("index")?.as_u64()? as usize;
            let block = json.get("content_block")?;
            let block_type = block.get("type")?.as_str()?.to_string();
            let tool_id = block.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
            let tool_name = block.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
            Some(ClaudeStreamEvent::BlockStart { index, block_type, tool_id, tool_name })
        }
        "content_block_delta" => {
            let index = json.get("index")?.as_u64()? as usize;
            let delta = json.get("delta")?;
            let delta_type = delta.get("type")?.as_str()?.to_string();
            // Text can come from "text" (text blocks), "thinking" (thinking blocks),
            // or "partial_json" (tool_use input deltas)
            let text = delta.get("text")
                .or_else(|| delta.get("thinking"))
                .or_else(|| delta.get("partial_json"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(ClaudeStreamEvent::BlockDelta { index, delta_type, text })
        }
        "content_block_stop" => {
            let index = json.get("index")?.as_u64()? as usize;
            Some(ClaudeStreamEvent::BlockStop { index })
        }
        "message_delta" => {
            let delta = json.get("delta")?;
            let stop_reason = delta.get("stop_reason").and_then(|v| v.as_str()).map(|s| s.to_string());
            Some(ClaudeStreamEvent::MessageDelta { stop_reason })
        }
        "message_stop" => Some(ClaudeStreamEvent::MessageStop),
        "error" => {
            let err = json.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown stream error")
                .to_string();
            Some(ClaudeStreamEvent::Error(err))
        }
        _ => None, // message_start, ping, etc. — ignored
    }
}

/// Try to get a streaming channel for builtin:claude hooks.
/// Returns Err for non-claude hooks (codex, pool, external) so caller falls back to blocking.
pub(crate) fn call_agent_hook_streaming(
    hook: &HookSpec,
    request: &AgentHookRequest,
) -> Result<mpsc::Receiver<ClaudeStreamEvent>, String> {
    let hook_cmd = match &hook.command {
        CommandSpec::String(cmd) => cmd.trim().to_ascii_lowercase(),
        CommandSpec::Array(items) => items
            .first()
            .map(|cmd| cmd.trim().to_ascii_lowercase())
            .unwrap_or_default(),
    };
    let is_builtin_claude = hook_cmd == "builtin:claude" || hook_cmd == "claude";
    if !is_builtin_claude {
        return Err("streaming only supported for builtin:claude".to_string());
    }
    call_claude_streaming(request)
}

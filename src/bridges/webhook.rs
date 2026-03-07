use std::io::Read;

use serde_json;
use tiny_http::{Method, Response, Server};

use std::env;
use std::io;

use crate::bridges::run_agent_for_bridge;
use crate::{BridgeAgentConfig, blake3_hash, try_handle_approval_chat};

const WEBHOOK_MAX_BODY_BYTES_ENV: &str = "AETHERVAULT_WEBHOOK_MAX_BODY_BYTES";
const DEFAULT_WEBHOOK_MAX_BODY_BYTES: usize = 1024 * 1024;

fn parse_webhook_max_body_bytes() -> usize {
    env::var(WEBHOOK_MAX_BODY_BYTES_ENV)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_WEBHOOK_MAX_BODY_BYTES)
}

fn read_request_body(request: &mut tiny_http::Request) -> Result<String, String> {
    let max_bytes = parse_webhook_max_body_bytes();

    if let Some(content_length) = request.body_length() {
        if content_length > max_bytes {
            return Err("payload too large".to_string());
        }
    }

    let mut body = String::new();
    request
        .as_reader()
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_string(&mut body)
        .map_err(|e| format!("read body: {e}"))?;

    if body.len() > max_bytes {
        return Err("payload too large".to_string());
    }

    Ok(body)
}

pub(crate) fn parse_json_body(
    request: &mut tiny_http::Request,
) -> Result<serde_json::Value, String> {
    let body = read_request_body(request)?;

    if body.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }

    serde_json::from_str(&body).map_err(|e| format!("json: {e}"))
}

pub(crate) fn run_webhook_bridge(
    name: &str,
    bind: String,
    port: u16,
    agent_config: BridgeAgentConfig,
    extract_event: fn(&serde_json::Value) -> Option<(String, String)>,
    reply: fn(&BridgeAgentConfig, &str) -> Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{bind}:{port}");
    let server = Server::http(&addr)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("server: {e}")))?;
    eprintln!("{name} bridge listening on http://{addr}");

    for mut request in server.incoming_requests() {
        if *request.method() != Method::Post {
            let response = Response::from_string("ok");
            let _ = request.respond(response);
            continue;
        }
        let payload = match parse_json_body(&mut request) {
            Ok(payload) => payload,
            Err(err) => {
                if err == "payload too large" {
                    let response = Response::from_string("payload too large").with_status_code(413);
                    let _ = request.respond(response);
                    continue;
                }

                eprintln!("{name} bridge malformed JSON payload: {err}");
                let response =
                    Response::from_string("bad request: malformed JSON body").with_status_code(400);
                let _ = request.respond(response);
                continue;
            }
        };
        if let Some(challenge) = payload.get("challenge").and_then(|v| v.as_str()) {
            let response = Response::from_string(challenge.to_string());
            let _ = request.respond(response);
            continue;
        }
        let Some((session_key, text)) = extract_event(&payload) else {
            let response = Response::from_string("ok");
            let _ = request.respond(response);
            continue;
        };
        if let Some(output) = try_handle_approval_chat(&agent_config.db_path, &text) {
            if let Some(response_text) = reply(&agent_config, &output) {
                let response = Response::from_string(response_text);
                let _ = request.respond(response);
            } else {
                let response = Response::from_string("ok");
                let _ = request.respond(response);
            }
            continue;
        }
        let session = format!("{}{}", agent_config.session_prefix, session_key);
        let result = run_agent_for_bridge(&agent_config, &text, session, None, None, None);
        let output = match result {
            Ok(output) => output.final_text.unwrap_or_else(|| "\u{2705}".to_string()),
            Err(err) => format!("Agent error: {err}"),
        };
        if let Some(response_text) = reply(&agent_config, &output) {
            let response = Response::from_string(response_text);
            let _ = request.respond(response);
        } else {
            let response = Response::from_string("ok");
            let _ = request.respond(response);
        }
    }
    Ok(())
}

pub(crate) fn payload_session_fallback(prefix: &str, payload: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    format!("{prefix}:{}", blake3_hash(&bytes).to_hex())
}

pub(crate) fn extract_discord_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload.get("content").and_then(|v| v.as_str())?.to_string();
    let channel = payload
        .get("channel_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let user = payload
        .get("author")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session = if channel != "unknown" || user != "unknown" {
        format!("discord:{channel}:{user}")
    } else {
        payload_session_fallback("discord", payload)
    };
    Some((session, text))
}

pub(crate) fn extract_teams_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            payload
                .get("body")
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })?;
    let convo = payload
        .get("conversation")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let from = payload
        .get("from")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session = if convo != "unknown" || from != "unknown" {
        format!("teams:{convo}:{from}")
    } else {
        payload_session_fallback("teams", payload)
    };
    Some((session, text))
}

pub(crate) fn extract_signal_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload.get("text").and_then(|v| v.as_str())?.to_string();
    let source = payload
        .get("source")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("from").and_then(|v| v.as_str()))
        .unwrap_or("unknown");
    let session = if source != "unknown" {
        format!("signal:{source}")
    } else {
        payload_session_fallback("signal", payload)
    };
    Some((session, text))
}

pub(crate) fn extract_matrix_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload.get("text").and_then(|v| v.as_str())?.to_string();
    let room = payload
        .get("room_id")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("room").and_then(|v| v.as_str()))
        .unwrap_or("unknown");
    let sender = payload
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session = if room != "unknown" || sender != "unknown" {
        format!("matrix:{room}:{sender}")
    } else {
        payload_session_fallback("matrix", payload)
    };
    Some((session, text))
}

pub(crate) fn extract_imessage_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload.get("text").and_then(|v| v.as_str())?.to_string();
    let from = payload
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session = if from != "unknown" {
        format!("imessage:{from}")
    } else {
        payload_session_fallback("imessage", payload)
    };
    Some((session, text))
}

pub(crate) fn reply_none(_: &BridgeAgentConfig, _: &str) -> Option<String> {
    None
}

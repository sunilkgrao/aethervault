#[allow(unused_imports)]
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::{mpsc, Arc};
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use reqwest::blocking::{multipart, Client};
use serde_json;
use tungstenite::{connect, Message};

use crate::{
    load_session_turns, run_agent_with_prompt, save_session_turns, try_handle_approval_chat,
    AgentRunOutput, BridgeAgentConfig, SessionTurn,
};

const DEFAULT_HTTP_TIMEOUT_MS: u64 = 120_000;
const SLACK_API_BASE: &str = "https://slack.com/api";
const VOICEBOX_API: &str = "http://raoDesktop:8000/generate";
const MAX_QUEUED_PER_SESSION: usize = 5;
const MAX_FILE_BYTES: u64 = 25_000_000;
const MAX_TEXT_CHUNK_CHARS: usize = 3900;

#[derive(Debug)]
struct SlackIncomingEvent {
    session_key: String,
    channel_id: String,
    thread_ts: Option<String>,
    text: String,
}

#[derive()]
struct SlackCompletionEvent {
    session_key: String,
    channel_id: String,
    thread_ts: Option<String>,
    result: Result<AgentRunOutput, String>,
}

#[derive(Debug)]
struct SlackRunState {
    queued_messages: Vec<(String, Option<String>)>,
}

#[derive(Debug)]
enum SocketFrame {
    Event {
        envelope_id: Option<String>,
        payload: serde_json::Value,
    },
    Disconnected(String),
}

#[derive(Debug)]
struct VoiceDirective {
    text: String,
    profile_id: String,
    instruct: String,
}

fn split_text_chunks(text: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;

    for ch in text.chars() {
        if count >= max_chars {
            chunks.push(current);
            current = String::new();
            count = 0;
        }
        current.push(ch);
        count += 1;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

fn is_duplicate_event(seen: &mut VecDeque<String>, event_id: &str) -> bool {
    if seen.iter().any(|item| item == event_id) {
        return true;
    }
    seen.push_back(event_id.to_string());
    while seen.len() > 512 {
        let _ = seen.pop_front();
    }
    false
}

fn append_session_turn(session_key: &str, role: &str, text: &str, config: &BridgeAgentConfig) {
    let session = format!("{}slack:{session_key}", config.session_prefix);
    let mut turns = load_session_turns(&session, 8);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    turns.push(SessionTurn {
        role: role.to_string(),
        content: text.to_string(),
        timestamp: now,
    });
    save_session_turns(&session, &turns, 8);
}

fn slack_api_post_json(
    http_agent: &ureq::Agent,
    auth: &str,
    method: &str,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let response = http_agent
        .post(&format!("{SLACK_API_BASE}/{method}"))
        .set("Authorization", &format!("Bearer {auth}"))
        .set("Content-Type", "application/json")
        .send_json(payload)
        .map_err(|e| format!("{method} request error: {e}"))?
        .into_json::<serde_json::Value>()
        .map_err(|e| format!("{method} decode error: {e}"))?;

    if response.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let err = response
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return Err(format!("{method} error: {err}"));
    }

    Ok(response)
}

fn fetch_slack_bot_user_id(http_agent: &ureq::Agent, bot_token: &str) -> Option<String> {
    slack_api_post_json(http_agent, bot_token, "auth.test", &serde_json::json!({}))
        .ok()
        .and_then(|payload| payload.get("user_id").and_then(|v| v.as_str()).map(ToString::to_string))
}

fn open_slack_socket_url(http_agent: &ureq::Agent, app_token: &str) -> Result<String, String> {
    let response = slack_api_post_json(
        http_agent,
        app_token,
        "apps.connections.open",
        &serde_json::json!({}),
    )?;

    response
        .get("url")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .ok_or_else(|| "missing websocket url in apps.connections.open response".to_string())
}

fn normalize_slack_payload(raw: &serde_json::Value) -> Option<serde_json::Value> {
    let payload = raw.get("payload").unwrap_or(raw);
    if let Some(payload_str) = payload.as_str() {
        serde_json::from_str::<serde_json::Value>(payload_str).ok()
    } else {
        if payload.is_object() {
            Some(payload.clone())
        } else {
            None
        }
    }
}

fn is_slack_dm_channel(channel_id: &str, channel_type: Option<&str>) -> bool {
    channel_type == Some("im")
        || channel_type == Some("mpim")
        || channel_id.starts_with('D')
}

fn bot_mention_present(text: &str, bot_user_id: &Option<String>) -> bool {
    bot_user_id
        .as_ref()
        .is_some_and(|bot_id| text.contains(&format!("<@{bot_id}>")) || text.contains(&format!("<@{bot_id}|")))
}

fn strip_bot_mentions(text: &str, bot_user_id: &Option<String>) -> String {
    let Some(bot_user_id) = bot_user_id.as_ref() else {
        return text.trim().to_string();
    };

    let mut clean = text.to_string();
    clean = clean.replace(&format!("<@{}>", bot_user_id), "");

    let mention_start = format!("<@{}|", bot_user_id);
    loop {
        let Some(start) = clean.find(&mention_start) else {
            break;
        };
        let remainder = &clean[start + mention_start.len()..];
        let Some(close) = remainder.find('>') else {
            break;
        };
        let end = start + mention_start.len() + close + 1;
        clean.replace_range(start..end, "");
    }

    clean.trim().to_string()
}

fn guess_media_type(file_mime: &str, file_name: &str) -> String {
    if file_mime.starts_with("image/") {
        return file_mime.to_string();
    }
    if file_name.ends_with(".jpg") || file_name.ends_with(".jpeg") {
        return "image/jpeg".to_string();
    }
    if file_name.ends_with(".png") {
        return "image/png".to_string();
    }
    if file_name.ends_with(".webp") {
        return "image/webp".to_string();
    }
    if file_name.ends_with(".gif") {
        return "image/gif".to_string();
    }
    "application/octet-stream".to_string()
}

fn is_image_file(file_mime: &str, file_name: &str) -> bool {
    if file_mime.starts_with("image/") {
        return true;
    }
    matches!(
        file_name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .unwrap_or_default()
            .as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "avif"
    )
}

fn is_text_file(file_mime: &str, file_name: &str) -> bool {
    if file_mime.starts_with("text/") {
        return true;
    }
    matches!(
        file_name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .unwrap_or_default()
            .as_str(),
        "txt" | "md" | "json" | "toml" | "yaml" | "yml" | "csv" | "rs" | "py" | "js" | "ts" | "log"
    )
}

fn slack_download_file_bytes(
    http_agent: &ureq::Agent,
    bot_token: &str,
    url: &str,
) -> Option<(Vec<u8>, String)> {
    let response = http_agent
        .get(url)
        .set("Authorization", &format!("Bearer {bot_token}"))
        .call()
        .ok()?;

    let content_type = response
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();

    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_FILE_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;

    if bytes.is_empty() {
        return None;
    }

    Some((bytes, content_type))
}

fn summarize_text_file(bytes: &[u8], max_chars: usize) -> Option<String> {
    let text = String::from_utf8(bytes.to_vec()).ok()?;
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return None;
    }

    let char_count = trimmed.chars().count();
    let preview: String = trimmed.chars().take(max_chars).collect();
    if char_count > max_chars {
        Some(format!("{preview}\n... (truncated, {} total chars)", char_count))
    } else {
        Some(preview)
    }
}

fn extract_slack_file_context(
    files: Option<&serde_json::Value>,
    http_agent: &ureq::Agent,
    bot_token: &str,
) -> Vec<String> {
    let mut notes = Vec::new();
    let Some(list) = files.and_then(|v| v.as_array()) else {
        return notes;
    };

    for file in list {
        let name = file
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("file")
            .to_string();
        let mime = file
            .get("mimetype")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream")
            .to_string();
        let file_url = file
            .get("url_private_download")
            .or_else(|| file.get("url_private"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if file_url.is_empty() {
            notes.push(format!("[Slack file: {name} ({mime})]"));
            continue;
        }

        let Some((bytes, remote_mime)) = slack_download_file_bytes(http_agent, bot_token, file_url) else {
            notes.push(format!("[Slack file: {name} ({mime}) download failed]"));
            continue;
        };

        if is_image_file(&mime, &name) || is_image_file(&remote_mime, &name) {
            let media_type = guess_media_type(&mime, &name);
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            notes.push(format!("[AV_IMAGE:{media_type}:{encoded}]"));
            continue;
        }

        if is_text_file(&mime, &name) || is_text_file(&remote_mime, &name) {
            if let Some(preview) = summarize_text_file(&bytes, 16_000) {
                notes.push(format!("[Slack text file: {name}]\n```")
                    .chars()
                    .chain(preview.chars())
                    .chain("\n```".chars())
                    .collect());
                continue;
            }
        }

        notes.push(format!("[Slack file: {name} ({mime}), {} bytes]", bytes.len()));
    }

    notes
}

fn parse_message_event(
    event: &serde_json::Value,
    bot_user_id: &Option<String>,
    allow_without_mention: bool,
    http_agent: &ureq::Agent,
    bot_token: &str,
) -> Option<SlackIncomingEvent> {
    if event.get("bot_id").is_some() {
        return None;
    }

    if let Some(subtype) = event.get("subtype").and_then(|v| v.as_str()) {
        match subtype {
            "bot_message" | "message_changed" | "message_deleted" => return None,
            _ => {}
        }
    }

    let user_id = event.get("user").and_then(|v| v.as_str())?.to_string();
    let channel_id = event.get("channel").and_then(|v| v.as_str())?.to_string();
    let raw_text = event.get("text").and_then(|v| v.as_str()).unwrap_or("");

    let channel_type = event.get("channel_type").and_then(|v| v.as_str());
    let is_direct = is_slack_dm_channel(&channel_id, channel_type);
    let has_bot_mention = bot_mention_present(raw_text, bot_user_id);
    if !is_direct && !allow_without_mention && !has_bot_mention {
        return None;
    }

    let text = strip_bot_mentions(raw_text, bot_user_id);

    let mut parts = Vec::new();
    if !text.trim().is_empty() {
        parts.push(text);
    }

    let file_notes = extract_slack_file_context(event.get("files"), http_agent, bot_token);
    for note in file_notes {
        parts.push(note);
    }

    if parts.is_empty() {
        return None;
    }

    let thread_ts = event
        .get("thread_ts")
        .and_then(|v| v.as_str())
        .or_else(|| event.get("ts").and_then(|v| v.as_str()))
        .map(ToString::to_string);
    let thread_key = event
        .get("thread_ts")
        .and_then(|v| v.as_str())
        .unwrap_or("main")
        .to_string();

    Some(SlackIncomingEvent {
        session_key: format!("{user_id}:{channel_id}:{thread_key}"),
        channel_id,
        thread_ts,
        text: parts.join("\n\n"),
    })
}

fn parse_events_api_payload(
    payload: &serde_json::Value,
    bot_user_id: &Option<String>,
    http_agent: &ureq::Agent,
    bot_token: &str,
) -> Option<SlackIncomingEvent> {
    let event = payload.get("event")?;
    match event.get("type").and_then(|v| v.as_str())? {
        "message" => parse_message_event(event, bot_user_id, false, http_agent, bot_token),
        "app_mention" => parse_message_event(event, bot_user_id, true, http_agent, bot_token),
        _ => None,
    }
}

fn parse_slash_command_payload(
    payload: &serde_json::Value,
) -> Option<SlackIncomingEvent> {
    let command = payload.get("command").and_then(|v| v.as_str())?.trim();
    let channel_id = payload
        .get("channel_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let user_id = payload
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let arg_text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut text = String::new();
    if !command.is_empty() {
        text.push_str(command);
    }
    if !arg_text.trim().is_empty() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(arg_text);
    }

    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }

    let thread_key = payload
        .get("thread_ts")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("message_ts").and_then(|v| v.as_str()))
        .unwrap_or("main")
        .to_string();

    Some(SlackIncomingEvent {
        session_key: format!("{user_id}:{channel_id}:{thread_key}"),
        channel_id,
        thread_ts: payload
            .get("thread_ts")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        text,
    })
}

fn parse_slack_incoming(
    raw: serde_json::Value,
    bot_user_id: &Option<String>,
    http_agent: &ureq::Agent,
    bot_token: &str,
) -> Option<SlackIncomingEvent> {
    let wrapper_type = raw.get("type").and_then(|v| v.as_str());
    let payload = normalize_slack_payload(&raw)?;

    match wrapper_type {
        Some("events_api") | Some("event_callback") => {
            parse_events_api_payload(&payload, bot_user_id, http_agent, bot_token)
        }
        Some("slash_commands") => parse_slash_command_payload(&payload),
        _ => {
            if payload.get("event").is_some() {
                parse_events_api_payload(&payload, bot_user_id, http_agent, bot_token)
            } else if payload.get("command").is_some() {
                parse_slash_command_payload(&payload)
            } else {
                None
            }
        }
    }
}

fn send_slack_message(
    http_agent: &ureq::Agent,
    bot_token: &str,
    channel_id: &str,
    thread_ts: Option<&str>,
    text: &str,
) -> Result<(), String> {
    let message = text.trim();
    if message.is_empty() {
        return Ok(());
    }

    for chunk in split_text_chunks(message, MAX_TEXT_CHUNK_CHARS) {
        let mut payload = serde_json::json!({
            "channel": channel_id,
            "text": chunk,
        });
        if let Some(ts) = thread_ts {
            payload["thread_ts"] = serde_json::json!(ts);
        }

        slack_api_post_json(http_agent, bot_token, "chat.postMessage", &payload)?;
    }

    Ok(())
}

fn parse_voice_directive(output: &str) -> (Option<VoiceDirective>, Option<String>) {
    let trimmed = output.trim();
    if !trimmed.starts_with("[VOICE:") {
        return (None, None);
    }

    let close_idx = match trimmed.find(']') {
        Some(idx) => idx,
        None => return (None, Some(trimmed.to_string())),
    };

    let body = trimmed[7..close_idx].trim();
    let remainder = trimmed[close_idx + 1..].trim().to_string();
    let remainder = (!remainder.is_empty()).then_some(remainder);

    let mut directive = VoiceDirective {
        text: String::new(),
        profile_id: "default".to_string(),
        instruct: String::new(),
    };

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(text) = value
            .get("text")
            .or_else(|| value.get("message"))
            .and_then(|v| v.as_str())
        {
            directive.text = text.trim().to_string();
        }
        if let Some(profile_id) = value.get("profile_id").and_then(|v| v.as_str()) {
            directive.profile_id = profile_id.to_string();
        }
        if let Some(instruct) = value.get("instruct").and_then(|v| v.as_str()) {
            directive.instruct = instruct.to_string();
        }
    } else {
        for part in body.split(|c| c == ';' || c == '|') {
            let kv = part.trim();
            if let Some((key, value)) = kv.split_once('=') {
                let key = key.trim().to_ascii_lowercase();
                let value = value.trim();
                match key.as_str() {
                    "text" => directive.text = value.to_string(),
                    "profile_id" => directive.profile_id = value.to_string(),
                    "instruct" => directive.instruct = value.to_string(),
                    _ => {}
                }
            } else if !kv.is_empty() && directive.text.is_empty() {
                directive.text = kv.to_string();
            }
        }
    }

    if directive.text.trim().is_empty() {
        return (None, Some(trimmed.to_string()));
    }

    directive.text = directive.text.trim().to_string();
    directive.profile_id = directive.profile_id.trim().to_string();
    directive.instruct = directive.instruct.trim().to_string();

    (Some(directive), remainder)
}

fn generate_voice_audio(http_agent: &ureq::Agent, directive: &VoiceDirective) -> Result<Vec<u8>, String> {
    let payload = serde_json::json!({
        "text": directive.text,
        "profile_id": directive.profile_id,
        "instruct": directive.instruct,
    });

    let response = http_agent
        .post(VOICEBOX_API)
        .set("Content-Type", "application/json")
        .send_json(payload)
        .map_err(|e| format!("voicebox request error: {e}"))?;

    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("voicebox read error: {e}"))?;

    if bytes.is_empty() {
        return Err("voicebox returned empty audio".to_string());
    }

    if let Ok(text) = String::from_utf8(bytes.clone()) {
        if text.trim_start().starts_with('{') {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(encoded) = value.get("audio").and_then(|v| v.as_str()) {
                    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) {
                        if !decoded.is_empty() {
                            return Ok(decoded);
                        }
                    }
                }

                if let Some(download_url) = value
                    .get("url")
                    .or_else(|| value.get("audio_url"))
                    .and_then(|v| v.as_str())
                {
                    let follow = http_agent
                        .get(download_url)
                        .call()
                        .map_err(|e| format!("voicebox follow-up download error: {e}"))?;
                    let mut wav = Vec::new();
                    follow
                        .into_reader()
                        .read_to_end(&mut wav)
                        .map_err(|e| format!("voicebox follow-up read error: {e}"))?;
                    if !wav.is_empty() {
                        return Ok(wav);
                    }
                }
            }
        }
    }

    Ok(bytes)
}

fn upload_voice_note(
    upload_client: &Client,
    bot_token: &str,
    channel_id: &str,
    thread_ts: Option<&str>,
    audio: &[u8],
    initial_comment: &str,
) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let filename = format!("voice-{now}.wav");

    let mut form = multipart::Form::new()
        .text("channels", channel_id.to_string())
        .part(
            "file",
            multipart::Part::bytes(audio.to_vec())
                .file_name(filename)
                .mime_str("audio/wav")
                .map_err(|e| format!("voice upload prepare error: {e}"))?,
        );

    if let Some(ts) = thread_ts {
        form = form.text("thread_ts", ts.to_string());
    }
    if !initial_comment.trim().is_empty() {
        form = form.text("initial_comment", initial_comment.to_string());
    }

    let response = upload_client
        .post(format!("{SLACK_API_BASE}/files.upload"))
        .bearer_auth(bot_token)
        .multipart(form)
        .send()
        .map_err(|e| format!("files.upload request error: {e}"))?;

    let result: serde_json::Value = response
        .json()
        .map_err(|e| format!("files.upload decode error: {e}"))?;

    if result.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let err = result
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return Err(format!("files.upload error: {err}"));
    }

    Ok(())
}

fn spawn_slack_run(
    config: &Arc<BridgeAgentConfig>,
    completion_tx: mpsc::Sender<SlackCompletionEvent>,
    session_key: String,
    channel_id: String,
    thread_ts: Option<String>,
    text: String,
) {
    let config = Arc::clone(config);
    thread::spawn(move || {
        let session = format!("{}slack:{session_key}", config.session_prefix);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_agent_with_prompt(
                config.mv2.clone(),
                text,
                Some(session),
                config.model_hook.clone(),
                config.system.clone(),
                config.no_memory,
                config.context_query.clone(),
                config.context_results,
                config.context_max_bytes,
                config.max_steps,
                config.log_commit_interval,
                config.log,
                None,
            )
            .map_err(|e| e.to_string())
        }));

        let result = match result {
            Ok(agent_result) => agent_result,
            Err(panic_info) => {
                let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "agent panicked".to_string()
                };
                Err(format!("Agent crashed: {msg}"))
            }
        };

        let _ = completion_tx.send(SlackCompletionEvent {
            session_key,
            channel_id,
            thread_ts,
            result,
        });
    });
}

fn handle_slack_completion(
    completion: SlackCompletionEvent,
    http_agent: &ureq::Agent,
    upload_client: &Client,
    config: &Arc<BridgeAgentConfig>,
    bot_token: &str,
    active_runs: &mut HashMap<String, SlackRunState>,
    completion_tx: &mpsc::Sender<SlackCompletionEvent>,
) {
    let output = match completion.result {
        Ok(outcome) => outcome.final_text.unwrap_or_else(|| "\u{2705}".to_string()),
        Err(err) => format!("Agent error: {err}"),
    };

    append_session_turn(&completion.session_key, "assistant", &output, config);

    let (directive, trailing) = parse_voice_directive(&output);
    if let Some(voice) = directive {
        if let Some(text) = trailing {
            if !text.trim().is_empty() {
                if let Err(err) =
                    send_slack_message(http_agent, bot_token, &completion.channel_id, completion.thread_ts.as_deref(), &text)
                {
                    eprintln!("Slack send error: {err}");
                }
            }
        }

        match generate_voice_audio(http_agent, &voice) {
            Ok(audio) => {
                if let Err(err) = upload_voice_note(
                    upload_client,
                    bot_token,
                    &completion.channel_id,
                    completion.thread_ts.as_deref(),
                    &audio,
                    "Voice note generated by AetherVault",
                ) {
                    let _ = send_slack_message(
                        http_agent,
                        bot_token,
                        &completion.channel_id,
                        completion.thread_ts.as_deref(),
                        &format!("Voice upload failed: {err}"),
                    );
                }
            }
            Err(err) => {
                let _ = send_slack_message(
                    http_agent,
                    bot_token,
                    &completion.channel_id,
                    completion.thread_ts.as_deref(),
                    &format!("Voice generation failed: {err}"),
                );
            }
        }
    } else if let Err(err) = send_slack_message(
        http_agent,
        bot_token,
        &completion.channel_id,
        completion.thread_ts.as_deref(),
        &output,
    ) {
        eprintln!("Slack send error: {err}");
    }

    let mut state = match active_runs.remove(&completion.session_key) {
        Some(state) => state,
        None => {
            return;
        }
    };

    if state.queued_messages.is_empty() {
        return;
    }

    let merged = if state.queued_messages.len() == 1 {
        state.queued_messages[0].0.clone()
    } else {
        state
            .queued_messages
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let merged_thread = state
        .queued_messages
        .last()
        .and_then(|(_, thread_ts)| thread_ts.clone());
    state.queued_messages.clear();
    append_session_turn(&completion.session_key, "user", &merged, config);

    active_runs.insert(
        completion.session_key.clone(),
        SlackRunState {
            queued_messages: Vec::new(),
        },
    );

    spawn_slack_run(
        config,
        completion_tx.clone(),
        completion.session_key,
        completion.channel_id,
        merged_thread,
        merged,
    );
}

fn handle_incoming_message(
    incoming: SlackIncomingEvent,
    http_agent: &ureq::Agent,
    bot_token: &str,
    config: &Arc<BridgeAgentConfig>,
    active_runs: &mut HashMap<String, SlackRunState>,
    completion_tx: &mpsc::Sender<SlackCompletionEvent>,
) {
    if let Some(output) = try_handle_approval_chat(&config.mv2, &incoming.text) {
        if let Err(err) = send_slack_message(
            http_agent,
            bot_token,
            &incoming.channel_id,
            incoming.thread_ts.as_deref(),
            &output,
        ) {
            eprintln!("Slack approval response send error: {err}");
        }
        return;
    }

    if let Some(state) = active_runs.get_mut(&incoming.session_key) {
        if state.queued_messages.len() < MAX_QUEUED_PER_SESSION {
            state.queued_messages
                .push((incoming.text, incoming.thread_ts));
        }
        return;
    }

    append_session_turn(&incoming.session_key, "user", &incoming.text, config);
    active_runs.insert(
        incoming.session_key.clone(),
        SlackRunState {
            queued_messages: Vec::new(),
        },
    );
    spawn_slack_run(
        config,
        completion_tx.clone(),
        incoming.session_key,
        incoming.channel_id,
        incoming.thread_ts,
        incoming.text,
    );
}

fn spawn_socket_listener(ws_url: String, tx: mpsc::Sender<SocketFrame>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut socket: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>> = match connect(&ws_url) {
            Ok((socket, _)) => socket,
            Err(err) => {
                let _ = tx.send(SocketFrame::Disconnected(format!("connect error: {err}")));
                return;
            }
        };

        loop {
            let message = match socket.read() {
                Ok(message) => message,
                Err(err) => {
                    let _ = tx.send(SocketFrame::Disconnected(format!("read error: {err}")));
                    break;
                }
            };

            match message {
                Message::Text(text) => {
                    let payload = match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(value) => value,
                        Err(err) => {
                            eprintln!("Slack socket payload parse error: {err}");
                            continue;
                        }
                    };

                    let envelope_id = payload
                        .get("envelope_id")
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string());

                    if let Some(ref eid) = envelope_id {
                        let mut ack_payload = serde_json::json!({});
                        if let Some(resp_url) = payload
                            .get("payload")
                            .and_then(|v| v.get("response_url"))
                            .or_else(|| payload.get("response_url"))
                            .and_then(|v| v.as_str())
                        {
                            ack_payload = serde_json::json!({"response_url": resp_url});
                        }
                        let ack = serde_json::json!({
                            "envelope_id": eid,
                            "payload": ack_payload,
                        });
                        let _ = socket.send(Message::Text(ack.to_string().into()));
                    }

                    if tx
                        .send(SocketFrame::Event {
                            envelope_id,
                            payload,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Message::Binary(binary) => {
                    let payload = match String::from_utf8(binary.into()) {
                        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                            Ok(value) => value,
                            Err(err) => {
                                eprintln!("Slack socket payload parse error: {err}");
                                continue;
                            }
                        },
                        Err(_) => {
                            continue;
                        }
                    };

                    let envelope_id = payload
                        .get("envelope_id")
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string());

                    if let Some(ref eid) = envelope_id {
                        let mut ack_payload = serde_json::json!({});
                        if let Some(resp_url) = payload
                            .get("payload")
                            .and_then(|v| v.get("response_url"))
                            .or_else(|| payload.get("response_url"))
                            .and_then(|v| v.as_str())
                        {
                            ack_payload = serde_json::json!({"response_url": resp_url});
                        }
                        let ack = serde_json::json!({
                            "envelope_id": eid,
                            "payload": ack_payload,
                        });
                        let _ = socket.send(Message::Text(ack.to_string().into()));
                    }

                    if tx.send(SocketFrame::Event { envelope_id, payload }).is_err() {
                        break;
                    }
                }
                Message::Ping(payload) => {
                    let _ = socket.send(Message::Pong(payload));
                }
                Message::Pong(_) => {}
                Message::Close(frame) => {
                    let reason = frame
                        .map(|frame| frame.reason)
                        .map(|reason| reason.to_string())
                        .unwrap_or_else(|| "socket closed".to_string());
                    let _ = tx.send(SocketFrame::Disconnected(format!(
                        "close: {}",
                        reason
                    )));
                    break;
                }
                _ => {}
            }
        }
    })
}

pub(crate) fn run_slack_bridge(
    agent_config: BridgeAgentConfig,
    bot_token: Option<String>,
    app_token: Option<String>,
    signing_secret: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let bot_token = bot_token
        .or_else(|| crate::env_optional("SLACK_BOT_TOKEN"))
        .ok_or("Missing SLACK_BOT_TOKEN")?;
    let app_token = app_token
        .or_else(|| crate::env_optional("SLACK_APP_TOKEN"))
        .ok_or("Missing SLACK_APP_TOKEN")?;
    let _signing_secret = signing_secret
        .or_else(|| crate::env_optional("SLACK_SIGNING_SECRET"))
        .ok_or("Missing SLACK_SIGNING_SECRET")?;

    let http_agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(DEFAULT_HTTP_TIMEOUT_MS))
        .timeout_write(Duration::from_millis(DEFAULT_HTTP_TIMEOUT_MS))
        .timeout_read(Duration::from_millis(DEFAULT_HTTP_TIMEOUT_MS))
        .build();

    let upload_client = Client::builder()
        .user_agent("aethervault-slack-bridge")
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let bot_user_id = fetch_slack_bot_user_id(&http_agent, &bot_token);
    if bot_user_id.is_none() {
        eprintln!("Slack auth.test did not return bot user id.");
    }

    // Best-effort cleanup of orphaned temp files from previous sessions.
    {
        if let Some(parent) = agent_config.mv2.parent() {
            if let Some(stem) = agent_config.mv2.file_name().and_then(|f| f.to_str()) {
                let prefix = format!(".{}.", stem);
                if let Ok(entries) = std::fs::read_dir(parent) {
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            if name.starts_with(&prefix) {
                                let _ = std::fs::remove_file(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }

    let mut active_runs: HashMap<String, SlackRunState> = HashMap::new();
    let (completion_tx, completion_rx) = mpsc::channel::<SlackCompletionEvent>();
    let mut last_vault_check = Instant::now();
    let vault_check_interval = Duration::from_secs(300);

    let mut reconnect_delay = Duration::from_secs(1);
    let max_reconnect_delay = Duration::from_secs(30);

    let config = Arc::new(agent_config);
    eprintln!("Slack Socket Mode bridge starting...");

    loop {
        let ws_url = match open_slack_socket_url(&http_agent, &app_token) {
            Ok(url) => url,
            Err(err) => {
                eprintln!("apps.connections.open failed: {err}");
                thread::sleep(reconnect_delay);
                reconnect_delay = (reconnect_delay * 2).min(max_reconnect_delay);
                continue;
            }
        };

        let (socket_tx, socket_rx) = mpsc::channel::<SocketFrame>();
        let _listener = spawn_socket_listener(ws_url, socket_tx);
        let mut seen_events = VecDeque::new();
        reconnect_delay = Duration::from_secs(1);

        eprintln!("Slack bridge connected to Socket Mode");

        loop {
            if last_vault_check.elapsed() >= vault_check_interval {
                last_vault_check = Instant::now();
                if let Ok(meta) = std::fs::metadata(&config.mv2) {
                    let size_mb = meta.len() / 1_000_000;
                    if size_mb > 200 {
                        eprintln!(
                            "[bridge] WARNING: vault size {size_mb}MB — approaching hard cap"
                        );
                    }
                }
            }

            match socket_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(SocketFrame::Event { payload, envelope_id }) => {
                    let event_id = payload
                        .get("event_id")
                        .and_then(|v| v.as_str())
                        .or_else(|| payload.get("envelope_id").and_then(|v| v.as_str()))
                        .or(envelope_id.as_deref())
                        .unwrap_or("");

                    if !event_id.is_empty() && is_duplicate_event(&mut seen_events, event_id) {
                        continue;
                    }

                    if let Some(incoming) =
                        parse_slack_incoming(payload, &bot_user_id, &http_agent, &bot_token)
                    {
                        handle_incoming_message(
                            incoming,
                            &http_agent,
                            &bot_token,
                            &config,
                            &mut active_runs,
                            &completion_tx,
                        );
                    }
                }
                Ok(SocketFrame::Disconnected(reason)) => {
                    eprintln!("Slack websocket disconnected: {reason}");
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    break;
                }
            }

            while let Ok(completion) = completion_rx.try_recv() {
                handle_slack_completion(
                    completion,
                    &http_agent,
                    &upload_client,
                    &config,
                    &bot_token,
                    &mut active_runs,
                    &completion_tx,
                );
            }
        }

        thread::sleep(reconnect_delay);
        reconnect_delay = (reconnect_delay * 2).min(max_reconnect_delay);
    }
}

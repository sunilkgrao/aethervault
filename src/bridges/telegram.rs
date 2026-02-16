use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json;

use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};
use base64::Engine;

use crate::{
    AgentProgress, BridgeAgentConfig, CompletionEvent, ActiveRun,
    ContinuationCheckpoint,
    SessionTurn, load_session_turns, save_session_turns,
    run_agent_with_prompt, try_handle_approval_chat, env_optional,
};

const NO_TIMEOUT_MS: u64 = u64::MAX;

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramUpdateResponse {
    pub(crate) ok: bool,
    #[serde(default)]
    pub(crate) result: Vec<TelegramUpdate>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramUpdate {
    pub(crate) update_id: i64,
    #[serde(default)]
    pub(crate) message: Option<TelegramMessage>,
    #[serde(default)]
    pub(crate) edited_message: Option<TelegramMessage>,
    #[serde(default)]
    pub(crate) channel_post: Option<TelegramMessage>,
    #[serde(default)]
    pub(crate) callback_query: Option<TelegramCallbackQuery>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct TelegramUser {
    pub(crate) id: i64,
    #[serde(default)]
    pub(crate) is_bot: Option<bool>,
    #[serde(default)]
    pub(crate) first_name: Option<String>,
    #[serde(default)]
    pub(crate) last_name: Option<String>,
    #[serde(default)]
    pub(crate) username: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramSticker {
    #[serde(default)]
    pub(crate) emoji: Option<String>,
    #[serde(default)]
    pub(crate) set_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramContact {
    pub(crate) phone_number: String,
    #[serde(default)]
    pub(crate) first_name: Option<String>,
    #[serde(default)]
    pub(crate) last_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramLocation {
    pub(crate) longitude: f64,
    pub(crate) latitude: f64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramCallbackQuery {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) from: Option<TelegramUser>,
    #[serde(default)]
    pub(crate) message: Option<Box<TelegramMessage>>,
    #[serde(default)]
    pub(crate) data: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct TelegramPhotoSize {
    pub(crate) file_id: String,
    #[serde(default)]
    pub(crate) file_size: Option<i64>,
    #[serde(default)]
    pub(crate) width: Option<i64>,
    #[serde(default)]
    pub(crate) height: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct TelegramVoice {
    pub(crate) file_id: String,
    #[serde(default)]
    pub(crate) duration: Option<i64>,
    #[serde(default)]
    pub(crate) mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct TelegramAudio {
    pub(crate) file_id: String,
    #[serde(default)]
    pub(crate) duration: Option<i64>,
    #[serde(default)]
    pub(crate) mime_type: Option<String>,
    #[serde(default)]
    pub(crate) title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramDocument {
    pub(crate) file_id: String,
    #[serde(default)]
    pub(crate) file_name: Option<String>,
    #[serde(default)]
    pub(crate) mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramMessage {
    pub(crate) chat: TelegramChat,
    #[serde(default)]
    pub(crate) message_id: Option<i64>,
    #[serde(default)]
    pub(crate) from: Option<TelegramUser>,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) caption: Option<String>,
    #[serde(default)]
    pub(crate) photo: Option<Vec<TelegramPhotoSize>>,
    #[serde(default)]
    pub(crate) voice: Option<TelegramVoice>,
    #[serde(default)]
    pub(crate) audio: Option<TelegramAudio>,
    #[serde(default)]
    pub(crate) document: Option<TelegramDocument>,
    #[serde(default)]
    pub(crate) sticker: Option<TelegramSticker>,
    #[serde(default)]
    pub(crate) contact: Option<TelegramContact>,
    #[serde(default)]
    pub(crate) location: Option<TelegramLocation>,
    #[serde(default)]
    pub(crate) forward_from: Option<TelegramUser>,
    #[serde(default)]
    pub(crate) forward_from_chat: Option<TelegramChat>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramChat {
    pub(crate) id: i64,
}

pub(crate) fn telegram_download_file_bytes(agent: &ureq::Agent, base_url: &str, file_id: &str) -> Option<(Vec<u8>, String)> {
    let url = format!("{base_url}/getFile");
    let payload = serde_json::json!({"file_id": file_id});
    let resp = agent.post(&url)
        .set("content-type", "application/json")
        .send_json(payload).ok()?;
    let data: serde_json::Value = resp.into_json().ok()?;
    let file_path = data["result"]["file_path"].as_str()?;
    // Build download URL: need token from base_url and correct API base
    let token_part = base_url.split("/bot").last()?;
    let api_base = if let Ok(base) = std::env::var("TELEGRAM_API_BASE") {
        base
    } else {
        "https://api.telegram.org".to_string()
    };
    let download_url = format!("{api_base}/file/bot{token_part}/{file_path}");
    let dl_resp = agent.get(&download_url).call().ok()?;
    let content_type = dl_resp.header("content-type")
        .unwrap_or("application/octet-stream").to_string();
    let mut bytes = Vec::new();
    dl_resp.into_reader().take(20_000_000).read_to_end(&mut bytes).ok()?;
    if bytes.is_empty() { return None; }
    Some((bytes, content_type))
}

pub(crate) fn transcribe_audio_deepgram(audio_bytes: &[u8], mime_type: &str) -> Option<String> {
    let api_key = std::env::var("DEEPGRAM_API_KEY").ok()?;
    if api_key.trim().is_empty() { return None; }
    let agent = ureq::AgentBuilder::new()
        .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
        .build();
    let resp = agent.post("https://api.deepgram.com/v1/listen?model=nova-2&smart_format=true")
        .set("Authorization", &format!("Token {api_key}"))
        .set("Content-Type", mime_type)
        .send_bytes(audio_bytes).ok()?;
    let data: serde_json::Value = resp.into_json().ok()?;
    let transcript = data["results"]["channels"][0]["alternatives"][0]["transcript"]
        .as_str()
        .map(|s| s.to_string())?;
    if transcript.trim().is_empty() { return None; }
    Some(transcript)
}

pub(crate) fn guess_image_media_type(ct: &str, file_path: &str) -> String {
    if ct.starts_with("image/") { return ct.to_string(); }
    if file_path.ends_with(".jpg") || file_path.ends_with(".jpeg") { return "image/jpeg".to_string(); }
    if file_path.ends_with(".png") { return "image/png".to_string(); }
    if file_path.ends_with(".webp") { return "image/webp".to_string(); }
    if file_path.ends_with(".gif") { return "image/gif".to_string(); }
    "image/jpeg".to_string()
}

/// Extract content from a Telegram update. Returns (chat_id, message_id, text).
/// For photos, the text will contain an [AV_IMAGE:base64:media_type:DATA] marker.
/// For voice/audio, the transcription is prepended to any caption/text.
pub(crate) fn extract_telegram_content(update: &TelegramUpdate, agent: &ureq::Agent, base_url: &str) -> Option<(i64, Option<i64>, String)> {
    // Handle callback queries (inline keyboard presses)
    if let Some(cb) = &update.callback_query {
        if let Some(data) = &cb.data {
            let chat_id = cb.message.as_ref().map(|m| m.chat.id).unwrap_or(0);
            let user_name = cb.from.as_ref()
                .and_then(|u| u.first_name.clone())
                .unwrap_or_else(|| "User".to_string());
            let msg_id = cb.message.as_ref().and_then(|m| m.message_id);
            return Some((chat_id, msg_id, format!("[Callback button pressed by {user_name}]: {data}")));
        }
    }

    let msg = update
        .message
        .as_ref()
        .or(update.edited_message.as_ref())
        .or(update.channel_post.as_ref())?;
    let chat_id = msg.chat.id;
    let msg_id = msg.message_id;
    let base_text = msg.text.clone()
        .or_else(|| msg.caption.clone())
        .unwrap_or_default();
    let user_name = msg.from.as_ref()
        .and_then(|u| u.first_name.clone())
        .unwrap_or_else(|| "User".to_string());

    // Handle forwarded messages
    if let Some(fwd) = &msg.forward_from {
        let fwd_name = fwd.first_name.clone().unwrap_or_else(|| "someone".to_string());
        let fwd_text = if base_text.trim().is_empty() {
            format!("[Forwarded message from {fwd_name} \u{2014} no text content]")
        } else {
            format!("[Forwarded message from {fwd_name}]:\n{base_text}")
        };
        return Some((chat_id, msg_id, fwd_text));
    }
    if let Some(fwd_chat) = &msg.forward_from_chat {
        let fwd_text = format!("[Forwarded from chat {}]:\n{base_text}", fwd_chat.id);
        return Some((chat_id, msg_id, fwd_text));
    }

    // Handle stickers
    if let Some(sticker) = &msg.sticker {
        let emoji = sticker.emoji.clone().unwrap_or_else(|| "unknown".to_string());
        let set_name = sticker.set_name.clone().unwrap_or_default();
        let sticker_text = format!("[{user_name} sent a sticker: {emoji} from set '{set_name}']");
        return Some((chat_id, msg_id, sticker_text));
    }

    // Handle contacts
    if let Some(contact) = &msg.contact {
        let name = contact.first_name.clone().unwrap_or_else(|| "Unknown".to_string());
        let last = contact.last_name.clone().unwrap_or_default();
        let phone = &contact.phone_number;
        let contact_text = format!("[{user_name} shared a contact: {name} {last}, phone: {phone}]");
        return Some((chat_id, msg_id, contact_text));
    }

    // Handle locations
    if let Some(loc) = &msg.location {
        let loc_text = format!(
            "[{user_name} shared a location: latitude {:.6}, longitude {:.6}]\nPlease describe this location or look it up.",
            loc.latitude, loc.longitude
        );
        return Some((chat_id, msg_id, loc_text));
    }

    // Handle photos: download largest, base64 encode, create image marker
    if let Some(photos) = &msg.photo {
        if !photos.is_empty() {
            // Telegram sends multiple sizes; pick the largest (last in array)
            let best = photos.iter().max_by_key(|p| p.file_size.unwrap_or(0))?;
            if let Some((bytes, ct)) = telegram_download_file_bytes(agent, base_url, &best.file_id) {
                let media_type = guess_image_media_type(&ct, &best.file_id);
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let marker = format!("[AV_IMAGE:{}:{}]", media_type, b64);
                let text = if base_text.trim().is_empty() {
                    format!("{marker}\nDescribe what you see in this image.")
                } else {
                    format!("{marker}\n{base_text}")
                };
                return Some((chat_id, msg_id, text));
            }
            // Download failed, fall through to caption/text
            let text = if base_text.trim().is_empty() {
                "[User sent a photo but it could not be downloaded]".to_string()
            } else {
                format!("[User sent a photo but it could not be downloaded]\n{base_text}")
            };
            return Some((chat_id, msg_id, text));
        }
    }

    // Handle voice messages
    if let Some(voice) = &msg.voice {
        let mime = voice.mime_type.clone().unwrap_or_else(|| "audio/ogg".to_string());
        if let Some((bytes, _ct)) = telegram_download_file_bytes(agent, base_url, &voice.file_id) {
            if let Some(transcript) = transcribe_audio_deepgram(&bytes, &mime) {
                let text = if base_text.trim().is_empty() {
                    format!("[Voice message transcription]: {transcript}")
                } else {
                    format!("[Voice message transcription]: {transcript}\n\nUser also wrote: {base_text}")
                };
                return Some((chat_id, msg_id, text));
            }
            return Some((chat_id, msg_id, "[User sent a voice message but transcription failed]".to_string()));
        }
        return Some((chat_id, msg_id, "[User sent a voice message but it could not be downloaded]".to_string()));
    }

    // Handle audio files
    if let Some(audio) = &msg.audio {
        let mime = audio.mime_type.clone().unwrap_or_else(|| "audio/mpeg".to_string());
        let title_note = audio.title.as_deref().map(|t| format!(" (title: {t})")).unwrap_or_default();
        if let Some((bytes, _ct)) = telegram_download_file_bytes(agent, base_url, &audio.file_id) {
            if let Some(transcript) = transcribe_audio_deepgram(&bytes, &mime) {
                let text = format!("[Audio{title_note} transcription]: {transcript}");
                return Some((chat_id, msg_id, text));
            }
            return Some((chat_id, msg_id, format!("[User sent an audio file{title_note} but transcription failed]")));
        }
        return Some((chat_id, msg_id, format!("[User sent an audio file{title_note} but it could not be downloaded]")));
    }

    // Handle documents (text-based ones)
    if let Some(doc) = &msg.document {
        let fname = doc.file_name.clone().unwrap_or_else(|| "unknown".to_string());
        let mime = doc.mime_type.clone().unwrap_or_default();
        let is_text = mime.starts_with("text/")
            || mime == "application/json"
            || mime == "application/xml"
            || fname.ends_with(".txt") || fname.ends_with(".md")
            || fname.ends_with(".json") || fname.ends_with(".csv")
            || fname.ends_with(".py") || fname.ends_with(".rs")
            || fname.ends_with(".js") || fname.ends_with(".ts")
            || fname.ends_with(".sh") || fname.ends_with(".yaml")
            || fname.ends_with(".yml") || fname.ends_with(".toml");
        if is_text {
            if let Some((bytes, _ct)) = telegram_download_file_bytes(agent, base_url, &doc.file_id) {
                if let Ok(text_content) = String::from_utf8(bytes) {
                    let truncated = if text_content.len() > 50000 {
                        let safe: String = text_content.chars().take(50000).collect();
                        format!("{safe}\n... (truncated, {} total chars)", text_content.chars().count())
                    } else {
                        text_content
                    };
                    let text = format!("[Document: {fname}]\n```\n{truncated}\n```\n\n{base_text}");
                    return Some((chat_id, msg_id, text));
                }
            }
        }
        // Non-text document or download failed
        let text = if base_text.trim().is_empty() {
            format!("[User sent a document: {fname} ({mime}). This file type is not supported for direct reading.]")
        } else {
            format!("[User sent a document: {fname} ({mime})]\n{base_text}")
        };
        return Some((chat_id, msg_id, text));
    }

    // Plain text message
    if base_text.trim().is_empty() {
        return None;
    }
    Some((chat_id, msg_id, base_text))
}

pub(crate) fn split_text_chunks(text: &str, max_chars: usize) -> Vec<String> {
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

pub(crate) fn telegram_send_typing(agent: &ureq::Agent, base_url: &str, chat_id: i64) {
    let url = format!("{base_url}/sendChatAction");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "action": "typing"
    });
    let _ = agent.post(&url)
        .set("content-type", "application/json")
        .send_json(payload);
}

pub(crate) fn telegram_answer_callback(agent: &ureq::Agent, base_url: &str, callback_id: &str, text: Option<&str>) {
    let url = format!("{base_url}/answerCallbackQuery");
    let mut payload = serde_json::json!({"callback_query_id": callback_id});
    if let Some(t) = text {
        payload["text"] = serde_json::json!(t);
    }
    let _ = agent.post(&url)
        .set("content-type", "application/json")
        .send_json(payload);
}

pub(crate) fn telegram_send_message(
    agent: &ureq::Agent,
    base_url: &str,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    telegram_send_message_ext(agent, base_url, chat_id, text, None)
}

pub(crate) fn telegram_send_message_ext(
    agent: &ureq::Agent,
    base_url: &str,
    chat_id: i64,
    text: &str,
    reply_to: Option<i64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{base_url}/sendMessage");
    let chunks = split_text_chunks(text, 3900);
    for (i, chunk) in chunks.iter().enumerate() {
        // Try Markdown first, fall back to plain text
        let mut payload = serde_json::json!({
            "chat_id": chat_id,
            "text": chunk,
            "parse_mode": "Markdown"
        });
        // Only reply to original on first chunk
        if i == 0 {
            if let Some(mid) = reply_to {
                payload["reply_to_message_id"] = serde_json::json!(mid);
                payload["allow_sending_without_reply"] = serde_json::json!(true);
            }
        }
        let response = agent
            .post(&url)
            .set("content-type", "application/json")
            .send_json(payload);
        match response {
            Ok(_) => {},
            Err(_) => {
                // Markdown failed, retry as plain text
                let mut plain_payload = serde_json::json!({
                    "chat_id": chat_id,
                    "text": chunk
                });
                if i == 0 {
                    if let Some(mid) = reply_to {
                        plain_payload["reply_to_message_id"] = serde_json::json!(mid);
                        plain_payload["allow_sending_without_reply"] = serde_json::json!(true);
                    }
                }
                let fallback = agent
                    .post(&url)
                    .set("content-type", "application/json")
                    .send_json(plain_payload);
                if let Err(err) = fallback {
                    return Err(format!("Telegram send error: {err}").into());
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn spawn_agent_run(
    agent_config: &BridgeAgentConfig,
    chat_id: i64,
    reply_to_id: Option<i64>,
    user_text: &str,
    session: String,
    completion_tx: &mpsc::Sender<CompletionEvent>,
    http_agent: &ureq::Agent,
    base_url: &str,
) -> Arc<Mutex<AgentProgress>> {
    let progress = Arc::new(Mutex::new(AgentProgress {
        step: 0,
        max_steps: agent_config.max_steps,
        phase: "starting".to_string(),
        text_preview: None,
        started_at: std::time::Instant::now(),
        tools_used: HashMap::new(),
        checkpoint_sent: false,
        checkpoint_response: None,
        extended_max_steps: None,
        interim_messages: Vec::new(),
        first_ack_sent: false,
        opus_steps: 0,
        delegated_steps: 0,
        steering_messages: Vec::new(),
    }));

    // Worker thread -- calls run_agent_with_prompt directly (no middle thread)
    let worker_progress = progress.clone();
    let mv2 = agent_config.mv2.clone();
    let model_hook = agent_config.model_hook.clone();
    let system_text = agent_config.system.clone();
    let no_memory = agent_config.no_memory;
    let context_query = agent_config.context_query.clone();
    let context_results = agent_config.context_results;
    let context_max_bytes = agent_config.context_max_bytes;
    let max_steps = agent_config.max_steps;
    let log_commit_interval = agent_config.log_commit_interval;
    let log = agent_config.log;
    let worker_prompt = user_text.to_string();
    let worker_session = session;
    let worker_tx = completion_tx.clone();
    thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_agent_with_prompt(
                mv2,
                worker_prompt,
                Some(worker_session),
                model_hook,
                system_text,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log_commit_interval,
                log,
                Some(worker_progress.clone()),
            )
            .map_err(|e| e.to_string())
        }));
        // Mark done -- recover from poison so progress thread always sees completion
        let mut p = worker_progress.lock().unwrap_or_else(|e| e.into_inner());
        p.phase = "done".to_string();
        drop(p);
        let event = match result {
            Ok(agent_result) => CompletionEvent {
                chat_id,
                reply_to_id,
                result: agent_result,
            },
            Err(panic_info) => {
                let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "agent panicked".to_string()
                };
                CompletionEvent {
                    chat_id,
                    reply_to_id,
                    result: Err(format!("Agent crashed: {msg}")),
                }
            }
        };
        let _ = worker_tx.send(event);
    });

    // Progress reporter thread -- interim messages + typing indicators + checkpoint
    let prog_ref = progress.clone();
    let prog_agent = http_agent.clone();
    let prog_url = base_url.to_string();
    thread::spawn(move || {
        let mut tick_count: usize = 0;
        loop {
            thread::sleep(Duration::from_secs(4));
            tick_count += 1;

            // Drain and send any interim messages from the agent
            let pending: Vec<String> = {
                let mut guard = prog_ref.lock().unwrap_or_else(|e| e.into_inner());
                guard.interim_messages.drain(..).collect()
            };
            for msg in &pending {
                let _ = telegram_send_message(&prog_agent, &prog_url, chat_id, msg);
                // Mark that we've sent something
                if let Ok(mut guard) = prog_ref.lock() {
                    guard.first_ack_sent = true;
                }
            }

            let (done, should_checkpoint, first_ack_needed) = {
                let guard = prog_ref.lock().unwrap_or_else(|e| e.into_inner());
                let done = guard.phase == "done";
                let effective_max = guard.extended_max_steps.unwrap_or(guard.max_steps);
                let at_checkpoint = guard.step >= effective_max * 3 / 4
                    && !guard.checkpoint_sent
                    && effective_max > 4;
                // Send a first ack after ~12s if nothing has been sent yet and agent is working
                let needs_first_ack = !guard.first_ack_sent
                    && tick_count >= 3  // ~12 seconds
                    && guard.step > 0   // agent has started processing
                    && !done;
                (done, at_checkpoint, needs_first_ack)
            };
            if done {
                break;
            }

            // First-response acknowledgment after ~12s of silence
            if first_ack_needed {
                let ack_msg = {
                    let guard = prog_ref.lock().unwrap_or_else(|e| e.into_inner());
                    let tools: Vec<String> = guard.tools_used.keys().take(3).cloned().collect();
                    if tools.is_empty() {
                        "Working on it...".to_string()
                    } else {
                        format!("On it \u{2014} using {}...", tools.join(", "))
                    }
                };
                let _ = telegram_send_message(&prog_agent, &prog_url, chat_id, &ack_msg);
                if let Ok(mut guard) = prog_ref.lock() {
                    guard.first_ack_sent = true;
                }
            }
            if should_checkpoint {
                // Build checkpoint message from progress state
                let (step, max, tools, preview, elapsed) = {
                    let mut guard = prog_ref.lock().unwrap_or_else(|e| e.into_inner());
                    guard.checkpoint_sent = true;
                    let elapsed = guard.started_at.elapsed().as_secs();
                    let tools: Vec<String> = {
                        let mut sorted: Vec<_> = guard.tools_used.iter()
                            .map(|(k, v)| (k.clone(), *v))
                            .collect();
                        sorted.sort_by(|a, b| b.1.cmp(&a.1));
                        sorted.into_iter().take(5)
                            .map(|(k, v)| format!("{k} ({v}x)"))
                            .collect()
                    };
                    (guard.step, guard.extended_max_steps.unwrap_or(guard.max_steps),
                     tools, guard.text_preview.clone(), elapsed)
                };
                let tools_str = if tools.is_empty() {
                    "none yet".to_string()
                } else {
                    tools.join(", ")
                };
                let preview_str = preview
                    .map(|p| format!("\nLast update: {p}"))
                    .unwrap_or_default();
                let mins = elapsed / 60;
                let secs = elapsed % 60;
                let msg = format!(
                    "I'm at step {step}/{max} ({mins}m{secs}s elapsed).{preview_str}\n\
                     Tools used: {tools_str}\n\n\
                     Reply \"continue\" to extend by {max} more steps, \
                     or \"wrap up\" to finish with what I have."
                );
                let _ = telegram_send_message(&prog_agent, &prog_url, chat_id, &msg);
            } else {
                telegram_send_typing(&prog_agent, &prog_url, chat_id);
            }
        }
    });

    progress
}

pub(crate) fn handle_telegram_completion(
    event: CompletionEvent,
    http_agent: &ureq::Agent,
    base_url: &str,
    agent_config: &BridgeAgentConfig,
    active_runs: &mut HashMap<i64, ActiveRun>,
    completion_tx: &mpsc::Sender<CompletionEvent>,
) {
    let chat_id = event.chat_id;
    let reply_to_id = event.reply_to_id;

    let output = match event.result {
        Ok(result) => {
            let mut text = result.final_text.unwrap_or_default();
            if text.trim().is_empty() {
                text = "\u{2705}".to_string();
            }
            text
        }
        Err(err) => {
            err.chars().take(500).collect::<String>()
        }
    };

    // Self-continuation: check for continuation marker and auto-chain
    let max_chain_depth: usize = env_optional("AGENT_MAX_CHAIN_DEPTH")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    if let Some(checkpoint_path) = output.strip_prefix("[CONTINUATION_NEEDED:")
        .and_then(|s| s.strip_suffix(']'))
    {
        // Load the checkpoint and spawn a continuation session
        if let Ok(checkpoint_json) = std::fs::read_to_string(checkpoint_path) {
            if let Ok(checkpoint) = serde_json::from_str::<ContinuationCheckpoint>(&checkpoint_json) {
                if checkpoint.chain_depth <= max_chain_depth {
                    // Send progress message
                    let _ = telegram_send_message(
                        http_agent, base_url, chat_id,
                        &format!("Continuing task (phase {}/{max_chain_depth})...", checkpoint.chain_depth),
                    );

                    // Build continuation prompt
                    let key_decisions_str = if checkpoint.key_decisions.is_empty() {
                        "None recorded yet.".to_string()
                    } else {
                        checkpoint.key_decisions.join("\n- ")
                    };
                    let continuation_prompt = format!(
                        "[Continuation from previous session — chain depth {}]\n\n\
                         ORIGINAL GOAL: {}\n\n\
                         WHAT WAS DONE:\n{}\n\n\
                         KEY DECISIONS SO FAR:\n- {}\n\n\
                         REMAINING WORK:\n{}\n\n\
                         Total steps so far: {}. Continue working toward the goal.",
                        checkpoint.chain_depth,
                        checkpoint.goal,
                        checkpoint.summary,
                        key_decisions_str,
                        checkpoint.remaining_work,
                        checkpoint.total_steps,
                    );

                    let continuation_session = format!(
                        "{}:chain:{}",
                        checkpoint.session.split(":chain:").next().unwrap_or(&checkpoint.session),
                        checkpoint.chain_depth,
                    );

                    let progress = spawn_agent_run(
                        agent_config,
                        chat_id,
                        reply_to_id,
                        &continuation_prompt,
                        continuation_session,
                        completion_tx,
                        http_agent,
                        base_url,
                    );

                    if let Some(run) = active_runs.get_mut(&chat_id) {
                        run.progress = progress;
                    } else {
                        active_runs.insert(chat_id, ActiveRun {
                            progress,
                            queued_messages: Vec::new(),
                        });
                    }
                    return;
                }
                // Max chain depth reached — fall through to send final output
                let _ = telegram_send_message(
                    http_agent, base_url, chat_id,
                    &format!("Reached maximum chain depth ({max_chain_depth}). Summary of progress:\n{}", checkpoint.summary),
                );
                if let Some(run) = active_runs.get_mut(&chat_id) {
                    if run.queued_messages.is_empty() {
                        active_runs.remove(&chat_id);
                    }
                }
                return;
            }
        }
        // Checkpoint load failed — send error and fall through
        eprintln!("[continuation] failed to load checkpoint: {checkpoint_path}");
    }

    // Save conversation turns for session continuity
    let session_id = format!("{}telegram:{chat_id}", agent_config.session_prefix);
    {
        let mut turns = load_session_turns(&session_id, 8);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        // We don't have the original user_text here in the completion event,
        // but session turns were already saved by the agent run itself via the session.
        turns.push(SessionTurn {
            role: "assistant".to_string(),
            content: output.clone(),
            timestamp: now,
        });
        save_session_turns(&session_id, &turns, 8);
    }

    if let Err(err) = telegram_send_message_ext(http_agent, base_url, chat_id, &output, reply_to_id) {
        eprintln!("Telegram send failed: {err}");
    }

    // Check for queued messages -- merge all into one prompt
    if let Some(run) = active_runs.get_mut(&chat_id) {
        if run.queued_messages.is_empty() {
            active_runs.remove(&chat_id);
        } else {
            // Merge all queued messages into a single prompt
            let merged_text = if run.queued_messages.len() == 1 {
                run.queued_messages[0].0.clone()
            } else {
                // Multiple messages -- combine them so the agent sees the full context
                run.queued_messages.iter()
                    .map(|(text, _)| text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            let last_reply_id = run.queued_messages.last().map(|(_, rid)| *rid).flatten();
            run.queued_messages.clear();

            let session = format!("{}telegram:{chat_id}", agent_config.session_prefix);

            // Save merged user message to session turns
            {
                let mut turns = load_session_turns(&session, 8);
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                turns.push(SessionTurn {
                    role: "user".to_string(),
                    content: merged_text.clone(),
                    timestamp: now,
                });
                save_session_turns(&session, &turns, 8);
            }

            let progress = spawn_agent_run(
                agent_config,
                chat_id,
                last_reply_id,
                &merged_text,
                session,
                completion_tx,
                http_agent,
                base_url,
            );
            run.progress = progress;
        }
    }
}

pub(crate) fn run_telegram_bridge(
    token: String,
    poll_timeout: u64,
    poll_limit: usize,
    agent_config: BridgeAgentConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let base_url = match std::env::var("TELEGRAM_API_BASE") {
        Ok(base) => format!("{base}/bot{token}"),
        Err(_) => format!("https://api.telegram.org/bot{token}"),
    };
    let http_agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_write(Duration::from_millis(NO_TIMEOUT_MS))
        .timeout_read(Duration::from_millis(NO_TIMEOUT_MS))
        .build();

    // Clean up orphaned vault temp files from previous crashes.
    // These are created during atomic writes and left behind if the process is killed.
    {
        let vault_path = &agent_config.mv2;
        if let Some(parent) = vault_path.parent() {
            if let Some(stem) = vault_path.file_name().and_then(|f| f.to_str()) {
                let prefix = format!(".{}.", stem);
                if let Ok(entries) = std::fs::read_dir(parent) {
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            if name.starts_with(&prefix) {
                                let _ = std::fs::remove_file(entry.path());
                                eprintln!("[bridge] cleaned up orphaned temp file: {}", name);
                            }
                        }
                    }
                }
            }
        }
    }

    let mut active_runs: HashMap<i64, ActiveRun> = HashMap::new();
    let (completion_tx, completion_rx) = mpsc::channel::<CompletionEvent>();

    let mut offset: Option<i64> = None;
    let mut last_vault_check = std::time::Instant::now();
    let vault_check_interval = Duration::from_secs(300); // every 5 min
    loop {
        // 0. Periodic vault health check
        if last_vault_check.elapsed() >= vault_check_interval {
            last_vault_check = std::time::Instant::now();
            if let Ok(meta) = std::fs::metadata(&agent_config.mv2) {
                let size_mb = meta.len() / 1_000_000;
                if size_mb > 200 {
                    eprintln!("[bridge] WARNING: vault size {size_mb}MB \u{2014} approaching hard cap");
                }
            }
            // Also clean temp files that may have accumulated during runtime
            if let Some(parent) = agent_config.mv2.parent() {
                if let Some(stem) = agent_config.mv2.file_name().and_then(|f| f.to_str()) {
                    let prefix = format!(".{stem}.");
                    if let Ok(entries) = std::fs::read_dir(parent) {
                        for entry in entries.flatten() {
                            if let Some(name) = entry.file_name().to_str() {
                                if name.starts_with(&prefix) {
                                    let _ = std::fs::remove_file(entry.path());
                                    eprintln!("[bridge] cleaned runtime temp file: {name}");
                                }
                            }
                        }
                    }
                }
            }
        }

        // 1. Drain completions (non-blocking)
        while let Ok(event) = completion_rx.try_recv() {
            handle_telegram_completion(
                event,
                &http_agent,
                &base_url,
                &agent_config,
                &mut active_runs,
                &completion_tx,
            );
        }

        // 2. Long-poll getUpdates (short-poll when runs are active to drain completions faster)
        let mut request = http_agent
            .get(&format!("{base_url}/getUpdates"))
            .query("limit", &poll_limit.to_string());
        if let Some(effective_timeout) = if active_runs.is_empty() {
            if poll_timeout == u64::MAX {
                None
            } else {
                Some(poll_timeout.min(100))
            }
        } else {
            Some(2)
        } {
            request = request.query("timeout", &effective_timeout.to_string());
        }
        if let Some(last) = offset {
            request = request.query("offset", &(last + 1).to_string());
        }

        let response = request.call();
        let payload = match response {
            Ok(resp) => resp.into_json::<TelegramUpdateResponse>(),
            Err(err) => {
                eprintln!("Telegram poll error: {err}");
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        };

        let update = match payload {
            Ok(update) => update,
            Err(err) => {
                eprintln!("Telegram decode error: {err}");
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        if !update.ok {
            eprintln!("Telegram API returned ok=false");
            thread::sleep(Duration::from_secs(2));
            continue;
        }

        // 3. Process updates
        for entry in update.result {
            offset = Some(entry.update_id);

            // Handle callback queries (inline keyboard presses)
            if let Some(cb) = &entry.callback_query {
                telegram_answer_callback(&http_agent, &base_url, &cb.id, None);
            }

            let Some((chat_id, reply_to_id, user_text)) = extract_telegram_content(&entry, &http_agent, &base_url) else {
                continue;
            };
            if let Some(output) = try_handle_approval_chat(&agent_config.mv2, &user_text) {
                if let Err(err) = telegram_send_message(&http_agent, &base_url, chat_id, &output) {
                    eprintln!("Telegram send failed: {err}");
                }
                continue;
            }

            // Check if there's already an active run for this chat
            if let Some(run) = active_runs.get_mut(&chat_id) {
                // Check if user is responding to a checkpoint
                let lower = user_text.trim().to_lowercase();
                let is_checkpoint_response = {
                    let guard = run.progress.lock().unwrap_or_else(|e| e.into_inner());
                    guard.checkpoint_sent && guard.checkpoint_response.is_none()
                };
                if is_checkpoint_response {
                    if lower.contains("continue") || lower.contains("keep going") || lower.contains("yes") || lower.contains("extend") {
                        let mut guard = run.progress.lock().unwrap_or_else(|e| e.into_inner());
                        let current_max = guard.extended_max_steps.unwrap_or(guard.max_steps);
                        guard.extended_max_steps = Some(current_max + guard.max_steps);
                        guard.checkpoint_response = Some(true);
                        guard.checkpoint_sent = false; // allow another checkpoint at new 75%
                        drop(guard);
                        let _ = telegram_send_message(&http_agent, &base_url, chat_id,
                            &format!("Got it, extending by {} more steps.", run.progress.lock().unwrap_or_else(|e| e.into_inner()).max_steps));
                        continue;
                    } else if lower.contains("wrap") || lower.contains("stop") || lower.contains("finish") || lower.contains("no") {
                        let mut guard = run.progress.lock().unwrap_or_else(|e| e.into_inner());
                        guard.checkpoint_response = Some(false);
                        drop(guard);
                        let _ = telegram_send_message(&http_agent, &base_url, chat_id,
                            "Wrapping up with what I have so far.");
                        continue;
                    }
                    // Not a clear checkpoint response -- treat as queued message
                }
                // Inject into the running agent as a steering message.
                // The agent loop drains these at each step and injects them
                // as user messages so the LLM can adjust course immediately.
                // We do NOT also queue them — that would cause double-handling
                // (once mid-run, once as a new prompt after completion).
                {
                    let mut guard = run.progress.lock().unwrap_or_else(|e| e.into_inner());
                    guard.steering_messages.push(user_text);
                }
                continue;
            }

            // No active run -- spawn a new one
            telegram_send_typing(&http_agent, &base_url, chat_id);

            let session = format!("{}telegram:{chat_id}", agent_config.session_prefix);

            // Save user message to session turns
            {
                let mut turns = load_session_turns(&session, 8);
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                turns.push(SessionTurn {
                    role: "user".to_string(),
                    content: user_text.clone(),
                    timestamp: now,
                });
                save_session_turns(&session, &turns, 8);
            }

            let progress = spawn_agent_run(
                &agent_config,
                chat_id,
                reply_to_id,
                &user_text,
                session,
                &completion_tx,
                &http_agent,
                &base_url,
            );

            active_runs.insert(chat_id, ActiveRun {
                progress,
                queued_messages: Vec::new(),
            });
        }
    }
}

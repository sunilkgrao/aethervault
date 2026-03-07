use std::collections::HashMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;
use tiny_http::{Header, Method, Response, Server};
use url::form_urlencoded;

use crate::agent_runtime::{BridgeAgentConfig, build_bridge_agent_config, run_agent_for_bridge};
use crate::policy::try_handle_approval_chat;
use crate::{BridgeCommand, blake3_hash, env_optional, resolve_mv2_path};

#[derive(Debug, Deserialize)]
struct TelegramUpdateResponse {
    ok: bool,
    #[serde(default)]
    result: Vec<TelegramUpdate>,
}

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    #[serde(default)]
    message: Option<TelegramMessage>,
    #[serde(default)]
    edited_message: Option<TelegramMessage>,
    #[serde(default)]
    channel_post: Option<TelegramMessage>,
    #[serde(default)]
    callback_query: Option<TelegramCallbackQuery>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TelegramUser {
    id: i64,
    #[serde(default)]
    is_bot: Option<bool>,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramSticker {
    #[serde(default)]
    emoji: Option<String>,
    #[serde(default)]
    set_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramContact {
    phone_number: String,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramLocation {
    longitude: f64,
    latitude: f64,
}

#[derive(Debug, Deserialize)]
struct TelegramCallbackQuery {
    id: String,
    #[serde(default)]
    from: Option<TelegramUser>,
    #[serde(default)]
    message: Option<Box<TelegramMessage>>,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TelegramPhotoSize {
    file_id: String,
    #[serde(default)]
    file_size: Option<i64>,
    #[serde(default)]
    width: Option<i64>,
    #[serde(default)]
    height: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TelegramVoice {
    file_id: String,
    #[serde(default)]
    duration: Option<i64>,
    #[serde(default)]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TelegramAudio {
    file_id: String,
    #[serde(default)]
    duration: Option<i64>,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramDocument {
    file_id: String,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    chat: TelegramChat,
    #[serde(default)]
    message_id: Option<i64>,
    #[serde(default)]
    from: Option<TelegramUser>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    caption: Option<String>,
    #[serde(default)]
    photo: Option<Vec<TelegramPhotoSize>>,
    #[serde(default)]
    voice: Option<TelegramVoice>,
    #[serde(default)]
    audio: Option<TelegramAudio>,
    #[serde(default)]
    document: Option<TelegramDocument>,
    #[serde(default)]
    sticker: Option<TelegramSticker>,
    #[serde(default)]
    contact: Option<TelegramContact>,
    #[serde(default)]
    location: Option<TelegramLocation>,
    #[serde(default)]
    forward_from: Option<TelegramUser>,
    #[serde(default)]
    forward_from_chat: Option<TelegramChat>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
}

fn telegram_download_file_bytes(
    agent: &ureq::Agent,
    base_url: &str,
    file_id: &str,
) -> Option<(Vec<u8>, String)> {
    let url = format!("{base_url}/getFile");
    let payload = serde_json::json!({ "file_id": file_id });
    let resp = agent
        .post(&url)
        .set("content-type", "application/json")
        .send_json(payload)
        .ok()?;
    let data: serde_json::Value = resp.into_json().ok()?;
    let file_path = data["result"]["file_path"].as_str()?;
    let token_part = base_url.split("/bot").last()?;
    let api_base = if let Ok(base) = std::env::var("TELEGRAM_API_BASE") {
        base
    } else {
        "https://api.telegram.org".to_string()
    };
    let download_url = format!("{api_base}/file/bot{token_part}/{file_path}");
    let dl_resp = agent.get(&download_url).call().ok()?;
    let content_type = dl_resp
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();
    let mut bytes = Vec::new();
    dl_resp
        .into_reader()
        .take(20_000_000)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some((bytes, content_type))
}

fn transcribe_audio_deepgram(audio_bytes: &[u8], mime_type: &str) -> Option<String> {
    let api_key = std::env::var("DEEPGRAM_API_KEY").ok()?;
    if api_key.trim().is_empty() {
        return None;
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_read(Duration::from_secs(60))
        .timeout_connect(Duration::from_secs(10))
        .build();
    let resp = agent
        .post("https://api.deepgram.com/v1/listen?model=nova-2&smart_format=true")
        .set("Authorization", &format!("Token {api_key}"))
        .set("Content-Type", mime_type)
        .send_bytes(audio_bytes)
        .ok()?;
    let data: serde_json::Value = resp.into_json().ok()?;
    let transcript = data["results"]["channels"][0]["alternatives"][0]["transcript"]
        .as_str()
        .map(|s| s.to_string())?;
    if transcript.trim().is_empty() {
        return None;
    }
    Some(transcript)
}

fn guess_image_media_type(ct: &str, file_path: &str) -> String {
    if ct.starts_with("image/") {
        return ct.to_string();
    }
    if file_path.ends_with(".jpg") || file_path.ends_with(".jpeg") {
        return "image/jpeg".to_string();
    }
    if file_path.ends_with(".png") {
        return "image/png".to_string();
    }
    if file_path.ends_with(".webp") {
        return "image/webp".to_string();
    }
    if file_path.ends_with(".gif") {
        return "image/gif".to_string();
    }
    "image/jpeg".to_string()
}

fn extract_telegram_content(
    update: &TelegramUpdate,
    agent: &ureq::Agent,
    base_url: &str,
) -> Option<(i64, Option<i64>, String)> {
    if let Some(cb) = &update.callback_query {
        if let Some(data) = &cb.data {
            let chat_id = cb.message.as_ref().map(|m| m.chat.id).unwrap_or(0);
            let user_name = cb
                .from
                .as_ref()
                .and_then(|u| u.first_name.clone())
                .unwrap_or_else(|| "User".to_string());
            let msg_id = cb.message.as_ref().and_then(|m| m.message_id);
            return Some((
                chat_id,
                msg_id,
                format!("[Callback button pressed by {user_name}]: {data}"),
            ));
        }
    }

    let msg = update
        .message
        .as_ref()
        .or(update.edited_message.as_ref())
        .or(update.channel_post.as_ref())?;
    let chat_id = msg.chat.id;
    let msg_id = msg.message_id;
    let base_text = msg
        .text
        .clone()
        .or_else(|| msg.caption.clone())
        .unwrap_or_default();
    let user_name = msg
        .from
        .as_ref()
        .and_then(|u| u.first_name.clone())
        .unwrap_or_else(|| "User".to_string());

    if let Some(fwd) = &msg.forward_from {
        let fwd_name = fwd
            .first_name
            .clone()
            .unwrap_or_else(|| "someone".to_string());
        let fwd_text = if base_text.trim().is_empty() {
            format!("[Forwarded message from {fwd_name} — no text content]")
        } else {
            format!("[Forwarded message from {fwd_name}]:\n{base_text}")
        };
        return Some((chat_id, msg_id, fwd_text));
    }
    if let Some(fwd_chat) = &msg.forward_from_chat {
        let fwd_text = format!("[Forwarded from chat {}]:\n{base_text}", fwd_chat.id);
        return Some((chat_id, msg_id, fwd_text));
    }

    if let Some(sticker) = &msg.sticker {
        let emoji = sticker
            .emoji
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let set_name = sticker.set_name.clone().unwrap_or_default();
        let sticker_text = format!("[{user_name} sent a sticker: {emoji} from set '{set_name}']");
        return Some((chat_id, msg_id, sticker_text));
    }

    if let Some(contact) = &msg.contact {
        let name = contact
            .first_name
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        let last = contact.last_name.clone().unwrap_or_default();
        let phone = &contact.phone_number;
        let contact_text = format!("[{user_name} shared a contact: {name} {last}, phone: {phone}]");
        return Some((chat_id, msg_id, contact_text));
    }

    if let Some(loc) = &msg.location {
        let loc_text = format!(
            "[{user_name} shared a location: latitude {:.6}, longitude {:.6}]\nPlease describe this location or look it up.",
            loc.latitude, loc.longitude
        );
        return Some((chat_id, msg_id, loc_text));
    }

    if let Some(photos) = &msg.photo {
        if !photos.is_empty() {
            let best = photos.iter().max_by_key(|p| p.file_size.unwrap_or(0))?;
            if let Some((bytes, ct)) = telegram_download_file_bytes(agent, base_url, &best.file_id)
            {
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
            let text = if base_text.trim().is_empty() {
                "[User sent a photo but it could not be downloaded]".to_string()
            } else {
                format!("[User sent a photo but it could not be downloaded]\n{base_text}")
            };
            return Some((chat_id, msg_id, text));
        }
    }

    if let Some(voice) = &msg.voice {
        let mime = voice
            .mime_type
            .clone()
            .unwrap_or_else(|| "audio/ogg".to_string());
        if let Some((bytes, _ct)) = telegram_download_file_bytes(agent, base_url, &voice.file_id) {
            if let Some(transcript) = transcribe_audio_deepgram(&bytes, &mime) {
                let text = if base_text.trim().is_empty() {
                    format!("[Voice message transcription]: {transcript}")
                } else {
                    format!(
                        "[Voice message transcription]: {transcript}\n\nUser also wrote: {base_text}"
                    )
                };
                return Some((chat_id, msg_id, text));
            }
            return Some((
                chat_id,
                msg_id,
                "[User sent a voice message but transcription failed]".to_string(),
            ));
        }
        return Some((
            chat_id,
            msg_id,
            "[User sent a voice message but it could not be downloaded]".to_string(),
        ));
    }

    if let Some(audio) = &msg.audio {
        let mime = audio
            .mime_type
            .clone()
            .unwrap_or_else(|| "audio/mpeg".to_string());
        let title_note = audio
            .title
            .as_deref()
            .map(|t| format!(" (title: {t})"))
            .unwrap_or_default();
        if let Some((bytes, _ct)) = telegram_download_file_bytes(agent, base_url, &audio.file_id) {
            if let Some(transcript) = transcribe_audio_deepgram(&bytes, &mime) {
                let text = format!("[Audio{title_note} transcription]: {transcript}");
                return Some((chat_id, msg_id, text));
            }
            return Some((
                chat_id,
                msg_id,
                format!("[User sent an audio file{title_note} but transcription failed]"),
            ));
        }
        return Some((
            chat_id,
            msg_id,
            format!("[User sent an audio file{title_note} but it could not be downloaded]"),
        ));
    }

    if let Some(doc) = &msg.document {
        let fname = doc
            .file_name
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let mime = doc.mime_type.clone().unwrap_or_default();
        let is_text = mime.starts_with("text/")
            || mime == "application/json"
            || mime == "application/xml"
            || fname.ends_with(".txt")
            || fname.ends_with(".md")
            || fname.ends_with(".json")
            || fname.ends_with(".csv")
            || fname.ends_with(".py")
            || fname.ends_with(".rs")
            || fname.ends_with(".js")
            || fname.ends_with(".ts")
            || fname.ends_with(".sh")
            || fname.ends_with(".yaml")
            || fname.ends_with(".yml")
            || fname.ends_with(".toml");
        if is_text {
            if let Some((bytes, _ct)) = telegram_download_file_bytes(agent, base_url, &doc.file_id)
            {
                if let Ok(text_content) = String::from_utf8(bytes) {
                    let truncated = if text_content.len() > 50_000 {
                        format!(
                            "{}\n... (truncated, {} total chars)",
                            &text_content[..50_000],
                            text_content.len()
                        )
                    } else {
                        text_content
                    };
                    let text = format!("[Document: {fname}]\n```\n{truncated}\n```\n\n{base_text}");
                    return Some((chat_id, msg_id, text));
                }
            }
        }
        let text = if base_text.trim().is_empty() {
            format!(
                "[User sent a document: {fname} ({mime}). This file type is not supported for direct reading.]"
            )
        } else {
            format!("[User sent a document: {fname} ({mime})]\n{base_text}")
        };
        return Some((chat_id, msg_id, text));
    }

    if base_text.trim().is_empty() {
        return None;
    }
    Some((chat_id, msg_id, base_text))
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

fn telegram_send_typing(agent: &ureq::Agent, base_url: &str, chat_id: i64) {
    let url = format!("{base_url}/sendChatAction");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "action": "typing"
    });
    let _ = agent
        .post(&url)
        .set("content-type", "application/json")
        .send_json(payload);
}

fn telegram_answer_callback(
    agent: &ureq::Agent,
    base_url: &str,
    callback_id: &str,
    text: Option<&str>,
) {
    let url = format!("{base_url}/answerCallbackQuery");
    let mut payload = serde_json::json!({ "callback_query_id": callback_id });
    if let Some(t) = text {
        payload["text"] = serde_json::json!(t);
    }
    let _ = agent
        .post(&url)
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

fn telegram_send_message_ext(
    agent: &ureq::Agent,
    base_url: &str,
    chat_id: i64,
    text: &str,
    reply_to: Option<i64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{base_url}/sendMessage");
    let chunks = split_text_chunks(text, 3900);
    for (i, chunk) in chunks.iter().enumerate() {
        let mut payload = serde_json::json!({
            "chat_id": chat_id,
            "text": chunk,
            "parse_mode": "Markdown"
        });
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
            Ok(_) => {}
            Err(_) => {
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

fn run_telegram_bridge(
    token: String,
    poll_timeout: u64,
    poll_limit: usize,
    agent_config: BridgeAgentConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let base_url = match std::env::var("TELEGRAM_API_BASE") {
        Ok(base) => format!("{base}/bot{token}"),
        Err(_) => format!("https://api.telegram.org/bot{token}"),
    };
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(poll_timeout.saturating_add(10)))
        .build();

    let mut offset: Option<i64> = None;
    loop {
        let mut request = agent
            .get(&format!("{base_url}/getUpdates"))
            .query("timeout", &poll_timeout.to_string())
            .query("limit", &poll_limit.to_string());
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

        for entry in update.result {
            offset = Some(entry.update_id);

            if let Some(cb) = &entry.callback_query {
                telegram_answer_callback(&agent, &base_url, &cb.id, Some("Processing..."));
            }

            let Some((chat_id, reply_to_id, user_text)) =
                extract_telegram_content(&entry, &agent, &base_url)
            else {
                continue;
            };
            if let Some(output) = try_handle_approval_chat(&agent_config.mv2, &user_text) {
                if let Err(err) = telegram_send_message(&agent, &base_url, chat_id, &output) {
                    eprintln!("Telegram send failed: {err}");
                }
                continue;
            }

            telegram_send_typing(&agent, &base_url, chat_id);

            let session = format!("{}telegram:{chat_id}", agent_config.session_prefix);
            let typing_agent = agent.clone();
            let typing_url = base_url.clone();
            let typing_chat = chat_id;
            let typing_active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let typing_flag = typing_active.clone();
            let typing_thread = thread::spawn(move || {
                while typing_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    thread::sleep(Duration::from_secs(4));
                    if !typing_flag.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    telegram_send_typing(&typing_agent, &typing_url, typing_chat);
                }
            });

            let response = run_agent_for_bridge(&agent_config, &user_text, session, None, None);

            typing_active.store(false, std::sync::atomic::Ordering::Relaxed);
            let _ = typing_thread.join();

            let output = match response {
                Ok(result) => {
                    let mut text = result.final_text.unwrap_or_default();
                    if text.trim().is_empty() {
                        text = "Done.".to_string();
                    }
                    text
                }
                Err(err) => {
                    let err_str = err.to_string();
                    let detail = err_str.chars().take(500).collect::<String>();
                    format!(
                        "Something went wrong while processing your request.\n\nError: {detail}\n\nThis wasn't your fault. I can retry if you send the message again, or you can rephrase if you'd like to try a different approach."
                    )
                }
            };

            if let Err(err) =
                telegram_send_message_ext(&agent, &base_url, chat_id, &output, reply_to_id)
            {
                eprintln!("Telegram send failed: {err}");
            }
        }
    }
}

fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn run_whatsapp_bridge(
    bind: String,
    port: u16,
    agent_config: BridgeAgentConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{bind}:{port}");
    let server = Server::http(&addr)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("server: {e}")))?;
    eprintln!("WhatsApp bridge listening on http://{addr}");
    for mut request in server.incoming_requests() {
        if *request.method() != Method::Post {
            let response = Response::from_string("ok");
            let _ = request.respond(response);
            continue;
        }

        let mut body = String::new();
        request.as_reader().read_to_string(&mut body)?;
        let params: HashMap<String, String> = form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();

        let from = params.get("From").cloned().unwrap_or_default();
        let text = params.get("Body").cloned().unwrap_or_default();
        if from.trim().is_empty() || text.trim().is_empty() {
            let response = Response::from_string("missing body");
            let _ = request.respond(response);
            continue;
        }

        if let Some(output) = try_handle_approval_chat(&agent_config.mv2, &text) {
            let twiml = format!(
                "<Response><Message>{}</Message></Response>",
                escape_xml(&output)
            );
            let mut response = Response::from_string(twiml);
            let header = Header::from_bytes("Content-Type", "text/xml; charset=utf-8")
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "invalid header"))?;
            response.add_header(header);
            let _ = request.respond(response);
            continue;
        }

        let session = format!("{}whatsapp:{from}", agent_config.session_prefix);
        let response = run_agent_for_bridge(&agent_config, &text, session, None, None);
        let mut output = match response {
            Ok(result) => result.final_text.unwrap_or_default(),
            Err(err) => format!("Agent error: {err}"),
        };
        if output.trim().is_empty() {
            output = "Done.".to_string();
        }

        let twiml = format!(
            "<Response><Message>{}</Message></Response>",
            escape_xml(&output)
        );
        let mut response = Response::from_string(twiml);
        let header = Header::from_bytes("Content-Type", "text/xml; charset=utf-8")
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "invalid header"))?;
        response.add_header(header);
        let _ = request.respond(response);
    }
    Ok(())
}

fn parse_json_body(request: &mut tiny_http::Request) -> Result<serde_json::Value, String> {
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .map_err(|e| format!("read body: {e}"))?;
    if body.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&body).map_err(|e| format!("json: {e}"))
}

fn run_webhook_bridge(
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
        let payload = parse_json_body(&mut request).unwrap_or_else(|_| serde_json::json!({}));
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
        if let Some(output) = try_handle_approval_chat(&agent_config.mv2, &text) {
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
        let result = run_agent_for_bridge(&agent_config, &text, session, None, None);
        let output = match result {
            Ok(output) => output.final_text.unwrap_or_else(|| "Done.".to_string()),
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

fn payload_session_fallback(prefix: &str, payload: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    format!("{prefix}:{}", blake3_hash(&bytes).to_hex())
}

fn extract_slack_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload
        .get("event")
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            payload
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })?;
    let channel = payload
        .get("event")
        .and_then(|v| v.get("channel"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let user = payload
        .get("event")
        .and_then(|v| v.get("user"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session = if channel != "unknown" || user != "unknown" {
        format!("slack:{channel}:{user}")
    } else {
        payload_session_fallback("slack", payload)
    };
    Some((session, text))
}

fn extract_discord_event(payload: &serde_json::Value) -> Option<(String, String)> {
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

fn extract_teams_event(payload: &serde_json::Value) -> Option<(String, String)> {
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

fn extract_signal_event(payload: &serde_json::Value) -> Option<(String, String)> {
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

fn extract_matrix_event(payload: &serde_json::Value) -> Option<(String, String)> {
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

fn extract_imessage_event(payload: &serde_json::Value) -> Option<(String, String)> {
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

fn reply_none(_: &BridgeAgentConfig, _: &str) -> Option<String> {
    None
}

fn reply_slack(_: &BridgeAgentConfig, text: &str) -> Option<String> {
    Some(serde_json::json!({ "text": text }).to_string())
}

fn build_config(
    mv2: PathBuf,
    model_hook: Option<String>,
    system: Option<String>,
    no_memory: bool,
    context_query: Option<String>,
    context_results: usize,
    context_max_bytes: usize,
    max_steps: usize,
    log: bool,
    log_commit_interval: usize,
) -> Result<BridgeAgentConfig, Box<dyn std::error::Error>> {
    Ok(build_bridge_agent_config(
        mv2,
        model_hook,
        system,
        no_memory,
        context_query,
        context_results,
        context_max_bytes,
        max_steps,
        log,
        log_commit_interval,
    )?)
}

pub(crate) fn run_bridge(command: BridgeCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        BridgeCommand::Telegram {
            mv2,
            token,
            poll_timeout,
            poll_limit,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let token = token
                .or_else(|| env_optional("TELEGRAM_BOT_TOKEN"))
                .ok_or("Missing TELEGRAM_BOT_TOKEN")?;
            let config = build_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_telegram_bridge(token, poll_timeout, poll_limit, config)
        }
        BridgeCommand::Whatsapp {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let config = build_config(
                resolve_mv2_path(mv2),
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_whatsapp_bridge(bind, port, config)
        }
        BridgeCommand::Slack {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => run_webhook_bridge(
            "slack",
            bind,
            port,
            build_config(
                resolve_mv2_path(mv2),
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?,
            extract_slack_event,
            reply_slack,
        ),
        BridgeCommand::Discord {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => run_webhook_bridge(
            "discord",
            bind,
            port,
            build_config(
                resolve_mv2_path(mv2),
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?,
            extract_discord_event,
            reply_none,
        ),
        BridgeCommand::Teams {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => run_webhook_bridge(
            "teams",
            bind,
            port,
            build_config(
                resolve_mv2_path(mv2),
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?,
            extract_teams_event,
            reply_none,
        ),
        BridgeCommand::Signal {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
            sender: _,
        } => run_webhook_bridge(
            "signal",
            bind,
            port,
            build_config(
                resolve_mv2_path(mv2),
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?,
            extract_signal_event,
            reply_none,
        ),
        BridgeCommand::Matrix {
            mv2,
            room: _,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => run_webhook_bridge(
            "matrix",
            bind,
            port,
            build_config(
                resolve_mv2_path(mv2),
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?,
            extract_matrix_event,
            reply_none,
        ),
        BridgeCommand::IMessage {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => run_webhook_bridge(
            "imessage",
            bind,
            port,
            build_config(
                resolve_mv2_path(mv2),
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?,
            extract_imessage_event,
            reply_none,
        ),
    }
}

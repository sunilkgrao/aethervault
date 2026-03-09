use base64::Engine;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{MemoryDb, env_optional, load_config_json, save_config_entry};

const PHONE_CALL_CONFIG_PREFIX: &str = "phone.call.";
const DEFAULT_PHONE_VOICE: &str = "alice";

#[derive(Debug, Clone)]
pub(crate) struct TwilioCredentials {
    pub(crate) account_sid: String,
    pub(crate) auth_token: String,
    pub(crate) from_number: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PhoneCallAnswer {
    pub(crate) question_index: usize,
    pub(crate) question: String,
    #[serde(default)]
    pub(crate) digits: Option<String>,
    #[serde(default)]
    pub(crate) speech_result: Option<String>,
    #[serde(default)]
    pub(crate) confidence: Option<f64>,
    pub(crate) received_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PhoneCallRecord {
    pub(crate) request_id: String,
    #[serde(default)]
    pub(crate) call_sid: Option<String>,
    #[serde(default)]
    pub(crate) session: Option<String>,
    pub(crate) to: String,
    pub(crate) from: String,
    pub(crate) objective: String,
    pub(crate) script: String,
    pub(crate) voice: String,
    pub(crate) status: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) record: bool,
    pub(crate) machine_detection: bool,
    #[serde(default)]
    pub(crate) callback_base_url: Option<String>,
    #[serde(default)]
    pub(crate) questions: Vec<String>,
    #[serde(default)]
    pub(crate) answers: Vec<PhoneCallAnswer>,
    #[serde(default)]
    pub(crate) status_events: Vec<serde_json::Value>,
}

pub(crate) fn default_phone_voice() -> String {
    env_optional("TWILIO_VOICE_DEFAULT")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PHONE_VOICE.to_string())
}

pub(crate) fn resolve_public_base_url() -> Option<String> {
    env_optional("AETHERVAULT_PUBLIC_BASE_URL")
        .or_else(|| env_optional("PUBLIC_BASE_URL"))
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn phone_call_config_key(request_id: &str) -> String {
    format!("{PHONE_CALL_CONFIG_PREFIX}{request_id}")
}

pub(crate) fn load_phone_call_record(db: &MemoryDb, request_id: &str) -> Option<PhoneCallRecord> {
    load_config_json(db, &phone_call_config_key(request_id))
        .and_then(|value| serde_json::from_value(value).ok())
}

pub(crate) fn save_phone_call_record(
    db: &MemoryDb,
    record: &PhoneCallRecord,
) -> Result<(), String> {
    let payload = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
    save_config_entry(db, &phone_call_config_key(&record.request_id), &payload)
}

pub(crate) fn load_twilio_credentials(
    explicit_from: Option<&str>,
) -> Result<TwilioCredentials, String> {
    let account_sid = env_optional("TWILIO_ACCOUNT_SID")
        .filter(|value| !value.trim().is_empty())
        .ok_or("missing TWILIO_ACCOUNT_SID")?;
    let auth_token = env_optional("TWILIO_AUTH_TOKEN")
        .filter(|value| !value.trim().is_empty())
        .ok_or("missing TWILIO_AUTH_TOKEN")?;
    let from_number = explicit_from
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env_optional("TWILIO_VOICE_FROM")
                .or_else(|| env_optional("TWILIO_FROM_NUMBER"))
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .ok_or("missing TWILIO_VOICE_FROM or TWILIO_FROM_NUMBER")?;
    Ok(TwilioCredentials {
        account_sid,
        auth_token,
        from_number,
    })
}

pub(crate) fn twilio_calls_url(account_sid: &str) -> String {
    format!("https://api.twilio.com/2010-04-01/Accounts/{account_sid}/Calls.json")
}

pub(crate) fn twilio_call_url(account_sid: &str, call_sid: &str) -> String {
    format!("https://api.twilio.com/2010-04-01/Accounts/{account_sid}/Calls/{call_sid}.json")
}

pub(crate) fn twilio_basic_auth_header(account_sid: &str, auth_token: &str) -> String {
    let token = format!("{account_sid}:{auth_token}");
    let encoded = base64::engine::general_purpose::STANDARD.encode(token.as_bytes());
    format!("Basic {encoded}")
}

pub(crate) fn escape_twiml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
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

pub(crate) fn build_phone_call_completion_twiml(voice: &str, message: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<Response><Say voice=\"{}\">{}</Say><Hangup/></Response>",
        escape_twiml(voice),
        escape_twiml(message)
    )
}

pub(crate) fn build_phone_call_gather_twiml(
    record: &PhoneCallRecord,
    question_index: usize,
) -> Result<String, String> {
    if question_index >= record.questions.len() {
        return Ok(build_phone_call_completion_twiml(
            &record.voice,
            "Thank you. I've captured the information and will pass it along.",
        ));
    }
    let Some(base_url) = record.callback_base_url.as_ref() else {
        return Err("interactive phone calls require AETHERVAULT_PUBLIC_BASE_URL".to_string());
    };
    let action = format!(
        "{}/twilio/voice/gather?request_id={}&question_index={}",
        base_url.trim_end_matches('/'),
        urlencoding::encode(&record.request_id),
        question_index
    );
    let mut parts = Vec::new();
    parts.push("<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_string());
    parts.push("<Response>".to_string());
    if question_index == 0 && !record.script.trim().is_empty() {
        parts.push(format!(
            "<Say voice=\"{}\">{}</Say>",
            escape_twiml(&record.voice),
            escape_twiml(&record.script)
        ));
    }
    parts.push(format!(
        "<Gather input=\"speech dtmf\" action=\"{}\" method=\"POST\" speechTimeout=\"auto\" timeout=\"6\">\
<Say voice=\"{}\">{}</Say></Gather>",
        escape_twiml(&action),
        escape_twiml(&record.voice),
        escape_twiml(&record.questions[question_index])
    ));
    parts.push(format!(
        "<Say voice=\"{}\">{}</Say>",
        escape_twiml(&record.voice),
        escape_twiml("I didn't catch that. Please call or text Sunil back. Goodbye.")
    ));
    parts.push("<Hangup/></Response>".to_string());
    Ok(parts.join(""))
}

pub(crate) fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::{
        PhoneCallRecord, build_phone_call_completion_twiml, build_phone_call_gather_twiml,
        escape_twiml,
    };

    #[test]
    fn escape_twiml_handles_special_characters() {
        assert_eq!(
            escape_twiml("Call <Mom> & say \"hi\""),
            "Call &lt;Mom&gt; &amp; say &quot;hi&quot;"
        );
    }

    #[test]
    fn build_completion_twiml_contains_message_and_hangup() {
        let xml = build_phone_call_completion_twiml("alice", "Done.");
        assert!(xml.contains("<Say voice=\"alice\">Done.</Say>"));
        assert!(xml.contains("<Hangup/>"));
    }

    #[test]
    fn build_gather_twiml_includes_first_question_and_callback() {
        let record = PhoneCallRecord {
            request_id: "req-1".to_string(),
            call_sid: None,
            session: None,
            to: "+15551234567".to_string(),
            from: "+15557654321".to_string(),
            objective: "doctor appointment".to_string(),
            script: "This is Linus calling for Sunil Rao.".to_string(),
            voice: "alice".to_string(),
            status: "queued".to_string(),
            created_at: "2026-03-07T00:00:00Z".to_string(),
            updated_at: "2026-03-07T00:00:00Z".to_string(),
            record: false,
            machine_detection: true,
            callback_base_url: Some("https://linus.example.com".to_string()),
            questions: vec!["What appointments are available next week?".to_string()],
            answers: Vec::new(),
            status_events: Vec::new(),
        };
        let xml = build_phone_call_gather_twiml(&record, 0).unwrap();
        assert!(xml.contains(
            "https://linus.example.com/twilio/voice/gather?request_id=req-1&amp;question_index=0"
        ));
        assert!(xml.contains("What appointments are available next week?"));
    }
}

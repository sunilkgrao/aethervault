use std::collections::HashMap;
use std::io;
use std::io::Read;
use std::path::PathBuf;

use tiny_http::{Header, Method, Response, Server};
use url::form_urlencoded;

use crate::{
    PhoneCallAnswer, build_phone_call_completion_twiml, build_phone_call_gather_twiml,
    load_phone_call_record, now_rfc3339, open_or_create_db, save_phone_call_record,
};

const DEFAULT_MAX_BODY_BYTES: usize = 512 * 1024;

fn read_request_body(request: &mut tiny_http::Request) -> Result<String, String> {
    if let Some(content_length) = request.body_length() {
        if content_length > DEFAULT_MAX_BODY_BYTES {
            return Err("payload too large".to_string());
        }
    }
    let mut body = String::new();
    request
        .as_reader()
        .take(DEFAULT_MAX_BODY_BYTES.saturating_add(1) as u64)
        .read_to_string(&mut body)
        .map_err(|e| format!("read body: {e}"))?;
    if body.len() > DEFAULT_MAX_BODY_BYTES {
        return Err("payload too large".to_string());
    }
    Ok(body)
}

fn xml_response(
    body: String,
) -> Result<Response<std::io::Cursor<Vec<u8>>>, Box<dyn std::error::Error>> {
    let mut response = Response::from_string(body);
    let header = Header::from_bytes("Content-Type", "text/xml; charset=utf-8")
        .map_err(|_| io::Error::other("invalid header"))?;
    response.add_header(header);
    Ok(response)
}

fn parse_query(url: &str) -> (String, HashMap<String, String>) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let params = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect::<HashMap<_, _>>();
    (path.to_string(), params)
}

pub(crate) fn run_twilio_voice_bridge(
    bind: String,
    port: u16,
    mv2: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{bind}:{port}");
    let server = Server::http(&addr).map_err(|e| io::Error::other(format!("server: {e}")))?;
    eprintln!("Twilio Voice bridge listening on http://{addr}");

    for mut request in server.incoming_requests() {
        if *request.method() == Method::Get {
            let response = Response::from_string("ok");
            let _ = request.respond(response);
            continue;
        }
        if *request.method() != Method::Post {
            let response = Response::from_string("method not allowed").with_status_code(405);
            let _ = request.respond(response);
            continue;
        }

        let body = match read_request_body(&mut request) {
            Ok(body) => body,
            Err(err) => {
                let status = if err == "payload too large" { 413 } else { 400 };
                let response = Response::from_string(err).with_status_code(status);
                let _ = request.respond(response);
                continue;
            }
        };
        let params: HashMap<String, String> = form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
        let (path, query) = parse_query(request.url());
        let request_id = match query.get("request_id") {
            Some(value) if !value.trim().is_empty() => value.trim().to_string(),
            _ => {
                let response = Response::from_string("missing request_id").with_status_code(400);
                let _ = request.respond(response);
                continue;
            }
        };

        let db = match open_or_create_db(&mv2) {
            Ok(db) => db,
            Err(err) => {
                let response =
                    Response::from_string(format!("db error: {err}")).with_status_code(500);
                let _ = request.respond(response);
                continue;
            }
        };
        let Some(mut record) = load_phone_call_record(&db, &request_id) else {
            let response = Response::from_string("unknown request_id").with_status_code(404);
            let _ = request.respond(response);
            continue;
        };

        let now = now_rfc3339();
        if let Some(call_sid) = params.get("CallSid").cloned() {
            record.call_sid = Some(call_sid);
        }
        record.updated_at = now.clone();

        let response = match path.as_str() {
            "/twilio/voice/status" => {
                if let Some(status) = params.get("CallStatus").cloned() {
                    record.status = status;
                }
                record.status_events.push(serde_json::json!({
                    "kind": "status",
                    "at": now,
                    "payload": params,
                }));
                if let Err(err) = save_phone_call_record(&db, &record) {
                    Response::from_string(format!("save error: {err}")).with_status_code(500)
                } else {
                    xml_response("<Response/>".to_string())?
                }
            }
            "/twilio/voice/gather" => {
                let question_index = query
                    .get("question_index")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                if let Some(question) = record.questions.get(question_index).cloned() {
                    let answer = PhoneCallAnswer {
                        question_index,
                        question,
                        digits: params.get("Digits").cloned(),
                        speech_result: params.get("SpeechResult").cloned(),
                        confidence: params.get("Confidence").and_then(|v| v.parse::<f64>().ok()),
                        received_at: now.clone(),
                    };
                    record
                        .answers
                        .retain(|item| item.question_index != question_index);
                    record.answers.push(answer);
                    record
                        .answers
                        .sort_by(|left, right| left.question_index.cmp(&right.question_index));
                }
                record.status_events.push(serde_json::json!({
                    "kind": "gather",
                    "at": now,
                    "payload": params,
                }));

                let speech = params
                    .get("SpeechResult")
                    .map(|value| value.trim())
                    .unwrap_or_default();
                let digits = params
                    .get("Digits")
                    .map(|value| value.trim())
                    .unwrap_or_default();
                let reply_twiml = if speech.is_empty() && digits.is_empty() {
                    record.status = "no_response".to_string();
                    build_phone_call_completion_twiml(
                        &record.voice,
                        "I didn't receive a response. Please call or text Sunil back when convenient.",
                    )
                } else if question_index + 1 < record.questions.len() {
                    record.status = "collecting".to_string();
                    build_phone_call_gather_twiml(&record, question_index + 1).unwrap_or_else(
                        |_| {
                            build_phone_call_completion_twiml(
                                &record.voice,
                                "Thank you. I've captured the information and will pass it along.",
                            )
                        },
                    )
                } else {
                    record.status = "completed".to_string();
                    build_phone_call_completion_twiml(
                        &record.voice,
                        "Thank you. I've captured the information and will pass it along.",
                    )
                };

                if let Err(err) = save_phone_call_record(&db, &record) {
                    Response::from_string(format!("save error: {err}")).with_status_code(500)
                } else {
                    xml_response(reply_twiml)?
                }
            }
            _ => Response::from_string("not found").with_status_code(404),
        };
        let _ = request.respond(response);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_query;

    #[test]
    fn parse_query_extracts_path_and_params() {
        let (path, query) = parse_query("/twilio/voice/gather?request_id=req-1&question_index=2");
        assert_eq!(path, "/twilio/voice/gather");
        assert_eq!(query.get("request_id").map(|s| s.as_str()), Some("req-1"));
        assert_eq!(query.get("question_index").map(|s| s.as_str()), Some("2"));
    }
}

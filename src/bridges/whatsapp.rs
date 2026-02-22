use std::io::Read;
use std::collections::HashMap;
use std::io;
use std::env;

use tiny_http::{Header, Method, Response, Server};
use url::form_urlencoded;

use crate::{
    try_handle_approval_chat, BridgeAgentConfig,
};
use crate::bridges::run_agent_for_bridge;

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

pub(crate) fn escape_xml(input: &str) -> String {
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

pub(crate) fn run_whatsapp_bridge(
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

        let body = match read_request_body(&mut request) {
            Ok(body) => body,
            Err(err) => {
                if err == "payload too large" {
                    let response = Response::from_string("payload too large")
                        .with_status_code(413);
                    let _ = request.respond(response);
                    continue;
                }

                let response = Response::from_string(format!("bad request: {err}"))
                    .with_status_code(400);
                let _ = request.respond(response);
                continue;
            }
        };

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

        if let Some(output) = try_handle_approval_chat(&agent_config.db_path, &text) {
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
        let response = run_agent_for_bridge(&agent_config, &text, session, None, None, None);
        let mut output = match response {
            Ok(result) => result.final_text.unwrap_or_default(),
            Err(err) => format!("Agent error: {err}"),
        };
        if output.trim().is_empty() {
            output = "\u{2705}".to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_xml_no_special_chars() {
        assert_eq!(escape_xml("hello world"), "hello world");
    }

    #[test]
    fn escape_xml_ampersand() {
        assert_eq!(escape_xml("a & b"), "a &amp; b");
    }

    #[test]
    fn escape_xml_angle_brackets() {
        assert_eq!(escape_xml("<tag>"), "&lt;tag&gt;");
    }

    #[test]
    fn escape_xml_quotes() {
        assert_eq!(escape_xml(r#"say "hello""#), "say &quot;hello&quot;");
    }

    #[test]
    fn escape_xml_apostrophe() {
        assert_eq!(escape_xml("it's"), "it&apos;s");
    }

    #[test]
    fn escape_xml_all_special() {
        assert_eq!(
            escape_xml(r#"<a href="x">&'test'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&apos;test&apos;&lt;/a&gt;"
        );
    }

    #[test]
    fn escape_xml_empty() {
        assert_eq!(escape_xml(""), "");
    }
}

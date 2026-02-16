#[allow(unused_imports)]
use std::io::Write;
use std::collections::HashMap;
#[allow(unused_imports)]
use std::io::{self, Read};

use tiny_http::{Header, Method, Response, Server};
use url::form_urlencoded;

use aether_core::Vault;
use crate::{
    config_file_path, env_optional, load_capsule_config, load_config_from_file,
    load_subagents_from_config, try_handle_approval_chat, BridgeAgentConfig,
};
use crate::bridges::run_agent_for_bridge;

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
    let (_config, _subagent_specs) = {
        // Try flat file config first, fall back to capsule.
        let ws_env = env_optional("AETHERVAULT_WORKSPACE").map(std::path::PathBuf::from);
        let config = if let Some(ref ws) = ws_env {
            let cfg_path = config_file_path(ws);
            if cfg_path.exists() {
                load_config_from_file(ws)
            } else {
                let mut mem = Vault::open_read_only(&agent_config.mv2)?;
                load_capsule_config(&mut mem).unwrap_or_default()
            }
        } else {
            let mut mem = Vault::open_read_only(&agent_config.mv2)?;
            load_capsule_config(&mut mem).unwrap_or_default()
        };
        let subagent_specs = load_subagents_from_config(&config);
        (config, subagent_specs)
    };

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

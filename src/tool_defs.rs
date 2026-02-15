#[allow(unused_imports)]
use std::collections::{HashMap, HashSet};

use serde_json;

use super::{CapsuleConfig, SubagentSpec};

pub(crate) fn tool_definitions_json() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "query",
            "description": "Hybrid search over the capsule (expansion + fusion + rerank).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "collection": { "type": "string" },
                    "limit": { "type": "integer" },
                    "snippet_chars": { "type": "integer" },
                    "no_expand": { "type": "boolean" },
                    "max_expansions": { "type": "integer" },
                    "no_vector": { "type": "boolean" },
                    "rerank": { "type": "string" },
                    "asof": { "type": "string" },
                    "before": { "type": "string" },
                    "after": { "type": "string" },
                    "feedback_weight": { "type": "number" }
                },
                "required": ["query"]
            }
        }),
        serde_json::json!({
            "name": "context",
            "description": "Build a prompt-ready context pack from the capsule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "collection": { "type": "string" },
                    "limit": { "type": "integer" },
                    "snippet_chars": { "type": "integer" },
                    "max_bytes": { "type": "integer" },
                    "full": { "type": "boolean" },
                    "no_expand": { "type": "boolean" },
                    "max_expansions": { "type": "integer" },
                    "no_vector": { "type": "boolean" },
                    "rerank": { "type": "string" },
                    "asof": { "type": "string" },
                    "before": { "type": "string" },
                    "after": { "type": "string" },
                    "feedback_weight": { "type": "number" }
                },
                "required": ["query"]
            }
        }),
        serde_json::json!({
            "name": "search",
            "description": "Lexical search over the capsule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "collection": { "type": "string" },
                    "limit": { "type": "integer" },
                    "snippet_chars": { "type": "integer" }
                },
                "required": ["query"]
            }
        }),
        serde_json::json!({
            "name": "get",
            "description": "Fetch a document by URI or frame id (#123).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" }
                },
                "required": ["id"]
            }
        }),
        serde_json::json!({
            "name": "put",
            "description": "Store a text payload into the capsule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "uri": { "type": "string" },
                    "title": { "type": "string" },
                    "text": { "type": "string" },
                    "kind": { "type": "string" },
                    "track": { "type": "string" }
                },
                "required": ["uri", "text"]
            }
        }),
        serde_json::json!({
            "name": "log",
            "description": "Append an agent turn to the capsule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "role": { "type": "string" },
                    "text": { "type": "string" },
                    "meta": { "type": "object" }
                },
                "required": ["text"]
            }
        }),
        serde_json::json!({
            "name": "feedback",
            "description": "Store feedback for a URI (range -1.0 to 1.0).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "uri": { "type": "string" },
                    "score": { "type": "number" },
                    "note": { "type": "string" },
                    "session": { "type": "string" }
                },
                "required": ["uri", "score"]
            }
        }),
        serde_json::json!({
            "name": "config_set",
            "description": "Set a config JSON document at aethervault://config/<key>.json.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "json": { "type": "object" }
                },
                "required": ["key", "json"]
            }
        }),
        serde_json::json!({
            "name": "memory_append_daily",
            "description": "Append a line to the daily memory log (workspace) and store in the capsule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "date": { "type": "string" }
                },
                "required": ["text"]
            }
        }),
        serde_json::json!({
            "name": "memory_remember",
            "description": "Append a line to MEMORY.md (workspace) and store in the capsule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            }
        }),
        serde_json::json!({
            "name": "memory_sync",
            "description": "Sync workspace memory files into the capsule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace": { "type": "string" },
                    "include_daily": { "type": "boolean" }
                }
            }
        }),
        serde_json::json!({
            "name": "memory_export",
            "description": "Export capsule memory back to workspace files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace": { "type": "string" },
                    "include_daily": { "type": "boolean" }
                }
            }
        }),
        serde_json::json!({
            "name": "memory_search",
            "description": "Search memory stored in the capsule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                },
                "required": ["query"]
            }
        }),
        serde_json::json!({
            "name": "email_list",
            "description": "List email envelopes via Himalaya.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder": { "type": "string" },
                    "limit": { "type": "number" }
                }
            }
        }),
        serde_json::json!({
            "name": "email_read",
            "description": "Read a full message via Himalaya.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "account": { "type": "string" },
                    "folder": { "type": "string" }
                },
                "required": ["id"]
            }
        }),
        serde_json::json!({
            "name": "email_send",
            "description": "Send an email via Himalaya template.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": { "type": "string" },
                    "cc": { "type": "string" },
                    "bcc": { "type": "string" },
                    "subject": { "type": "string" },
                    "body": { "type": "string" },
                    "from": { "type": "string" },
                    "in_reply_to": { "type": "string" },
                    "references": { "type": "string" }
                },
                "required": ["to", "subject", "body"]
            }
        }),
        serde_json::json!({
            "name": "email_archive",
            "description": "Archive an email (move to Archive) via Himalaya.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "account": { "type": "string" },
                    "folder": { "type": "string" }
                },
                "required": ["id"]
            }
        }),
        serde_json::json!({
            "name": "exec",
            "description": "Execute a shell command on the host (use with care).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "cwd": { "type": "string" },
                    "timeout_ms": { "type": "integer" },
                    "estimated_ms": { "type": "integer", "description": "Optional runtime estimate used by background scheduling (milliseconds)." },
                    "background": { "type": "boolean", "description": "Force background execution even below five-minute threshold." }
                },
                "required": ["command"]
            }
        }),
        serde_json::json!({
            "name": "notify",
            "description": "Send a notification to Slack/Discord/Teams via webhook.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "channel": { "type": "string" },
                    "text": { "type": "string" },
                    "webhook": { "type": "string" }
                },
                "required": ["text"]
            }
        }),
        serde_json::json!({
            "name": "signal_send",
            "description": "Send a Signal message via signal-cli.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": { "type": "string" },
                    "text": { "type": "string" },
                    "sender": { "type": "string" }
                },
                "required": ["to", "text"]
            }
        }),
        serde_json::json!({
            "name": "imessage_send",
            "description": "Send an iMessage (macOS only).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": { "type": "string" },
                    "text": { "type": "string" }
                },
                "required": ["to", "text"]
            }
        }),
        serde_json::json!({
            "name": "http_request",
            "description": "Generic HTTP request (GET allowed without approval; other methods may require approval).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "method": { "type": "string" },
                    "url": { "type": "string" },
                    "headers": { "type": "object" },
                    "body": { "type": "string" },
                    "json": { "type": "boolean" },
                    "timeout_ms": { "type": "integer" }
                },
                "required": ["url"]
            }
        }),
        serde_json::json!({
            "name": "browser",
            "description": "Browser automation via agent-browser CLI. Uses ref-based element selection from accessibility snapshots. Workflow: 1) 'open <url>' to navigate, 2) 'snapshot' to get element refs (@e1, @e2...), 3) interact using refs ('click @e1', 'fill @e2 text'). Sessions persist across calls. Commands: open, snapshot, click, fill, type, press, select, scroll, screenshot, pdf, get text/html/value, wait, eval, cookies, tab, back, forward, reload, close. Use 'find role/text/label' for semantic element finding.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The agent-browser command (e.g., 'open https://example.com', 'snapshot', 'click @e2', 'fill @e3 hello')" },
                    "session": { "type": "string", "description": "Session name for browser isolation. Defaults to 'default'." },
                    "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds. Default u64::MAX (no deadline)." }
                },
                "required": ["command"]
            }
        }),
        serde_json::json!({
            "name": "excalidraw",
            "description": "Create hand-drawn diagrams via Excalidraw MCP server. Actions: 'read_me' returns the element format reference (call before first create_view), 'create_view' renders a diagram from Excalidraw JSON elements. Requires excalidraw-mcp server (set EXCALIDRAW_MCP_CMD to override startup command, default: 'npx excalidraw-mcp --stdio').",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "description": "Action: 'read_me' (get element format reference) or 'create_view' (render diagram)" },
                    "elements": { "type": "string", "description": "JSON array of Excalidraw elements (required for create_view)" }
                },
                "required": ["action"]
            }
        }),
        serde_json::json!({
            "name": "fs_list",
            "description": "List files within allowed roots.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "recursive": { "type": "boolean" },
                    "max_entries": { "type": "integer" }
                },
                "required": ["path"]
            }
        }),
        serde_json::json!({
            "name": "fs_read",
            "description": "Read a file within allowed roots.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "max_bytes": { "type": "integer" }
                },
                "required": ["path"]
            }
        }),
        serde_json::json!({
            "name": "fs_write",
            "description": "Write a file within allowed roots.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "text": { "type": "string" },
                    "append": { "type": "boolean" }
                },
                "required": ["path", "text"]
            }
        }),
        serde_json::json!({
            "name": "approval_list",
            "description": "List pending approval requests.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "trigger_add",
            "description": "Add an event trigger. Kinds: email (Gmail query), calendar_free (Google Calendar window), cron (cron expression schedule), webhook (HTTP endpoint change detection).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "description": "Trigger kind: email, calendar_free, cron, or webhook" },
                    "name": { "type": "string", "description": "Human-readable trigger name" },
                    "query": { "type": "string", "description": "Gmail query (for kind=email)" },
                    "prompt": { "type": "string", "description": "Prompt to send to agent when trigger fires" },
                    "start": { "type": "string", "description": "Window start (for kind=calendar_free)" },
                    "end": { "type": "string", "description": "Window end (for kind=calendar_free)" },
                    "cron": { "type": "string", "description": "Cron expression: 'min hour dom month dow' (for kind=cron). Example: '0 9 * * 1-5' = weekdays 9am" },
                    "webhook_url": { "type": "string", "description": "URL to poll (for kind=webhook)" },
                    "webhook_method": { "type": "string", "description": "HTTP method for webhook (default: GET)" },
                    "enabled": { "type": "boolean" }
                },
                "required": ["kind"]
            }
        }),
        serde_json::json!({
            "name": "trigger_list",
            "description": "List configured triggers.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "trigger_remove",
            "description": "Remove a trigger by id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" }
                },
                "required": ["id"]
            }
        }),
        serde_json::json!({
            "name": "tool_search",
            "description": "Search available tools by name/description.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                },
                "required": ["query"]
            }
        }),
        serde_json::json!({
            "name": "session_context",
            "description": "Fetch recent log entries for a session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "limit": { "type": "integer" }
                },
                "required": ["session"]
            }
        }),
        serde_json::json!({
            "name": "reflect",
            "description": "Store a self-critique reflection in the capsule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "session": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["text"]
            }
        }),
        serde_json::json!({
            "name": "skill_store",
            "description": "Store a reusable procedure as a skill.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "trigger": { "type": "string" },
                    "steps": { "type": "array", "items": { "type": "string" } },
                    "tools": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["name"]
            }
        }),
        serde_json::json!({
            "name": "skill_search",
            "description": "Search stored skills.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                },
                "required": ["query"]
            }
        }),
        serde_json::json!({
            "name": "subagent_list",
            "description": "List configured subagents.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "subagent_invoke",
            "description": "Invoke a named subagent with a prompt.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "prompt": { "type": "string" },
                    "system": { "type": "string" },
                    "model_hook": { "type": "string" }
                },
                "required": ["name", "prompt"]
            }
        }),
        serde_json::json!({
            "name": "subagent_batch",
            "description": "Invoke multiple subagents concurrently. Each invocation runs in its own thread with independent capsule access. Returns all results once every subagent completes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "invocations": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "prompt": { "type": "string" },
                                "system": { "type": "string" },
                                "model_hook": { "type": "string" }
                            },
                            "required": ["name", "prompt"]
                        }
                    }
                },
                "required": ["invocations"]
            }
        }),
        serde_json::json!({
            "name": "gmail_list",
            "description": "List Gmail messages (OAuth).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "max_results": { "type": "integer" }
                }
            }
        }),
        serde_json::json!({
            "name": "gmail_read",
            "description": "Read a Gmail message by id (OAuth).",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
        serde_json::json!({
            "name": "gmail_send",
            "description": "Send a Gmail message (OAuth).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": { "type": "string" },
                    "subject": { "type": "string" },
                    "body": { "type": "string" }
                },
                "required": ["to", "subject", "body"]
            }
        }),
        serde_json::json!({
            "name": "gcal_list",
            "description": "List Google Calendar events (OAuth).",
            "inputSchema": {
                "type": "object",
                "properties": { "max_results": { "type": "integer" } }
            }
        }),
        serde_json::json!({
            "name": "gcal_create",
            "description": "Create a Google Calendar event on primary calendar (OAuth).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "summary": { "type": "string" },
                    "start": { "type": "string" },
                    "end": { "type": "string" },
                    "description": { "type": "string" }
                },
                "required": ["summary", "start", "end"]
            }
        }),
        serde_json::json!({
            "name": "ms_mail_list",
            "description": "List Microsoft mail messages (OAuth).",
            "inputSchema": {
                "type": "object",
                "properties": { "top": { "type": "integer" } }
            }
        }),
        serde_json::json!({
            "name": "ms_mail_read",
            "description": "Read Microsoft mail message by id (OAuth).",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
        serde_json::json!({
            "name": "ms_calendar_list",
            "description": "List Microsoft calendar events (OAuth).",
            "inputSchema": {
                "type": "object",
                "properties": { "top": { "type": "integer" } }
            }
        }),
        serde_json::json!({
            "name": "ms_calendar_create",
            "description": "Create Microsoft calendar event (OAuth).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "subject": { "type": "string" },
                    "start": { "type": "string" },
                    "end": { "type": "string" },
                    "body": { "type": "string" }
                },
                "required": ["subject", "start", "end"]
            }
        }),
        serde_json::json!({
            "name": "scale",
            "description": "Monitor and scale infrastructure resources. Actions: 'status' (CPU/RAM/disk/load), 'sizes' (list available DigitalOcean droplet sizes with pricing), 'resize' (scale droplet up/down, requires size param and approval).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "resize", "sizes"]
                    },
                    "size": {
                        "type": "string",
                        "description": "Target droplet size slug (e.g. s-2vcpu-4gb). Required for resize."
                    }
                },
                "required": ["action"]
            }
        }),
    ]
}

pub(crate) fn tool_score(query_tokens: &[String], name: &str, description: &str) -> i32 {
    let mut score = 0;
    let name_lc = name.to_ascii_lowercase();
    let desc_lc = description.to_ascii_lowercase();
    let query_joined = query_tokens.join(" ");
    for token in query_tokens {
        if token.is_empty() {
            continue;
        }
        if name_lc == *token {
            score += 6;
        } else if name_lc.contains(token) {
            score += 3;
        }
        if desc_lc.contains(token) {
            score += 1;
        }
    }
    if name_lc.contains(&query_joined) {
        score += 4;
    }
    if desc_lc.contains(&query_joined) {
        score += 2;
    }
    score
}

pub(crate) fn load_subagents_from_config(config: &CapsuleConfig) -> Vec<SubagentSpec> {
    config
        .agent
        .as_ref()
        .map(|a| a.subagents.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.name.trim().is_empty())
        .collect()
}

pub(crate) fn tool_catalog_map(catalog: &[serde_json::Value]) -> HashMap<String, serde_json::Value> {
    let mut map = HashMap::new();
    for tool in catalog {
        if let Some(name) = tool.get("name").and_then(|v| v.as_str()) {
            map.insert(name.to_string(), tool.clone());
        }
    }
    map
}

pub(crate) fn base_tool_names() -> HashSet<String> {
    [
        "tool_search",
        "query",
        "context",
        "search",
        "get",
        "session_context",
        "config_set",
        "memory_append_daily",
        "memory_remember",
        "memory_search",
        "memory_sync",
        "memory_export",
        "reflect",
        "skill_store",
        "skill_search",
        "trigger_add",
        "trigger_list",
        "trigger_remove",
        "subagent_list",
        "subagent_invoke",
        "subagent_batch",
        "approval_list",
        "scale",
        "browser",
        "excalidraw",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}

pub(crate) fn tools_from_active(
    map: &HashMap<String, serde_json::Value>,
    active: &HashSet<String>,
) -> Vec<serde_json::Value> {
    let mut tools = Vec::new();
    for name in active {
        if let Some(tool) = map.get(name) {
            tools.push(tool.clone());
        }
    }
    tools.sort_by(|a, b| {
        a.get("name")
            .and_then(|v| v.as_str())
            .cmp(&b.get("name").and_then(|v| v.as_str()))
    });
    tools
}

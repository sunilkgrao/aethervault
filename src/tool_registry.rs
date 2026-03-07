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
            "name": "state_list",
            "description": "List executive-state items such as priorities, open loops, waiting-fors, drafts, and follow-ups.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string" },
                    "status": { "type": "string" },
                    "include_closed": { "type": "boolean" },
                    "limit": { "type": "integer" }
                }
            }
        }),
        serde_json::json!({
            "name": "state_focus",
            "description": "Render a concise executive-state focus brief with top open loops, upcoming deadlines, and waiting-fors.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer" },
                    "include_notes": { "type": "boolean" }
                }
            }
        }),
        serde_json::json!({
            "name": "state_capture",
            "description": "Create or update an executive-state item. Use this to track commitments, priorities, blockers, waiting-fors, drafts, and next actions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "kind": { "type": "string" },
                    "status": { "type": "string" },
                    "next_action": { "type": "string" },
                    "due": { "type": "string" },
                    "waiting_on": { "type": "string" },
                    "note": { "type": "string" },
                    "source": { "type": "string" },
                    "session": { "type": "string" }
                }
            }
        }),
        serde_json::json!({
            "name": "state_close",
            "description": "Close an executive-state item when it is done, canceled, or no longer relevant.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "resolution": { "type": "string" }
                },
                "required": ["id"]
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
                    "timeout_ms": { "type": "integer" }
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
            "name": "browser_request",
            "description": "Send a browser automation request to the configured browser broker.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string" },
                    "url": { "type": "string" },
                    "selector": { "type": "string" },
                    "text": { "type": "string" },
                    "data": { "type": "object" }
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
            "description": "Add an event trigger (email or calendar_free).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string" },
                    "name": { "type": "string" },
                    "query": { "type": "string" },
                    "prompt": { "type": "string" },
                    "start": { "type": "string" },
                    "end": { "type": "string" },
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
            "description": "List configured reusable subagent templates. You can also create ad-hoc subagents by providing a custom system prompt to subagent_invoke or subagent_batch.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "subagent_invoke",
            "description": "Invoke a subagent with a prompt. The name can match a configured template, or be an ad-hoc role if you provide a custom system prompt. Runtime policy can inherit from the capsule config or be overridden per invocation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "prompt": { "type": "string" },
                    "system": { "type": "string" },
                    "model_hook": { "type": "string" },
                    "context_query": { "type": "string" },
                    "max_context_results": { "type": "integer" },
                    "max_context_bytes": { "type": "integer" },
                    "max_steps": { "type": "integer" },
                    "log": { "type": "boolean" },
                    "log_commit_interval": { "type": "integer" },
                    "no_memory": { "type": "boolean" }
                },
                "required": ["name", "prompt"]
            }
        }),
        serde_json::json!({
            "name": "subagent_batch",
            "description": "Invoke any number of subagents concurrently. Each invocation runs in its own thread with independent capsule access. Use this when the task benefits from decomposition or parallel specialist work; names can be configured templates or ad-hoc roles when system prompts are provided, and each invocation can override its own runtime policy.",
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
                                "model_hook": { "type": "string" },
                                "context_query": { "type": "string" },
                                "max_context_results": { "type": "integer" },
                                "max_context_bytes": { "type": "integer" },
                                "max_steps": { "type": "integer" },
                                "log": { "type": "boolean" },
                                "log_commit_interval": { "type": "integer" },
                                "no_memory": { "type": "boolean" }
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
            "description": "Report local host resource status: CPU, load, memory, and disk usage.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status"]
                    }
                },
                "required": ["action"]
            }
        }),
    ]
}

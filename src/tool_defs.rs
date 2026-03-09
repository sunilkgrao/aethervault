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
                    "limit": { "type": "integer" }
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
            "description": "Execute a shell command. Default timeout: 2 minutes. SSH commands auto-timeout at 60s. Build commands (cargo, npm, make) get 5 minutes. Set timeout_ms to override. Use background=true for commands expected to run >5 minutes. Do NOT use exec to spawn LLM processes (codex, ollama) — use subagent_invoke or subagent_batch instead.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute. For LLM delegation, use subagent_invoke instead." },
                    "cwd": { "type": "string" },
                    "timeout_ms": { "type": "integer", "description": "Hard timeout in ms. Default: 120000 (2min). Max: 600000 (10min). SSH auto-gets 60s, builds auto-get 300s. Use background=true for longer." },
                    "estimated_ms": { "type": "integer", "description": "Expected runtime in ms. Helps the system choose appropriate monitoring." },
                    "background": { "type": "boolean", "description": "Run in background job queue. Required for commands expected to run >10 minutes. Returns a job ID for status checking." }
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
            "name": "phone_call",
            "description": "Place an outbound phone call using Twilio Voice. Supports a spoken script plus optional structured question capture via the Twilio voice callback bridge. Requires approval.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "Destination phone number in E.164 format." },
                    "objective": { "type": "string", "description": "Why the assistant is calling." },
                    "script": { "type": "string", "description": "Opening script Linus should say before any questions." },
                    "session": { "type": "string", "description": "Optional agent session id to associate with the call record." },
                    "from": { "type": "string", "description": "Override caller ID. Defaults to TWILIO_VOICE_FROM / TWILIO_FROM_NUMBER." },
                    "voice": { "type": "string", "description": "Twilio <Say> voice name. Defaults to TWILIO_VOICE_DEFAULT or 'alice'." },
                    "questions": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional structured questions. Requires the Twilio voice callback bridge and AETHERVAULT_PUBLIC_BASE_URL."
                    },
                    "record": { "type": "boolean", "description": "Request Twilio call recording." },
                    "machine_detection": { "type": "boolean", "description": "Enable answering-machine detection before speaking." }
                },
                "required": ["to", "objective", "script"]
            }
        }),
        serde_json::json!({
            "name": "phone_call_status",
            "description": "Inspect a Twilio-backed phone call by local request id or provider call SID, including gathered answers and provider status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "request_id": { "type": "string" },
                    "call_sid": { "type": "string" }
                }
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
            "name": "exa_search",
            "description": "Search the web via Exa API. Use ONLY when free web search cannot access the data — e.g., people/company lookup, research papers, tweets, paywalled content. For general web queries, prefer browser or http_request first. Supports category filters: people, company, news, research paper, tweet. Returns full text or highlights from results.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language search query"
                    },
                    "category": {
                        "type": "string",
                        "enum": ["people", "company", "news", "research paper", "tweet"],
                        "description": "Optional category filter. Use 'people' for person lookup, 'company' for company search, etc."
                    },
                    "num_results": {
                        "type": "integer",
                        "description": "Number of results (default 5, max 20)"
                    },
                    "content_mode": {
                        "type": "string",
                        "enum": ["text", "highlights", "none"],
                        "description": "Content extraction mode. 'text' for full content, 'highlights' for key excerpts, 'none' for URLs only. Default: highlights"
                    },
                    "max_characters": {
                        "type": "integer",
                        "description": "Max characters per result content (default 3000)"
                    },
                    "include_domains": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only search these domains (e.g. ['arxiv.org', 'github.com'])"
                    },
                    "exclude_domains": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Exclude these domains from results"
                    },
                    "start_date": {
                        "type": "string",
                        "description": "Only results published after this date (YYYY-MM-DD)"
                    },
                    "end_date": {
                        "type": "string",
                        "description": "Only results published before this date (YYYY-MM-DD)"
                    }
                },
                "required": ["query"]
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
                    "description": { "type": "string", "description": "One-line summary of what this skill does" },
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
            "name": "credential_check",
            "description": "Check if credentials exist for a service (Stripe, GitHub, Vercel, Twitter, etc.). Checks env vars, config files, and CLI auth. Call this BEFORE attempting API calls to avoid wasted retries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "service": { "type": "string", "description": "Service name (e.g., 'stripe', 'github', 'vercel', 'twitter')" }
                },
                "required": ["service"]
            }
        }),
        serde_json::json!({
            "name": "subagent_list",
            "description": "Check subagent configuration. Shows whether dynamic spawning is enabled and any pre-existing agent configs. You can use subagent_invoke with ANY name — you don't need to call this first.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "subagent_invoke",
            "description": "Spawn a subagent to perform a task. Use ANY descriptive name — the name should describe what the agent does (e.g., 'log-analyzer', 'api-tester', 'deploy-checker'). The subagent runs with its own session, tools, and memory. For swarm tasks, set branch to run in an isolated git worktree.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Descriptive name for this subagent (e.g., 'log-analyzer', 'code-reviewer'). Choose a name that describes the task." },
                    "prompt": { "type": "string", "description": "Detailed task description for the subagent. Be specific — the subagent has its own context." },
                    "system": { "type": "string", "description": "Override the subagent's system prompt" },
                    "model_hook": { "type": "string", "description": "Override the subagent's model hook" },
                    "max_steps": { "type": "integer", "description": "Override max reasoning steps for this invocation. Default: from subagent config, fallback 64." },
                    "branch": { "type": "string", "description": "Git branch name for worktree isolation. When set, the agent runs in a fresh git worktree. Use for swarm dev tasks to prevent conflicts between concurrent agents." }
                },
                "required": ["name", "prompt"]
            }
        }),
        serde_json::json!({
            "name": "subagent_batch",
            "description": "Spawn multiple subagents concurrently for parallel work. Each invocation runs independently with its own session. Use descriptive names for each agent. Use max_concurrent to limit parallelism.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "invocations": {
                        "type": "array",
                        "description": "Array of subagent invocations to run concurrently. Each has name, prompt, and optional overrides.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "Descriptive name for this subagent (e.g., 'log-analyzer', 'security-scanner')." },
                                "prompt": { "type": "string", "description": "Task/prompt for this subagent" },
                                "system": { "type": "string" },
                                "model_hook": { "type": "string" },
                                "max_steps": { "type": "integer" }
                            },
                            "required": ["name", "prompt"]
                        }
                    },
                    "max_concurrent": { "type": "integer", "description": "Maximum concurrent subagents. Default: all at once. Set lower to reduce resource usage." }
                },
                "required": ["invocations"]
            }
        }),
        serde_json::json!({
            "name": "session_start",
            "description": "Start a persistent subagent session that you can interact with. The subagent runs in the background with full tool access (file I/O, exec, search, etc). Write large inputs to files instead of passing as arguments. Use session_send to send follow-up instructions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Descriptive name for the session (e.g., 'crm-analyzer', 'data-pipeline')" },
                    "prompt": { "type": "string", "description": "Initial task description for the subagent" },
                    "system": { "type": "string", "description": "Override the subagent's system prompt" },
                    "model_hook": { "type": "string", "description": "Override the subagent's model hook" },
                    "max_steps": { "type": "integer", "description": "Max reasoning steps (default: 64)" },
                    "input_file": { "type": "string", "description": "Path to a file to copy into the session workspace as input.md" }
                },
                "required": ["name", "prompt"]
            }
        }),
        serde_json::json!({
            "name": "session_send",
            "description": "Send a follow-up message to a running subagent session. Injected as a user message on the subagent's next reasoning step.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Session ID returned by session_start" },
                    "message": { "type": "string", "description": "Message to inject into the subagent's conversation" },
                    "file": { "type": "string", "description": "Path to a file to copy into the session workspace" }
                },
                "required": ["session_id", "message"]
            }
        }),
        serde_json::json!({
            "name": "session_status",
            "description": "Check status of subagent sessions. Without session_id, lists all. With session_id, shows progress, last output, and workspace files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Check a specific session (omit to list all)" },
                    "list_files": { "type": "boolean", "description": "List files in the session workspace" },
                    "read_file": { "type": "string", "description": "Read a file from the session workspace (relative path)" }
                }
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
        serde_json::json!({
            "name": "bg_status",
            "description": "Check status of background tasks. Returns a traffic-light scorecard of all running, completed, and failed background sub-agents.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "Optional: check a specific task by ID" }
                }
            }
        }),
        serde_json::json!({
            "name": "self_upgrade",
            "description": "Trigger a self-upgrade: pull latest code from git, compile, validate, and hot-swap the binary. Uses blue-green deployment with automatic rollback. Requires approval. IMPORTANT: If you edited source files, you MUST git add, commit, and push your changes BEFORE calling this tool — it does `git reset --hard origin/<branch>` which wipes uncommitted changes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "branch": { "type": "string", "description": "Git branch to pull from (default: main)" },
                    "skip_tests": { "type": "boolean", "description": "Skip smoke test (not recommended)" }
                }
            }
        }),
        serde_json::json!({
            "name": "project_update",
            "description": "Create or update a project entry for tracking ongoing work across context resets. Projects are stored in capsule/projects.json.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Project name (used as unique key)" },
                    "status": { "type": "string", "description": "Project status: active, paused, or completed" },
                    "description": { "type": "string", "description": "Brief project description" },
                    "current_step": { "type": "string", "description": "What is currently being worked on" },
                    "notes": { "type": "string", "description": "A note to append to the project log" }
                },
                "required": ["name"]
            }
        }),
        serde_json::json!({
            "name": "project_list",
            "description": "List all tracked projects, optionally filtered by status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "Filter by status: active, paused, or completed" }
                }
            }
        }),
        // ── Swarm tools ──────────────────────────────────────────────
        serde_json::json!({
            "name": "swarm_create",
            "description": "Register a new dev task in the swarm registry. Creates a persistent task entry that survives restarts. Use with subagent_invoke (branch param) to spawn isolated coding agents.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Descriptive task name (e.g., 'fix-auth-bug', 'add-webhook-support')" },
                    "prompt": { "type": "string", "description": "Full task prompt for the coding agent — be specific about what to change, which files, acceptance criteria" },
                    "max_retries": { "type": "integer", "description": "Max auto-retry attempts on CI failure (default: 3)" }
                },
                "required": ["name", "prompt"]
            }
        }),
        serde_json::json!({
            "name": "swarm_list",
            "description": "List swarm tasks, optionally filtered by status. Shows task history across restarts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "Filter: queued, running, pr_open, reviewing, done, failed" },
                    "limit": { "type": "integer", "description": "Max results (default: 50)" }
                }
            }
        }),
        serde_json::json!({
            "name": "swarm_update",
            "description": "Update fields on a swarm task (status, branch, PR info, CI status, etc.).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Task ID (e.g., 'swarm-1')" },
                    "status": { "type": "string", "description": "New status: queued, running, pr_open, reviewing, done, failed" },
                    "branch": { "type": "string" },
                    "worktree_path": { "type": "string" },
                    "pr_number": { "type": "integer" },
                    "pr_url": { "type": "string" },
                    "ci_status": { "type": "string", "description": "pending, passing, or failing" },
                    "review_status": { "type": "string", "description": "pending, approved, or changes_requested" },
                    "error_context": { "type": "string" },
                    "agent_backend": { "type": "string", "description": "codex or claude-code" }
                },
                "required": ["id"]
            }
        }),
        serde_json::json!({
            "name": "swarm_check",
            "description": "Check CI and review status for all open swarm tasks. Runs `gh pr checks` and `gh pr view` to update the registry. Call periodically or after spawning agents.",
            "inputSchema": {
                "type": "object",
                "properties": {}
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

pub(crate) fn tool_catalog_map(
    catalog: &[serde_json::Value],
) -> HashMap<String, serde_json::Value> {
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
        "put",
        "log",
        "feedback",
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
        "credential_check",
        "trigger_add",
        "trigger_list",
        "trigger_remove",
        "subagent_list",
        "subagent_invoke",
        "subagent_batch",
        "session_start",
        "session_send",
        "session_status",
        "bg_status",
        "approval_list",
        "exec",
        "notify",
        "phone_call",
        "phone_call_status",
        "http_request",
        "exa_search",
        "fs_list",
        "fs_read",
        "fs_write",
        "scale",
        "self_upgrade",
        "browser",
        "excalidraw",
        "project_update",
        "project_list",
        "swarm_create",
        "swarm_list",
        "swarm_update",
        "swarm_check",
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

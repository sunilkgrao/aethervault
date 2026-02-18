use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aether_core::types::TemporalFilter;
use aether_core::Vault;
use serde::{Deserialize, Serialize};

use super::open_or_create;

#[derive(Debug, Serialize)]
pub(crate) struct GetResponse {
    pub(crate) frame_id: u64,
    pub(crate) uri: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) text: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct StatusResponse {
    pub(crate) mv2: String,
    pub(crate) frame_count: usize,
    pub(crate) next_frame_id: u64,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct FrameSummary {
    pub(crate) uri: String,
    pub(crate) frame_id: u64,
    pub(crate) timestamp: i64,
    pub(crate) checksum: String,
    pub(crate) title: Option<String>,
    pub(crate) track: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) status: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct DiffChange {
    pub(crate) uri: String,
    pub(crate) left: FrameSummary,
    pub(crate) right: FrameSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct DiffReport {
    pub(crate) left: String,
    pub(crate) right: String,
    pub(crate) only_left: Vec<FrameSummary>,
    pub(crate) only_right: Vec<FrameSummary>,
    pub(crate) changed: Vec<DiffChange>,
}

#[derive(Debug, Serialize)]
pub(crate) struct MergeReport {
    pub(crate) left: String,
    pub(crate) right: String,
    pub(crate) out: String,
    pub(crate) written: usize,
    pub(crate) deduped: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConfigEntry {
    pub(crate) key: String,
    pub(crate) frame_id: u64,
    pub(crate) timestamp: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct QueryPlan {
    pub(crate) cleaned_query: String,
    pub(crate) scope: Option<String>,
    pub(crate) as_of_ts: Option<i64>,
    pub(crate) temporal: Option<TemporalFilter>,
    pub(crate) skipped_expansion: bool,
    pub(crate) lex_queries: Vec<String>,
    pub(crate) vec_queries: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct QueryResult {
    pub(crate) rank: usize,
    pub(crate) frame_id: u64,
    pub(crate) uri: String,
    pub(crate) title: Option<String>,
    pub(crate) snippet: String,
    pub(crate) score: f32,
    pub(crate) rrf_rank: usize,
    pub(crate) rrf_score: f32,
    pub(crate) rerank_score: Option<f32>,
    pub(crate) feedback_score: Option<f32>,
    pub(crate) sources: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct QueryResponse {
    pub(crate) query: String,
    pub(crate) plan: QueryPlan,
    pub(crate) warnings: Vec<String>,
    pub(crate) results: Vec<QueryResult>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct FeedbackEvent {
    pub(crate) uri: String,
    pub(crate) score: f32,
    #[serde(default)]
    pub(crate) note: Option<String>,
    #[serde(default)]
    pub(crate) session: Option<String>,
    #[serde(default)]
    pub(crate) ts_utc: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AgentLogEntry {
    #[serde(default)]
    pub(crate) session: Option<String>,
    pub(crate) role: String,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) meta: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) ts_utc: Option<i64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ContextCitation {
    pub(crate) rank: usize,
    pub(crate) frame_id: u64,
    pub(crate) uri: String,
    pub(crate) title: Option<String>,
    pub(crate) score: f32,
}

#[derive(Debug, Serialize)]
pub(crate) struct ContextPack {
    pub(crate) query: String,
    pub(crate) plan: QueryPlan,
    pub(crate) warnings: Vec<String>,
    pub(crate) citations: Vec<ContextCitation>,
    pub(crate) context: String,
}

#[derive(Debug)]
pub(crate) struct QueryArgs {
    pub(crate) raw_query: String,
    pub(crate) collection: Option<String>,
    pub(crate) limit: usize,
    pub(crate) snippet_chars: usize,
    pub(crate) no_expand: bool,
    pub(crate) max_expansions: usize,
    pub(crate) expand_hook: Option<String>,
    pub(crate) expand_hook_timeout_ms: u64,
    pub(crate) no_vector: bool,
    pub(crate) rerank: String,
    pub(crate) rerank_hook: Option<String>,
    pub(crate) rerank_hook_timeout_ms: u64,
    pub(crate) rerank_hook_full_text: bool,
    pub(crate) embed_model: Option<String>,
    pub(crate) embed_cache: usize,
    pub(crate) embed_no_cache: bool,
    pub(crate) rerank_docs: usize,
    pub(crate) rerank_chunk_chars: usize,
    pub(crate) rerank_chunk_overlap: usize,
    pub(crate) plan: bool,
    pub(crate) asof: Option<String>,
    pub(crate) before: Option<String>,
    pub(crate) after: Option<String>,
    pub(crate) feedback_weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CapsuleConfig {
    #[serde(default)]
    pub(crate) context: Option<String>,
    #[serde(default)]
    pub(crate) collections: HashMap<String, CollectionConfig>,
    #[serde(default)]
    pub(crate) hooks: Option<HookConfig>,
    #[serde(default)]
    pub(crate) agent: Option<AgentConfig>,
    #[serde(default, flatten)]
    pub(crate) extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CollectionConfig {
    #[serde(default)]
    pub(crate) roots: Vec<String>,
    #[serde(default)]
    pub(crate) globs: Vec<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct HookConfig {
    #[serde(default)]
    pub(crate) expansion: Option<HookSpec>,
    #[serde(default)]
    pub(crate) rerank: Option<HookSpec>,
    #[serde(default)]
    pub(crate) llm: Option<HookSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AgentConfig {
    #[serde(default)]
    pub(crate) system: Option<String>,
    #[serde(default)]
    pub(crate) workspace: Option<String>,
    #[serde(default)]
    pub(crate) onboarding_complete: Option<bool>,
    #[serde(default)]
    pub(crate) timezone: Option<String>,
    #[serde(default)]
    pub(crate) telegram_token: Option<String>,
    #[serde(default)]
    pub(crate) telegram_chat_id: Option<String>,
    #[serde(default)]
    pub(crate) context_query: Option<String>,
    #[serde(default)]
    pub(crate) max_context_bytes: Option<usize>,
    #[serde(default)]
    pub(crate) max_context_results: Option<usize>,
    #[serde(default)]
    pub(crate) max_steps: Option<usize>,
    #[serde(default)]
    pub(crate) log: Option<bool>,
    #[serde(default)]
    pub(crate) log_commit_interval: Option<usize>,
    #[serde(default)]
    pub(crate) model_hook: Option<HookSpec>,
    #[serde(default)]
    /// Default model hook for dynamically spawned subagents that don't match
    /// a named config entry. Enables ad-hoc agent creation without pre-configuration.
    pub(crate) default_subagent_hook: Option<String>,
    #[serde(default)]
    pub(crate) subagents: Vec<SubagentSpec>,
    /// MCP servers to spawn as long-lived sidecars (generic plugin system)
    #[serde(default)]
    pub(crate) mcp_servers: Vec<McpServerConfig>,
}

/// Configuration for an external MCP server (tool plugin)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct McpServerConfig {
    /// Human-readable name (used for tool prefixing: mcp__{name}__{tool})
    pub(crate) name: String,
    /// Command to spawn the server (e.g. "npx excalidraw-mcp --stdio")
    pub(crate) command: String,
    /// Timeout in seconds for each tools/call (default: 30)
    #[serde(default)]
    pub(crate) timeout_secs: Option<u64>,
    /// Environment variables to pass to the server
    #[serde(default)]
    pub(crate) env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum CommandSpec {
    String(String),
    Array(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HookSpec {
    pub(crate) command: CommandSpec,
    #[serde(default)]
    pub(crate) timeout_ms: Option<u64>,
    #[serde(default)]
    pub(crate) full_text: Option<bool>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ExpansionHookInput {
    pub(crate) query: String,
    pub(crate) max_expansions: usize,
    pub(crate) scope: Option<String>,
    pub(crate) temporal: Option<TemporalFilter>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ExpansionHookOutput {
    #[serde(default)]
    pub(crate) lex: Vec<String>,
    #[serde(default)]
    pub(crate) vec: Vec<String>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RerankHookInput {
    pub(crate) query: String,
    pub(crate) candidates: Vec<RerankHookCandidate>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RerankHookCandidate {
    pub(crate) key: String,
    pub(crate) uri: String,
    pub(crate) title: Option<String>,
    pub(crate) snippet: String,
    pub(crate) frame_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct RerankHookOutput {
    #[serde(default)]
    pub(crate) scores: HashMap<String, f32>,
    #[serde(default)]
    pub(crate) snippets: HashMap<String, String>,
    #[serde(default)]
    pub(crate) items: Vec<RerankHookScore>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RerankHookScore {
    pub(crate) key: String,
    pub(crate) score: f32,
    #[serde(default)]
    pub(crate) snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AgentMessage {
    pub(crate) role: String,
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(default)]
    pub(crate) tool_calls: Vec<AgentToolCall>,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) is_error: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AgentToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) args: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AgentHookRequest {
    pub(crate) messages: Vec<AgentMessage>,
    pub(crate) tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AgentHookResponse {
    pub(crate) message: AgentMessage,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentToolResult {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) output: String,
    pub(crate) details: serde_json::Value,
    pub(crate) is_error: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentSession {
    pub(crate) session: Option<String>,
    pub(crate) context: Option<ContextPack>,
    pub(crate) messages: Vec<AgentMessage>,
    pub(crate) tool_results: Vec<AgentToolResult>,
}

pub(crate) struct AgentRunOutput {
    pub(crate) session: Option<String>,
    pub(crate) context: Option<ContextPack>,
    pub(crate) messages: Vec<AgentMessage>,
    pub(crate) tool_results: Vec<AgentToolResult>,
    pub(crate) final_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ContinuationCheckpoint {
    pub(crate) session: String,
    pub(crate) summary: String,
    pub(crate) goal: String,
    pub(crate) remaining_work: String,
    pub(crate) key_decisions: Vec<String>,
    pub(crate) total_steps: usize,
    pub(crate) chain_depth: usize,
}

/// A skill pattern distilled from an agent trajectory by the SkillRL system.
#[derive(Debug, Deserialize)]
pub(crate) struct DistilledSkill {
    pub(crate) title: String,
    pub(crate) principle: String,
    pub(crate) when_to_apply: String,
}

pub(crate) struct AgentProgress {
    pub(crate) step: usize,
    pub(crate) max_steps: usize,
    pub(crate) phase: String,
    pub(crate) text_preview: Option<String>,
    pub(crate) started_at: std::time::Instant,
    /// Tools invoked so far (name -> count)
    pub(crate) tools_used: HashMap<String, usize>,
    /// Whether the checkpoint message has been sent
    pub(crate) checkpoint_sent: bool,
    /// User responded to checkpoint: Some(true) = continue, Some(false) = wrap up
    pub(crate) checkpoint_response: Option<bool>,
    /// Extended step budget (set when user says "continue")
    pub(crate) extended_max_steps: Option<usize>,
    /// Interim messages to send to the user (drained by progress reporter)
    pub(crate) interim_messages: Vec<String>,
    /// Whether the first interim/acknowledgment has been sent
    pub(crate) first_ack_sent: bool,
    /// Steps using Opus reasoning directly
    pub(crate) opus_steps: usize,
    /// Steps delegated to Codex CLI or Ollama via exec
    pub(crate) delegated_steps: usize,
    /// Messages from the user injected mid-run (steering).
    /// The Telegram bridge pushes here; the agent loop drains and injects.
    pub(crate) steering_messages: Vec<String>,
}

pub(crate) struct CompletionEvent {
    pub(crate) chat_id: i64,
    pub(crate) reply_to_id: Option<i64>,
    pub(crate) result: Result<AgentRunOutput, String>,
}

pub(crate) struct ActiveRun {
    pub(crate) progress: Arc<Mutex<AgentProgress>>,
    pub(crate) queued_messages: Vec<(String, Option<i64>)>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ProgressEvent {
    #[serde(default)]
    pub(crate) session: Option<String>,
    pub(crate) milestone: String,
    pub(crate) percent: u8,
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) ts_utc: i64,
}

#[derive(Default)]
pub(crate) struct ReminderState {
    pub(crate) last_tool_failed: bool,
    pub(crate) same_tool_fail_streak: usize,
    pub(crate) approval_required_count: usize,
    pub(crate) sequential_read_ops: usize,
    pub(crate) last_tool_was_irreversible: bool,
    pub(crate) user_confirmed: bool,
    pub(crate) no_progress_streak: usize,
    pub(crate) reminder_ignored_count: usize,
}

/// Tracks a single critic correction event within an agent session.
/// Used to build a history of corrections for escalation and audit.
#[derive(Debug, Clone)]
pub(crate) struct CriticCorrection {
    pub(crate) step: usize,
    pub(crate) issues: Vec<String>,
    pub(crate) correction_text: String,
    pub(crate) acknowledged: bool,
}

#[derive(Default)]
pub(crate) struct DriftState {
    pub(crate) ema: f32,
    pub(crate) turns: usize,
    pub(crate) violations: HashMap<String, usize>,
    pub(crate) reminder_violations: usize,
    pub(crate) last_score: f32,
    /// History of critic corrections issued during this session.
    /// Used by the agent loop to track escalation and decide critic frequency.
    pub(crate) critic_history: Vec<CriticCorrection>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolAutonomyLevel {
    SuggestOnly,
    Confirm,
    Autonomous,
    Background,
}

impl Default for ToolAutonomyLevel {
    fn default() -> Self {
        ToolAutonomyLevel::Confirm
    }
}

#[derive(Clone)]
pub(crate) struct BridgeAgentConfig {
    pub(crate) mv2: PathBuf,
    pub(crate) model_hook: Option<String>,
    pub(crate) system: Option<String>,
    pub(crate) no_memory: bool,
    pub(crate) context_query: Option<String>,
    pub(crate) context_results: usize,
    pub(crate) context_max_bytes: usize,
    pub(crate) max_steps: usize,
    pub(crate) log: bool,
    pub(crate) log_commit_interval: usize,
    pub(crate) session_prefix: String,
}

    #[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct SubagentSpec {
    pub(crate) name: String,
    /// Human-readable description used for auto-routing.
    /// The orchestrator matches user intent against descriptions
    /// to decide which subagent to invoke.
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) system: Option<String>,
    #[serde(default)]
    pub(crate) model_hook: Option<String>,
    /// Allowlist of tool names this subagent can use.
    /// If empty, inherits all tools from parent.
    #[serde(default)]
    pub(crate) tools: Vec<String>,
    /// Denylist of tool names to exclude.
    #[serde(default)]
    pub(crate) disallowed_tools: Vec<String>,
    /// Maximum agent loop iterations (default: parent's max_steps).
    #[serde(default)]
    pub(crate) max_steps: Option<usize>,
    /// Hard timeout in seconds (default: none — bounded by max_steps).
    #[serde(default)]
    pub(crate) timeout_secs: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct ToolExecution {
    pub(crate) output: String,
    pub(crate) details: serde_json::Value,
    pub(crate) is_error: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ApprovalEntry {
    pub(crate) id: String,
    pub(crate) tool: String,
    pub(crate) args_hash: String,
    pub(crate) args: serde_json::Value,
    pub(crate) status: String,
    pub(crate) created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TriggerEntry {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) name: Option<String>,
    pub(crate) query: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) start: Option<String>,
    pub(crate) end: Option<String>,
    pub(crate) enabled: bool,
    pub(crate) last_seen: Option<String>,
    pub(crate) last_fired: Option<String>,
    /// Cron expression: "minute hour day_of_month month day_of_week"
    /// Supports: *, specific values, and ranges (e.g. "0 9 * * 1-5" = weekdays at 9am)
    #[serde(default)]
    pub(crate) cron: Option<String>,
    /// URL for webhook triggers (kind=webhook)
    #[serde(default)]
    pub(crate) webhook_url: Option<String>,
    /// HTTP method for webhook (default: GET)
    #[serde(default)]
    pub(crate) webhook_method: Option<String>,
    /// Custom schedule name (e.g. "morning_standup", "weekly_review")
    #[serde(default)]
    pub(crate) schedule_name: Option<String>,
}

/// Simple cron expression matcher (minute hour dom month dow)
pub(crate) struct CronExpr {
    pub(crate) minute: CronField,
    pub(crate) hour: CronField,
    pub(crate) dom: CronField,     // day of month
    pub(crate) month: CronField,
    pub(crate) dow: CronField,     // day of week (0=Sun, 1=Mon, ..., 6=Sat)
}

pub(crate) enum CronField {
    Any,
    Values(Vec<u32>),
}

impl CronExpr {
    pub(crate) fn parse(expr: &str) -> Result<Self, String> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(format!("cron: expected 5 fields, got {}", parts.len()));
        }
        Ok(CronExpr {
            minute: Self::parse_field(parts[0], 0, 59)?,
            hour: Self::parse_field(parts[1], 0, 23)?,
            dom: Self::parse_field(parts[2], 1, 31)?,
            month: Self::parse_field(parts[3], 1, 12)?,
            dow: Self::parse_field(parts[4], 0, 6)?,
        })
    }

    pub(crate) fn parse_field(field: &str, min: u32, max: u32) -> Result<CronField, String> {
        if field == "*" {
            return Ok(CronField::Any);
        }
        let mut values = Vec::new();
        for part in field.split(',') {
            if let Some((start_s, end_s)) = part.split_once('-') {
                let start: u32 = start_s.parse().map_err(|_| format!("cron: bad value '{start_s}'"))?;
                let end: u32 = end_s.parse().map_err(|_| format!("cron: bad value '{end_s}'"))?;
                if start < min || end > max || start > end {
                    return Err(format!("cron: range {start}-{end} out of bounds [{min}-{max}]"));
                }
                for v in start..=end {
                    values.push(v);
                }
            } else if let Some(step_s) = part.strip_prefix("*/") {
                let step: u32 = step_s.parse().map_err(|_| format!("cron: bad step '{step_s}'"))?;
                if step == 0 { return Err("cron: step cannot be 0".into()); }
                let mut v = min;
                while v <= max {
                    values.push(v);
                    v += step;
                }
            } else {
                let val: u32 = part.parse().map_err(|_| format!("cron: bad value '{part}'"))?;
                if val < min || val > max {
                    return Err(format!("cron: value {val} out of bounds [{min}-{max}]"));
                }
                values.push(val);
            }
        }
        Ok(CronField::Values(values))
    }

    pub(crate) fn matches(&self, minute: u32, hour: u32, dom: u32, month: u32, dow: u32) -> bool {
        Self::field_matches(&self.minute, minute)
            && Self::field_matches(&self.hour, hour)
            && Self::field_matches(&self.dom, dom)
            && Self::field_matches(&self.month, month)
            && Self::field_matches(&self.dow, dow)
    }

    pub(crate) fn field_matches(field: &CronField, value: u32) -> bool {
        match field {
            CronField::Any => true,
            CronField::Values(vals) => vals.contains(&value),
        }
    }
}

// === Session Context Buffer ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionTurn {
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) timestamp: i64,
}

pub(crate) fn session_file_path(session_id: &str) -> PathBuf {
    let safe_id = session_id.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    PathBuf::from("/root/.aethervault/workspace/sessions").join(format!("{safe_id}.json"))
}

pub(crate) fn load_session_turns(session_id: &str, max_turns: usize) -> Vec<SessionTurn> {
    let path = session_file_path(session_id);
    match std::fs::read_to_string(&path) {
        Ok(data) => {
            match serde_json::from_str::<Vec<SessionTurn>>(&data) {
                Ok(mut turns) => {
                    let keep = max_turns * 2;
                    if turns.len() > keep {
                        turns.drain(..turns.len() - keep);
                    }
                    turns
                }
                Err(_) => Vec::new(),
            }
        }
        Err(_) => Vec::new(),
    }
}

pub(crate) fn save_session_turns(session_id: &str, turns: &[SessionTurn], max_turns: usize) {
    let path = session_file_path(session_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let keep = max_turns * 2;
    let to_save: Vec<&SessionTurn> = if turns.len() > keep {
        turns[turns.len() - keep..].iter().collect()
    } else {
        turns.iter().collect()
    };
    if let Ok(json) = serde_json::to_string_pretty(&to_save) {
        let tmp_path = path.with_extension("json.tmp");
        if std::fs::write(&tmp_path, &json).is_ok() {
            let _ = std::fs::rename(&tmp_path, &path);
        }
    }
}


// === Capsule File Locking ===
// The Vault itself manages shared (read) and exclusive (write) flock() on the .mv2
// file directly. Readers can operate concurrently; writers upgrade to exclusive only
// during commit and immediately downgrade back. No external sidecar lock is needed.

pub(crate) const TOOL_DETAILS_MAX_CHARS: usize = 4_000;
pub(crate) const TOOL_OUTPUT_MAX_FOR_DETAILS: usize = 2_000;
pub(crate) const DEFAULT_WORKSPACE_DIR: &str = "./assistant";

pub(crate) fn format_tool_message_content(name: &str, output: &str, details: &serde_json::Value) -> String {
    if output.is_empty() {
        return String::new();
    }
    if details.is_null() {
        return output.to_string();
    }
    if output.len() > TOOL_OUTPUT_MAX_FOR_DETAILS {
        return output.to_string();
    }
    if matches!(name, "context") {
        return output.to_string();
    }
    let details_str = match serde_json::to_string(details) {
        Ok(value) => value,
        Err(_) => return output.to_string(),
    };
    if details_str.len() > TOOL_DETAILS_MAX_CHARS {
        return output.to_string();
    }
    format!("{output}\n\n[details]\n{details_str}")
}

pub(crate) fn with_read_mem<F, R>(
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
    mv2: &Path,
    f: F,
) -> Result<R, String>
where
    F: FnOnce(&mut Vault) -> Result<R, String>,
{
    if let Some(mem) = mem_write.as_mut() {
        return f(mem);
    }
    // Open fresh each time -- don't hold a shared lock between tool calls.
    // This allows concurrent subagents to acquire exclusive locks for writes.
    let mut mem = Vault::open_read_only(mv2).map_err(|e| e.to_string())?;
    let result = f(&mut mem);
    // `mem` is dropped here, releasing the shared lock immediately.
    *mem_read = None;
    result
}

pub(crate) fn with_write_mem<F, R>(
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
    mv2: &Path,
    allow_create: bool,
    f: F,
) -> Result<R, String>
where
    F: FnOnce(&mut Vault) -> Result<R, String>,
{
    // Hard cap: refuse ALL writes if vault exceeds size limit (default 500MB).
    // This is the single chokepoint for every vault write -- agent logs, tool puts,
    // observational memory, feedback, everything. Prevents runaway index bloat.
    let vault_hard_cap: u64 = std::env::var("VAULT_HARD_CAP_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500_000_000);
    if let Ok(meta) = std::fs::metadata(mv2) {
        if meta.len() > vault_hard_cap {
            return Err(format!(
                "vault write blocked: size {}MB exceeds {}MB hard cap (set VAULT_HARD_CAP_BYTES to adjust)",
                meta.len() / 1_000_000,
                vault_hard_cap / 1_000_000
            ));
        }
    }

    // Always open fresh -- don't reuse a stale handle that holds a lock.
    *mem_read = None;
    *mem_write = None;
    let opened = if allow_create {
        open_or_create(mv2).map_err(|e| e.to_string())?
    } else {
        Vault::open(mv2).map_err(|e| e.to_string())?
    };
    *mem_write = Some(opened);
    let result = f(mem_write.as_mut().unwrap());
    // Drop the handle entirely so no lock (shared or exclusive) persists between calls.
    // This allows concurrent subagents to acquire exclusive access for their own writes.
    *mem_write = None;
    result
}

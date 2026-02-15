#[allow(unused_imports)]
use serde::Deserialize;

use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub(crate) struct ToolQueryArgs {
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) collection: Option<String>,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
    #[serde(default)]
    pub(crate) snippet_chars: Option<usize>,
    #[serde(default)]
    pub(crate) no_expand: Option<bool>,
    #[serde(default)]
    pub(crate) max_expansions: Option<usize>,
    #[serde(default)]
    pub(crate) no_vector: Option<bool>,
    #[serde(default)]
    pub(crate) rerank: Option<String>,
    #[serde(default)]
    pub(crate) asof: Option<String>,
    #[serde(default)]
    pub(crate) before: Option<String>,
    #[serde(default)]
    pub(crate) after: Option<String>,
    #[serde(default)]
    pub(crate) feedback_weight: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolMemoryAppendArgs {
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolMemoryRememberArgs {
    pub(crate) text: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolEmailListArgs {
    #[serde(default)]
    pub(crate) account: Option<String>,
    #[serde(default)]
    pub(crate) folder: Option<String>,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolEmailReadArgs {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) account: Option<String>,
    #[serde(default)]
    pub(crate) folder: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolEmailSendArgs {
    pub(crate) to: String,
    #[serde(default)]
    pub(crate) cc: Option<String>,
    #[serde(default)]
    pub(crate) bcc: Option<String>,
    pub(crate) subject: String,
    pub(crate) body: String,
    #[serde(default)]
    pub(crate) from: Option<String>,
    #[serde(default)]
    pub(crate) in_reply_to: Option<String>,
    #[serde(default)]
    pub(crate) references: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolEmailArchiveArgs {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) account: Option<String>,
    #[serde(default)]
    pub(crate) folder: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolConfigSetArgs {
    pub(crate) key: String,
    pub(crate) json: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolMemorySyncArgs {
    #[serde(default)]
    pub(crate) workspace: Option<String>,
    #[serde(default)]
    pub(crate) include_daily: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolMemoryExportArgs {
    #[serde(default)]
    pub(crate) workspace: Option<String>,
    #[serde(default)]
    pub(crate) include_daily: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolMemorySearchArgs {
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolExecArgs {
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
    #[serde(default)]
    pub(crate) timeout_ms: Option<u64>,
    #[serde(default)]
    pub(crate) estimated_ms: Option<u64>,
    #[serde(default)]
    pub(crate) background: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolNotifyArgs {
    #[serde(default)]
    pub(crate) channel: Option<String>,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) webhook: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolSignalSendArgs {
    pub(crate) to: String,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) sender: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolIMessageSendArgs {
    pub(crate) to: String,
    pub(crate) text: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolGmailListArgs {
    #[serde(default)]
    pub(crate) query: Option<String>,
    #[serde(default)]
    pub(crate) max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolGmailReadArgs {
    pub(crate) id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolGmailSendArgs {
    pub(crate) to: String,
    pub(crate) subject: String,
    pub(crate) body: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolGCalListArgs {
    #[serde(default)]
    pub(crate) max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolGCalCreateArgs {
    pub(crate) summary: String,
    pub(crate) start: String,
    pub(crate) end: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolMsMailListArgs {
    #[serde(default)]
    pub(crate) top: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolMsMailReadArgs {
    pub(crate) id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolMsCalendarListArgs {
    #[serde(default)]
    pub(crate) top: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolMsCalendarCreateArgs {
    pub(crate) subject: String,
    pub(crate) start: String,
    pub(crate) end: String,
    #[serde(default)]
    pub(crate) body: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolHttpRequestArgs {
    #[serde(default)]
    pub(crate) method: Option<String>,
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub(crate) body: Option<String>,
    #[serde(default)]
    pub(crate) json: Option<bool>,
    #[serde(default)]
    pub(crate) timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolBrowserArgs {
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) session: Option<String>,
    #[serde(default)]
    pub(crate) timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolExcalidrawArgs {
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) elements: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolFsListArgs {
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) recursive: Option<bool>,
    #[serde(default)]
    pub(crate) max_entries: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolFsReadArgs {
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolFsWriteArgs {
    pub(crate) path: String,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) append: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolTriggerAddArgs {
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) query: Option<String>,
    #[serde(default)]
    pub(crate) prompt: Option<String>,
    #[serde(default)]
    pub(crate) start: Option<String>,
    #[serde(default)]
    pub(crate) end: Option<String>,
    #[serde(default)]
    pub(crate) enabled: Option<bool>,
    #[serde(default)]
    pub(crate) cron: Option<String>,
    #[serde(default)]
    pub(crate) webhook_url: Option<String>,
    #[serde(default)]
    pub(crate) webhook_method: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolTriggerRemoveArgs {
    pub(crate) id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolToolSearchArgs {
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolSessionContextArgs {
    pub(crate) session: String,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolReflectArgs {
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) session: Option<String>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolSkillStoreArgs {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) trigger: Option<String>,
    #[serde(default)]
    pub(crate) steps: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) tools: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolSkillSearchArgs {
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolSubagentInvokeArgs {
    pub(crate) name: String,
    pub(crate) prompt: String,
    #[serde(default)]
    pub(crate) system: Option<String>,
    #[serde(default)]
    pub(crate) model_hook: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolSubagentBatchArgs {
    /// Array of subagent invocations to run concurrently.
    pub(crate) invocations: Vec<ToolSubagentInvokeArgs>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolContextArgs {
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) collection: Option<String>,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
    #[serde(default)]
    pub(crate) snippet_chars: Option<usize>,
    #[serde(default)]
    pub(crate) max_bytes: Option<usize>,
    #[serde(default)]
    pub(crate) full: Option<bool>,
    #[serde(default)]
    pub(crate) no_expand: Option<bool>,
    #[serde(default)]
    pub(crate) max_expansions: Option<usize>,
    #[serde(default)]
    pub(crate) no_vector: Option<bool>,
    #[serde(default)]
    pub(crate) rerank: Option<String>,
    #[serde(default)]
    pub(crate) asof: Option<String>,
    #[serde(default)]
    pub(crate) before: Option<String>,
    #[serde(default)]
    pub(crate) after: Option<String>,
    #[serde(default)]
    pub(crate) feedback_weight: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolSearchArgs {
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) collection: Option<String>,
    #[serde(default)]
    pub(crate) limit: Option<usize>,
    #[serde(default)]
    pub(crate) snippet_chars: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolGetArgs {
    pub(crate) id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolPutArgs {
    pub(crate) uri: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) kind: Option<String>,
    #[serde(default)]
    pub(crate) track: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolLogArgs {
    #[serde(default)]
    pub(crate) session: Option<String>,
    #[serde(default)]
    pub(crate) role: Option<String>,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) meta: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolFeedbackArgs {
    pub(crate) uri: String,
    pub(crate) score: f32,
    #[serde(default)]
    pub(crate) note: Option<String>,
    #[serde(default)]
    pub(crate) session: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolScaleArgs {
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) size: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolSelfUpgradeArgs {
    #[serde(default)]
    pub(crate) branch: Option<String>,
    #[serde(default)]
    pub(crate) skip_tests: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(crate) enum LaneKind {
    Lex,
    Vec,
}

impl LaneKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            LaneKind::Lex => "lex",
            LaneKind::Vec => "vec",
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct Candidate {
    pub(crate) key: String,
    pub(crate) frame_id: u64,
    pub(crate) uri: String,
    pub(crate) title: Option<String>,
    pub(crate) snippet: String,
    pub(crate) score: Option<f32>,
    pub(crate) lane: LaneKind,
    pub(crate) query: String,
    pub(crate) rank: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct RankedList {
    pub(crate) lane: LaneKind,
    pub(crate) query: String,
    pub(crate) is_base: bool,
    pub(crate) items: Vec<Candidate>,
}

#[derive(Debug)]
pub(crate) struct FusedCandidate {
    pub(crate) key: String,
    pub(crate) frame_id: u64,
    pub(crate) uri: String,
    pub(crate) title: Option<String>,
    pub(crate) snippet: String,
    pub(crate) best_rank: usize,
    pub(crate) rrf_score: f32,
    pub(crate) rrf_bonus: f32,
    pub(crate) sources: Vec<String>,
}

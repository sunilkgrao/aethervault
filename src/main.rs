use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use blake3::Hash;
use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use clap::{Parser, Subcommand};
use aether_core::types::{Frame, FrameStatus, SearchHit, SearchRequest, TemporalFilter};
use aether_core::{DoctorOptions, DoctorReport, PutOptions, Vault, VaultError};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

#[cfg(feature = "vec")]
use aether_core::text_embed::{LocalTextEmbedder, TextEmbedConfig};
#[cfg(feature = "vec")]
use aether_core::types::EmbeddingProvider;

#[derive(Parser)]
#[command(name = "aethervault")]
#[command(about = "Hybrid retrieval over single-file .mv2 capsules", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new empty MV2 capsule.
    Init {
        mv2: PathBuf,
    },

    /// Ingest a folder of Markdown into the capsule (append-only, versioned by URI).
    Ingest {
        mv2: PathBuf,
        #[arg(short, long)]
        collection: String,
        #[arg(short, long)]
        root: PathBuf,
        /// File extensions to ingest (repeatable). Default: md
        #[arg(long = "ext")]
        exts: Vec<String>,
        /// Do not write anything; only report what would change.
        #[arg(long)]
        dry_run: bool,
    },

    /// Put a single text payload into the capsule.
    Put {
        mv2: PathBuf,
        /// Fully-qualified URI (aether://... or aethervault://...)
        #[arg(long)]
        uri: Option<String>,
        /// Collection name to build aether://<collection>/<path>
        #[arg(short, long)]
        collection: Option<String>,
        /// Path within collection (used with --collection)
        #[arg(long)]
        path: Option<String>,
        /// Title override
        #[arg(long)]
        title: Option<String>,
        /// Track label
        #[arg(long)]
        track: Option<String>,
        /// Mime type / kind
        #[arg(long)]
        kind: Option<String>,
        /// Text payload
        #[arg(long)]
        text: Option<String>,
        /// Read payload from file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Output JSON summary
        #[arg(long)]
        json: bool,
    },

    /// Lexical search (BM25 via Tantivy) over the capsule.
    Search {
        mv2: PathBuf,
        query: String,
        /// Number of results
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// Restrict search to a collection (URI prefix)
        #[arg(short, long)]
        collection: Option<String>,
        /// Snippet size in characters
        #[arg(long, default_value_t = 300)]
        snippet_chars: usize,
        /// Output JSON (full search response)
        #[arg(long)]
        json: bool,
    },

    /// Hybrid query: expansion → multi-lane retrieval → RRF → rerank → blend.
    Query {
        mv2: PathBuf,
        query: String,
        /// Number of results
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// Restrict search to a collection (URI prefix)
        #[arg(short, long)]
        collection: Option<String>,
        /// Snippet size in characters
        #[arg(long, default_value_t = 300)]
        snippet_chars: usize,
        /// Disable query expansion
        #[arg(long)]
        no_expand: bool,
        /// Max expansions per lane (lex/vector)
        #[arg(long, default_value_t = 2)]
        max_expansions: usize,
        /// Expansion hook command (overrides built-in expansion)
        #[arg(long)]
        expand_hook: Option<String>,
        /// Expansion hook timeout (ms)
        #[arg(long, default_value_t = 2000)]
        expand_hook_timeout_ms: u64,
        /// Disable vector lane (if enabled at build time)
        #[arg(long)]
        no_vector: bool,
        /// Reranker mode: local | hook | none
        #[arg(long, default_value = "local")]
        rerank: String,
        /// Rerank hook command (overrides local rerank)
        #[arg(long)]
        rerank_hook: Option<String>,
        /// Rerank hook timeout (ms)
        #[arg(long, default_value_t = 6000)]
        rerank_hook_timeout_ms: u64,
        /// Provide full text to rerank hook
        #[arg(long)]
        rerank_hook_full_text: bool,
        /// Embedding model for vector lane (bge-small, bge-base, nomic, gte-large)
        #[arg(long)]
        embed_model: Option<String>,
        /// Embedding cache capacity (in-memory)
        #[arg(long, default_value_t = 4096)]
        embed_cache: usize,
        /// Disable embedding cache
        #[arg(long)]
        embed_no_cache: bool,
        /// Max docs to rerank
        #[arg(long, default_value_t = 40)]
        rerank_docs: usize,
        /// Chunk size (chars) for reranking
        #[arg(long, default_value_t = 1200)]
        rerank_chunk_chars: usize,
        /// Overlap (chars) between rerank chunks
        #[arg(long, default_value_t = 200)]
        rerank_chunk_overlap: usize,
        /// Output JSON (includes plan + scores)
        #[arg(long)]
        json: bool,
        /// Output machine-friendly file list
        #[arg(long)]
        files: bool,
        /// Print the query plan / expansion tree to stderr
        #[arg(long)]
        plan: bool,
        /// Log query + results back into the capsule (append-only)
        #[arg(long)]
        log: bool,
        /// As-of timestamp (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
        #[arg(long)]
        asof: Option<String>,
        /// Temporal filter: before date (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
        #[arg(long)]
        before: Option<String>,
        /// Temporal filter: after date (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
        #[arg(long)]
        after: Option<String>,
        /// Feedback influence weight (0 disables)
        #[arg(long, default_value_t = 0.15)]
        feedback_weight: f32,
    },

    /// Build a prompt-ready context pack for agent harnesses.
    Context {
        mv2: PathBuf,
        query: String,
        /// Restrict to a collection (URI prefix)
        #[arg(short, long)]
        collection: Option<String>,
        /// Number of results to consider
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// Snippet size in characters
        #[arg(long, default_value_t = 300)]
        snippet_chars: usize,
        /// Max bytes for assembled context
        #[arg(long, default_value_t = 12_000)]
        max_bytes: usize,
        /// Use full document text instead of snippets
        #[arg(long)]
        full: bool,
        /// Disable query expansion
        #[arg(long)]
        no_expand: bool,
        /// Max expansions per lane (lex/vector)
        #[arg(long, default_value_t = 2)]
        max_expansions: usize,
        /// Expansion hook command (overrides built-in expansion)
        #[arg(long)]
        expand_hook: Option<String>,
        /// Expansion hook timeout (ms)
        #[arg(long, default_value_t = 2000)]
        expand_hook_timeout_ms: u64,
        /// Disable vector lane (if enabled at build time)
        #[arg(long)]
        no_vector: bool,
        /// Reranker mode: local | hook | none
        #[arg(long, default_value = "local")]
        rerank: String,
        /// Rerank hook command (overrides local rerank)
        #[arg(long)]
        rerank_hook: Option<String>,
        /// Rerank hook timeout (ms)
        #[arg(long, default_value_t = 6000)]
        rerank_hook_timeout_ms: u64,
        /// Provide full text to rerank hook
        #[arg(long)]
        rerank_hook_full_text: bool,
        /// Embedding model for vector lane
        #[arg(long)]
        embed_model: Option<String>,
        /// Embedding cache capacity (in-memory)
        #[arg(long, default_value_t = 4096)]
        embed_cache: usize,
        /// Disable embedding cache
        #[arg(long)]
        embed_no_cache: bool,
        /// Print the query plan / expansion tree to stderr
        #[arg(long)]
        plan: bool,
        /// As-of timestamp (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
        #[arg(long)]
        asof: Option<String>,
        /// Temporal filter: before date (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
        #[arg(long)]
        before: Option<String>,
        /// Temporal filter: after date (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
        #[arg(long)]
        after: Option<String>,
        /// Feedback influence weight (0 disables)
        #[arg(long, default_value_t = 0.15)]
        feedback_weight: f32,
    },

    /// Log an agent turn into the capsule.
    Log {
        mv2: PathBuf,
        /// Session identifier (optional)
        #[arg(long)]
        session: Option<String>,
        /// Role (user | assistant | system | tool)
        #[arg(long, default_value = "user")]
        role: String,
        /// Text payload
        #[arg(long)]
        text: Option<String>,
        /// Read text from file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Extra JSON metadata (string)
        #[arg(long)]
        meta: Option<String>,
    },

    /// Record feedback for a result (used to boost or suppress future rankings).
    Feedback {
        mv2: PathBuf,
        /// URI of the item
        #[arg(long)]
        uri: String,
        /// Score in [-1.0, 1.0] (negative suppresses)
        #[arg(long)]
        score: f32,
        /// Optional note or reason
        #[arg(long)]
        note: Option<String>,
        /// Session identifier (optional)
        #[arg(long)]
        session: Option<String>,
    },

    /// Precompute local embeddings for active frames (vector lane acceleration).
    Embed {
        mv2: PathBuf,
        /// Restrict to a collection (URI prefix)
        #[arg(short, long)]
        collection: Option<String>,
        /// Max frames to embed (0 = all)
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// Batch size for embedding
        #[arg(long, default_value_t = 32)]
        batch: usize,
        /// Re-embed even if embeddings exist
        #[arg(long)]
        force: bool,
        /// Embedder model (bge-small, bge-base, nomic, gte-large)
        #[arg(long)]
        model: Option<String>,
        /// Embedding cache capacity (in-memory)
        #[arg(long, default_value_t = 4096)]
        embed_cache: usize,
        /// Disable embedding cache
        #[arg(long)]
        embed_no_cache: bool,
        /// Dry run (no writes)
        #[arg(long)]
        dry_run: bool,
        /// Output JSON summary
        #[arg(long)]
        json: bool,
    },

    /// Retrieve a document by URI (aether://...) or frame id (#123).
    Get {
        mv2: PathBuf,
        id: String,
        #[arg(long)]
        json: bool,
    },

    /// Capsule summary.
    Status {
        mv2: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// Manage capsule config stored at aethervault://config/...
    Config {
        mv2: PathBuf,
        #[command(subcommand)]
        command: ConfigCommand,
    },

    /// Diff two capsules (by latest URI version).
    Diff {
        left: PathBuf,
        right: PathBuf,
        /// Include inactive frames
        #[arg(long)]
        all: bool,
        /// Limit listing size (0 = unlimited)
        #[arg(long, default_value_t = 200)]
        limit: usize,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },

    /// Merge two capsules into a new output file.
    Merge {
        left: PathBuf,
        right: PathBuf,
        out: PathBuf,
        /// Overwrite output if it exists
        #[arg(long)]
        force: bool,
        /// Disable deduplication across inputs
        #[arg(long)]
        no_dedup: bool,
        /// Output JSON summary
        #[arg(long)]
        json: bool,
    },

    /// MCP-compatible tool server (stdio JSON-RPC).
    Mcp {
        mv2: PathBuf,
        /// Read-only mode (disables write tools)
        #[arg(long)]
        read_only: bool,
    },

    /// Minimal agent harness (hook-based LLM).
    Agent {
        mv2: PathBuf,
        /// Prompt text (if omitted, read from stdin)
        #[arg(long)]
        prompt: Option<String>,
        /// Prompt file (overrides --prompt)
        #[arg(long)]
        file: Option<PathBuf>,
        /// Session identifier
        #[arg(long)]
        session: Option<String>,
        /// LLM hook command (overrides config)
        #[arg(long)]
        model_hook: Option<String>,
        /// System prompt text
        #[arg(long)]
        system: Option<String>,
        /// System prompt file
        #[arg(long)]
        system_file: Option<PathBuf>,
        /// Disable auto memory context
        #[arg(long)]
        no_memory: bool,
        /// Override memory query (defaults to prompt)
        #[arg(long)]
        context_query: Option<String>,
        /// Max results for memory context
        #[arg(long, default_value_t = 8)]
        context_results: usize,
        /// Max bytes for memory context
        #[arg(long, default_value_t = 12_000)]
        context_max_bytes: usize,
        /// Max tool/LLM steps before aborting
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        /// Emit JSON events
        #[arg(long)]
        json: bool,
        /// Log turns to capsule
        #[arg(long)]
        log: bool,
        /// Commit agent logs every N entries (1 = fsync each log)
        #[arg(long, default_value_t = 8)]
        log_commit_interval: usize,
    },

    /// Built-in model hooks (stdio JSON).
    Hook {
        #[command(subcommand)]
        provider: HookCommand,
    },

    /// Capsule maintenance (verification, index rebuild, compaction).
    Doctor {
        mv2: PathBuf,
        /// Run vacuum compaction
        #[arg(long)]
        vacuum: bool,
        /// Rebuild time index
        #[arg(long)]
        rebuild_time: bool,
        /// Rebuild lexical index
        #[arg(long)]
        rebuild_lex: bool,
        /// Rebuild vector index
        #[arg(long)]
        rebuild_vec: bool,
        /// Plan only (no changes)
        #[arg(long)]
        dry_run: bool,
        /// Suppress debug output
        #[arg(long)]
        quiet: bool,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },

    /// Compact a capsule with SOTA defaults (vacuum + index rebuilds).
    Compact {
        mv2: PathBuf,
        /// Plan only (no changes)
        #[arg(long)]
        dry_run: bool,
        /// Suppress debug output
        #[arg(long)]
        quiet: bool,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HookCommand {
    /// Anthropic Claude hook (stdio JSON)
    Claude,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Set a config document (stored as JSON).
    Set {
        /// Config key (stored at aethervault://config/<key>.json)
        #[arg(long, default_value = "index")]
        key: String,
        /// Read JSON from file
        #[arg(long)]
        file: Option<PathBuf>,
        /// JSON string payload
        #[arg(long)]
        json: Option<String>,
        /// Pretty-print stored JSON
        #[arg(long)]
        pretty: bool,
    },
    /// Get a config document.
    Get {
        /// Config key (stored at aethervault://config/<key>.json)
        #[arg(long, default_value = "index")]
        key: String,
        /// Output raw bytes (no pretty print)
        #[arg(long)]
        raw: bool,
    },
    /// List available config keys.
    List {
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Serialize)]
struct GetResponse {
    frame_id: u64,
    uri: Option<String>,
    title: Option<String>,
    text: String,
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    mv2: String,
    frame_count: usize,
    next_frame_id: u64,
}

#[derive(Debug, Serialize, Clone)]
struct FrameSummary {
    uri: String,
    frame_id: u64,
    timestamp: i64,
    checksum: String,
    title: Option<String>,
    track: Option<String>,
    kind: Option<String>,
    status: String,
}

#[derive(Debug, Serialize)]
struct DiffChange {
    uri: String,
    left: FrameSummary,
    right: FrameSummary,
}

#[derive(Debug, Serialize)]
struct DiffReport {
    left: String,
    right: String,
    only_left: Vec<FrameSummary>,
    only_right: Vec<FrameSummary>,
    changed: Vec<DiffChange>,
}

#[derive(Debug, Serialize)]
struct MergeReport {
    left: String,
    right: String,
    out: String,
    written: usize,
    deduped: usize,
}

#[derive(Debug, Serialize)]
struct ConfigEntry {
    key: String,
    frame_id: u64,
    timestamp: i64,
}

#[derive(Debug, Serialize)]
struct QueryPlan {
    cleaned_query: String,
    scope: Option<String>,
    as_of_ts: Option<i64>,
    temporal: Option<TemporalFilter>,
    skipped_expansion: bool,
    lex_queries: Vec<String>,
    vec_queries: Vec<String>,
}

#[derive(Debug, Serialize)]
struct QueryResult {
    rank: usize,
    frame_id: u64,
    uri: String,
    title: Option<String>,
    snippet: String,
    score: f32,
    rrf_rank: usize,
    rrf_score: f32,
    rerank_score: Option<f32>,
    feedback_score: Option<f32>,
    sources: Vec<String>,
}

#[derive(Debug, Serialize)]
struct QueryResponse {
    query: String,
    plan: QueryPlan,
    warnings: Vec<String>,
    results: Vec<QueryResult>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FeedbackEvent {
    uri: String,
    score: f32,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    ts_utc: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentLogEntry {
    #[serde(default)]
    session: Option<String>,
    role: String,
    text: String,
    #[serde(default)]
    meta: Option<serde_json::Value>,
    #[serde(default)]
    ts_utc: Option<i64>,
}

#[derive(Debug, Serialize)]
struct ContextCitation {
    rank: usize,
    frame_id: u64,
    uri: String,
    title: Option<String>,
    score: f32,
}

#[derive(Debug, Serialize)]
struct ContextPack {
    query: String,
    plan: QueryPlan,
    warnings: Vec<String>,
    citations: Vec<ContextCitation>,
    context: String,
}

#[derive(Debug)]
struct QueryArgs {
    raw_query: String,
    collection: Option<String>,
    limit: usize,
    snippet_chars: usize,
    no_expand: bool,
    max_expansions: usize,
    expand_hook: Option<String>,
    expand_hook_timeout_ms: u64,
    no_vector: bool,
    rerank: String,
    rerank_hook: Option<String>,
    rerank_hook_timeout_ms: u64,
    rerank_hook_full_text: bool,
    embed_model: Option<String>,
    embed_cache: usize,
    embed_no_cache: bool,
    rerank_docs: usize,
    rerank_chunk_chars: usize,
    rerank_chunk_overlap: usize,
    plan: bool,
    asof: Option<String>,
    before: Option<String>,
    after: Option<String>,
    feedback_weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CapsuleConfig {
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    collections: HashMap<String, CollectionConfig>,
    #[serde(default)]
    hooks: Option<HookConfig>,
    #[serde(default)]
    agent: Option<AgentConfig>,
    #[serde(default, flatten)]
    extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CollectionConfig {
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    globs: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HookConfig {
    #[serde(default)]
    expansion: Option<HookSpec>,
    #[serde(default)]
    rerank: Option<HookSpec>,
    #[serde(default)]
    llm: Option<HookSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AgentConfig {
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    context_query: Option<String>,
    #[serde(default)]
    max_context_bytes: Option<usize>,
    #[serde(default)]
    max_context_results: Option<usize>,
    #[serde(default)]
    max_steps: Option<usize>,
    #[serde(default)]
    log: Option<bool>,
    #[serde(default)]
    log_commit_interval: Option<usize>,
    #[serde(default)]
    model_hook: Option<HookSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum CommandSpec {
    String(String),
    Array(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HookSpec {
    command: CommandSpec,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    full_text: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ExpansionHookInput {
    query: String,
    max_expansions: usize,
    scope: Option<String>,
    temporal: Option<TemporalFilter>,
}

#[derive(Debug, Deserialize, Default)]
struct ExpansionHookOutput {
    #[serde(default)]
    lex: Vec<String>,
    #[serde(default)]
    vec: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RerankHookInput {
    query: String,
    candidates: Vec<RerankHookCandidate>,
}

#[derive(Debug, Serialize)]
struct RerankHookCandidate {
    key: String,
    uri: String,
    title: Option<String>,
    snippet: String,
    frame_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RerankHookOutput {
    #[serde(default)]
    scores: HashMap<String, f32>,
    #[serde(default)]
    snippets: HashMap<String, String>,
    #[serde(default)]
    items: Vec<RerankHookScore>,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RerankHookScore {
    key: String,
    score: f32,
    #[serde(default)]
    snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentMessage {
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<AgentToolCall>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentToolCall {
    id: String,
    name: String,
    #[serde(default)]
    args: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentHookRequest {
    messages: Vec<AgentMessage>,
    tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentHookResponse {
    message: AgentMessage,
}

#[derive(Debug, Serialize)]
struct AgentToolResult {
    id: String,
    name: String,
    output: String,
    details: serde_json::Value,
    is_error: bool,
}

#[derive(Debug, Serialize)]
struct AgentSession {
    session: Option<String>,
    context: Option<ContextPack>,
    messages: Vec<AgentMessage>,
    tool_results: Vec<AgentToolResult>,
}

#[derive(Debug)]
struct ToolExecution {
    output: String,
    details: serde_json::Value,
    is_error: bool,
}

const TOOL_DETAILS_MAX_CHARS: usize = 4_000;
const TOOL_OUTPUT_MAX_FOR_DETAILS: usize = 2_000;

fn format_tool_message_content(name: &str, output: &str, details: &serde_json::Value) -> String {
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

fn with_read_mem<F, R>(
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
    if mem_read.is_none() {
        *mem_read = Some(Vault::open_read_only(mv2).map_err(|e| e.to_string())?);
    }
    f(mem_read.as_mut().unwrap())
}

fn with_write_mem<F, R>(
    mem_write: &mut Option<Vault>,
    mv2: &Path,
    allow_create: bool,
    f: F,
) -> Result<R, String>
where
    F: FnOnce(&mut Vault) -> Result<R, String>,
{
    if mem_write.is_none() {
        let opened = if allow_create {
            open_or_create(mv2).map_err(|e| e.to_string())?
        } else {
            Vault::open(mv2).map_err(|e| e.to_string())?
        };
        *mem_write = Some(opened);
    }
    f(mem_write.as_mut().unwrap())
}

#[derive(Debug, Deserialize)]
struct ToolQueryArgs {
    query: String,
    #[serde(default)]
    collection: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    snippet_chars: Option<usize>,
    #[serde(default)]
    no_expand: Option<bool>,
    #[serde(default)]
    max_expansions: Option<usize>,
    #[serde(default)]
    no_vector: Option<bool>,
    #[serde(default)]
    rerank: Option<String>,
    #[serde(default)]
    asof: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    feedback_weight: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct ToolContextArgs {
    query: String,
    #[serde(default)]
    collection: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    snippet_chars: Option<usize>,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    full: Option<bool>,
    #[serde(default)]
    no_expand: Option<bool>,
    #[serde(default)]
    max_expansions: Option<usize>,
    #[serde(default)]
    no_vector: Option<bool>,
    #[serde(default)]
    rerank: Option<String>,
    #[serde(default)]
    asof: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    feedback_weight: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct ToolSearchArgs {
    query: String,
    #[serde(default)]
    collection: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    snippet_chars: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolGetArgs {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ToolPutArgs {
    uri: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    track: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolLogArgs {
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    role: Option<String>,
    text: String,
    #[serde(default)]
    meta: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ToolFeedbackArgs {
    uri: String,
    score: f32,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    session: Option<String>,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
enum LaneKind {
    Lex,
    Vec,
}

impl LaneKind {
    fn as_str(&self) -> &'static str {
        match self {
            LaneKind::Lex => "lex",
            LaneKind::Vec => "vec",
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Candidate {
    key: String,
    frame_id: u64,
    uri: String,
    title: Option<String>,
    snippet: String,
    score: Option<f32>,
    lane: LaneKind,
    query: String,
    rank: usize,
}

#[derive(Debug, Clone)]
struct RankedList {
    lane: LaneKind,
    query: String,
    is_base: bool,
    items: Vec<Candidate>,
}

#[derive(Debug)]
struct FusedCandidate {
    key: String,
    frame_id: u64,
    uri: String,
    title: Option<String>,
    snippet: String,
    best_rank: usize,
    rrf_score: f32,
    rrf_bonus: f32,
    sources: Vec<String>,
}

fn normalize_collection(name: &str) -> String {
    name.trim().trim_matches('/').to_string()
}

fn scope_prefix(collection: &str) -> String {
    format!("aether://{}/", normalize_collection(collection))
}

fn uri_for_path(collection: &str, relative: &Path) -> String {
    let rel = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    format!("aether://{}/{rel}", normalize_collection(collection))
}

fn infer_title(path: &Path, bytes: &[u8]) -> String {
    let fallback = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("untitled")
        .to_string();

    let Ok(text) = std::str::from_utf8(bytes) else {
        return fallback;
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return title.to_string();
            }
        }
    }

    fallback
}

fn blake3_hash(bytes: &[u8]) -> Hash {
    blake3::hash(bytes)
}

fn open_or_create(mv2: &Path) -> aether_core::Result<Vault> {
    if mv2.exists() {
        Vault::open(mv2)
    } else {
        Vault::create(mv2)
    }
}

fn is_extension_allowed(path: &Path, exts: &[String]) -> bool {
    let ext = path.extension().and_then(OsStr::to_str).unwrap_or("");
    if exts.is_empty() {
        return ext.eq_ignore_ascii_case("md");
    }
    exts.iter().any(|allowed| ext.eq_ignore_ascii_case(allowed))
}

#[derive(Default, Debug)]
struct ParsedMarkup {
    collection: Option<String>,
    asof_ts: Option<i64>,
    before_ts: Option<i64>,
    after_ts: Option<i64>,
}

fn parse_date_to_ts(value: &str) -> Option<i64> {
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M") {
        return Some(Utc.from_utc_datetime(&dt).timestamp());
    }
    if let Ok(d) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let dt = d.and_hms_opt(0, 0, 0)?;
        return Some(Utc.from_utc_datetime(&dt).timestamp());
    }
    None
}

fn parse_query_markup(raw: &str) -> (String, ParsedMarkup) {
    let mut parsed = ParsedMarkup::default();
    let mut kept = Vec::new();

    for token in raw.split_whitespace() {
        let Some((key, value)) = token.split_once(':') else {
            kept.push(token);
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "in" | "collection" => {
                if !value.trim().is_empty() {
                    parsed.collection = Some(value.trim().to_string());
                }
            }
            "asof" => {
                parsed.asof_ts = parse_date_to_ts(value);
            }
            "before" => {
                parsed.before_ts = parse_date_to_ts(value);
            }
            "after" => {
                parsed.after_ts = parse_date_to_ts(value);
            }
            _ => kept.push(token),
        }
    }

    let cleaned = kept.join(" ").trim().to_string();
    (cleaned, parsed)
}

fn is_stopword(token: &str) -> bool {
    matches!(
        token,
        "a" | "an" | "and" | "are" | "as" | "at" | "be" | "but" | "by" | "for" | "from"
            | "has" | "have" | "if" | "in" | "into" | "is" | "it" | "its" | "of" | "on"
            | "or" | "that" | "the" | "their" | "then" | "there" | "these" | "they"
            | "this" | "to" | "was" | "were" | "with" | "you" | "your"
    )
}

fn tokenize(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

fn dedup_keep_order(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for v in values {
        if seen.insert(v.clone()) {
            out.push(v);
        }
    }
    out
}

fn build_expansions(base: &str, max: usize) -> Vec<String> {
    let tokens = tokenize(base);
    if tokens.len() <= 1 || max == 0 {
        return vec![base.to_string()];
    }

    let mut expansions = vec![base.trim().to_string()];

    let reduced_tokens: Vec<String> = tokens
        .iter()
        .filter(|t| !is_stopword(t))
        .cloned()
        .collect();
    let reduced = reduced_tokens.join(" ");
    if !reduced.is_empty() && reduced != base {
        expansions.push(reduced);
    }

    if !base.trim().starts_with('"') && !base.trim().ends_with('"') {
        expansions.push(format!("\"{}\"", base.trim()));
    }

    let expansions = dedup_keep_order(expansions);
    expansions.into_iter().take(max.max(1)).collect()
}

fn config_key_to_uri(key: &str) -> String {
    let mut key = key.trim().to_string();
    if key.is_empty() {
        key = "index".to_string();
    }
    if !key.ends_with(".json") {
        key.push_str(".json");
    }
    format!("aethervault://config/{key}")
}

fn config_uri_to_key(uri: &str) -> Option<String> {
    let prefix = "aethervault://config/";
    if !uri.starts_with(prefix) {
        return None;
    }
    let mut key = uri.trim_start_matches(prefix).to_string();
    if key.ends_with(".json") {
        key.truncate(key.len().saturating_sub(5));
    }
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

fn load_config_entry(mem: &mut Vault, key: &str) -> Option<Vec<u8>> {
    let uri = config_key_to_uri(key);
    let frame = mem.frame_by_uri(&uri).ok()?;
    mem.frame_canonical_payload(frame.id).ok()
}

fn load_capsule_config(mem: &mut Vault) -> Option<CapsuleConfig> {
    let bytes = load_config_entry(mem, "index")?;
    serde_json::from_slice(&bytes).ok()
}

fn save_config_entry(mem: &mut Vault, key: &str, bytes: &[u8]) -> Result<u64, Box<dyn std::error::Error>> {
    let mut options = PutOptions::default();
    options.uri = Some(config_key_to_uri(key));
    options.title = Some(format!("config:{key}"));
    options.kind = Some("application/json".to_string());
    options.track = Some("aethervault.config".to_string());
    options.search_text = Some(format!("config {key}"));
    options.auto_tag = false;
    options.extract_dates = false;
    options.extract_triplets = false;
    options.instant_index = true;
    let id = mem.put_bytes_with_options(bytes, options)?;
    mem.commit()?;
    Ok(id)
}

fn list_config_entries(mem: &mut Vault) -> Vec<ConfigEntry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    let total = mem.frame_count() as i64;
    for idx in (0..total).rev() {
        let frame_id = idx as u64;
        let frame = match mem.frame_by_id(frame_id) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let uri = match frame.uri.as_deref() {
            Some(u) => u,
            None => continue,
        };
        let key = match config_uri_to_key(uri) {
            Some(k) => k,
            None => continue,
        };
        if seen.insert(key.clone()) {
            entries.push(ConfigEntry {
                key,
                frame_id: frame.id,
                timestamp: frame.timestamp,
            });
        }
    }
    entries
}

fn command_spec_to_vec(spec: &CommandSpec) -> Vec<String> {
    match spec {
        CommandSpec::Array(items) => items.clone(),
        CommandSpec::String(cmd) => {
            if cfg!(windows) {
                vec!["cmd".to_string(), "/C".to_string(), cmd.clone()]
            } else {
                vec!["sh".to_string(), "-c".to_string(), cmd.clone()]
            }
        }
    }
}

fn run_hook_command(
    command: &[String],
    input: &serde_json::Value,
    timeout_ms: u64,
    kind: &str,
) -> Result<String, String> {
    if command.is_empty() {
        return Err("hook command is empty".into());
    }
    let mut cmd = ProcessCommand::new(&command[0]);
    cmd.args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("KAIROS_HOOK", kind);

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let payload = serde_json::to_vec(input).map_err(|e| format!("encode input: {e}"))?;
        stdin
            .write_all(&payload)
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("write stdin: {e}"))?;
    }

    let timeout = Duration::from_millis(timeout_ms.max(1));
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return Err(format!("hook timed out after {timeout_ms}ms"));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(format!("hook wait failed: {e}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("hook output failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            return Err("hook exited with error".into());
        }
        return Err(format!("hook error: {stderr}"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err("hook returned empty output".into());
    }
    Ok(stdout)
}

fn resolve_hook_spec(
    cli_command: Option<String>,
    cli_timeout_ms: u64,
    config_spec: Option<HookSpec>,
    force_full_text: Option<bool>,
) -> Option<HookSpec> {
    if let Some(cmd) = cli_command {
        return Some(HookSpec {
            command: CommandSpec::String(cmd),
            timeout_ms: Some(cli_timeout_ms),
            full_text: force_full_text,
        });
    }
    config_spec.map(|mut spec| {
        if spec.timeout_ms.is_none() {
            spec.timeout_ms = Some(cli_timeout_ms);
        }
        if force_full_text.is_some() {
            spec.full_text = force_full_text;
        }
        spec
    })
}

fn run_expansion_hook(
    hook: &HookSpec,
    input: &ExpansionHookInput,
) -> Result<ExpansionHookOutput, String> {
    let cmd = command_spec_to_vec(&hook.command);
    let timeout = hook.timeout_ms.unwrap_or(2000);
    let value = serde_json::to_value(input).map_err(|e| format!("hook input: {e}"))?;
    let raw = run_hook_command(&cmd, &value, timeout, "expansion")?;
    let mut output: ExpansionHookOutput =
        serde_json::from_str(&raw).map_err(|e| format!("hook output: {e}"))?;
    output.lex = dedup_keep_order(output.lex);
    output.vec = dedup_keep_order(output.vec);
    Ok(output)
}

fn run_rerank_hook(
    hook: &HookSpec,
    input: &RerankHookInput,
) -> Result<RerankHookOutput, String> {
    let cmd = command_spec_to_vec(&hook.command);
    let timeout = hook.timeout_ms.unwrap_or(6000);
    let value = serde_json::to_value(input).map_err(|e| format!("hook input: {e}"))?;
    let raw = run_hook_command(&cmd, &value, timeout, "rerank")?;
    let mut output: RerankHookOutput =
        serde_json::from_str(&raw).map_err(|e| format!("hook output: {e}"))?;
    for item in output.items.drain(..) {
        output.scores.insert(item.key.clone(), item.score);
        if let Some(snippet) = item.snippet {
            output.snippets.insert(item.key, snippet);
        }
    }
    Ok(output)
}

fn checksum_hex(checksum: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in checksum {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

fn frame_to_summary(frame: &Frame) -> Option<FrameSummary> {
    let uri = frame.uri.clone()?;
    Some(FrameSummary {
        uri,
        frame_id: frame.id,
        timestamp: frame.timestamp,
        checksum: checksum_hex(&frame.checksum),
        title: frame.title.clone(),
        track: frame.track.clone(),
        kind: frame.kind.clone(),
        status: format!("{:?}", frame.status).to_ascii_lowercase(),
    })
}

fn collect_latest_frames(
    mem: &mut Vault,
    include_inactive: bool,
) -> HashMap<String, FrameSummary> {
    let mut out = HashMap::new();
    let total = mem.frame_count() as i64;
    for idx in (0..total).rev() {
        let frame_id = idx as u64;
        let frame = match mem.frame_by_id(frame_id) {
            Ok(f) => f,
            Err(_) => continue,
        };
        if !include_inactive && frame.status != FrameStatus::Active {
            continue;
        }
        let summary = match frame_to_summary(&frame) {
            Some(s) => s,
            None => continue,
        };
        if !out.contains_key(&summary.uri) {
            out.insert(summary.uri.clone(), summary);
        }
    }
    out
}

fn has_strong_signal(hits: &[SearchHit]) -> bool {
    let s1 = hits.get(0).and_then(|h| h.score).unwrap_or(0.0);
    let s2 = hits.get(1).and_then(|h| h.score).unwrap_or(0.0);
    if s1 <= 0.0 {
        return false;
    }
    if s1 <= 1.5 {
        s1 >= 0.85 && (s1 - s2) >= 0.15
    } else {
        let ratio = if s2 > 0.0 { s1 / s2 } else { 10.0 };
        s1 >= 2.0 && ratio >= 1.3
    }
}

fn build_ranked_list(
    lane: LaneKind,
    query: &str,
    is_base: bool,
    hits: &[SearchHit],
) -> RankedList {
    let items = hits
        .iter()
        .enumerate()
        .map(|(i, hit)| Candidate {
            key: hit.uri.clone(),
            frame_id: hit.frame_id,
            uri: hit.uri.clone(),
            title: hit.title.clone(),
            snippet: hit.text.clone(),
            score: hit.score,
            lane,
            query: query.to_string(),
            rank: i + 1,
        })
        .collect();
    RankedList {
        lane,
        query: query.to_string(),
        is_base,
        items,
    }
}

fn rrf_fuse(lists: &[RankedList], k: f32) -> Vec<FusedCandidate> {
    let mut map: HashMap<String, FusedCandidate> = HashMap::new();

    for list in lists {
        let weight = if list.is_base { 2.0 } else { 1.0 };
        for (i, item) in list.items.iter().enumerate() {
            let rank = i + 1;
            let rrf = weight / (k + rank as f32);
            let bonus = if rank == 1 {
                0.05
            } else if rank <= 3 {
                0.02
            } else {
                0.0
            };

            let entry = map.entry(item.key.clone()).or_insert(FusedCandidate {
                key: item.key.clone(),
                frame_id: item.frame_id,
                uri: item.uri.clone(),
                title: item.title.clone(),
                snippet: item.snippet.clone(),
                best_rank: rank,
                rrf_score: 0.0,
                rrf_bonus: 0.0,
                sources: Vec::new(),
            });

            if rank < entry.best_rank {
                entry.best_rank = rank;
                entry.snippet = item.snippet.clone();
                entry.title = item.title.clone();
                entry.frame_id = item.frame_id;
                entry.uri = item.uri.clone();
            }

            entry.rrf_score += rrf;
            entry.rrf_bonus += bonus;
            entry
                .sources
                .push(format!("{}:{}#{}", list.lane.as_str(), list.query, rank));
        }
    }

    let mut fused: Vec<FusedCandidate> = map.into_values().collect();
    fused.sort_by(|a, b| {
        let sa = a.rrf_score + a.rrf_bonus;
        let sb = b.rrf_score + b.rrf_bonus;
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    fused
}

fn chunk_text(text: &str, max_chars: usize, overlap: usize) -> Vec<(String, usize)> {
    let len = text.len();
    if len == 0 {
        return vec![];
    }
    if len <= max_chars {
        return vec![(text.to_string(), 0)];
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut chunk_count = 0usize;
    let max_chunks = 200usize;
    while start < len && chunk_count < max_chunks {
        let mut end = (start + max_chars).min(len);
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        let chunk = text[start..end].to_string();
        chunks.push((chunk, start));
        if end == len {
            break;
        }
        let mut next_start = end.saturating_sub(overlap);
        while next_start > 0 && !text.is_char_boundary(next_start) {
            next_start -= 1;
        }
        if next_start == start {
            break;
        }
        start = next_start;
        chunk_count += 1;
    }
    chunks
}

fn rerank_score(query: &str, chunk: &str) -> f32 {
    let query_lower = query.to_ascii_lowercase();
    let terms: Vec<String> = query_lower
        .split_whitespace()
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_string())
        .collect();
    if terms.is_empty() {
        return 0.0;
    }

    let chunk_lower = chunk.to_ascii_lowercase();
    let mut matched = 0usize;
    let mut freq = 0usize;
    for term in &terms {
        if chunk_lower.contains(term) {
            matched += 1;
        }
        freq += chunk_lower.matches(term).count();
    }
    let coverage = matched as f32 / terms.len() as f32;
    let phrase_bonus = if chunk_lower.contains(&query_lower) { 0.2 } else { 0.0 };
    let freq_bonus = (freq as f32).ln_1p() * 0.05;
    let raw = coverage + phrase_bonus + freq_bonus;
    raw / (1.0 + raw)
}

fn print_plan(plan: &QueryPlan) {
    eprintln!("├─ {}", plan.cleaned_query);
    if !plan.lex_queries.is_empty() {
        for (i, q) in plan.lex_queries.iter().enumerate() {
            let prefix = if i == plan.lex_queries.len() - 1 && plan.vec_queries.is_empty() {
                "└─"
            } else {
                "├─"
            };
            eprintln!("{prefix} lex: {q}");
        }
    }
    if !plan.vec_queries.is_empty() {
        for (i, q) in plan.vec_queries.iter().enumerate() {
            let prefix = if i == plan.vec_queries.len() - 1 { "└─" } else { "├─" };
            eprintln!("{prefix} vec: {q}");
        }
    }
}

fn load_feedback_scores(
    mem: &mut Vault,
    targets: &std::collections::HashSet<String>,
) -> HashMap<String, f32> {
    let mut scores = HashMap::new();
    if targets.is_empty() {
        return scores;
    }

    let mut remaining = targets.clone();
    let total = mem.frame_count() as i64;
    for idx in (0..total).rev() {
        if remaining.is_empty() {
            break;
        }
        let frame_id = idx as u64;
        let frame = match mem.frame_by_id(frame_id) {
            Ok(f) => f,
            Err(_) => continue,
        };
        if frame.track.as_deref() != Some("aethervault.feedback") {
            continue;
        }

        let bytes = match mem.frame_canonical_payload(frame.id) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let event: FeedbackEvent = match serde_json::from_slice(&bytes) {
            Ok(e) => e,
            Err(_) => continue,
        };

        if remaining.remove(&event.uri) {
            scores.insert(event.uri, event.score);
        }
    }

    scores
}

fn execute_query(mem: &mut Vault, args: QueryArgs) -> Result<QueryResponse, Box<dyn std::error::Error>> {
    let mut warnings = Vec::new();

    let (cleaned_query, parsed) = parse_query_markup(&args.raw_query);
    if cleaned_query.trim().is_empty() {
        return Err("Query is empty after removing markup tokens.".into());
    }

    #[cfg(not(feature = "vec"))]
    let _ = (&args.embed_model, args.embed_cache, args.embed_no_cache);

    let config = load_capsule_config(mem);
    let hook_config = config.as_ref().and_then(|c| c.hooks.clone());
    let expansion_hook = resolve_hook_spec(
        args.expand_hook.clone(),
        args.expand_hook_timeout_ms,
        hook_config.as_ref().and_then(|h| h.expansion.clone()),
        None,
    );
    let rerank_hook = resolve_hook_spec(
        args.rerank_hook.clone(),
        args.rerank_hook_timeout_ms,
        hook_config.as_ref().and_then(|h| h.rerank.clone()),
        if args.rerank_hook_full_text { Some(true) } else { None },
    );

    let scope_collection = args.collection.or(parsed.collection);
    let scope = scope_collection.as_deref().map(scope_prefix);

    let asof_ts = args
        .asof
        .as_deref()
        .and_then(parse_date_to_ts)
        .or(parsed.asof_ts);

    let before_ts = args
        .before
        .as_deref()
        .and_then(parse_date_to_ts)
        .or(parsed.before_ts);
    let after_ts = args
        .after
        .as_deref()
        .and_then(parse_date_to_ts)
        .or(parsed.after_ts);

    let temporal = if before_ts.is_some() || after_ts.is_some() {
        Some(TemporalFilter {
            start_utc: after_ts,
            end_utc: before_ts,
            phrase: None,
            tz: None,
        })
    } else {
        None
    };

    let lane_limit = args.limit.max(20);

    // Probe for strong lexical signal to optionally skip expansion.
    let mut strong_signal = false;
    if !args.no_expand {
        let probe_request = SearchRequest {
            query: cleaned_query.clone(),
            top_k: 2,
            snippet_chars: 80,
            uri: None,
            scope: scope.clone(),
            cursor: None,
            temporal: temporal.clone(),
            as_of_frame: None,
            as_of_ts: asof_ts,
            no_sketch: false,
        };
        match mem.search(probe_request) {
            Ok(resp) => {
                strong_signal = has_strong_signal(&resp.hits);
            }
            Err(err) => {
                warnings.push(format!("lex probe failed: {err}"));
            }
        }
    }

    let skipped_expansion = !args.no_expand && strong_signal;
    let (lex_queries, mut vec_queries) = if args.no_expand || strong_signal {
        (vec![cleaned_query.clone()], vec![cleaned_query.clone()])
    } else if let Some(hook) = expansion_hook.as_ref() {
        let input = ExpansionHookInput {
            query: cleaned_query.clone(),
            max_expansions: args.max_expansions,
            scope: scope.clone(),
            temporal: temporal.clone(),
        };
        match run_expansion_hook(hook, &input) {
            Ok(output) => {
                if !output.warnings.is_empty() {
                    warnings.extend(output.warnings);
                }
                let mut lex = output.lex;
                let mut vec = output.vec;
                if lex.is_empty() {
                    lex = vec![cleaned_query.clone()];
                }
                if vec.is_empty() {
                    vec = lex.clone();
                }
                (
                    lex.into_iter().take(args.max_expansions.max(1)).collect(),
                    vec.into_iter().take(args.max_expansions.max(1)).collect(),
                )
            }
            Err(err) => {
                warnings.push(format!("expansion hook failed: {err}"));
                (
                    build_expansions(&cleaned_query, args.max_expansions),
                    build_expansions(&cleaned_query, args.max_expansions),
                )
            }
        }
    } else {
        (
            build_expansions(&cleaned_query, args.max_expansions),
            build_expansions(&cleaned_query, args.max_expansions),
        )
    };
    if args.no_vector {
        vec_queries.clear();
    }
    #[cfg(not(feature = "vec"))]
    {
        vec_queries.clear();
    }

    let plan_obj = QueryPlan {
        cleaned_query: cleaned_query.clone(),
        scope: scope.clone(),
        as_of_ts: asof_ts,
        temporal: temporal.clone(),
        skipped_expansion,
        lex_queries: lex_queries.clone(),
        vec_queries: vec_queries.clone(),
    };

    if args.plan {
        print_plan(&plan_obj);
    }

    let mut lists: Vec<RankedList> = Vec::new();

    for (i, q) in lex_queries.iter().enumerate() {
        let request = SearchRequest {
            query: q.clone(),
            top_k: lane_limit,
            snippet_chars: args.snippet_chars,
            uri: None,
            scope: scope.clone(),
            cursor: None,
            temporal: temporal.clone(),
            as_of_frame: None,
            as_of_ts: asof_ts,
            no_sketch: false,
        };
        let hits = match mem.search(request) {
            Ok(resp) => resp.hits,
            Err(err) => {
                warnings.push(format!("lex search failed for '{q}': {err}"));
                Vec::new()
            }
        };
        if !hits.is_empty() {
            lists.push(build_ranked_list(LaneKind::Lex, q, i == 0, &hits));
        }
    }

    #[cfg(feature = "vec")]
    if !args.no_vector {
        let embed_config = build_embed_config(
            args.embed_model.as_deref(),
            args.embed_cache,
            !args.embed_no_cache,
        );
        let embedder = match LocalTextEmbedder::new(embed_config) {
            Ok(e) => Some(e),
            Err(err) => {
                warnings.push(format!("vector embedder unavailable: {err}"));
                None
            }
        };

        if let Some(embedder) = embedder {
            let unique_vec_queries = dedup_keep_order(vec_queries.clone());
            let mut embed_map: HashMap<String, Vec<f32>> = HashMap::new();
            if !unique_vec_queries.is_empty() {
                let refs: Vec<&str> = unique_vec_queries.iter().map(|q| q.as_str()).collect();
                match embedder.embed_batch(&refs) {
                    Ok(embeddings) => {
                        for (q, emb) in unique_vec_queries
                            .iter()
                            .cloned()
                            .zip(embeddings.into_iter())
                        {
                            embed_map.insert(q, emb);
                        }
                    }
                    Err(err) => {
                        warnings.push(format!(
                            "embed batch failed ({err}), falling back to single embeddings"
                        ));
                        for q in &unique_vec_queries {
                            match embedder.embed_text(q) {
                                Ok(emb) => {
                                    embed_map.insert(q.clone(), emb);
                                }
                                Err(err) => {
                                    warnings.push(format!("embedding failed for '{q}': {err}"));
                                }
                            }
                        }
                    }
                }
            }

            for (i, q) in vec_queries.iter().enumerate() {
                let Some(embedding) = embed_map.get(q) else {
                    continue;
                };

                let mut resp = match mem.vec_search_with_embedding(
                    q,
                    embedding,
                    lane_limit,
                    args.snippet_chars,
                    scope.as_deref(),
                ) {
                    Ok(r) => r,
                    Err(err) => {
                        warnings.push(format!("vec search failed for '{q}': {err}"));
                        continue;
                    }
                };

                // Manual as-of / temporal filter for vector lane (best-effort).
                if asof_ts.is_some() || before_ts.is_some() || after_ts.is_some() {
                    resp.hits.retain(|hit| {
                        let frame = mem.frame_by_id(hit.frame_id).ok();
                        let Some(frame) = frame else { return false };
                        if let Some(ts) = asof_ts {
                            if frame.timestamp > ts {
                                return false;
                            }
                        }
                        if let Some(after_ts) = after_ts {
                            if frame.timestamp < after_ts {
                                return false;
                            }
                        }
                        if let Some(before_ts) = before_ts {
                            if frame.timestamp > before_ts {
                                return false;
                            }
                        }
                        true
                    });
                }

                if !resp.hits.is_empty() {
                    lists.push(build_ranked_list(LaneKind::Vec, q, i == 0, &resp.hits));
                }
            }
        }
    }

    #[cfg(not(feature = "vec"))]
    if !args.no_vector {
        warnings.push("vector lane disabled (build with --features vec)".to_string());
    }

    if lists.is_empty() {
        return Ok(QueryResponse {
            query: args.raw_query,
            plan: plan_obj,
            warnings,
            results: Vec::new(),
        });
    }

    let fused = rrf_fuse(&lists, 60.0);

    let rerank_mode = if rerank_hook.is_some() {
        "hook"
    } else {
        args.rerank.as_str()
    };
    let mut rerank_scores: HashMap<String, (f32, Option<String>)> = HashMap::new();
    let mut rerank_active = false;

    match rerank_mode {
        "none" => {}
        "local" => {
            for cand in fused.iter().take(args.rerank_docs) {
                let text = match mem.frame_text_by_id(cand.frame_id) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let chunks = chunk_text(&text, args.rerank_chunk_chars, args.rerank_chunk_overlap);
                let mut best_score = 0.0f32;
                let mut best_chunk = String::new();
                for (chunk, _) in chunks {
                    let score = rerank_score(&cleaned_query, &chunk);
                    if score > best_score {
                        best_score = score;
                        best_chunk = chunk;
                    }
                }
                rerank_scores.insert(cand.key.clone(), (best_score, Some(best_chunk)));
            }
            rerank_active = !rerank_scores.is_empty();
        }
        "hook" => {
            if let Some(hook) = rerank_hook.as_ref() {
                let include_text = hook.full_text.unwrap_or(false);
                let mut candidates = Vec::new();
                for cand in fused.iter().take(args.rerank_docs) {
                    let text = if include_text {
                        mem.frame_text_by_id(cand.frame_id).ok()
                    } else {
                        None
                    };
                    candidates.push(RerankHookCandidate {
                        key: cand.key.clone(),
                        uri: cand.uri.clone(),
                        title: cand.title.clone(),
                        snippet: cand.snippet.clone(),
                        frame_id: cand.frame_id,
                        text,
                    });
                }
                let input = RerankHookInput {
                    query: cleaned_query.clone(),
                    candidates,
                };
                match run_rerank_hook(hook, &input) {
                    Ok(output) => {
                        if !output.warnings.is_empty() {
                            warnings.extend(output.warnings);
                        }
                        for (key, score) in output.scores {
                            let snippet = output.snippets.get(&key).cloned();
                            rerank_scores.insert(key, (score, snippet));
                        }
                        rerank_active = !rerank_scores.is_empty();
                    }
                    Err(err) => {
                        warnings.push(format!("rerank hook failed: {err}"));
                    }
                }
            } else {
                warnings.push("rerank hook selected but no hook configured".to_string());
            }
        }
        other => {
            warnings.push(format!("unknown rerank mode '{other}', defaulting to none"));
        }
    }

    let feedback_weight = args.feedback_weight.clamp(0.0, 1.0);
    let mut feedback_scores: HashMap<String, f32> = HashMap::new();
    if feedback_weight.abs() > 0.0 {
        let targets: std::collections::HashSet<String> =
            fused.iter().map(|c| c.uri.clone()).collect();
        feedback_scores = load_feedback_scores(mem, &targets);
    }

    let mut results: Vec<QueryResult> = Vec::new();
    for (idx, cand) in fused.iter().enumerate() {
        let rrf_rank = idx + 1;
        let rrf_total = cand.rrf_score + cand.rrf_bonus;
        let rerank_score_opt = rerank_scores.get(&cand.key).map(|(s, _)| *s);
        let base_score = if rerank_active {
            let weight = if rrf_rank <= 3 {
                0.75
            } else if rrf_rank <= 10 {
                0.60
            } else {
                0.40
            };
            let rrf_rank_score = 1.0 / (rrf_rank as f32);
            let rerank_score = rerank_score_opt.unwrap_or(0.0);
            weight * rrf_rank_score + (1.0 - weight) * rerank_score
        } else {
            rrf_total
        };
        let feedback_score = feedback_scores.get(&cand.uri).copied();
        let score = if let Some(fb) = feedback_score {
            base_score + feedback_weight * fb
        } else {
            base_score
        };

        let mut snippet = cand.snippet.clone();
        if let Some((_, override_snippet)) = rerank_scores.get(&cand.key) {
            if let Some(override_snippet) = override_snippet {
                if !override_snippet.trim().is_empty() {
                    snippet = override_snippet.clone();
                }
            }
        }

        results.push(QueryResult {
            rank: rrf_rank,
            frame_id: cand.frame_id,
            uri: cand.uri.clone(),
            title: cand.title.clone(),
            snippet,
            score,
            rrf_rank,
            rrf_score: rrf_total,
            rerank_score: rerank_score_opt,
            feedback_score,
            sources: cand.sources.clone(),
        });
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(args.limit);
    for (i, r) in results.iter_mut().enumerate() {
        r.rank = i + 1;
    }

    Ok(QueryResponse {
        query: args.raw_query,
        plan: plan_obj,
        warnings,
        results,
    })
}

fn build_context_pack(
    mem: &mut Vault,
    args: QueryArgs,
    max_bytes: usize,
    full: bool,
) -> Result<ContextPack, Box<dyn std::error::Error>> {
    let response = execute_query(mem, args)?;
    let mut context = String::new();
    let mut citations = Vec::new();

    for r in &response.results {
        if context.len() >= max_bytes {
            break;
        }
        let header = format!(
            "[{}] {} {}\n",
            r.rank,
            r.uri,
            r.title.clone().unwrap_or_default()
        );
        let mut body = if full {
            mem.frame_text_by_id(r.frame_id).unwrap_or_else(|_| r.snippet.clone())
        } else {
            r.snippet.clone()
        };
        let remaining = max_bytes.saturating_sub(context.len() + header.len());
        if remaining == 0 {
            break;
        }
        if body.len() > remaining {
            body.truncate(remaining);
        }
        context.push_str(&header);
        context.push_str(&body);
        context.push_str("\n\n");

        citations.push(ContextCitation {
            rank: r.rank,
            frame_id: r.frame_id,
            uri: r.uri.clone(),
            title: r.title.clone(),
            score: r.score,
        });
    }

    Ok(ContextPack {
        query: response.query,
        plan: response.plan,
        warnings: response.warnings,
        citations,
        context,
    })
}

fn append_agent_log(mem: &mut Vault, entry: &AgentLogEntry) -> Result<String, Box<dyn std::error::Error>> {
    append_agent_log_with_commit(mem, entry, true)
}

fn append_agent_log_uncommitted(
    mem: &mut Vault,
    entry: &AgentLogEntry,
) -> Result<String, Box<dyn std::error::Error>> {
    append_agent_log_with_commit(mem, entry, false)
}

fn append_agent_log_with_commit(
    mem: &mut Vault,
    entry: &AgentLogEntry,
    commit: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = serde_json::to_vec(entry)?;
    let ts = Utc::now().timestamp();
    let hash = blake3_hash(&bytes);
    let session_slug = entry
        .session
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let uri = format!("aethervault://agent-log/{session_slug}/{ts}-{}", hash.to_hex());

    let mut options = PutOptions::default();
    options.uri = Some(uri.clone());
    options.title = Some(format!("agent log ({})", entry.role));
    options.kind = Some("application/json".to_string());
    options.track = Some("aethervault.agent".to_string());
    options.search_text = Some(entry.text.clone());
    options
        .extra_metadata
        .insert("session".into(), session_slug);
    options
        .extra_metadata
        .insert("role".into(), entry.role.clone());

    mem.put_bytes_with_options(&bytes, options)?;
    if commit {
        mem.commit()?;
    }
    Ok(uri)
}

fn append_feedback(mem: &mut Vault, event: &FeedbackEvent) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = serde_json::to_vec(event)?;
    let ts = Utc::now().timestamp();
    let hash = blake3_hash(&bytes);
    let uri_log = format!("aethervault://feedback/{ts}-{}", hash.to_hex());

    let mut options = PutOptions::default();
    options.uri = Some(uri_log.clone());
    options.title = Some("aethervault feedback".to_string());
    options.kind = Some("application/json".to_string());
    options.track = Some("aethervault.feedback".to_string());
    let mut search_text = event.uri.clone();
    if let Some(note) = event.note.clone() {
        search_text.push(' ');
        search_text.push_str(&note);
    }
    options.search_text = Some(search_text);
    mem.put_bytes_with_options(&bytes, options)?;
    mem.commit()?;
    Ok(uri_log)
}

fn merge_capsule_into(
    out: &mut Vault,
    src_path: &Path,
    dedup: bool,
    dedup_map: &mut HashMap<String, u64>,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let mut src = Vault::open_read_only(src_path)?;
    let mut written = 0usize;
    let mut deduped = 0usize;
    let mut id_map: HashMap<u64, u64> = HashMap::new();
    let total = src.frame_count() as u64;

    for frame_id in 0..total {
        let frame = match src.frame_by_id(frame_id) {
            Ok(f) => f,
            Err(_) => continue,
        };
        if frame.status != FrameStatus::Active {
            continue;
        }
        let uri = frame.uri.clone().unwrap_or_default();
        let key = format!("{}|{}|{}", uri, checksum_hex(&frame.checksum), frame.timestamp);
        if dedup {
            if let Some(existing) = dedup_map.get(&key).copied() {
                id_map.insert(frame_id, existing);
                deduped += 1;
                continue;
            }
        }

        let payload = src.frame_canonical_payload(frame_id)?;
        let mut options = PutOptions::default();
        options.timestamp = Some(frame.timestamp);
        options.track = frame.track.clone();
        options.kind = frame.kind.clone();
        options.uri = frame.uri.clone();
        options.title = frame.title.clone();
        options.metadata = frame.metadata.clone();
        options.search_text = frame.search_text.clone();
        options.tags = frame.tags.clone();
        options.labels = frame.labels.clone();
        options.extra_metadata = frame.extra_metadata.clone();
        options.role = frame.role;
        options.parent_id = frame.parent_id.and_then(|pid| id_map.get(&pid).copied());
        options.auto_tag = false;
        options.extract_dates = false;
        options.extract_triplets = false;

        let new_id = out.put_bytes_with_options(&payload, options)?;
        id_map.insert(frame_id, new_id);
        if dedup {
            dedup_map.insert(key, new_id);
        }
        written += 1;
    }

    Ok((written, deduped))
}

fn tool_definitions_json() -> Vec<serde_json::Value> {
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
        })
    ]
}

#[allow(dead_code)]
fn execute_tool(
    name: &str,
    args: serde_json::Value,
    mv2: &Path,
    read_only: bool,
) -> Result<ToolExecution, String> {
    let mut mem_read = None;
    let mut mem_write = None;
    execute_tool_with_handles(name, args, mv2, read_only, &mut mem_read, &mut mem_write)
}

fn execute_tool_with_handles(
    name: &str,
    args: serde_json::Value,
    mv2: &Path,
    read_only: bool,
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
) -> Result<ToolExecution, String> {
    let is_write = matches!(name, "put" | "log" | "feedback");
    if read_only && is_write {
        return Err("tool disabled in read-only mode".into());
    }

    match name {
        "query" => {
            let parsed: ToolQueryArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let qargs = QueryArgs {
                    raw_query: parsed.query.clone(),
                    collection: parsed.collection,
                    limit: parsed.limit.unwrap_or(10),
                    snippet_chars: parsed.snippet_chars.unwrap_or(300),
                    no_expand: parsed.no_expand.unwrap_or(false),
                    max_expansions: parsed.max_expansions.unwrap_or(2),
                    expand_hook: None,
                    expand_hook_timeout_ms: 2000,
                    no_vector: parsed.no_vector.unwrap_or(false),
                    rerank: parsed.rerank.unwrap_or_else(|| "local".to_string()),
                    rerank_hook: None,
                    rerank_hook_timeout_ms: 6000,
                    rerank_hook_full_text: false,
                    embed_model: None,
                    embed_cache: 4096,
                    embed_no_cache: false,
                    rerank_docs: 40,
                    rerank_chunk_chars: 1200,
                    rerank_chunk_overlap: 200,
                    plan: false,
                    asof: parsed.asof,
                    before: parsed.before,
                    after: parsed.after,
                    feedback_weight: parsed.feedback_weight.unwrap_or(0.15),
                };
                let response = execute_query(mem, qargs).map_err(|e| e.to_string())?;
                let mut lines = Vec::new();
                for r in response.results.iter().take(5) {
                    lines.push(format!("{}. {} ({:.3})", r.rank, r.uri, r.score));
                }
                let output = if lines.is_empty() {
                    "No results.".to_string()
                } else {
                    lines.join("\n")
                };
                let details = serde_json::to_value(response).map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })
        }
        "context" => {
            let parsed: ToolContextArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let qargs = QueryArgs {
                    raw_query: parsed.query.clone(),
                    collection: parsed.collection,
                    limit: parsed.limit.unwrap_or(10),
                    snippet_chars: parsed.snippet_chars.unwrap_or(300),
                    no_expand: parsed.no_expand.unwrap_or(false),
                    max_expansions: parsed.max_expansions.unwrap_or(2),
                    expand_hook: None,
                    expand_hook_timeout_ms: 2000,
                    no_vector: parsed.no_vector.unwrap_or(false),
                    rerank: parsed.rerank.unwrap_or_else(|| "local".to_string()),
                    rerank_hook: None,
                    rerank_hook_timeout_ms: 6000,
                    rerank_hook_full_text: false,
                    embed_model: None,
                    embed_cache: 4096,
                    embed_no_cache: false,
                    rerank_docs: parsed.limit.unwrap_or(10).max(20),
                    rerank_chunk_chars: 1200,
                    rerank_chunk_overlap: 200,
                    plan: false,
                    asof: parsed.asof,
                    before: parsed.before,
                    after: parsed.after,
                    feedback_weight: parsed.feedback_weight.unwrap_or(0.15),
                };
                let pack = build_context_pack(
                    mem,
                    qargs,
                    parsed.max_bytes.unwrap_or(12_000),
                    parsed.full.unwrap_or(false),
                )
                .map_err(|e| e.to_string())?;
                let output = pack.context.clone();
                let details = serde_json::to_value(pack).map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })
        }
        "search" => {
            let parsed: ToolSearchArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let scope = parsed.collection.as_deref().map(scope_prefix);
                let request = SearchRequest {
                    query: parsed.query.clone(),
                    top_k: parsed.limit.unwrap_or(10),
                    snippet_chars: parsed.snippet_chars.unwrap_or(300),
                    uri: None,
                    scope,
                    cursor: None,
                    temporal: None,
                    as_of_frame: None,
                    as_of_ts: None,
                    no_sketch: false,
                };
                let response = mem.search(request).map_err(|e| e.to_string())?;
                let mut lines = Vec::new();
                for hit in response.hits.iter().take(5) {
                    let title = hit.title.clone().unwrap_or_default();
                    lines.push(format!("{}. {} {}", hit.rank, hit.uri, title));
                }
                let output = if lines.is_empty() {
                    "No results.".to_string()
                } else {
                    lines.join("\n")
                };
                let details = serde_json::to_value(response).map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })
        }
        "get" => {
            let parsed: ToolGetArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let (frame_id, frame) = if let Some(rest) = parsed.id.strip_prefix('#') {
                    let frame_id: u64 = rest.parse().map_err(|_| "invalid frame id")?;
                    let frame = mem.frame_by_id(frame_id).map_err(|e| e.to_string())?;
                    (frame_id, frame)
                } else {
                    let frame = mem.frame_by_uri(&parsed.id).map_err(|e| e.to_string())?;
                    (frame.id, frame)
                };
                let text = mem.frame_text_by_id(frame_id).unwrap_or_default();
                let details = serde_json::json!({
                    "frame_id": frame_id,
                    "uri": frame.uri,
                    "title": frame.title,
                    "text": text
                });
                let output = if details["text"].as_str().unwrap_or("").is_empty() {
                    format!("Frame #{frame_id} (non-text payload)")
                } else {
                    details["text"].as_str().unwrap_or("").to_string()
                };
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })
        }
        "put" => {
            let parsed: ToolPutArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let Some(text) = parsed.text else {
                return Err("put requires text".into());
            };
            let result = with_write_mem(mem_write, mv2, true, |mem| {
                let mut options = PutOptions::default();
                options.uri = Some(parsed.uri.clone());
                options.title = Some(parsed.title.unwrap_or_else(|| parsed.uri.clone()));
                options.track = parsed.track;
                options.kind = parsed.kind;
                options.search_text = Some(text.clone());
                let frame_id = mem
                    .put_bytes_with_options(text.as_bytes(), options)
                    .map_err(|e| e.to_string())?;
                mem.commit().map_err(|e| e.to_string())?;
                let details = serde_json::json!({
                    "frame_id": frame_id,
                    "uri": parsed.uri
                });
                let output = format!("Stored frame #{frame_id}");
                Ok(ToolExecution {
                    output,
                    details,
                    is_error: false,
                })
            })?;
            *mem_read = None;
            Ok(result)
        }
        "log" => {
            let parsed: ToolLogArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let entry = AgentLogEntry {
                session: parsed.session.clone(),
                role: parsed.role.unwrap_or_else(|| "user".to_string()),
                text: parsed.text.clone(),
                meta: parsed.meta.clone(),
                ts_utc: Some(Utc::now().timestamp()),
            };
            let result = with_write_mem(mem_write, mv2, false, |mem| {
                let uri = append_agent_log(mem, &entry).map_err(|e| e.to_string())?;
                let details = serde_json::json!({ "uri": uri });
                Ok(ToolExecution {
                    output: "Logged agent turn.".to_string(),
                    details,
                    is_error: false,
                })
            })?;
            *mem_read = None;
            Ok(result)
        }
        "feedback" => {
            let parsed: ToolFeedbackArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let event = FeedbackEvent {
                uri: parsed.uri.clone(),
                score: parsed.score.clamp(-1.0, 1.0),
                note: parsed.note.clone(),
                session: parsed.session.clone(),
                ts_utc: Some(Utc::now().timestamp()),
            };
            let result = with_write_mem(mem_write, mv2, false, |mem| {
                let uri_log = append_feedback(mem, &event).map_err(|e| e.to_string())?;
                let details = serde_json::json!({ "uri": uri_log });
                Ok(ToolExecution {
                    output: "Feedback recorded.".to_string(),
                    details,
                    is_error: false,
                })
            })?;
            *mem_read = None;
            Ok(result)
        }
        _ => Err("unknown tool".into()),
    }
}

fn read_mcp_message(reader: &mut BufReader<impl Read>) -> io::Result<Option<serde_json::Value>> {
    let mut first_line = String::new();
    if reader.read_line(&mut first_line)? == 0 {
        return Ok(None);
    }
    if first_line.trim().is_empty() {
        return Ok(None);
    }

    if first_line.to_ascii_lowercase().starts_with("content-length:") {
        let mut content_length = first_line
            .split(':')
            .nth(1)
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);

        // Read remaining headers
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line == "\r\n" || line == "\n" || line.is_empty() {
                break;
            }
            if line.to_ascii_lowercase().starts_with("content-length:") {
                content_length = line
                    .split(':')
                    .nth(1)
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(content_length);
            }
        }

        if content_length == 0 {
            return Ok(None);
        }
        let mut buffer = vec![0u8; content_length];
        reader.read_exact(&mut buffer)?;
        let value = serde_json::from_slice(&buffer).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {e}"))
        })?;
        Ok(Some(value))
    } else {
        let value = serde_json::from_str(first_line.trim()).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {e}"))
        })?;
        Ok(Some(value))
    }
}

fn print_doctor_report(report: &DoctorReport) {
    println!("status: {:?}", report.status);
    println!(
        "actions: executed={} skipped={}",
        report.metrics.actions_completed,
        report.metrics.actions_skipped
    );
    println!("duration_ms: {}", report.metrics.total_duration_ms);
    if let Some(verification) = &report.verification {
        println!("verification: {:?}", verification.overall_status);
    }
    if report.findings.is_empty() {
        println!("findings: none");
    } else {
        println!("findings:");
        for finding in &report.findings {
            println!(
                "- {:?} {:?}: {}",
                finding.severity, finding.code, finding.message
            );
        }
    }
}

fn write_mcp_response(writer: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    let payload = serde_json::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e}")))?;
    write!(writer, "Content-Length: {}\r\n\r\n", payload.len())?;
    writer.write_all(&payload)?;
    writer.flush()
}

fn run_mcp_server(mv2: PathBuf, read_only: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = BufReader::new(io::stdin());
    let mut writer = io::stdout();
    let tools = tool_definitions_json();
    let mut mem_read: Option<Vault> = None;
    let mut mem_write: Option<Vault> = None;

    loop {
        let Some(msg) = read_mcp_message(&mut reader)? else {
            break;
        };
        let id = msg.get("id").cloned();
        let has_id = id.as_ref().is_some_and(|v| !v.is_null());
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or_else(|| serde_json::json!({}));

        let response = match method {
            "initialize" => {
                let protocol = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0.1");
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": protocol,
                        "capabilities": {
                            "tools": {
                                "list": true,
                                "call": true
                            }
                        },
                        "serverInfo": {
                            "name": "kairos-vault",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                })
            }
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tools }
            }),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or_else(|| serde_json::json!({}));
                match execute_tool_with_handles(
                    name,
                    arguments,
                    &mv2,
                    read_only,
                    &mut mem_read,
                    &mut mem_write,
                ) {
                    Ok(result) => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [
                                { "type": "text", "text": result.output }
                            ],
                            "details": result.details,
                            "isError": false
                        }
                    }),
                    Err(err) => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32000, "message": err }
                    }),
                }
            }
            "shutdown" => {
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": null
                });
                write_mcp_response(&mut writer, &response)?;
                break;
            }
            _ => {
                if !has_id {
                    continue;
                }
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "method not found" }
                })
            }
        };

        if has_id || method == "initialize" || method == "tools/list" || method == "tools/call" {
            write_mcp_response(&mut writer, &response)?;
        }
    }

    Ok(())
}

fn env_required(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value = env::var(name).unwrap_or_default();
    if value.trim().is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("Missing {name}"))
            .into());
    }
    Ok(value)
}

fn env_optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn env_u64(name: &str, default: u64) -> Result<u64, Box<dyn std::error::Error>> {
    match env_optional(name) {
        Some(value) => Ok(value.parse::<u64>().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid {name}"))
        })?),
        None => Ok(default),
    }
}

fn env_usize(name: &str, default: usize) -> Result<usize, Box<dyn std::error::Error>> {
    match env_optional(name) {
        Some(value) => Ok(value.parse::<usize>().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid {name}"))
        })?),
        None => Ok(default),
    }
}

fn env_f64(name: &str, default: f64) -> Result<f64, Box<dyn std::error::Error>> {
    match env_optional(name) {
        Some(value) => Ok(value.parse::<f64>().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid {name}"))
        })?),
        None => Ok(default),
    }
}

fn merge_system_messages(messages: &[AgentMessage]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        if msg.role == "system" {
            if let Some(content) = &msg.content {
                if !content.trim().is_empty() {
                    parts.push(content.trim().to_string());
                }
            }
        }
    }
    parts.join("\n\n")
}

fn to_anthropic_messages(messages: &[AgentMessage]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for msg in messages {
        match msg.role.as_str() {
            "system" => continue,
            "user" => {
                let content = msg.content.clone().unwrap_or_default();
                out.push(serde_json::json!({
                    "role": "user",
                    "content": [{"type": "text", "text": content}]
                }));
            }
            "assistant" => {
                let mut blocks = Vec::new();
                if let Some(content) = &msg.content {
                    if !content.is_empty() {
                        blocks.push(serde_json::json!({"type": "text", "text": content}));
                    }
                }
                for call in &msg.tool_calls {
                    blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": call.id.clone(),
                        "name": call.name.clone(),
                        "input": call.args.clone()
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(serde_json::json!({"type": "text", "text": ""}));
                }
                out.push(serde_json::json!({"role": "assistant", "content": blocks}));
            }
            "tool" => {
                let Some(tool_id) = msg.tool_call_id.clone() else {
                    continue;
                };
                let mut block = serde_json::Map::new();
                block.insert("type".to_string(), serde_json::json!("tool_result"));
                block.insert("tool_use_id".to_string(), serde_json::json!(tool_id));
                block.insert(
                    "content".to_string(),
                    serde_json::json!(msg.content.clone().unwrap_or_default()),
                );
                if msg.is_error.unwrap_or(false) {
                    block.insert("is_error".to_string(), serde_json::json!(true));
                }
                out.push(serde_json::json!({
                    "role": "user",
                    "content": [serde_json::Value::Object(block)]
                }));
            }
            _ => {}
        }
    }
    out
}

fn to_anthropic_tools(tools: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for tool in tools {
        let Some(obj) = tool.as_object() else {
            continue;
        };
        let Some(name) = obj.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let mut entry = serde_json::Map::new();
        entry.insert("name".to_string(), serde_json::json!(name));
        if let Some(desc) = obj.get("description").and_then(|v| v.as_str()) {
            entry.insert("description".to_string(), serde_json::json!(desc));
        }
        if let Some(schema) = obj.get("inputSchema").or_else(|| obj.get("input_schema")) {
            entry.insert("input_schema".to_string(), schema.clone());
        }
        out.push(serde_json::Value::Object(entry));
    }
    out
}

fn parse_claude_response(payload: &serde_json::Value) -> Result<AgentHookResponse, Box<dyn std::error::Error>> {
    let content = payload
        .get("content")
        .and_then(|v| v.as_array())
        .ok_or("Claude response missing content")?;
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for block in content {
        let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match btype {
            "text" => {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        text_parts.push(text.to_string());
                    }
                }
            }
            "tool_use" => {
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = block.get("input").cloned().unwrap_or_else(|| serde_json::json!({}));
                tool_calls.push(AgentToolCall { id, name, args });
            }
            _ => {}
        }
    }

    let content_text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    };

    Ok(AgentHookResponse {
        message: AgentMessage {
            role: "assistant".to_string(),
            content: content_text,
            tool_calls,
            name: None,
            tool_call_id: None,
            is_error: None,
        },
    })
}

fn run_claude_hook() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    if input.trim().is_empty() {
        return Err("Claude hook received empty input".into());
    }
    let req: AgentHookRequest = serde_json::from_str(&input)?;

    let api_key = env_required("ANTHROPIC_API_KEY")?;
    let model = env_required("ANTHROPIC_MODEL")?;
    let base_url = env_optional("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string());
    let max_tokens = env_u64("ANTHROPIC_MAX_TOKENS", 1024)?;
    let temperature = env_optional("ANTHROPIC_TEMPERATURE")
        .map(|v| v.parse::<f64>())
        .transpose()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid ANTHROPIC_TEMPERATURE"))?;
    let top_p = env_optional("ANTHROPIC_TOP_P")
        .map(|v| v.parse::<f64>())
        .transpose()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid ANTHROPIC_TOP_P"))?;
    let timeout = env_f64("ANTHROPIC_TIMEOUT", 60.0)?;
    let max_retries = env_usize("ANTHROPIC_MAX_RETRIES", 2)?;
    let retry_base = env_f64("ANTHROPIC_RETRY_BASE", 0.5)?;
    let retry_max = env_f64("ANTHROPIC_RETRY_MAX", 4.0)?;
    let version = env_optional("ANTHROPIC_VERSION")
        .unwrap_or_else(|| "2023-06-01".to_string());
    let beta = env_optional("ANTHROPIC_BETA");

    let system = merge_system_messages(&req.messages);
    let mut payload = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": to_anthropic_messages(&req.messages),
    });
    if !system.is_empty() {
        payload["system"] = serde_json::json!(system);
    }
    let tools = to_anthropic_tools(&req.tools);
    if !tools.is_empty() {
        payload["tools"] = serde_json::json!(tools);
    }
    if let Some(temp) = temperature {
        payload["temperature"] = serde_json::json!(temp);
    }
    if let Some(p) = top_p {
        payload["top_p"] = serde_json::json!(p);
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs_f64(timeout))
        .timeout_read(Duration::from_secs_f64(timeout))
        .timeout_write(Duration::from_secs_f64(timeout))
        .build();

    let retryable = |status: u16| matches!(status, 429 | 500 | 502 | 503 | 504 | 529);
    let mut body = None;

    for attempt in 0..=max_retries {
        let mut request = agent
            .post(&base_url)
            .set("content-type", "application/json")
            .set("x-api-key", &api_key)
            .set("anthropic-version", &version);
        if let Some(beta) = &beta {
            request = request.set("anthropic-beta", beta);
        }

        let response = request.send_json(payload.clone());
        match response {
            Ok(resp) => {
                body = Some(resp.into_string()?);
                break;
            }
            Err(ureq::Error::Status(code, resp)) => {
                let text = resp.into_string().unwrap_or_default();
                if attempt < max_retries && retryable(code) {
                    let delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                    thread::sleep(Duration::from_secs_f64(delay));
                    continue;
                }
                return Err(format!("Anthropic API error: {code} {text}").into());
            }
            Err(ureq::Error::Transport(err)) => {
                if attempt < max_retries {
                    let delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                    thread::sleep(Duration::from_secs_f64(delay));
                    continue;
                }
                return Err(format!("Anthropic API request failed: {err}").into());
            }
        }
    }

    let body = body.ok_or("Anthropic API request failed without a response")?;
    let payload: serde_json::Value = serde_json::from_str(&body)?;
    let response = parse_claude_response(&payload)?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn call_agent_hook(hook: &HookSpec, request: &AgentHookRequest) -> Result<AgentMessage, String> {
    let cmd = command_spec_to_vec(&hook.command);
    let timeout = hook.timeout_ms.unwrap_or(60000);
    let value = serde_json::to_value(request).map_err(|e| format!("hook input: {e}"))?;
    let raw = run_hook_command(&cmd, &value, timeout, "agent")?;
    let response: AgentHookResponse =
        serde_json::from_str(&raw).map_err(|e| format!("hook output: {e}"))?;
    Ok(response.message)
}

fn run_agent(
    mv2: PathBuf,
    prompt: Option<String>,
    file: Option<PathBuf>,
    session: Option<String>,
    model_hook: Option<String>,
    system: Option<String>,
    system_file: Option<PathBuf>,
    no_memory: bool,
    context_query: Option<String>,
    context_results: usize,
    context_max_bytes: usize,
    max_steps: usize,
    log_commit_interval: usize,
    json: bool,
    log: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let prompt_text = if let Some(file) = file {
        fs::read_to_string(file)?
    } else if let Some(prompt) = prompt {
        prompt
    } else {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer
    };
    if prompt_text.trim().is_empty() {
        return Err("agent prompt is empty".into());
    }

    let mut mem_read = Some(Vault::open_read_only(&mv2)?);
    let config = load_capsule_config(mem_read.as_mut().unwrap()).unwrap_or_default();
    let agent_cfg = config.agent.clone().unwrap_or_default();
    let hook_cfg = config.hooks.clone().unwrap_or_default();
    let model_spec = resolve_hook_spec(
        model_hook,
        60000,
        agent_cfg.model_hook.or(hook_cfg.llm),
        None,
    )
    .ok_or("agent requires --model-hook or config.agent.model_hook or config.hooks.llm")?;

    let mut system_prompt = if let Some(path) = system_file {
        fs::read_to_string(path)?
    } else if let Some(system) = system {
        system
    } else if let Some(system) = agent_cfg.system {
        system
    } else {
        "You are a concise, high-performance personal assistant. Use tools when needed. Avoid plans and TODO lists unless asked.".to_string()
    };

    if let Some(global_context) = config.context {
        if !global_context.trim().is_empty() {
            system_prompt.push_str("\n\n# Global Context\n");
            system_prompt.push_str(&global_context);
        }
    }

    let mut context_pack = None;
    let effective_max_steps = agent_cfg.max_steps.unwrap_or(max_steps);
    let effective_log_commit_interval = agent_cfg
        .log_commit_interval
        .unwrap_or(log_commit_interval)
        .max(1);
    if !no_memory {
        let query = context_query
            .or(agent_cfg.context_query)
            .unwrap_or_else(|| prompt_text.clone());
        let qargs = QueryArgs {
            raw_query: query,
            collection: None,
            limit: agent_cfg.max_context_results.unwrap_or(context_results),
            snippet_chars: 300,
            no_expand: false,
            max_expansions: 2,
            expand_hook: None,
            expand_hook_timeout_ms: 2000,
            no_vector: false,
            rerank: "local".to_string(),
            rerank_hook: None,
            rerank_hook_timeout_ms: 6000,
            rerank_hook_full_text: false,
            embed_model: None,
            embed_cache: 4096,
            embed_no_cache: false,
            rerank_docs: 40,
            rerank_chunk_chars: 1200,
            rerank_chunk_overlap: 200,
            plan: false,
            asof: None,
            before: None,
            after: None,
            feedback_weight: 0.15,
        };
        if let Ok(pack) = build_context_pack(
            mem_read.as_mut().unwrap(),
            qargs,
            agent_cfg.max_context_bytes.unwrap_or(context_max_bytes),
            false,
        ) {
            if !pack.context.trim().is_empty() {
                system_prompt.push_str("\n\n# Memory Context\n");
                system_prompt.push_str(&pack.context);
                context_pack = Some(pack);
            }
        }
    }

    let mut messages = Vec::new();
    messages.push(AgentMessage {
        role: "system".to_string(),
        content: Some(system_prompt),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });
    messages.push(AgentMessage {
        role: "user".to_string(),
        content: Some(prompt_text.clone()),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });

    let tools = tool_definitions_json();
    let mut tool_results: Vec<AgentToolResult> = Vec::new();
    let should_log = log || agent_cfg.log.unwrap_or(false);
    let mut final_text = None;

    let mut mem_write: Option<Vault> = None;
    let mut pending_log_writes = 0usize;

    let flush_logs = |mem_read: &mut Option<Vault>, mem_write: &mut Option<Vault>, pending: &mut usize| {
        if *pending == 0 {
            return Ok(()) as Result<(), Box<dyn std::error::Error>>;
        }
        if let Some(mem) = mem_write.as_mut() {
            mem.commit()?;
            *pending = 0;
            *mem_read = None;
        }
        Ok(())
    };
    if should_log {
        mem_write = Some(Vault::open(&mv2)?);
        let entry = AgentLogEntry {
            session: session.clone(),
            role: "user".to_string(),
            text: prompt_text.clone(),
            meta: None,
            ts_utc: Some(Utc::now().timestamp()),
        };
        if let Some(mem) = mem_write.as_mut() {
            let _ = append_agent_log_uncommitted(mem, &entry)?;
            pending_log_writes += 1;
            if pending_log_writes >= effective_log_commit_interval {
                flush_logs(&mut mem_read, &mut mem_write, &mut pending_log_writes)?;
            }
        }
    }

    let mut completed = false;
    for _ in 0..effective_max_steps {
        let request = AgentHookRequest {
            messages: messages.clone(),
            tools: tools.clone(),
            session: session.clone(),
        };
        let message = call_agent_hook(&model_spec, &request)?;
        if let Some(content) = message.content.clone() {
            final_text = Some(content.clone());
            if should_log {
                let entry = AgentLogEntry {
                    session: session.clone(),
                    role: "assistant".to_string(),
                    text: content,
                    meta: None,
                    ts_utc: Some(Utc::now().timestamp()),
                };
                if let Some(mem) = mem_write.as_mut() {
                    let _ = append_agent_log_uncommitted(mem, &entry)?;
                    pending_log_writes += 1;
                    if pending_log_writes >= effective_log_commit_interval {
                        flush_logs(&mut mem_read, &mut mem_write, &mut pending_log_writes)?;
                    }
                }
            }
        }
        let tool_calls = message.tool_calls.clone();
        messages.push(message);
        if tool_calls.is_empty() {
            completed = true;
            break;
        }

        for call in tool_calls {
            if call.id.trim().is_empty() {
                return Err("tool call is missing an id".into());
            }
            if call.name.trim().is_empty() {
                return Err("tool call is missing a name".into());
            }
            let result = match execute_tool_with_handles(
                &call.name,
                call.args.clone(),
                &mv2,
                false,
                &mut mem_read,
                &mut mem_write,
            ) {
                Ok(result) => result,
                Err(err) => ToolExecution {
                    output: format!("Tool error: {err}"),
                    details: serde_json::json!({ "error": err }),
                    is_error: true,
                },
            };
            let tool_content = format_tool_message_content(&call.name, &result.output, &result.details);
            tool_results.push(AgentToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                output: result.output.clone(),
                details: result.details.clone(),
                is_error: result.is_error,
            });
            let tool_message = AgentMessage {
                role: "tool".to_string(),
                content: if tool_content.is_empty() { None } else { Some(tool_content) },
                tool_calls: Vec::new(),
                name: Some(call.name.clone()),
                tool_call_id: Some(call.id.clone()),
                is_error: Some(result.is_error),
            };
            messages.push(tool_message);

            if should_log {
                let entry = AgentLogEntry {
                    session: session.clone(),
                    role: "tool".to_string(),
                    text: result.output,
                    meta: Some(result.details),
                    ts_utc: Some(Utc::now().timestamp()),
                };
                if let Some(mem) = mem_write.as_mut() {
                    let _ = append_agent_log_uncommitted(mem, &entry)?;
                    pending_log_writes += 1;
                    if pending_log_writes >= effective_log_commit_interval {
                        flush_logs(&mut mem_read, &mut mem_write, &mut pending_log_writes)?;
                    }
                }
            }

            if matches!(call.name.as_str(), "put" | "log" | "feedback") && !result.is_error {
                pending_log_writes = 0;
            }
        }
    }

    if should_log {
        flush_logs(&mut mem_read, &mut mem_write, &mut pending_log_writes)?;
    }

    if !completed {
        return Err(format!(
            "agent exceeded {} steps without completing",
            effective_max_steps
        )
        .into());
    }

    if json {
        let payload = AgentSession {
            session,
            context: context_pack,
            messages,
            tool_results,
        };
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if let Some(text) = final_text {
        println!("{text}");
    }
    Ok(())
}

#[cfg(feature = "vec")]
fn collect_active_frame_ids(mem: &Vault, scope: Option<&str>) -> Vec<u64> {
    let mut latest: HashMap<String, u64> = HashMap::new();
    let count = mem.frame_count() as u64;
    for frame_id in 0..count {
        let frame = match mem.frame_by_id(frame_id) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let Some(uri) = frame.uri.clone() else {
            continue;
        };
        if let Some(prefix) = scope {
            if !uri.starts_with(prefix) {
                continue;
            }
        }
        if frame.status == FrameStatus::Active {
            latest.insert(uri, frame.id);
        } else {
            latest.remove(&uri);
        }
    }
    let mut ids: Vec<u64> = latest.values().copied().collect();
    ids.sort();
    ids
}

#[cfg(feature = "vec")]
fn build_embed_config(
    model: Option<&str>,
    cache_capacity: usize,
    enable_cache: bool,
) -> TextEmbedConfig {
    let mut config = match model.map(|m| m.to_ascii_lowercase()) {
        Some(ref name) if name == "bge-small" || name == "bge-small-en-v1.5" => {
            TextEmbedConfig::bge_small()
        }
        Some(ref name) if name == "bge-base" || name == "bge-base-en-v1.5" => {
            TextEmbedConfig::bge_base()
        }
        Some(ref name) if name == "nomic" || name == "nomic-embed-text-v1.5" => {
            TextEmbedConfig::nomic()
        }
        Some(ref name) if name == "gte-large" => TextEmbedConfig::gte_large(),
        Some(name) => {
            let mut cfg = TextEmbedConfig::default();
            cfg.model_name = name;
            cfg
        }
        None => TextEmbedConfig::default(),
    };
    config.cache_capacity = cache_capacity;
    config.enable_cache = enable_cache;
    config
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init { mv2 } => {
            if mv2.exists() {
                eprintln!("Refusing to overwrite existing file: {}", mv2.display());
                std::process::exit(2);
            }
            let _ = Vault::create(&mv2)?;
            println!("Created {}", mv2.display());
            Ok(())
        }

        Command::Ingest {
            mv2,
            collection,
            root,
            exts,
            dry_run,
        } => {
            let root = root.canonicalize().unwrap_or(root);
            if !root.exists() {
                eprintln!("Root does not exist: {}", root.display());
                std::process::exit(2);
            }

            let mut mem = open_or_create(&mv2)?;
            // Ensure lex is enabled so ingestion is immediately searchable.
            // (No-op if already enabled.)
            mem.enable_lex()?;

            let mut scanned = 0usize;
            let mut ingested = 0usize;
            let mut updated = 0usize;
            let mut skipped = 0usize;

            for entry in WalkDir::new(&root).follow_links(false) {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if !is_extension_allowed(path, &exts) {
                    continue;
                }

                let Ok(relative) = path.strip_prefix(&root) else {
                    continue;
                };

                scanned += 1;

                let bytes = fs::read(path)?;
                let file_hash = blake3_hash(&bytes);
                let uri = uri_for_path(&collection, relative);
                let title = infer_title(path, &bytes);

                let existing_checksum = mem
                    .frame_by_uri(&uri)
                    .ok()
                    .map(|frame| frame.checksum);

                if existing_checksum.is_some_and(|c| c == *file_hash.as_bytes()) {
                    skipped += 1;
                    continue;
                }

                if dry_run {
                    if existing_checksum.is_some() {
                        updated += 1;
                    } else {
                        ingested += 1;
                    }
                    continue;
                }

                let meta = entry.metadata().ok();
                let mtime_ms = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis().to_string())
                    .unwrap_or_default();
                let size_bytes = meta
                    .as_ref()
                    .map(|m| m.len().to_string())
                    .unwrap_or_default();

                let mut options = PutOptions::default();
                options.uri = Some(uri);
                options.title = Some(title);
                options.track = Some(normalize_collection(&collection));
                options.kind = Some("text/markdown".to_string());
                options
                    .extra_metadata
                    .insert("source_path".into(), path.to_string_lossy().into_owned());
                options.extra_metadata.insert(
                    "relative_path".into(),
                    relative.to_string_lossy().into_owned(),
                );
                if !mtime_ms.is_empty() {
                    options.extra_metadata.insert("mtime_ms".into(), mtime_ms);
                }
                if !size_bytes.is_empty() {
                    options.extra_metadata.insert("size_bytes".into(), size_bytes);
                }

                mem.put_bytes_with_options(&bytes, options)?;

                if existing_checksum.is_some() {
                    updated += 1;
                } else {
                    ingested += 1;
                }
            }

            if dry_run {
                println!(
                    "Dry run: scanned={scanned} ingest={ingested} update={updated} skip={skipped}"
                );
                return Ok(());
            }

            mem.commit()?;
            println!("Done: scanned={scanned} ingest={ingested} update={updated} skip={skipped}");
            Ok(())
        }

        Command::Put {
            mv2,
            uri,
            collection,
            path,
            title,
            track,
            kind,
            text,
            file,
            json,
        } => {
            let payload = if let Some(file) = file {
                fs::read(file)?
            } else if let Some(text) = text {
                text.into_bytes()
            } else {
                return Err("put requires --text or --file".into());
            };

            let uri = if let Some(uri) = uri {
                uri
            } else if let (Some(collection), Some(path)) = (collection, path) {
                uri_for_path(&collection, Path::new(&path))
            } else {
                return Err("put requires --uri or --collection + --path".into());
            };

            let inferred_title = uri
                .split('/')
                .last()
                .filter(|s| !s.is_empty())
                .unwrap_or(&uri)
                .to_string();

            let mut options = PutOptions::default();
            options.uri = Some(uri.clone());
            options.title = Some(title.unwrap_or(inferred_title));
            options.track = track;
            options.kind = kind;
            if let Ok(text) = String::from_utf8(payload.clone()) {
                options.search_text = Some(text);
            }

            let mut mem = open_or_create(&mv2)?;
            let frame_id = mem.put_bytes_with_options(&payload, options)?;
            mem.commit()?;

            if json {
                let response = serde_json::json!({
                    "frame_id": frame_id,
                    "uri": uri
                });
                println!("{}", serde_json::to_string_pretty(&response)?);
            } else {
                println!("Added frame #{frame_id} {}", uri);
            }
            Ok(())
        }

        Command::Search {
            mv2,
            query,
            limit,
            collection,
            snippet_chars,
            json,
        } => {
            let mut mem = Vault::open_read_only(&mv2)?;
            let scope = collection.as_deref().map(scope_prefix);

            let request = SearchRequest {
                query: query.clone(),
                top_k: limit,
                snippet_chars,
                uri: None,
                scope,
                cursor: None,
                temporal: None,
                as_of_frame: None,
                as_of_ts: None,
                no_sketch: false,
            };

            let response = mem.search(request)?;

            if json {
                println!("{}", serde_json::to_string_pretty(&response)?);
                return Ok(());
            }

            for hit in response.hits {
                let title = hit.title.unwrap_or_default();
                if let Some(score) = hit.score {
                    println!("{:>2}. {:>6.3}  {}  {}", hit.rank, score, hit.uri, title);
                } else {
                    println!("{:>2}. {}  {}", hit.rank, hit.uri, title);
                }
                if !hit.text.trim().is_empty() {
                    println!("    {}", hit.text.replace('\n', " "));
                }
            }

            Ok(())
        }


        Command::Query {
            mv2,
            query,
            limit,
            collection,
            snippet_chars,
            no_expand,
            max_expansions,
            expand_hook,
            expand_hook_timeout_ms,
            no_vector,
            rerank,
            rerank_hook,
            rerank_hook_timeout_ms,
            rerank_hook_full_text,
            embed_model,
            embed_cache,
            embed_no_cache,
            rerank_docs,
            rerank_chunk_chars,
            rerank_chunk_overlap,
            json,
            files,
            plan,
            log,
            asof,
            before,
            after,
            feedback_weight,
        } => {
            let mut mem = if log {
                Vault::open(&mv2)?
            } else {
                Vault::open_read_only(&mv2)?
            };

            let args = QueryArgs {
                raw_query: query.clone(),
                collection,
                limit,
                snippet_chars,
                no_expand,
                max_expansions,
                expand_hook,
                expand_hook_timeout_ms,
                no_vector,
                rerank,
                rerank_hook,
                rerank_hook_timeout_ms,
                rerank_hook_full_text,
                embed_model,
                embed_cache,
                embed_no_cache,
                rerank_docs,
                rerank_chunk_chars,
                rerank_chunk_overlap,
                plan,
                asof,
                before,
                after,
                feedback_weight,
            };

            let response = execute_query(&mut mem, args)?;

            if log {
                #[derive(Serialize)]
                struct QueryLog<'a> {
                    query: &'a str,
                    plan: &'a QueryPlan,
                    results: &'a [QueryResult],
                }

                let log_payload = QueryLog {
                    query: &response.query,
                    plan: &response.plan,
                    results: &response.results,
                };
                let bytes = serde_json::to_vec(&log_payload)?;
                let ts = Utc::now().timestamp();
                let hash = blake3_hash(&bytes);
                let uri = format!("aethervault://query-log/{ts}-{}", hash.to_hex());
                let mut options = PutOptions::default();
                options.uri = Some(uri);
                options.title = Some("aethervault query log".to_string());
                options.kind = Some("application/json".to_string());
                options.track = Some("aethervault.query".to_string());
                options.search_text = Some(response.plan.cleaned_query.clone());
                mem.put_bytes_with_options(&bytes, options)?;
                mem.commit()?;
            }

            if !response.warnings.is_empty() && !json {
                for warning in &response.warnings {
                    eprintln!("Warning: {warning}");
                }
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&response)?);
                return Ok(());
            }

            if files {
                for r in response.results {
                    println!(
                        "{:.4}	{}	{}	{}",
                        r.score,
                        r.frame_id,
                        r.uri,
                        r.title.unwrap_or_default()
                    );
                }
                return Ok(());
            }

            if response.results.is_empty() {
                println!("No results found.");
                return Ok(());
            }

            for r in response.results {
                let title = r.title.clone().unwrap_or_default();
                println!("{:>2}. {:>6.3}  {}  {}", r.rank, r.score, r.uri, title);
                if !r.snippet.trim().is_empty() {
                    println!("    {}", r.snippet.replace('\n', " "));
                }
            }

            Ok(())
        }

        Command::Context {
            mv2,
            query,
            collection,
            limit,
            snippet_chars,
            max_bytes,
            full,
            no_expand,
            max_expansions,
            expand_hook,
            expand_hook_timeout_ms,
            no_vector,
            rerank,
            rerank_hook,
            rerank_hook_timeout_ms,
            rerank_hook_full_text,
            embed_model,
            embed_cache,
            embed_no_cache,
            plan,
            asof,
            before,
            after,
            feedback_weight,
        } => {
            let mut mem = Vault::open_read_only(&mv2)?;
            let args = QueryArgs {
                raw_query: query.clone(),
                collection,
                limit,
                snippet_chars,
                no_expand,
                max_expansions,
                expand_hook,
                expand_hook_timeout_ms,
                no_vector,
                rerank,
                rerank_hook,
                rerank_hook_timeout_ms,
                rerank_hook_full_text,
                embed_model,
                embed_cache,
                embed_no_cache,
                rerank_docs: limit.max(20),
                rerank_chunk_chars: 1200,
                rerank_chunk_overlap: 200,
                plan,
                asof,
                before,
                after,
                feedback_weight,
            };

            let pack = build_context_pack(&mut mem, args, max_bytes, full)?;
            if !pack.warnings.is_empty() {
                for warning in &pack.warnings {
                    eprintln!("Warning: {warning}");
                }
            }
            println!("{}", serde_json::to_string_pretty(&pack)?);
            Ok(())
        }

        Command::Log {
            mv2,
            session,
            role,
            text,
            file,
            meta,
        } => {
            let payload_text = if let Some(path) = file {
                fs::read_to_string(path)?
            } else if let Some(text) = text {
                text
            } else {
                return Err("log requires --text or --file".into());
            };

            let meta_value = if let Some(meta) = meta {
                Some(serde_json::from_str(&meta)?)
            } else {
                None
            };

            let entry = AgentLogEntry {
                session: session.clone(),
                role: role.clone(),
                text: payload_text.clone(),
                meta: meta_value,
                ts_utc: Some(Utc::now().timestamp()),
            };
            let mut mem = Vault::open(&mv2)?;
            let _ = append_agent_log(&mut mem, &entry)?;
            println!("Logged agent turn.");
            Ok(())
        }

        Command::Feedback {
            mv2,
            uri,
            score,
            note,
            session,
        } => {
            let score = score.clamp(-1.0, 1.0);
            let event = FeedbackEvent {
                uri: uri.clone(),
                score,
                note: note.clone(),
                session: session.clone(),
                ts_utc: Some(Utc::now().timestamp()),
            };
            let mut mem = Vault::open(&mv2)?;
            let _ = append_feedback(&mut mem, &event)?;
            println!("Feedback recorded.");
            Ok(())
        }

        Command::Embed {

            mv2,
            collection,
            limit,
            batch,
            force,
            model,
            embed_cache,
            embed_no_cache,
            dry_run,
            json,
        } => {
            #[cfg(feature = "vec")]
            {
                let mut mem = Vault::open(&mv2)?;
                mem.enable_vec()?;

                let embed_config =
                    build_embed_config(model.as_deref(), embed_cache, !embed_no_cache);
                let embedder = LocalTextEmbedder::new(embed_config)?;
                mem.set_vec_model(embedder.model())?;

                let scope = collection.as_deref().map(scope_prefix);
                let mut frame_ids = collect_active_frame_ids(&mem, scope.as_deref());
                if limit > 0 && frame_ids.len() > limit {
                    frame_ids.truncate(limit);
                }

                let batch_size = batch.max(1);
                let mut embedded = 0usize;
                let mut skipped = 0usize;
                let mut failed = 0usize;

                for chunk in frame_ids.chunks(batch_size) {
                    let mut targets: Vec<(u64, String)> = Vec::new();
                    for &frame_id in chunk {
                        if !force {
                            match mem.frame_embedding(frame_id) {
                                Ok(Some(_)) => {
                                    skipped += 1;
                                    continue;
                                }
                                Ok(None) => {}
                                Err(_) => {}
                            }
                        }

                        let frame = match mem.frame_by_id(frame_id) {
                            Ok(f) => f,
                            Err(_) => {
                                failed += 1;
                                continue;
                            }
                        };
                        let text = if let Some(search) = frame.search_text.clone() {
                            search
                        } else {
                            match mem.frame_text_by_id(frame_id) {
                                Ok(t) => t,
                                Err(_) => {
                                    failed += 1;
                                    continue;
                                }
                            }
                        };

                        if text.trim().is_empty() {
                            skipped += 1;
                            continue;
                        }
                        targets.push((frame_id, text));
                    }

                    if targets.is_empty() {
                        continue;
                    }

                    let refs: Vec<&str> = targets.iter().map(|(_, t)| t.as_str()).collect();
                    let embeddings = match embedder.embed_batch(&refs) {
                        Ok(e) => e,
                        Err(err) => {
                            eprintln!("Embedding batch failed: {err}");
                            failed += targets.len();
                            continue;
                        }
                    };

                    for ((frame_id, _), embedding) in
                        targets.into_iter().zip(embeddings.into_iter())
                    {
                        if dry_run {
                            embedded += 1;
                            continue;
                        }
                        let mut options = PutOptions::default();
                        options.auto_tag = false;
                        options.extract_dates = false;
                        options.extract_triplets = false;
                        options.instant_index = false;
                        options.enable_embedding = false;
                        if mem.update_frame(frame_id, None, options, Some(embedding)).is_ok() {
                            embedded += 1;
                        } else {
                            failed += 1;
                        }
                    }
                }

                if !dry_run {
                    mem.commit()?;
                }

                if json {
                    #[derive(Serialize)]
                    struct EmbedSummary {
                        total: usize,
                        embedded: usize,
                        skipped: usize,
                        failed: usize,
                        dry_run: bool,
                    }

                    let summary = EmbedSummary {
                        total: frame_ids.len(),
                        embedded,
                        skipped,
                        failed,
                        dry_run,
                    };
                    println!("{}", serde_json::to_string_pretty(&summary)?);
                } else {
                    println!(
                        "Embedding complete: total={} embedded={} skipped={} failed={} dry_run={}",
                        frame_ids.len(),
                        embedded,
                        skipped,
                        failed,
                        dry_run
                    );
                }
                Ok(())
            }
            #[cfg(not(feature = "vec"))]
            {
                let _ = (
                    mv2,
                    collection,
                    limit,
                    batch,
                    force,
                    model,
                    embed_cache,
                    embed_no_cache,
                    dry_run,
                    json,
                );
                eprintln!("Embed requires --features vec");
                std::process::exit(2);
            }
        }

        Command::Get { mv2, id, json } => {
            let mut mem = Vault::open_read_only(&mv2)?;

            let (frame_id, frame) = if let Some(rest) = id.strip_prefix('#') {
                let frame_id: u64 = rest.parse().map_err(|_| VaultError::InvalidQuery {
                    reason: "invalid frame id (expected #123)".into(),
                })?;
                let frame = mem.frame_by_id(frame_id)?;
                (frame_id, frame)
            } else {
                let frame = mem.frame_by_uri(&id)?;
                (frame.id, frame)
            };

            let text = mem.frame_text_by_id(frame_id)?;

            if json {
                let payload = GetResponse {
                    frame_id,
                    uri: frame.uri.clone(),
                    title: frame.title.clone(),
                    text,
                };
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }

            println!("{text}");
            Ok(())
        }

        Command::Status { mv2, json } => {
            let mem = Vault::open_read_only(&mv2)?;
            let payload = StatusResponse {
                mv2: mv2.display().to_string(),
                frame_count: mem.frame_count(),
                next_frame_id: mem.next_frame_id(),
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("mv2: {}", payload.mv2);
                println!("frames: {}", payload.frame_count);
                println!("next_frame_id: {}", payload.next_frame_id);
            }

            Ok(())
        }

        Command::Config { mv2, command } => match command {
            ConfigCommand::Set {
                key,
                file,
                json,
                pretty,
            } => {
                let bytes = if let Some(path) = file {
                    fs::read(path)?
                } else if let Some(json) = json {
                    json.into_bytes()
                } else {
                    return Err("config set requires --file or --json".into());
                };
                let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                let payload = if pretty {
                    serde_json::to_vec_pretty(&value)?
                } else {
                    serde_json::to_vec(&value)?
                };
                let mut mem = open_or_create(&mv2)?;
                let frame_id = save_config_entry(&mut mem, &key, &payload)?;
                println!("Stored config {key} at frame #{frame_id}");
                Ok(())
            }
            ConfigCommand::Get { key, raw } => {
                let mut mem = Vault::open_read_only(&mv2)?;
                let Some(bytes) = load_config_entry(&mut mem, &key) else {
                    return Err("config not found".into());
                };
                if raw {
                    io::stdout().write_all(&bytes)?;
                } else {
                    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                    println!("{}", serde_json::to_string_pretty(&value)?);
                }
                Ok(())
            }
            ConfigCommand::List { json } => {
                let mut mem = Vault::open_read_only(&mv2)?;
                let entries = list_config_entries(&mut mem);
                if json {
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                } else {
                    for entry in entries {
                        println!("{}\t{}\t{}", entry.key, entry.frame_id, entry.timestamp);
                    }
                }
                Ok(())
            }
        },

        Command::Diff {
            left,
            right,
            all,
            limit,
            json,
        } => {
            let mut left_mem = Vault::open_read_only(&left)?;
            let mut right_mem = Vault::open_read_only(&right)?;
            let left_map = collect_latest_frames(&mut left_mem, all);
            let right_map = collect_latest_frames(&mut right_mem, all);

            let mut only_left = Vec::new();
            let mut only_right = Vec::new();
            let mut changed = Vec::new();

            for (uri, left_summary) in &left_map {
                if let Some(right_summary) = right_map.get(uri) {
                    if left_summary.checksum != right_summary.checksum {
                        changed.push(DiffChange {
                            uri: uri.clone(),
                            left: left_summary.clone(),
                            right: right_summary.clone(),
                        });
                    }
                } else {
                    only_left.push(left_summary.clone());
                }
            }

            for (uri, right_summary) in &right_map {
                if !left_map.contains_key(uri) {
                    only_right.push(right_summary.clone());
                }
            }

            only_left.sort_by(|a, b| a.uri.cmp(&b.uri));
            only_right.sort_by(|a, b| a.uri.cmp(&b.uri));
            changed.sort_by(|a, b| a.uri.cmp(&b.uri));

            if limit > 0 {
                only_left.truncate(limit);
                only_right.truncate(limit);
                changed.truncate(limit);
            }

            let report = DiffReport {
                left: left.display().to_string(),
                right: right.display().to_string(),
                only_left,
                only_right,
                changed,
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("left: {}", report.left);
                println!("right: {}", report.right);
                println!("only_left: {}", report.only_left.len());
                println!("only_right: {}", report.only_right.len());
                println!("changed: {}", report.changed.len());
            }
            Ok(())
        }

        Command::Merge {
            left,
            right,
            out,
            force,
            no_dedup,
            json,
        } => {
            if out.exists() {
                if force {
                    fs::remove_file(&out)?;
                } else {
                    return Err("output file exists (use --force to overwrite)".into());
                }
            }
            let mut out_mem = Vault::create(&out)?;
            let mut dedup_map: HashMap<String, u64> = HashMap::new();
            let mut written = 0usize;
            let mut deduped = 0usize;

            let (w1, d1) = merge_capsule_into(&mut out_mem, &left, !no_dedup, &mut dedup_map)?;
            written += w1;
            deduped += d1;
            let (w2, d2) = merge_capsule_into(&mut out_mem, &right, !no_dedup, &mut dedup_map)?;
            written += w2;
            deduped += d2;
            out_mem.commit()?;

            let report = MergeReport {
                left: left.display().to_string(),
                right: right.display().to_string(),
                out: out.display().to_string(),
                written,
                deduped,
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "merged {} + {} -> {} (written={}, deduped={})",
                    report.left, report.right, report.out, report.written, report.deduped
                );
            }
            Ok(())
        }

        Command::Mcp { mv2, read_only } => run_mcp_server(mv2, read_only),

        Command::Agent {
            mv2,
            prompt,
            file,
            session,
            model_hook,
            system,
            system_file,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log_commit_interval,
            json,
            log,
        } => run_agent(
            mv2,
            prompt,
            file,
            session,
            model_hook,
            system,
            system_file,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log_commit_interval,
            json,
            log,
        ),

        Command::Hook { provider } => match provider {
            HookCommand::Claude => run_claude_hook(),
        },

        Command::Doctor {
            mv2,
            vacuum,
            rebuild_time,
            rebuild_lex,
            rebuild_vec,
            dry_run,
            quiet,
            json,
        } => {
            let options = DoctorOptions {
                rebuild_time_index: rebuild_time,
                rebuild_lex_index: rebuild_lex,
                rebuild_vec_index: rebuild_vec,
                vacuum,
                dry_run,
                quiet,
            };
            let report = Vault::doctor(&mv2, options)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_doctor_report(&report);
            }
            Ok(())
        }

        Command::Compact {
            mv2,
            dry_run,
            quiet,
            json,
        } => {
            let options = DoctorOptions {
                rebuild_time_index: true,
                rebuild_lex_index: true,
                rebuild_vec_index: cfg!(feature = "vec"),
                vacuum: true,
                dry_run,
                quiet,
            };
            let report = Vault::doctor(&mv2, options)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_doctor_report(&report);
            }
            Ok(())
        }
    }
}

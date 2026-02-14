use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aether_core::types::{Frame, FrameStatus, SearchHit, SearchRequest, TemporalFilter};
use aether_core::{DoctorOptions, DoctorReport, PutOptions, Vault, VaultError};
use base64::Engine;
use blake3::Hash;
use chrono::{Datelike, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use tiny_http::{Header, Method, Response, Server};
use url::form_urlencoded;
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
    Init { mv2: PathBuf },

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
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
    },

    /// Built-in model hooks (stdio JSON).
    Hook {
        #[command(subcommand)]
        provider: HookCommand,
    },

    /// Bootstrap local workspace (soul + memory) and write default config.
    Bootstrap {
        mv2: PathBuf,
        /// Workspace folder (default: ./assistant or AETHERVAULT_WORKSPACE)
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Timezone offset (e.g. -05:00)
        #[arg(long)]
        timezone: Option<String>,
        /// Overwrite existing workspace files
        #[arg(long)]
        force: bool,
    },

    /// Run autonomous schedules (daily/weekly briefings).
    Schedule {
        mv2: PathBuf,
        /// Workspace folder (default: ./assistant or AETHERVAULT_WORKSPACE)
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Timezone offset (e.g. -05:00)
        #[arg(long)]
        timezone: Option<String>,
        /// Telegram bot token (env: TELEGRAM_BOT_TOKEN)
        #[arg(long)]
        telegram_token: Option<String>,
        /// Telegram chat id (env: AETHERVAULT_TELEGRAM_CHAT_ID)
        #[arg(long)]
        telegram_chat_id: Option<String>,
        /// Override model hook command
        #[arg(long)]
        model_hook: Option<String>,
        /// Max tool/LLM steps
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        /// Log turns to capsule
        #[arg(long)]
        log: bool,
        /// Commit agent logs every N entries (1 = fsync each log)
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
    },

    /// Run event-driven triggers (email/calendar).
    Watch {
        mv2: PathBuf,
        /// Workspace folder (default: ./assistant or AETHERVAULT_WORKSPACE)
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Timezone offset (e.g. -05:00)
        #[arg(long)]
        timezone: Option<String>,
        /// Override model hook command
        #[arg(long)]
        model_hook: Option<String>,
        /// Max tool/LLM steps
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        /// Log turns to capsule
        #[arg(long)]
        log: bool,
        /// Commit agent logs every N entries (1 = fsync each log)
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
        /// Poll interval in seconds
        #[arg(long, default_value_t = 60)]
        poll_seconds: u64,
    },

    /// OAuth broker for Google/Microsoft connectors.
    Connect {
        mv2: PathBuf,
        /// Provider: google | microsoft
        provider: String,
        /// Bind address
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        /// Bind port
        #[arg(long, default_value_t = 8787)]
        port: u16,
        /// Redirect base URL (defaults to http://<bind>:<port>)
        #[arg(long)]
        redirect_base: Option<String>,
    },

    /// Approve a pending tool execution (human-in-the-loop).
    Approve {
        mv2: PathBuf,
        id: String,
        /// Execute the approved tool immediately.
        #[arg(long)]
        execute: bool,
    },

    /// Reject a pending tool execution.
    Reject { mv2: PathBuf, id: String },

    /// Rust-native chat connectors (Telegram + WhatsApp).
    Bridge {
        #[command(subcommand)]
        command: BridgeCommand,
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
enum BridgeCommand {
    /// Telegram long-polling bridge.
    Telegram {
        /// Capsule path (defaults to AETHERVAULT_MV2 or ./data/knowledge.mv2)
        #[arg(long)]
        mv2: Option<PathBuf>,
        /// Telegram bot token (env: TELEGRAM_BOT_TOKEN)
        #[arg(long)]
        token: Option<String>,
        /// Long-poll timeout in seconds
        #[arg(long, default_value_t = 25)]
        poll_timeout: u64,
        /// Max updates per poll
        #[arg(long, default_value_t = 50)]
        poll_limit: usize,
        /// Override model hook command
        #[arg(long)]
        model_hook: Option<String>,
        /// Override system prompt
        #[arg(long)]
        system: Option<String>,
        /// Disable memory context
        #[arg(long)]
        no_memory: bool,
        /// Override memory query
        #[arg(long)]
        context_query: Option<String>,
        /// Max results for memory context
        #[arg(long, default_value_t = 8)]
        context_results: usize,
        /// Max bytes for memory context
        #[arg(long, default_value_t = 12_000)]
        context_max_bytes: usize,
        /// Max tool/LLM steps
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        /// Log turns to capsule
        #[arg(long)]
        log: bool,
        /// Commit agent logs every N entries (1 = fsync each log)
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
    },
    /// WhatsApp (Twilio) webhook bridge.
    Whatsapp {
        /// Capsule path (defaults to AETHERVAULT_MV2 or ./data/knowledge.mv2)
        #[arg(long)]
        mv2: Option<PathBuf>,
        /// Bind address
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        /// Bind port
        #[arg(long, default_value_t = 8080)]
        port: u16,
        /// Override model hook command
        #[arg(long)]
        model_hook: Option<String>,
        /// Override system prompt
        #[arg(long)]
        system: Option<String>,
        /// Disable memory context
        #[arg(long)]
        no_memory: bool,
        /// Override memory query
        #[arg(long)]
        context_query: Option<String>,
        /// Max results for memory context
        #[arg(long, default_value_t = 8)]
        context_results: usize,
        /// Max bytes for memory context
        #[arg(long, default_value_t = 12_000)]
        context_max_bytes: usize,
        /// Max tool/LLM steps
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        /// Log turns to capsule
        #[arg(long)]
        log: bool,
        /// Commit agent logs every N entries (1 = fsync each log)
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
    },
    /// Slack events bridge (webhook receiver).
    Slack {
        #[arg(long)]
        mv2: Option<PathBuf>,
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        #[arg(long, default_value_t = 8081)]
        port: u16,
        #[arg(long)]
        model_hook: Option<String>,
        #[arg(long)]
        system: Option<String>,
        #[arg(long)]
        no_memory: bool,
        #[arg(long)]
        context_query: Option<String>,
        #[arg(long, default_value_t = 8)]
        context_results: usize,
        #[arg(long, default_value_t = 12_000)]
        context_max_bytes: usize,
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        #[arg(long)]
        log: bool,
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
    },
    /// Discord bridge (webhook receiver).
    Discord {
        #[arg(long)]
        mv2: Option<PathBuf>,
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        #[arg(long, default_value_t = 8082)]
        port: u16,
        #[arg(long)]
        model_hook: Option<String>,
        #[arg(long)]
        system: Option<String>,
        #[arg(long)]
        no_memory: bool,
        #[arg(long)]
        context_query: Option<String>,
        #[arg(long, default_value_t = 8)]
        context_results: usize,
        #[arg(long, default_value_t = 12_000)]
        context_max_bytes: usize,
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        #[arg(long)]
        log: bool,
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
    },
    /// Teams bridge (webhook receiver).
    Teams {
        #[arg(long)]
        mv2: Option<PathBuf>,
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        #[arg(long, default_value_t = 8083)]
        port: u16,
        #[arg(long)]
        model_hook: Option<String>,
        #[arg(long)]
        system: Option<String>,
        #[arg(long)]
        no_memory: bool,
        #[arg(long)]
        context_query: Option<String>,
        #[arg(long, default_value_t = 8)]
        context_results: usize,
        #[arg(long, default_value_t = 12_000)]
        context_max_bytes: usize,
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        #[arg(long)]
        log: bool,
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
    },
    /// Signal bridge (requires signal-cli).
    Signal {
        #[arg(long)]
        mv2: Option<PathBuf>,
        #[arg(long)]
        sender: Option<String>,
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        #[arg(long, default_value_t = 8084)]
        port: u16,
        #[arg(long)]
        model_hook: Option<String>,
        #[arg(long)]
        system: Option<String>,
        #[arg(long)]
        no_memory: bool,
        #[arg(long)]
        context_query: Option<String>,
        #[arg(long, default_value_t = 8)]
        context_results: usize,
        #[arg(long, default_value_t = 12_000)]
        context_max_bytes: usize,
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        #[arg(long)]
        log: bool,
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
    },
    /// Matrix bridge (webhook receiver).
    Matrix {
        #[arg(long)]
        mv2: Option<PathBuf>,
        #[arg(long)]
        room: Option<String>,
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        #[arg(long, default_value_t = 8085)]
        port: u16,
        #[arg(long)]
        model_hook: Option<String>,
        #[arg(long)]
        system: Option<String>,
        #[arg(long)]
        no_memory: bool,
        #[arg(long)]
        context_query: Option<String>,
        #[arg(long, default_value_t = 8)]
        context_results: usize,
        #[arg(long, default_value_t = 12_000)]
        context_max_bytes: usize,
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        #[arg(long)]
        log: bool,
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
    },
    /// iMessage bridge (macOS only).
    IMessage {
        #[arg(long)]
        mv2: Option<PathBuf>,
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        #[arg(long, default_value_t = 8086)]
        port: u16,
        #[arg(long)]
        model_hook: Option<String>,
        #[arg(long)]
        system: Option<String>,
        #[arg(long)]
        no_memory: bool,
        #[arg(long)]
        context_query: Option<String>,
        #[arg(long, default_value_t = 8)]
        context_results: usize,
        #[arg(long, default_value_t = 12_000)]
        context_max_bytes: usize,
        #[arg(long, default_value_t = 64)]
        max_steps: usize,
        #[arg(long)]
        log: bool,
        #[arg(long, default_value_t = 1)]
        log_commit_interval: usize,
    },
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
    workspace: Option<String>,
    #[serde(default)]
    onboarding_complete: Option<bool>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    telegram_token: Option<String>,
    #[serde(default)]
    telegram_chat_id: Option<String>,
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
    #[serde(default)]
    subagents: Vec<SubagentSpec>,
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

struct AgentRunOutput {
    session: Option<String>,
    context: Option<ContextPack>,
    messages: Vec<AgentMessage>,
    tool_results: Vec<AgentToolResult>,
    final_text: Option<String>,
}

struct AgentProgress {
    step: usize,
    max_steps: usize,
    phase: String,
    text_preview: Option<String>,
    started_at: std::time::Instant,
}

struct CompletionEvent {
    chat_id: i64,
    reply_to_id: Option<i64>,
    result: Result<AgentRunOutput, String>,
}

struct ActiveRun {
    progress: Arc<Mutex<AgentProgress>>,
    queued_messages: Vec<(String, Option<i64>)>,
}

#[derive(Clone)]
struct BridgeAgentConfig {
    mv2: PathBuf,
    model_hook: Option<String>,
    system: Option<String>,
    no_memory: bool,
    context_query: Option<String>,
    context_results: usize,
    context_max_bytes: usize,
    max_steps: usize,
    log: bool,
    log_commit_interval: usize,
    session_prefix: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct SubagentSpec {
    name: String,
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    model_hook: Option<String>,
}

#[derive(Debug)]
struct ToolExecution {
    output: String,
    details: serde_json::Value,
    is_error: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ApprovalEntry {
    id: String,
    tool: String,
    args_hash: String,
    args: serde_json::Value,
    status: String,
    created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct TriggerEntry {
    id: String,
    kind: String,
    name: Option<String>,
    query: Option<String>,
    prompt: Option<String>,
    start: Option<String>,
    end: Option<String>,
    enabled: bool,
    last_seen: Option<String>,
    last_fired: Option<String>,
}

// === Session Context Buffer ===

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionTurn {
    role: String,
    content: String,
    timestamp: i64,
}

fn session_file_path(session_id: &str) -> PathBuf {
    let safe_id = session_id.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    PathBuf::from("/root/.aethervault/workspace/sessions").join(format!("{safe_id}.json"))
}

fn load_session_turns(session_id: &str, max_turns: usize) -> Vec<SessionTurn> {
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

fn save_session_turns(session_id: &str, turns: &[SessionTurn], max_turns: usize) {
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

const TOOL_DETAILS_MAX_CHARS: usize = 4_000;
const TOOL_OUTPUT_MAX_FOR_DETAILS: usize = 2_000;
const DEFAULT_WORKSPACE_DIR: &str = "./assistant";

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
    // Open fresh each time — don't hold a shared lock between tool calls.
    // This allows concurrent subagents to acquire exclusive locks for writes.
    let mut mem = Vault::open_read_only(mv2).map_err(|e| e.to_string())?;
    let result = f(&mut mem);
    // `mem` is dropped here, releasing the shared lock immediately.
    *mem_read = None;
    result
}

fn with_write_mem<F, R>(
    mem_read: &mut Option<Vault>,
    mem_write: &mut Option<Vault>,
    mv2: &Path,
    allow_create: bool,
    f: F,
) -> Result<R, String>
where
    F: FnOnce(&mut Vault) -> Result<R, String>,
{
    // Always open fresh — don't reuse a stale handle that holds a lock.
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
struct ToolMemoryAppendArgs {
    text: String,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolMemoryRememberArgs {
    text: String,
}

#[derive(Debug, Deserialize)]
struct ToolEmailListArgs {
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolEmailReadArgs {
    id: String,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    folder: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolEmailSendArgs {
    to: String,
    #[serde(default)]
    cc: Option<String>,
    #[serde(default)]
    bcc: Option<String>,
    subject: String,
    body: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    in_reply_to: Option<String>,
    #[serde(default)]
    references: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolEmailArchiveArgs {
    id: String,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    folder: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolConfigSetArgs {
    key: String,
    json: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ToolMemorySyncArgs {
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    include_daily: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ToolMemoryExportArgs {
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    include_daily: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ToolMemorySearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolExecArgs {
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ToolNotifyArgs {
    #[serde(default)]
    channel: Option<String>,
    text: String,
    #[serde(default)]
    webhook: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolSignalSendArgs {
    to: String,
    text: String,
    #[serde(default)]
    sender: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolIMessageSendArgs {
    to: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct ToolGmailListArgs {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolGmailReadArgs {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ToolGmailSendArgs {
    to: String,
    subject: String,
    body: String,
}

#[derive(Debug, Deserialize)]
struct ToolGCalListArgs {
    #[serde(default)]
    max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolGCalCreateArgs {
    summary: String,
    start: String,
    end: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolMsMailListArgs {
    #[serde(default)]
    top: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolMsMailReadArgs {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ToolMsCalendarListArgs {
    #[serde(default)]
    top: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolMsCalendarCreateArgs {
    subject: String,
    start: String,
    end: String,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolHttpRequestArgs {
    #[serde(default)]
    method: Option<String>,
    url: String,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    json: Option<bool>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ToolBrowserRequestArgs {
    action: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ToolFsListArgs {
    path: String,
    #[serde(default)]
    recursive: Option<bool>,
    #[serde(default)]
    max_entries: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolFsReadArgs {
    path: String,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolFsWriteArgs {
    path: String,
    text: String,
    #[serde(default)]
    append: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ToolTriggerAddArgs {
    kind: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ToolTriggerRemoveArgs {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ToolToolSearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolSessionContextArgs {
    session: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolReflectArgs {
    text: String,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolSkillStoreArgs {
    name: String,
    #[serde(default)]
    trigger: Option<String>,
    #[serde(default)]
    steps: Option<Vec<String>>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolSkillSearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ToolSubagentInvokeArgs {
    name: String,
    prompt: String,
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    model_hook: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolSubagentBatchArgs {
    /// Array of subagent invocations to run concurrently.
    invocations: Vec<ToolSubagentInvokeArgs>,
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

#[derive(Debug, Deserialize)]
struct ToolScaleArgs {
    action: String,
    #[serde(default)]
    size: Option<String>,
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
    format!("aethervault://{}/", normalize_collection(collection))
}

fn uri_for_path(collection: &str, relative: &Path) -> String {
    let rel = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    format!("aethervault://{}/{rel}", normalize_collection(collection))
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
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "but"
            | "by"
            | "for"
            | "from"
            | "has"
            | "have"
            | "if"
            | "in"
            | "into"
            | "is"
            | "it"
            | "its"
            | "of"
            | "on"
            | "or"
            | "that"
            | "the"
            | "their"
            | "then"
            | "there"
            | "these"
            | "they"
            | "this"
            | "to"
            | "was"
            | "were"
            | "with"
            | "you"
            | "your"
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

    let reduced_tokens: Vec<String> = tokens.iter().filter(|t| !is_stopword(t)).cloned().collect();
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
    if key.is_empty() { None } else { Some(key) }
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

fn save_config_entry(
    mem: &mut Vault,
    key: &str,
    bytes: &[u8],
) -> Result<u64, Box<dyn std::error::Error>> {
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
    let mut cmd = build_external_command(&command[0], &command[1..]);
    cmd.stdin(Stdio::piped())
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

fn run_rerank_hook(hook: &HookSpec, input: &RerankHookInput) -> Result<RerankHookOutput, String> {
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

fn collect_latest_frames(mem: &mut Vault, include_inactive: bool) -> HashMap<String, FrameSummary> {
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
    let s1 = hits.first().and_then(|h| h.score).unwrap_or(0.0);
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

fn build_ranked_list(lane: LaneKind, query: &str, is_base: bool, hits: &[SearchHit]) -> RankedList {
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
    let phrase_bonus = if chunk_lower.contains(&query_lower) {
        0.2
    } else {
        0.0
    };
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
            let prefix = if i == plan.vec_queries.len() - 1 {
                "└─"
            } else {
                "├─"
            };
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

fn execute_query(
    mem: &mut Vault,
    args: QueryArgs,
) -> Result<QueryResponse, Box<dyn std::error::Error>> {
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
        if args.rerank_hook_full_text {
            Some(true)
        } else {
            None
        },
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
        if let Some((_, Some(override_snippet))) = rerank_scores.get(&cand.key) {
            if !override_snippet.trim().is_empty() {
                snippet = override_snippet.clone();
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
            mem.frame_text_by_id(r.frame_id)
                .unwrap_or_else(|_| r.snippet.clone())
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

fn append_agent_log(
    mem: &mut Vault,
    entry: &AgentLogEntry,
) -> Result<String, Box<dyn std::error::Error>> {
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
    let uri = format!(
        "aethervault://agent-log/{session_slug}/{ts}-{}",
        hash.to_hex()
    );

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

fn append_feedback(
    mem: &mut Vault,
    event: &FeedbackEvent,
) -> Result<String, Box<dyn std::error::Error>> {
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
        let key = format!(
            "{}|{}|{}",
            uri,
            checksum_hex(&frame.checksum),
            frame.timestamp
        );
        if dedup {
            if let Some(existing) = dedup_map.get(&key).copied() {
                id_map.insert(frame_id, existing);
                deduped += 1;
                continue;
            }
        }

        let payload = match src.frame_canonical_payload(frame_id) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("merge: skipping corrupt frame {} ({})", frame_id, e);
                deduped += 1;  // count as skipped
                continue;
            }
        };
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
    let is_write = matches!(
        name,
        "put"
            | "log"
            | "feedback"
            | "config_set"
            | "memory_append_daily"
            | "memory_remember"
            | "trigger_add"
            | "trigger_remove"
            | "reflect"
            | "skill_store"
    );
    if read_only && is_write {
        return Err("tool disabled in read-only mode".into());
    }
    let workspace_override = env_optional("AETHERVAULT_WORKSPACE").map(PathBuf::from);
    if requires_approval(name, &args) {
        if read_only {
            return Err("approval required but tool disabled in read-only mode".into());
        }
        let args_hash = approval_hash(name, &args);
        let mut approval_id: Option<String> = None;
        let mut approved = false;
        with_write_mem(mem_read, mem_write, mv2, true, |mem| {
            let mut approvals = load_approvals(mem);
            if let Some(pos) = approvals
                .iter()
                .position(|e| e.tool == name && e.args_hash == args_hash && e.status == "approved")
            {
                approval_id = Some(approvals[pos].id.clone());
                approvals.remove(pos);
                save_approvals(mem, &approvals)?;
                approved = true;
                return Ok(());
            }
            if let Some(existing) = approvals
                .iter()
                .find(|e| e.tool == name && e.args_hash == args_hash && e.status == "pending")
            {
                approval_id = Some(existing.id.clone());
                return Ok(());
            }
            let now = chrono::Utc::now().to_rfc3339();
            let id = format!("apr_{}_{}", now.replace(':', ""), &args_hash[..8]);
            approvals.push(ApprovalEntry {
                id: id.clone(),
                tool: name.to_string(),
                args_hash: args_hash.clone(),
                args: args.clone(),
                status: "pending".to_string(),
                created_at: now,
            });
            save_approvals(mem, &approvals)?;
            approval_id = Some(id);
            Ok(())
        })?;
        if !approved {
            let id = approval_id.clone().unwrap_or_else(|| "unknown".to_string());
            return Ok(ToolExecution {
                output: format!("approval required: {id}\nReply `approve {id}` or `reject {id}`."),
                details: serde_json::json!({
                    "approval_id": approval_id,
                    "tool": name,
                    "args": args
                }),
                is_error: true,
            });
        }
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
            let result = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
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
            let result = with_write_mem(mem_read, mem_write, mv2, false, |mem| {
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
            let result = with_write_mem(mem_read, mem_write, mv2, false, |mem| {
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
        "config_set" => {
            let parsed: ToolConfigSetArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let payload = serde_json::to_vec(&parsed.json).map_err(|e| format!("json: {e}"))?;
            let result = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let id =
                    save_config_entry(mem, &parsed.key, &payload).map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output: format!("Config saved ({})", parsed.key),
                    details: serde_json::json!({ "frame_id": id }),
                    is_error: false,
                })
            })?;
            *mem_read = None;
            Ok(result)
        }
        "memory_sync" => {
            let parsed: ToolMemorySyncArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = parsed
                .workspace
                .map(PathBuf::from)
                .or_else(|| workspace_override.clone())
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let include_daily = parsed.include_daily.unwrap_or(true);
            let ids =
                sync_workspace_memory(mv2, &workspace, include_daily).map_err(|e| e.to_string())?;
            *mem_read = None;
            Ok(ToolExecution {
                output: format!("Synced {} memory files.", ids.len()),
                details: serde_json::json!({ "frame_ids": ids }),
                is_error: false,
            })
        }
        "memory_export" => {
            let parsed: ToolMemoryExportArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = parsed
                .workspace
                .map(PathBuf::from)
                .or_else(|| workspace_override.clone())
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let include_daily = parsed.include_daily.unwrap_or(true);
            let paths =
                export_capsule_memory(mv2, &workspace, include_daily).map_err(|e| e.to_string())?;
            Ok(ToolExecution {
                output: format!("Exported {} files.", paths.len()),
                details: serde_json::json!({ "paths": paths }),
                is_error: false,
            })
        }
        "memory_search" => {
            let parsed: ToolMemorySearchArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let request = SearchRequest {
                    query: parsed.query.clone(),
                    top_k: parsed.limit.unwrap_or(10),
                    snippet_chars: 300,
                    uri: None,
                    scope: Some("aethervault://memory/".to_string()),
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
        "memory_append_daily" => {
            let parsed: ToolMemoryAppendArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            let date = parsed
                .date
                .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
            let dir = workspace.join("memory");
            fs::create_dir_all(&dir).map_err(|e| format!("workspace: {e}"))?;
            let path = dir.join(format!("{date}.md"));
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| format!("memory open: {e}"))?;
            writeln!(file, "{}", parsed.text).map_err(|e| format!("memory write: {e}"))?;
            let uri = format!("aethervault://memory/daily/{date}.md");
            let result = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut options = PutOptions::default();
                options.uri = Some(uri.clone());
                options.title = Some(format!("memory daily {date}"));
                options.kind = Some("text/markdown".to_string());
                options.track = Some("aethervault.memory".to_string());
                options.search_text = Some(parsed.text.clone());
                let frame_id = mem
                    .put_bytes_with_options(parsed.text.as_bytes(), options)
                    .map_err(|e| e.to_string())?;
                mem.commit().map_err(|e| e.to_string())?;
                Ok(frame_id)
            })?;
            *mem_read = None;
            Ok(ToolExecution {
                output: format!("Appended to {}", path.display()),
                details: serde_json::json!({
                    "path": path.display().to_string(),
                    "uri": uri,
                    "frame_id": result
                }),
                is_error: false,
            })
        }
        "memory_remember" => {
            let parsed: ToolMemoryRememberArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let workspace = workspace_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            fs::create_dir_all(&workspace).map_err(|e| format!("workspace: {e}"))?;
            let path = workspace.join("MEMORY.md");
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| format!("memory open: {e}"))?;
            writeln!(file, "{}", parsed.text).map_err(|e| format!("memory write: {e}"))?;
            let uri = "aethervault://memory/longterm.md".to_string();
            let result = with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut options = PutOptions::default();
                options.uri = Some(uri.clone());
                options.title = Some("memory longterm".to_string());
                options.kind = Some("text/markdown".to_string());
                options.track = Some("aethervault.memory".to_string());
                options.search_text = Some(parsed.text.clone());
                let frame_id = mem
                    .put_bytes_with_options(parsed.text.as_bytes(), options)
                    .map_err(|e| e.to_string())?;
                mem.commit().map_err(|e| e.to_string())?;
                Ok(frame_id)
            })?;
            *mem_read = None;
            Ok(ToolExecution {
                output: format!("Appended to {}", path.display()),
                details: serde_json::json!({
                    "path": path.display().to_string(),
                    "uri": uri,
                    "frame_id": result
                }),
                is_error: false,
            })
        }
        "email_list" => {
            let parsed: ToolEmailListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let mut cmd = build_external_command("himalaya", &[]);
            cmd.arg("envelope").arg("list").arg("--output").arg("json");
            if let Some(limit) = parsed.limit {
                cmd.arg("--limit").arg(limit.to_string());
            }
            if let Some(folder) = parsed.folder {
                cmd.arg("--folder").arg(folder);
            }
            if let Some(account) = parsed.account {
                cmd.arg("--account").arg(account);
            }
            let output = cmd.output().map_err(|e| format!("himalaya: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("himalaya error: {stderr}"));
            }
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let details = serde_json::from_str(&stdout)
                .unwrap_or_else(|_| serde_json::json!({ "raw": stdout }));
            Ok(ToolExecution {
                output: "Listed envelopes.".to_string(),
                details,
                is_error: false,
            })
        }
        "email_read" => {
            let parsed: ToolEmailReadArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let mut cmd = build_external_command("himalaya", &[]);
            cmd.arg("message")
                .arg("read")
                .arg(parsed.id)
                .arg("--output")
                .arg("json");
            if let Some(folder) = parsed.folder {
                cmd.arg("--folder").arg(folder);
            }
            if let Some(account) = parsed.account {
                cmd.arg("--account").arg(account);
            }
            let output = cmd.output().map_err(|e| format!("himalaya: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("himalaya error: {stderr}"));
            }
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let details = serde_json::from_str(&stdout)
                .unwrap_or_else(|_| serde_json::json!({ "raw": stdout }));
            Ok(ToolExecution {
                output: "Read message.".to_string(),
                details,
                is_error: false,
            })
        }
        "email_send" => {
            let parsed: ToolEmailSendArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let mut template = String::new();
            if let Some(from) = parsed.from {
                template.push_str(&format!("From: {from}\n"));
            }
            template.push_str(&format!("To: {}\n", parsed.to));
            if let Some(cc) = parsed.cc {
                template.push_str(&format!("Cc: {cc}\n"));
            }
            if let Some(bcc) = parsed.bcc {
                template.push_str(&format!("Bcc: {bcc}\n"));
            }
            if let Some(in_reply_to) = parsed.in_reply_to {
                template.push_str(&format!("In-Reply-To: {in_reply_to}\n"));
            }
            if let Some(references) = parsed.references {
                template.push_str(&format!("References: {references}\n"));
            }
            template.push_str(&format!("Subject: {}\n", parsed.subject));
            template.push('\n');
            template.push_str(&parsed.body);
            template.push('\n');

            let mut cmd = build_external_command("himalaya", &[]);
            cmd.arg("template").arg("send");
            let mut child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("himalaya: {e}"))?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(template.as_bytes())
                    .map_err(|e| format!("send stdin: {e}"))?;
            }
            let output = child
                .wait_with_output()
                .map_err(|e| format!("send output: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("himalaya error: {stderr}"));
            }
            Ok(ToolExecution {
                output: "Sent email.".to_string(),
                details: serde_json::json!({ "status": "sent" }),
                is_error: false,
            })
        }
        "email_archive" => {
            let parsed: ToolEmailArchiveArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let mut cmd = build_external_command("himalaya", &[]);
            cmd.arg("message").arg("move").arg(parsed.id).arg("Archive");
            if let Some(folder) = parsed.folder {
                cmd.arg("--folder").arg(folder);
            }
            if let Some(account) = parsed.account {
                cmd.arg("--account").arg(account);
            }
            let output = cmd.output().map_err(|e| format!("himalaya: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("himalaya error: {stderr}"));
            }
            Ok(ToolExecution {
                output: "Archived email.".to_string(),
                details: serde_json::json!({ "status": "archived" }),
                is_error: false,
            })
        }
        "exec" => {
            let parsed: ToolExecArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let timeout_ms = parsed.timeout_ms.unwrap_or(60_000).max(1);
            let command = if cfg!(windows) {
                vec!["cmd".to_string(), "/C".to_string(), parsed.command]
            } else {
                vec!["sh".to_string(), "-c".to_string(), parsed.command]
            };
            let mut cmd = build_external_command(&command[0], &command[1..]);
            if let Some(cwd) = parsed.cwd {
                cmd.current_dir(cwd);
            }
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| format!("exec spawn: {e}"))?;
            let timeout = Duration::from_millis(timeout_ms);
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if start.elapsed() > timeout {
                            let _ = child.kill();
                            return Err(format!("exec timed out after {timeout_ms}ms"));
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => return Err(format!("exec wait failed: {err}")),
                }
            }
            let output = child
                .wait_with_output()
                .map_err(|e| format!("exec output: {e}"))?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let details = serde_json::json!({
                "status": output.status.code(),
                "stdout": stdout,
                "stderr": stderr
            });
            let output_text = if output.status.success() {
                "Command executed.".to_string()
            } else {
                "Command failed.".to_string()
            };
            Ok(ToolExecution {
                output: output_text,
                details,
                is_error: !output.status.success(),
            })
        }
        "notify" => {
            let parsed: ToolNotifyArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let channel = parsed
                .channel
                .unwrap_or_else(|| "slack".to_string())
                .to_ascii_lowercase();
            let webhook = parsed.webhook.or_else(|| match channel.as_str() {
                "discord" => env_optional("DISCORD_WEBHOOK_URL"),
                "teams" => env_optional("TEAMS_WEBHOOK_URL"),
                _ => env_optional("SLACK_WEBHOOK_URL"),
            });
            let Some(webhook) = webhook else {
                return Err("notify requires webhook url".into());
            };
            let payload = match channel.as_str() {
                "discord" => serde_json::json!({ "content": parsed.text }),
                "teams" => serde_json::json!({ "text": parsed.text }),
                _ => serde_json::json!({ "text": parsed.text }),
            };
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(20))
                .timeout_write(Duration::from_secs(10))
                .build();
            let response = agent
                .post(&webhook)
                .set("content-type", "application/json")
                .send_json(payload);
            match response {
                Ok(_) => Ok(ToolExecution {
                    output: "Notification sent.".to_string(),
                    details: serde_json::json!({ "channel": channel }),
                    is_error: false,
                }),
                Err(err) => Err(format!("notify error: {err}")),
            }
        }
        "signal_send" => {
            let parsed: ToolSignalSendArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let sender = parsed.sender.or_else(|| env_optional("SIGNAL_SENDER"));
            let Some(sender) = sender else {
                return Err("signal_send requires sender".into());
            };
            let mut cmd = build_external_command("signal-cli", &[]);
            cmd.arg("-u")
                .arg(sender)
                .arg("send")
                .arg("-m")
                .arg(parsed.text)
                .arg(parsed.to);
            let output = cmd.output().map_err(|e| format!("signal-cli: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("signal-cli error: {stderr}"));
            }
            Ok(ToolExecution {
                output: "Signal message sent.".to_string(),
                details: serde_json::json!({ "status": "sent" }),
                is_error: false,
            })
        }
        "imessage_send" => {
            let parsed: ToolIMessageSendArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            if !cfg!(target_os = "macos") {
                return Err("imessage_send requires macOS".into());
            }
            let script = format!(
                "tell application \"Messages\" to send \"{}\" to buddy \"{}\"",
                parsed.text.replace('"', "\\\""),
                parsed.to.replace('"', "\\\"")
            );
            let mut cmd = build_external_command("osascript", &[]);
            cmd.arg("-e").arg(script);
            let output = cmd.output().map_err(|e| format!("osascript: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("osascript error: {stderr}"));
            }
            Ok(ToolExecution {
                output: "iMessage sent.".to_string(),
                details: serde_json::json!({ "status": "sent" }),
                is_error: false,
            })
        }
        "http_request" => {
            let parsed: ToolHttpRequestArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let method = parsed
                .method
                .unwrap_or_else(|| "GET".to_string())
                .to_ascii_uppercase();
            let timeout = parsed.timeout_ms.unwrap_or(20_000);
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_millis(timeout))
                .timeout_write(Duration::from_millis(timeout))
                .timeout_read(Duration::from_millis(timeout))
                .build();
            let mut req = match method.as_str() {
                "GET" => agent.get(&parsed.url),
                "POST" => agent.post(&parsed.url),
                "PUT" => agent.put(&parsed.url),
                "PATCH" => agent.patch(&parsed.url),
                "DELETE" => agent.delete(&parsed.url),
                _ => return Err(format!("unsupported method: {method}")),
            };
            if let Some(headers) = parsed.headers {
                for (k, v) in headers {
                    req = req.set(&k, &v);
                }
            }
            let resp = if let Some(body) = parsed.body {
                if parsed.json.unwrap_or(false) {
                    req.set("content-type", "application/json")
                        .send_string(&body)
                } else {
                    req.send_string(&body)
                }
            } else {
                req.call()
            };
            let (status, text) = match resp {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.into_string().unwrap_or_default();
                    (status, text)
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    (code, text)
                }
                Err(err) => return Err(format!("http_request failed: {err}")),
            };
            let truncated = if text.len() > 20_000 {
                format!("{}...[truncated]", &text[..20_000])
            } else {
                text
            };
            Ok(ToolExecution {
                output: format!("http_request {method} {} -> {status}", parsed.url),
                details: serde_json::json!({
                    "status": status,
                    "body": truncated
                }),
                is_error: status >= 400,
            })
        }
        "browser_request" => {
            let parsed: ToolBrowserRequestArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let endpoint = env_optional("AETHERVAULT_BROWSER_ENDPOINT")
                .unwrap_or_else(|| "http://127.0.0.1:4040".to_string());
            let payload = serde_json::json!({
                "action": parsed.action,
                "url": parsed.url,
                "selector": parsed.selector,
                "text": parsed.text,
                "data": parsed.data,
            });
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_write(Duration::from_secs(20))
                .timeout_read(Duration::from_secs(30))
                .build();
            let resp = agent
                .post(&endpoint)
                .set("content-type", "application/json")
                .send_json(payload);
            match resp {
                Ok(resp) => Ok(ToolExecution {
                    output: "browser_request completed.".to_string(),
                    details: resp
                        .into_json::<serde_json::Value>()
                        .map_err(|e| e.to_string())?,
                    is_error: false,
                }),
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    Err(format!("browser_request error {code}: {text}"))
                }
                Err(err) => Err(format!("browser_request failed: {err}")),
            }
        }
        "fs_list" => {
            let parsed: ToolFsListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let roots = allowed_fs_roots(&workspace_override);
            let resolved = resolve_fs_path(&parsed.path, &roots)?;
            let mut items = Vec::new();
            let max_entries = parsed.max_entries.unwrap_or(200);
            if parsed.recursive.unwrap_or(false) {
                for entry in WalkDir::new(&resolved).max_depth(6) {
                    let entry = entry.map_err(|e| e.to_string())?;
                    if items.len() >= max_entries {
                        break;
                    }
                    items.push(entry.path().display().to_string());
                }
            } else if resolved.is_dir() {
                for entry in fs::read_dir(&resolved).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    items.push(entry.path().display().to_string());
                    if items.len() >= max_entries {
                        break;
                    }
                }
            } else if resolved.exists() {
                items.push(resolved.display().to_string());
            }
            Ok(ToolExecution {
                output: format!("Listed {} entries.", items.len()),
                details: serde_json::json!({ "entries": items }),
                is_error: false,
            })
        }
        "fs_read" => {
            let parsed: ToolFsReadArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let roots = allowed_fs_roots(&workspace_override);
            let resolved = resolve_fs_path(&parsed.path, &roots)?;
            let max_bytes = parsed.max_bytes.unwrap_or(200_000);
            let file = fs::File::open(&resolved).map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            file.take(max_bytes as u64)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&buf).to_string();
            Ok(ToolExecution {
                output: format!("Read {} bytes.", buf.len()),
                details: serde_json::json!({
                    "path": resolved.display().to_string(),
                    "text": text
                }),
                is_error: false,
            })
        }
        "fs_write" => {
            let parsed: ToolFsWriteArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let roots = allowed_fs_roots(&workspace_override);
            let resolved = resolve_fs_path(&parsed.path, &roots)?;
            if parsed.append.unwrap_or(false) {
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&resolved)
                    .map_err(|e| e.to_string())?;
                file.write_all(parsed.text.as_bytes())
                    .map_err(|e| e.to_string())?;
            } else {
                fs::write(&resolved, parsed.text.as_bytes()).map_err(|e| e.to_string())?;
            }
            Ok(ToolExecution {
                output: "File written.".to_string(),
                details: serde_json::json!({ "path": resolved.display().to_string() }),
                is_error: false,
            })
        }
        "approval_list" => with_read_mem(mem_read, mem_write, mv2, |mem| {
            let approvals = load_approvals(mem);
            let pending: Vec<ApprovalEntry> = approvals
                .into_iter()
                .filter(|a| a.status == "pending")
                .collect();
            Ok(ToolExecution {
                output: format!("{} pending approvals.", pending.len()),
                details: serde_json::json!({ "approvals": pending }),
                is_error: false,
            })
        }),
        "trigger_add" => {
            let parsed: ToolTriggerAddArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut triggers = load_triggers(mem);
                let id = format!(
                    "trg_{}_{}",
                    chrono::Utc::now().timestamp(),
                    triggers.len() + 1
                );
                let entry = TriggerEntry {
                    id: id.clone(),
                    kind: parsed.kind,
                    name: parsed.name,
                    query: parsed.query,
                    prompt: parsed.prompt,
                    start: parsed.start,
                    end: parsed.end,
                    enabled: parsed.enabled.unwrap_or(true),
                    last_seen: None,
                    last_fired: None,
                };
                triggers.push(entry);
                save_triggers(mem, &triggers)?;
                Ok(ToolExecution {
                    output: "Trigger added.".to_string(),
                    details: serde_json::json!({ "id": id }),
                    is_error: false,
                })
            })
        }
        "trigger_list" => with_write_mem(mem_read, mem_write, mv2, true, |mem| {
            let triggers = load_triggers(mem);
            Ok(ToolExecution {
                output: format!("{} triggers.", triggers.len()),
                details: serde_json::json!({ "triggers": triggers }),
                is_error: false,
            })
        }),
        "trigger_remove" => {
            let parsed: ToolTriggerRemoveArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut triggers = load_triggers(mem);
                let before = triggers.len();
                triggers.retain(|t| t.id != parsed.id);
                let updated = triggers.len() != before;
                if updated {
                    save_triggers(mem, &triggers)?;
                }
                Ok(ToolExecution {
                    output: if updated {
                        "Trigger removed.".to_string()
                    } else {
                        "Trigger not found.".to_string()
                    },
                    details: serde_json::json!({ "id": parsed.id, "updated": updated }),
                    is_error: !updated,
                })
            })
        }
        "tool_search" => {
            let parsed: ToolToolSearchArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let query_tokens: Vec<String> = parsed
                .query
                .to_ascii_lowercase()
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            let mut results = Vec::new();
            for tool in tool_definitions_json() {
                let name = tool
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let desc = tool
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let score = tool_score(&query_tokens, &name, &desc);
                if score > 0 {
                    results.push(serde_json::json!({
                        "name": name,
                        "description": desc,
                        "score": score
                    }));
                }
            }
            results.sort_by(|a, b| {
                b.get("score")
                    .and_then(|v| v.as_i64())
                    .cmp(&a.get("score").and_then(|v| v.as_i64()))
            });
            let limit = parsed.limit.unwrap_or(8);
            let results: Vec<serde_json::Value> = results.into_iter().take(limit).collect();
            Ok(ToolExecution {
                output: format!("Found {} tools.", results.len()),
                details: serde_json::json!({ "results": results }),
                is_error: false,
            })
        }
        "session_context" => {
            let parsed: ToolSessionContextArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let scope = format!("aethervault://agent-log/{}/", parsed.session);
            let limit = parsed.limit.unwrap_or(20);
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let request = SearchRequest {
                    query: parsed.session.clone(),
                    top_k: 200,
                    snippet_chars: 200,
                    uri: None,
                    scope: Some(scope),
                    cursor: None,
                    temporal: None,
                    as_of_frame: None,
                    as_of_ts: None,
                    no_sketch: true,
                };
                let response = mem.search(request).map_err(|e| e.to_string())?;
                let mut entries = Vec::new();
                for hit in response.hits {
                    let uri = hit.uri.clone();
                    let ts = parse_log_ts_from_uri(&uri).unwrap_or_default();
                    if let Ok(text) = mem.frame_text_by_id(hit.frame_id) {
                        if let Ok(entry) = serde_json::from_str::<AgentLogEntry>(&text) {
                            entries.push(serde_json::json!({
                                "ts": entry.ts_utc.unwrap_or(ts),
                                "role": entry.role,
                                "text": entry.text,
                                "meta": entry.meta,
                                "uri": uri
                            }));
                        }
                    }
                }
                entries.sort_by(|a, b| {
                    b.get("ts")
                        .and_then(|v| v.as_i64())
                        .cmp(&a.get("ts").and_then(|v| v.as_i64()))
                });
                let results: Vec<serde_json::Value> = entries.into_iter().take(limit).collect();
                Ok(ToolExecution {
                    output: format!("Loaded {} entries.", results.len()),
                    details: serde_json::json!({ "entries": results }),
                    is_error: false,
                })
            })
        }
        "reflect" => {
            let parsed: ToolReflectArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let session = parsed
                .session
                .clone()
                .unwrap_or_else(|| "default".to_string());
            let ts = Utc::now().timestamp();
            let payload = serde_json::json!({
                "session": session,
                "text": parsed.text,
                "reason": parsed.reason,
                "ts_utc": ts
            });
            let bytes = serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?;
            let hash = blake3_hash(&bytes);
            let uri = format!(
                "aethervault://memory/reflection/{}/{}-{}",
                session,
                ts,
                hash.to_hex()
            );
            with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut options = PutOptions::default();
                options.uri = Some(uri.clone());
                options.title = Some("reflection".to_string());
                options.kind = Some("application/json".to_string());
                options.track = Some("aethervault.reflection".to_string());
                options.search_text = Some(payload.to_string());
                mem.put_bytes_with_options(&bytes, options)
                    .map_err(|e| e.to_string())?;
                mem.commit().map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output: "Reflection stored.".to_string(),
                    details: serde_json::json!({ "uri": uri }),
                    is_error: false,
                })
            })
        }
        "skill_store" => {
            let parsed: ToolSkillStoreArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let ts = Utc::now().timestamp();
            let payload = serde_json::json!({
                "name": parsed.name,
                "trigger": parsed.trigger,
                "steps": parsed.steps,
                "tools": parsed.tools,
                "notes": parsed.notes,
                "ts_utc": ts
            });
            let bytes = serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?;
            let hash = blake3_hash(&bytes);
            let slug = payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("skill")
                .to_ascii_lowercase()
                .replace(' ', "-");
            let uri = format!("aethervault://skills/{}/{}-{}", slug, ts, hash.to_hex());
            with_write_mem(mem_read, mem_write, mv2, true, |mem| {
                let mut options = PutOptions::default();
                options.uri = Some(uri.clone());
                options.title = Some("skill".to_string());
                options.kind = Some("application/json".to_string());
                options.track = Some("aethervault.skill".to_string());
                options.search_text = Some(payload.to_string());
                mem.put_bytes_with_options(&bytes, options)
                    .map_err(|e| e.to_string())?;
                mem.commit().map_err(|e| e.to_string())?;
                Ok(ToolExecution {
                    output: "Skill stored.".to_string(),
                    details: serde_json::json!({ "uri": uri }),
                    is_error: false,
                })
            })
        }
        "skill_search" => {
            let parsed: ToolSkillSearchArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            with_read_mem(mem_read, mem_write, mv2, |mem| {
                let request = SearchRequest {
                    query: parsed.query.clone(),
                    top_k: parsed.limit.unwrap_or(10),
                    snippet_chars: 200,
                    uri: None,
                    scope: Some("aethervault://skills/".to_string()),
                    cursor: None,
                    temporal: None,
                    as_of_frame: None,
                    as_of_ts: None,
                    no_sketch: true,
                };
                let response = mem.search(request).map_err(|e| e.to_string())?;
                let mut out = Vec::new();
                for hit in response.hits {
                    out.push(serde_json::json!({
                        "uri": hit.uri,
                        "title": hit.title,
                        "text": hit.text,
                        "score": hit.score
                    }));
                }
                Ok(ToolExecution {
                    output: format!("Found {} skills.", out.len()),
                    details: serde_json::json!({ "results": out }),
                    is_error: false,
                })
            })
        }
        "subagent_list" => with_read_mem(mem_read, mem_write, mv2, |mem| {
            let config = load_capsule_config(mem).unwrap_or_default();
            let subagents = load_subagents_from_config(&config);
            Ok(ToolExecution {
                output: format!("{} subagents.", subagents.len()),
                details: serde_json::json!({ "subagents": subagents }),
                is_error: false,
            })
        }),
        "subagent_invoke" => {
            let parsed: ToolSubagentInvokeArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let config = with_read_mem(mem_read, mem_write, mv2, |mem| {
                Ok(load_capsule_config(mem).unwrap_or_default())
            })?;
            let subagents = load_subagents_from_config(&config);
            let mut system = parsed.system.clone();
            let mut model_hook = parsed.model_hook.clone();
            if let Some(spec) = subagents.iter().find(|s| s.name == parsed.name) {
                if system.is_none() {
                    system = spec.system.clone();
                }
                if model_hook.is_none() {
                    model_hook = spec.model_hook.clone();
                }
            } else if system.is_none() && model_hook.is_none() {
                return Err(format!("unknown subagent: {}", parsed.name));
            }
            let cfg = build_bridge_agent_config(
                mv2.to_path_buf(),
                model_hook,
                system,
                false,
                None,
                8,
                12_000,
                64,
                true,
                8,
            )
            .map_err(|e| e.to_string())?;
            // Release all capsule handles before spawning the subagent so it can
            // acquire its own locks without contending with the parent session.
            *mem_read = None;
            *mem_write = None;
            let session = format!("subagent:{}:{}", parsed.name, Utc::now().timestamp());
            let result = run_agent_for_bridge(&cfg, &parsed.prompt, session, None, None, None)
                .map_err(|e| e.to_string())?;
            Ok(ToolExecution {
                output: result.final_text.unwrap_or_default(),
                details: serde_json::json!({ "session": result.session, "messages": result.messages.len() }),
                is_error: false,
            })
        }
        "subagent_batch" => {
            let parsed: ToolSubagentBatchArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            if parsed.invocations.is_empty() {
                return Err("subagent_batch requires at least one invocation".into());
            }
            let config_snapshot = with_read_mem(mem_read, mem_write, mv2, |mem| {
                Ok(load_capsule_config(mem).unwrap_or_default())
            })?;
            let subagents = load_subagents_from_config(&config_snapshot);
            let ts = Utc::now().timestamp();

            // Release all capsule handles before spawning subagent threads so they can
            // each acquire their own locks without contending with the parent session.
            *mem_read = None;
            *mem_write = None;

            // Build configs for each invocation and spawn threads.
            let mut handles: Vec<(String, std::thread::JoinHandle<Result<AgentRunOutput, String>>)> = Vec::new();
            for (i, inv) in parsed.invocations.into_iter().enumerate() {
                let mut system = inv.system.clone();
                let mut model_hook = inv.model_hook.clone();
                if let Some(spec) = subagents.iter().find(|s| s.name == inv.name) {
                    if system.is_none() {
                        system = spec.system.clone();
                    }
                    if model_hook.is_none() {
                        model_hook = spec.model_hook.clone();
                    }
                } else if system.is_none() && model_hook.is_none() {
                    handles.push((inv.name.clone(), thread::spawn(move || {
                        Err(format!("unknown subagent: {}", inv.name))
                    })));
                    continue;
                }
                let cfg = build_bridge_agent_config(
                    mv2.to_path_buf(),
                    model_hook,
                    system,
                    false,
                    None,
                    8,
                    12_000,
                    64,
                    true,
                    8,
                )
                .map_err(|e| e.to_string())?;
                let session = format!("subagent:{}:{}:{}", inv.name, ts, i);
                let prompt = inv.prompt.clone();
                let name = inv.name.clone();
                handles.push((name, thread::spawn(move || {
                    run_agent_for_bridge(&cfg, &prompt, session, None, None, None)
                })));
            }

            // Collect results from all threads.
            let mut results = Vec::new();
            let mut all_ok = true;
            for (name, handle) in handles {
                match handle.join() {
                    Ok(Ok(output)) => {
                        results.push(serde_json::json!({
                            "name": name,
                            "status": "ok",
                            "output": output.final_text.unwrap_or_default(),
                            "session": output.session,
                            "messages": output.messages.len(),
                        }));
                    }
                    Ok(Err(err)) => {
                        all_ok = false;
                        results.push(serde_json::json!({
                            "name": name,
                            "status": "error",
                            "error": err,
                        }));
                    }
                    Err(_) => {
                        all_ok = false;
                        results.push(serde_json::json!({
                            "name": name,
                            "status": "error",
                            "error": "subagent thread panicked",
                        }));
                    }
                }
            }
            let summary = if all_ok {
                format!("{} subagents completed successfully.", results.len())
            } else {
                let ok_count = results.iter().filter(|r| r["status"] == "ok").count();
                let err_count = results.len() - ok_count;
                format!("{} subagents completed, {} failed.", ok_count, err_count)
            };
            Ok(ToolExecution {
                output: summary,
                details: serde_json::json!({ "results": results }),
                is_error: !all_ok,
            })
        }
        "gmail_list" => {
            let parsed: ToolGmailListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "google").map_err(|e| e.to_string())?;
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(20))
                .build();
            let mut url = format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults={}",
                parsed.max_results.unwrap_or(10)
            );
            if let Some(q) = parsed.query {
                url.push_str("&q=");
                url.push_str(&urlencoding::encode(&q));
            }
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("gmail_list error {code}: {text}").into());
                }
                Err(err) => return Err(format!("gmail_list failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Gmail messages listed.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "gmail_read" => {
            let parsed: ToolGmailReadArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "google").map_err(|e| e.to_string())?;
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(20))
                .build();
            let url = format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}?format=full",
                parsed.id
            );
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("gmail_read error {code}: {text}").into());
                }
                Err(err) => return Err(format!("gmail_read failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Gmail message read.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "gmail_send" => {
            let parsed: ToolGmailSendArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "google").map_err(|e| e.to_string())?;
            let raw = format!(
                "To: {}\r\nSubject: {}\r\n\r\n{}\r\n",
                parsed.to, parsed.subject, parsed.body
            );
            let encoded = base64::engine::general_purpose::STANDARD
                .encode(raw.as_bytes())
                .replace('+', "-")
                .replace('/', "_")
                .trim_end_matches('=')
                .to_string();
            let payload = serde_json::json!({ "raw": encoded });
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(20))
                .build();
            let resp = agent
                .post("https://gmail.googleapis.com/gmail/v1/users/me/messages/send")
                .set("authorization", &format!("Bearer {}", token))
                .set("content-type", "application/json")
                .send_json(payload);
            match resp {
                Ok(resp) => Ok(ToolExecution {
                    output: "Gmail message sent.".to_string(),
                    details: resp
                        .into_json::<serde_json::Value>()
                        .map_err(|e| e.to_string())?,
                    is_error: false,
                }),
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    Err(format!("gmail_send error {code}: {text}").into())
                }
                Err(err) => Err(format!("gmail_send failed: {err}").into()),
            }
        }
        "gcal_list" => {
            let parsed: ToolGCalListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "google").map_err(|e| e.to_string())?;
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(20))
                .build();
            let url = format!(
                "https://www.googleapis.com/calendar/v3/calendars/primary/events?maxResults={}",
                parsed.max_results.unwrap_or(10)
            );
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("gcal_list error {code}: {text}").into());
                }
                Err(err) => return Err(format!("gcal_list failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Calendar events listed.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "gcal_create" => {
            let parsed: ToolGCalCreateArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "google").map_err(|e| e.to_string())?;
            let payload = serde_json::json!({
                "summary": parsed.summary,
                "description": parsed.description,
                "start": { "dateTime": parsed.start },
                "end": { "dateTime": parsed.end }
            });
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(20))
                .build();
            let resp = agent
                .post("https://www.googleapis.com/calendar/v3/calendars/primary/events")
                .set("authorization", &format!("Bearer {}", token))
                .set("content-type", "application/json")
                .send_json(payload);
            match resp {
                Ok(resp) => Ok(ToolExecution {
                    output: "Calendar event created.".to_string(),
                    details: resp
                        .into_json::<serde_json::Value>()
                        .map_err(|e| e.to_string())?,
                    is_error: false,
                }),
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    Err(format!("gcal_create error {code}: {text}").into())
                }
                Err(err) => Err(format!("gcal_create failed: {err}").into()),
            }
        }
        "ms_mail_list" => {
            let parsed: ToolMsMailListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "microsoft").map_err(|e| e.to_string())?;
            let url = format!(
                "https://graph.microsoft.com/v1.0/me/messages?$top={}",
                parsed.top.unwrap_or(10)
            );
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(20))
                .build();
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("ms_mail_list error {code}: {text}").into());
                }
                Err(err) => return Err(format!("ms_mail_list failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Microsoft mail listed.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "ms_mail_read" => {
            let parsed: ToolMsMailReadArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "microsoft").map_err(|e| e.to_string())?;
            let url = format!("https://graph.microsoft.com/v1.0/me/messages/{}", parsed.id);
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(20))
                .build();
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("ms_mail_read error {code}: {text}").into());
                }
                Err(err) => return Err(format!("ms_mail_read failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Microsoft mail read.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "ms_calendar_list" => {
            let parsed: ToolMsCalendarListArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "microsoft").map_err(|e| e.to_string())?;
            let url = format!(
                "https://graph.microsoft.com/v1.0/me/events?$top={}",
                parsed.top.unwrap_or(10)
            );
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(20))
                .build();
            let resp = agent
                .get(&url)
                .set("authorization", &format!("Bearer {}", token))
                .call();
            let payload = match resp {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| e.to_string())?,
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    return Err(format!("ms_calendar_list error {code}: {text}").into());
                }
                Err(err) => return Err(format!("ms_calendar_list failed: {err}").into()),
            };
            Ok(ToolExecution {
                output: "Microsoft calendar listed.".to_string(),
                details: payload,
                is_error: false,
            })
        }
        "ms_calendar_create" => {
            let parsed: ToolMsCalendarCreateArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            let token = get_oauth_token(mv2, "microsoft").map_err(|e| e.to_string())?;
            let payload = serde_json::json!({
                "subject": parsed.subject,
                "body": {
                    "contentType": "Text",
                    "content": parsed.body.unwrap_or_default()
                },
                "start": { "dateTime": parsed.start, "timeZone": "UTC" },
                "end": { "dateTime": parsed.end, "timeZone": "UTC" }
            });
            let agent = ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(20))
                .build();
            let resp = agent
                .post("https://graph.microsoft.com/v1.0/me/events")
                .set("authorization", &format!("Bearer {}", token))
                .set("content-type", "application/json")
                .send_json(payload);
            match resp {
                Ok(resp) => Ok(ToolExecution {
                    output: "Microsoft calendar event created.".to_string(),
                    details: resp
                        .into_json::<serde_json::Value>()
                        .map_err(|e| e.to_string())?,
                    is_error: false,
                }),
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    Err(format!("ms_calendar_create error {code}: {text}").into())
                }
                Err(err) => Err(format!("ms_calendar_create failed: {err}").into()),
            }
        }
        "scale" => {
            let parsed: ToolScaleArgs =
                serde_json::from_value(args).map_err(|e| format!("args: {e}"))?;
            match parsed.action.as_str() {
                "status" => {
                    // Pure local: read /proc files + df for system stats
                    let cpu_count = std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(1);
                    let (load_1m, load_5m) =
                        std::fs::read_to_string("/proc/loadavg")
                            .ok()
                            .and_then(|s| {
                                let parts: Vec<&str> = s.split_whitespace().collect();
                                if parts.len() >= 2 {
                                    Some((
                                        parts[0].parse::<f64>().unwrap_or(0.0),
                                        parts[1].parse::<f64>().unwrap_or(0.0),
                                    ))
                                } else {
                                    None
                                }
                            })
                            .unwrap_or((0.0, 0.0));
                    let (mem_total_mb, mem_avail_mb) =
                        std::fs::read_to_string("/proc/meminfo")
                            .ok()
                            .map(|s| {
                                let mut total: u64 = 0;
                                let mut avail: u64 = 0;
                                for line in s.lines() {
                                    if line.starts_with("MemTotal:") {
                                        total = line
                                            .split_whitespace()
                                            .nth(1)
                                            .and_then(|v| v.parse::<u64>().ok())
                                            .unwrap_or(0)
                                            / 1024;
                                    } else if line.starts_with("MemAvailable:") {
                                        avail = line
                                            .split_whitespace()
                                            .nth(1)
                                            .and_then(|v| v.parse::<u64>().ok())
                                            .unwrap_or(0)
                                            / 1024;
                                    }
                                }
                                (total, avail)
                            })
                            .unwrap_or((0, 0));
                    let mem_used_pct = if mem_total_mb > 0 {
                        ((mem_total_mb - mem_avail_mb) as f64 / mem_total_mb as f64 * 100.0)
                            .round()
                    } else {
                        0.0
                    };
                    // Disk via df
                    let (disk_total_gb, disk_used_gb, disk_used_pct) = std::process::Command::new("df")
                        .args(["-BG", "/"])
                        .output()
                        .ok()
                        .and_then(|out| {
                            let text = String::from_utf8_lossy(&out.stdout);
                            let line = text.lines().nth(1)?;
                            let parts: Vec<&str> = line.split_whitespace().collect();
                            if parts.len() >= 5 {
                                let total = parts[1]
                                    .trim_end_matches('G')
                                    .parse::<f64>()
                                    .unwrap_or(0.0);
                                let used = parts[2]
                                    .trim_end_matches('G')
                                    .parse::<f64>()
                                    .unwrap_or(0.0);
                                let pct = parts[4]
                                    .trim_end_matches('%')
                                    .parse::<f64>()
                                    .unwrap_or(0.0);
                                Some((total, used, pct))
                            } else {
                                None
                            }
                        })
                        .unwrap_or((0.0, 0.0, 0.0));
                    let details = serde_json::json!({
                        "cpu_count": cpu_count,
                        "load_1m": load_1m,
                        "load_5m": load_5m,
                        "mem_total_mb": mem_total_mb,
                        "mem_avail_mb": mem_avail_mb,
                        "mem_used_pct": mem_used_pct,
                        "disk_total_gb": disk_total_gb,
                        "disk_used_gb": disk_used_gb,
                        "disk_used_pct": disk_used_pct,
                    });
                    Ok(ToolExecution {
                        output: format!(
                            "CPU: {} cores, load {:.1}/{:.1} | RAM: {}MB/{} MB ({:.0}% used) | Disk: {:.0}G/{:.0}G ({:.0}% used)",
                            cpu_count, load_1m, load_5m, mem_total_mb - mem_avail_mb, mem_total_mb, mem_used_pct,
                            disk_used_gb, disk_total_gb, disk_used_pct,
                        ),
                        details,
                        is_error: false,
                    })
                }
                "sizes" => {
                    let do_token = env_optional("DO_TOKEN")
                        .ok_or_else(|| "DO_TOKEN not set — cannot query DigitalOcean API".to_string())?;
                    let out = std::process::Command::new("curl")
                        .args([
                            "-s",
                            "-X", "GET",
                            "https://api.digitalocean.com/v2/sizes",
                            "-H", &format!("Authorization: Bearer {}", do_token),
                        ])
                        .output()
                        .map_err(|e| format!("curl failed: {e}"))?;
                    let body: serde_json::Value =
                        serde_json::from_slice(&out.stdout)
                            .map_err(|e| format!("invalid JSON from DO API: {e}"))?;
                    let sizes = body
                        .get("sizes")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    // Filter to ≤8 vCPU / ≤32GB to prevent cost overruns
                    let filtered: Vec<serde_json::Value> = sizes
                        .into_iter()
                        .filter(|s| {
                            let vcpus = s.get("vcpus").and_then(|v| v.as_u64()).unwrap_or(99);
                            let mem = s.get("memory").and_then(|v| v.as_u64()).unwrap_or(999999);
                            let available = s.get("available").and_then(|v| v.as_bool()).unwrap_or(false);
                            vcpus <= 8 && mem <= 32768 && available
                        })
                        .map(|s| {
                            serde_json::json!({
                                "slug": s.get("slug").and_then(|v| v.as_str()).unwrap_or(""),
                                "vcpus": s.get("vcpus").and_then(|v| v.as_u64()).unwrap_or(0),
                                "memory_mb": s.get("memory").and_then(|v| v.as_u64()).unwrap_or(0),
                                "disk_gb": s.get("disk").and_then(|v| v.as_u64()).unwrap_or(0),
                                "price_monthly": s.get("price_monthly").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            })
                        })
                        .collect();
                    let details = serde_json::json!({ "sizes": filtered });
                    Ok(ToolExecution {
                        output: format!("{} available sizes (≤8 vCPU, ≤32GB).", filtered.len()),
                        details,
                        is_error: false,
                    })
                }
                "resize" => {
                    let target_size = parsed
                        .size
                        .ok_or_else(|| "size parameter is required for resize".to_string())?;
                    let do_token = env_optional("DO_TOKEN")
                        .ok_or_else(|| "DO_TOKEN not set — cannot call DigitalOcean API".to_string())?;
                    // Get droplet ID: env var or auto-detect via DO metadata
                    let droplet_id = env_optional("DO_DROPLET_ID").or_else(|| {
                        std::process::Command::new("curl")
                            .args(["-s", "http://169.254.169.254/metadata/v1/id"])
                            .output()
                            .ok()
                            .and_then(|o| {
                                let id = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
                                    Some(id)
                                } else {
                                    None
                                }
                            })
                    }).ok_or_else(|| "DO_DROPLET_ID not set and metadata API unreachable".to_string())?;
                    let url = format!(
                        "https://api.digitalocean.com/v2/droplets/{}/actions",
                        droplet_id
                    );
                    let payload = serde_json::json!({
                        "type": "resize",
                        "disk": false,
                        "size": target_size,
                    });
                    let out = std::process::Command::new("curl")
                        .args([
                            "-s",
                            "-X", "POST",
                            &url,
                            "-H", &format!("Authorization: Bearer {}", do_token),
                            "-H", "Content-Type: application/json",
                            "-d", &payload.to_string(),
                        ])
                        .output()
                        .map_err(|e| format!("curl failed: {e}"))?;
                    let resp: serde_json::Value =
                        serde_json::from_slice(&out.stdout)
                            .map_err(|e| format!("invalid JSON from DO API: {e}"))?;
                    let action_status = resp
                        .get("action")
                        .and_then(|a| a.get("status"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let action_id = resp
                        .get("action")
                        .and_then(|a| a.get("id"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    if action_status == "errored" || resp.get("id").is_some_and(|v| v.as_str() == Some("not_found")) {
                        let msg = resp.get("message").and_then(|v| v.as_str()).unwrap_or("resize failed");
                        return Err(format!("DO resize error: {msg}"));
                    }
                    Ok(ToolExecution {
                        output: format!(
                            "Resize to {} initiated (action {}, status: {}). Note: CPU resizes require a power cycle to take effect.",
                            target_size, action_id, action_status
                        ),
                        details: resp,
                        is_error: false,
                    })
                }
                other => Err(format!("unknown scale action: {other} (use status, sizes, or resize)")),
            }
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

    if first_line
        .to_ascii_lowercase()
        .starts_with("content-length:")
    {
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
        report.metrics.actions_completed, report.metrics.actions_skipped
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
        let params = msg
            .get("params")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

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
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
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
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("Missing {name}")).into());
    }
    Ok(value)
}

fn env_optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn env_u64(name: &str, default: u64) -> Result<u64, Box<dyn std::error::Error>> {
    match env_optional(name) {
        Some(value) => Ok(value
            .parse::<u64>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid {name}")))?),
        None => Ok(default),
    }
}

fn env_usize(name: &str, default: usize) -> Result<usize, Box<dyn std::error::Error>> {
    match env_optional(name) {
        Some(value) => Ok(value
            .parse::<usize>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid {name}")))?),
        None => Ok(default),
    }
}

fn env_f64(name: &str, default: f64) -> Result<f64, Box<dyn std::error::Error>> {
    match env_optional(name) {
        Some(value) => Ok(value
            .parse::<f64>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid {name}")))?),
        None => Ok(default),
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    match env_optional(name) {
        Some(value) => {
            let v = value.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "y" | "on")
        }
        None => default,
    }
}

fn jitter_ratio() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1000) as f64 / 1000.0
}

fn parse_retry_after(resp: &ureq::Response) -> Option<f64> {
    resp.header("retry-after")
        .and_then(|v| v.trim().parse::<f64>().ok())
}

fn command_wrapper() -> Option<Vec<String>> {
    env_optional("AETHERVAULT_COMMAND_WRAPPER").map(|raw| {
        raw.split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    })
}

fn build_external_command(program: &str, args: &[String]) -> ProcessCommand {
    if let Some(wrapper) = command_wrapper() {
        let mut cmd = ProcessCommand::new(&wrapper[0]);
        cmd.args(&wrapper[1..]).arg(program).args(args);
        return cmd;
    }
    let mut cmd = ProcessCommand::new(program);
    cmd.args(args);
    cmd
}

fn resolve_workspace(cli: Option<PathBuf>, agent_cfg: &AgentConfig) -> Option<PathBuf> {
    if let Some(path) = cli {
        return Some(path);
    }
    if let Some(value) = env_optional("AETHERVAULT_WORKSPACE") {
        if !value.trim().is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    if let Some(value) = &agent_cfg.workspace {
        if !value.trim().is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    Some(PathBuf::from(DEFAULT_WORKSPACE_DIR))
}

fn read_optional_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().and_then(|text| {
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    })
}

fn daily_memory_path(workspace: &Path) -> PathBuf {
    let date = Utc::now().format("%Y-%m-%d").to_string();
    workspace.join("memory").join(format!("{date}.md"))
}

fn memory_uri(kind: &str) -> String {
    format!("aethervault://memory/{kind}.md")
}

fn memory_daily_uri(date: &str) -> String {
    format!("aethervault://memory/daily/{date}.md")
}

fn sync_memory_file(
    mem: &mut Vault,
    path: &Path,
    uri: String,
    title: &str,
    track: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut options = PutOptions::default();
    options.uri = Some(uri);
    options.title = Some(title.to_string());
    options.kind = Some("text/markdown".to_string());
    options.track = Some(track.to_string());
    options.search_text = Some(text.clone());
    let id = mem.put_bytes_with_options(text.as_bytes(), options)?;
    mem.commit()?;
    Ok(id)
}

fn sync_workspace_memory(
    mv2: &Path,
    workspace: &Path,
    include_daily: bool,
) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    let mut mem = open_or_create(mv2)?;
    let mut ids = Vec::new();
    let soul = workspace.join("SOUL.md");
    let user = workspace.join("USER.md");
    let memory = workspace.join("MEMORY.md");
    if soul.exists() {
        ids.push(sync_memory_file(
            &mut mem,
            &soul,
            memory_uri("soul"),
            "memory soul",
            "aethervault.memory",
        )?);
    }
    if user.exists() {
        ids.push(sync_memory_file(
            &mut mem,
            &user,
            memory_uri("user"),
            "memory user",
            "aethervault.memory",
        )?);
    }
    if memory.exists() {
        ids.push(sync_memory_file(
            &mut mem,
            &memory,
            memory_uri("longterm"),
            "memory longterm",
            "aethervault.memory",
        )?);
    }
    if include_daily {
        let daily_dir = workspace.join("memory");
        if daily_dir.exists() {
            for entry in WalkDir::new(&daily_dir).max_depth(1) {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let uri = memory_daily_uri(stem);
                let title = format!("memory daily {stem}");
                ids.push(sync_memory_file(
                    &mut mem,
                    path,
                    uri,
                    &title,
                    "aethervault.memory",
                )?);
            }
        }
    }
    Ok(ids)
}

fn export_capsule_memory(
    mv2: &Path,
    workspace: &Path,
    include_daily: bool,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut mem = Vault::open_read_only(mv2)?;
    let mut paths = Vec::new();
    let items = vec![
        (memory_uri("soul"), workspace.join("SOUL.md")),
        (memory_uri("user"), workspace.join("USER.md")),
        (memory_uri("longterm"), workspace.join("MEMORY.md")),
    ];
    for (uri, path) in items {
        if let Ok(frame) = mem.frame_by_uri(&uri) {
            if let Ok(text) = mem.frame_text_by_id(frame.id) {
                fs::create_dir_all(workspace)?;
                fs::write(&path, text)?;
                paths.push(path.display().to_string());
            }
        }
    }
    if include_daily {
        let daily_dir = workspace.join("memory");
        fs::create_dir_all(&daily_dir)?;
        let total = mem.frame_count() as u64;
        for frame_id in 0..total {
            let frame = match mem.frame_by_id(frame_id) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let Some(uri) = frame.uri.as_deref() else {
                continue;
            };
            if !uri.starts_with("aethervault://memory/daily/") {
                continue;
            }
            if let Some(name) = uri.rsplit('/').next() {
                let path = daily_dir.join(name);
                if let Ok(text) = mem.frame_text_by_id(frame_id) {
                    fs::write(&path, text)?;
                    paths.push(path.display().to_string());
                }
            }
        }
    }
    Ok(paths)
}

fn oauth_env(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    env_optional(name)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("Missing {name}").into())
}

fn build_oauth_redirect(base: &str, provider: &str) -> String {
    format!("{base}/oauth/{provider}/callback")
}

fn build_google_auth_url(client_id: &str, redirect_uri: &str, scope: &str, state: &str) -> String {
    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id={}&redirect_uri={}&scope={}&access_type=offline&prompt=consent&state={}",
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(scope),
        urlencoding::encode(state)
    )
}

fn build_microsoft_auth_url(
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    state: &str,
) -> String {
    format!(
        "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}&response_mode=query&state={}",
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(scope),
        urlencoding::encode(state)
    )
}

fn exchange_oauth_code(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .timeout_write(Duration::from_secs(10))
        .build();
    let payload = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", client_id)
        .append_pair("client_secret", client_secret)
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", redirect_uri)
        .finish();
    let response = agent
        .post(token_url)
        .set("content-type", "application/x-www-form-urlencoded")
        .send_string(&payload);
    match response {
        Ok(resp) => Ok(resp.into_json()?),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(format!("token error {code}: {text}").into())
        }
        Err(err) => Err(format!("token request failed: {err}").into()),
    }
}

fn run_oauth_broker(
    mv2: PathBuf,
    provider: String,
    bind: String,
    port: u16,
    redirect_base: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let provider = provider.to_ascii_lowercase();
    let redirect_base = redirect_base.unwrap_or_else(|| format!("http://{}:{}", bind, port));
    let redirect_uri = build_oauth_redirect(&redirect_base, &provider);
    let state = "aethervault";

    let (client_id, client_secret, _scope, token_url, auth_url) = if provider == "google" {
        let client_id = oauth_env("GOOGLE_CLIENT_ID")?;
        let client_secret = oauth_env("GOOGLE_CLIENT_SECRET")?;
        let scope = env_optional("GOOGLE_SCOPES").unwrap_or_else(|| {
            "https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/calendar https://www.googleapis.com/auth/gmail.send"
                .to_string()
        });
        let auth_url = build_google_auth_url(&client_id, &redirect_uri, &scope, state);
        (
            client_id,
            client_secret,
            scope,
            "https://oauth2.googleapis.com/token".to_string(),
            auth_url,
        )
    } else if provider == "microsoft" {
        let client_id = oauth_env("MICROSOFT_CLIENT_ID")?;
        let client_secret = oauth_env("MICROSOFT_CLIENT_SECRET")?;
        let scope = env_optional("MICROSOFT_SCOPES").unwrap_or_else(|| {
            "offline_access https://graph.microsoft.com/Mail.Read https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite"
                .to_string()
        });
        let auth_url = build_microsoft_auth_url(&client_id, &redirect_uri, &scope, state);
        (
            client_id,
            client_secret,
            scope,
            "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string(),
            auth_url,
        )
    } else {
        return Err("provider must be google or microsoft".into());
    };

    println!("Open this URL to authorize:\n{auth_url}");
    let addr = format!("{bind}:{port}");
    let server = Server::http(&addr)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("server: {e}")))?;
    eprintln!("OAuth broker listening on http://{addr}");

    for request in server.incoming_requests() {
        let url = request.url().to_string();
        if !url.starts_with(&format!("/oauth/{provider}/callback")) {
            let response = Response::from_string("ok");
            let _ = request.respond(response);
            continue;
        }
        let query = url.splitn(2, '?').nth(1).unwrap_or("");
        let params: HashMap<String, String> = form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        let code = match params.get("code") {
            Some(c) => c.to_string(),
            None => {
                let response = Response::from_string("missing code");
                let _ = request.respond(response);
                continue;
            }
        };
        let token =
            exchange_oauth_code(&token_url, &client_id, &client_secret, &redirect_uri, &code)?;
        let key = format!("oauth.{provider}");
        let payload = serde_json::to_vec_pretty(&token)?;
        let mut mem = open_or_create(&mv2)?;
        let _ = save_config_entry(&mut mem, &key, &payload)?;
        let response = Response::from_string("Authorized. You can close this tab.");
        let _ = request.respond(response);
        println!("Stored token in config key: {key}");
        break;
    }
    Ok(())
}

fn load_config_json(mem: &mut Vault, key: &str) -> Option<serde_json::Value> {
    let bytes = load_config_entry(mem, key)?;
    serde_json::from_slice(&bytes).ok()
}

fn approval_hash(tool: &str, args: &serde_json::Value) -> String {
    let payload = serde_json::json!({ "tool": tool, "args": args });
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    blake3_hash(&bytes).to_hex().to_string()
}

fn load_approvals(mem: &mut Vault) -> Vec<ApprovalEntry> {
    load_config_json(mem, "approvals")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn save_approvals(mem: &mut Vault, approvals: &[ApprovalEntry]) -> Result<(), String> {
    let json = serde_json::to_value(approvals).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&json).map_err(|e| e.to_string())?;
    save_config_entry(mem, "approvals", &bytes).map_err(|e| e.to_string())?;
    Ok(())
}

enum ApprovalChatCommand {
    Approve(String),
    Reject(String),
}

fn parse_approval_chat_command(text: &str) -> Option<ApprovalChatCommand> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = trimmed.split_whitespace();
    let cmd = parts.next()?.to_ascii_lowercase();
    let id = parts.next()?.trim();
    if id.is_empty() {
        return None;
    }
    match cmd.as_str() {
        "approve" => Some(ApprovalChatCommand::Approve(id.to_string())),
        "reject" => Some(ApprovalChatCommand::Reject(id.to_string())),
        _ => None,
    }
}

fn approve_and_maybe_execute(mv2: &Path, id: &str, execute: bool) -> Result<String, String> {
    let mut mem = open_or_create(mv2).map_err(|e| e.to_string())?;
    let mut approvals = load_approvals(&mut mem);
    let mut entry: Option<ApprovalEntry> = None;
    for a in approvals.iter_mut() {
        if a.id == id {
            a.status = "approved".to_string();
            entry = Some(a.clone());
            break;
        }
    }
    if entry.is_none() {
        return Ok("Approval id not found.".to_string());
    }
    save_approvals(&mut mem, &approvals)?;
    mem.commit().map_err(|e| e.to_string())?;

    if !execute {
        return Ok("Approved.".to_string());
    }
    let entry = entry.unwrap();
    let result = execute_tool(&entry.tool, entry.args, mv2, false);
    match result {
        Ok(exec) => Ok(exec.output),
        Err(err) => Ok(format!("Execution error: {err}")),
    }
}

fn reject_approval(mv2: &Path, id: &str) -> Result<String, String> {
    let mut mem = open_or_create(mv2).map_err(|e| e.to_string())?;
    let mut approvals = load_approvals(&mut mem);
    let before = approvals.len();
    approvals.retain(|a| a.id != id);
    let updated = approvals.len() != before;
    if updated {
        save_approvals(&mut mem, &approvals)?;
        mem.commit().map_err(|e| e.to_string())?;
        Ok("Rejected.".to_string())
    } else {
        Ok("Approval id not found.".to_string())
    }
}

fn try_handle_approval_chat(mv2: &Path, text: &str) -> Option<String> {
    let cmd = parse_approval_chat_command(text)?;
    let result = match cmd {
        ApprovalChatCommand::Approve(id) => approve_and_maybe_execute(mv2, &id, true),
        ApprovalChatCommand::Reject(id) => reject_approval(mv2, &id),
    };
    Some(result.unwrap_or_else(|e| format!("Approval error: {e}")))
}

fn requires_approval(name: &str, args: &serde_json::Value) -> bool {
    // In bridge mode (env AETHERVAULT_BRIDGE_AUTO_APPROVE=1), auto-approve ALL tools.
    // The user explicitly opted in to full agency — no approval gates.
    let bridge_auto = std::env::var("AETHERVAULT_BRIDGE_AUTO_APPROVE")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    if bridge_auto {
        return false;
    }
    match name {
        "exec" | "email_send" | "email_archive" | "config_set" | "gmail_send" | "gcal_create"
        | "ms_calendar_create" | "trigger_add" | "trigger_remove" | "notify" | "signal_send"
        | "imessage_send" | "memory_export" | "fs_write" | "browser_request" => true,
        "http_request" => {
            let method = args
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_ascii_uppercase();
            method != "GET"
        }
        "scale" => {
            args.get("action").and_then(|v| v.as_str()) == Some("resize")
        }
        _ => false,
    }
}

fn load_triggers(mem: &mut Vault) -> Vec<TriggerEntry> {
    load_config_json(mem, "triggers")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn save_triggers(mem: &mut Vault, triggers: &[TriggerEntry]) -> Result<(), String> {
    let json = serde_json::to_value(triggers).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&json).map_err(|e| e.to_string())?;
    save_config_entry(mem, "triggers", &bytes).map_err(|e| e.to_string())?;
    Ok(())
}

fn allowed_fs_roots(workspace_override: &Option<PathBuf>) -> Vec<PathBuf> {
    if let Some(raw) = env_optional("AETHERVAULT_FS_ROOTS") {
        let roots: Vec<PathBuf> = raw
            .split(':')
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .collect();
        if !roots.is_empty() {
            return roots;
        }
    }
    if let Some(ws) = workspace_override {
        return vec![ws.clone()];
    }
    vec![env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
}

fn resolve_fs_path(path: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let raw = PathBuf::from(path);
    let candidates: Vec<PathBuf> = if raw.is_absolute() {
        vec![raw.clone()]
    } else {
        roots.iter().map(|r| r.join(&raw)).collect()
    };
    for root in roots {
        let root_canon = fs::canonicalize(root).map_err(|e| e.to_string())?;
        for cand in &candidates {
            let cand_canon = if cand.exists() {
                fs::canonicalize(cand).map_err(|e| e.to_string())?
            } else if let Some(parent) = cand.parent() {
                let parent_canon = fs::canonicalize(parent).map_err(|e| e.to_string())?;
                parent_canon.join(cand.file_name().unwrap_or_default())
            } else {
                continue;
            };
            if cand_canon.starts_with(&root_canon) {
                return Ok(cand.clone());
            }
        }
    }
    Err("path outside allowed roots".into())
}

fn parse_log_ts_from_uri(uri: &str) -> Option<i64> {
    let tail = uri.rsplit('/').next()?;
    let ts_str = tail.split('-').next()?;
    ts_str.parse::<i64>().ok()
}

fn tool_score(query_tokens: &[String], name: &str, description: &str) -> i32 {
    let mut score = 0;
    let name_lc = name.to_ascii_lowercase();
    let desc_lc = description.to_ascii_lowercase();
    for token in query_tokens {
        if token.is_empty() {
            continue;
        }
        if name_lc.contains(token) {
            score += 3;
        }
        if desc_lc.contains(token) {
            score += 1;
        }
    }
    if name_lc.contains(&query_tokens.join(" ")) {
        score += 4;
    }
    score
}

fn load_subagents_from_config(config: &CapsuleConfig) -> Vec<SubagentSpec> {
    config
        .agent
        .as_ref()
        .map(|a| a.subagents.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.name.trim().is_empty())
        .collect()
}

fn tool_catalog_map(catalog: &[serde_json::Value]) -> HashMap<String, serde_json::Value> {
    let mut map = HashMap::new();
    for tool in catalog {
        if let Some(name) = tool.get("name").and_then(|v| v.as_str()) {
            map.insert(name.to_string(), tool.clone());
        }
    }
    map
}

fn base_tool_names() -> HashSet<String> {
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
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}

fn tools_from_active(
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

fn refresh_google_token(
    mv2: &Path,
    token: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let refresh_token = token
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or("missing refresh_token")?;
    let client_id = oauth_env("GOOGLE_CLIENT_ID")?;
    let client_secret = oauth_env("GOOGLE_CLIENT_SECRET")?;
    let payload = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &client_id)
        .append_pair("client_secret", &client_secret)
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", refresh_token)
        .finish();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .timeout_write(Duration::from_secs(10))
        .build();
    let resp = agent
        .post("https://oauth2.googleapis.com/token")
        .set("content-type", "application/x-www-form-urlencoded")
        .send_string(&payload);
    let refreshed = match resp {
        Ok(resp) => resp.into_json::<serde_json::Value>()?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            return Err(format!("refresh error {code}: {text}").into());
        }
        Err(err) => return Err(format!("refresh failed: {err}").into()),
    };
    let mut new_token = refreshed.clone();
    if refreshed.get("refresh_token").is_none() {
        if let Some(rt) = token.get("refresh_token") {
            new_token["refresh_token"] = rt.clone();
        }
    }
    let mut mem = open_or_create(mv2)?;
    let bytes = serde_json::to_vec_pretty(&new_token)?;
    let _ = save_config_entry(&mut mem, "oauth.google", &bytes)?;
    Ok(new_token)
}

fn refresh_microsoft_token(
    mv2: &Path,
    token: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let refresh_token = token
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or("missing refresh_token")?;
    let client_id = oauth_env("MICROSOFT_CLIENT_ID")?;
    let client_secret = oauth_env("MICROSOFT_CLIENT_SECRET")?;
    let payload = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &client_id)
        .append_pair("client_secret", &client_secret)
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", refresh_token)
        .append_pair("scope", "offline_access https://graph.microsoft.com/Mail.Read https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite")
        .finish();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .timeout_write(Duration::from_secs(10))
        .build();
    let resp = agent
        .post("https://login.microsoftonline.com/common/oauth2/v2.0/token")
        .set("content-type", "application/x-www-form-urlencoded")
        .send_string(&payload);
    let refreshed = match resp {
        Ok(resp) => resp.into_json::<serde_json::Value>()?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            return Err(format!("refresh error {code}: {text}").into());
        }
        Err(err) => return Err(format!("refresh failed: {err}").into()),
    };
    let mut new_token = refreshed.clone();
    if refreshed.get("refresh_token").is_none() {
        if let Some(rt) = token.get("refresh_token") {
            new_token["refresh_token"] = rt.clone();
        }
    }
    let mut mem = open_or_create(mv2)?;
    let bytes = serde_json::to_vec_pretty(&new_token)?;
    let _ = save_config_entry(&mut mem, "oauth.microsoft", &bytes)?;
    Ok(new_token)
}

fn get_oauth_token(mv2: &Path, provider: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut mem = Vault::open_read_only(mv2)?;
    let key = format!("oauth.{provider}");
    let token = load_config_json(&mut mem, &key).ok_or("missing oauth token")?;
    let access = token.get("access_token").and_then(|v| v.as_str());
    if let Some(access) = access {
        return Ok(access.to_string());
    }
    if provider == "google" {
        let refreshed = refresh_google_token(mv2, &token)?;
        let access = refreshed
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or("missing access_token")?;
        return Ok(access.to_string());
    }
    let refreshed = refresh_microsoft_token(mv2, &token)?;
    let access = refreshed
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("missing access_token")?;
    Ok(access.to_string())
}

// === Knowledge Graph Auto-Injection ===

#[derive(Debug, Deserialize)]
struct KgGraph {
    nodes: Vec<KgNode>,
    #[serde(default)]
    edges: Vec<KgEdge>,
}

#[derive(Debug, Deserialize)]
struct KgNode {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "type", default)]
    node_type: Option<String>,
    #[serde(default)]
    properties: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
struct KgEdge {
    source: String,
    target: String,
    #[serde(default)]
    relation: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    confidence: Option<f64>,
}

fn load_kg_graph(path: &std::path::Path) -> Option<KgGraph> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn find_kg_entities(text: &str, graph: &KgGraph) -> Vec<String> {
    let text_lower = text.to_lowercase();
    let mut matched = Vec::new();
    for node in &graph.nodes {
        let name = node.name.as_deref().unwrap_or(&node.id);
        // Skip very short names to avoid false positives
        if name.len() < 3 { continue; }
        let name_lower = name.to_lowercase();
        // For short names (3-5 chars), require word boundary
        if name.len() <= 5 {
            if let Some(pos) = text_lower.find(&name_lower) {
                let before_ok = pos == 0 || !text_lower.as_bytes()[pos - 1].is_ascii_alphanumeric();
                let after_pos = pos + name_lower.len();
                let after_ok = after_pos >= text_lower.len() || !text_lower.as_bytes()[after_pos].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    matched.push(name.to_string());
                }
            }
        } else if text_lower.contains(&name_lower) {
            matched.push(name.to_string());
        }
    }
    // Cap at 5 entities to avoid prompt bloat
    matched.truncate(5);
    matched
}

fn build_kg_context(entity_names: &[String], graph: &KgGraph) -> String {
    let mut ctx = String::new();
    for name in entity_names {
        let node = graph.nodes.iter().find(|n| {
            n.name.as_deref().unwrap_or(&n.id) == name
        });
        if let Some(node) = node {
            let node_type = node.node_type.as_deref().unwrap_or("unknown");
            ctx.push_str(&format!("## {} ({})\n", name, node_type));
            if let Some(ref props) = node.properties {
                if !props.is_empty() {
                    let props_str: Vec<String> = props.iter()
                        .filter(|(k, _)| *k != "name" && *k != "type")
                        .map(|(k, v)| format!("{}={}", k, v))
                        .collect();
                    if !props_str.is_empty() {
                        ctx.push_str(&format!("Properties: {}\n", props_str.join(", ")));
                    }
                }
            }
            for edge in &graph.edges {
                if edge.source == node.id {
                    let rel = edge.relation.as_deref().unwrap_or("related-to");
                    ctx.push_str(&format!("  -> {} -> {}\n", rel, edge.target));
                }
                if edge.target == node.id {
                    let rel = edge.relation.as_deref().unwrap_or("related-to");
                    ctx.push_str(&format!("  <- {} <- {}\n", rel, edge.source));
                }
            }
            ctx.push('\n');
        }
    }
    ctx
}

fn load_workspace_context(workspace: &Path) -> String {
    let mut sections = Vec::new();
    let soul = workspace.join("SOUL.md");
    let user = workspace.join("USER.md");
    let memory = workspace.join("MEMORY.md");
    if let Some(text) = read_optional_file(&soul) {
        sections.push(format!("# Soul\n{text}"));
    }
    if let Some(text) = read_optional_file(&user) {
        sections.push(format!("# User\n{text}"));
    }
    if let Some(text) = read_optional_file(&memory) {
        sections.push(format!("# Memory\n{text}"));
    }
    let daily = daily_memory_path(workspace);
    if let Some(text) = read_optional_file(&daily) {
        sections.push(format!("# Daily Log\n{text}"));
    }
    sections.join("\n\n")
}

fn bootstrap_workspace(
    mv2: &Path,
    workspace: &Path,
    timezone: Option<String>,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(workspace)?;
    fs::create_dir_all(workspace.join("memory"))?;

    let soul_path = workspace.join("SOUL.md");
    let user_path = workspace.join("USER.md");
    let memory_path = workspace.join("MEMORY.md");
    let daily_path = daily_memory_path(workspace);

    let create_file = |path: &Path, contents: &str| -> Result<(), Box<dyn std::error::Error>> {
        if path.exists() && !force {
            return Err(format!("File already exists: {}", path.display()).into());
        }
        fs::write(path, contents)?;
        Ok(())
    };

    let soul_template = "# Executive Assistant Soul\n\n- Act as a proactive executive assistant.\n- Be concise, direct, and high‑leverage.\n- Prefer action over explanation.\n- Ask for approval before external sends unless policy allows.\n";
    let user_template = "# User Profile\n\n- Name: Sunil Rao\n- Role: Executive\n- Preferences:\n  - Daily Overview at 8:30 AM\n  - Daily Recap at 3:30 PM\n  - Weekly Overview Monday 8:15 AM\n  - Weekly Recap Friday 3:15 PM\n";
    let memory_template =
        "# Long‑term Memory\n\n- Important contacts, preferences, and policies go here.\n";
    let daily_template = "# Daily Log\n\n- Created by bootstrap.\n";

    create_file(&soul_path, soul_template)?;
    create_file(&user_path, user_template)?;
    create_file(&memory_path, memory_template)?;
    create_file(&daily_path, daily_template)?;

    let mut mem = open_or_create(mv2)?;
    let mut config = load_capsule_config(&mut mem).unwrap_or_default();
    let mut agent_cfg = config.agent.unwrap_or_default();
    agent_cfg.workspace = Some(workspace.display().to_string());
    agent_cfg.onboarding_complete = Some(false);
    if timezone.is_some() {
        agent_cfg.timezone = timezone;
    }
    config.agent = Some(agent_cfg);
    let bytes = serde_json::to_vec_pretty(&config)?;
    let _ = save_config_entry(&mut mem, "index", &bytes)?;
    Ok(())
}

fn parse_timezone_offset(value: &str) -> Result<chrono::FixedOffset, Box<dyn std::error::Error>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(chrono::FixedOffset::east_opt(0).unwrap());
    }
    let sign = if trimmed.starts_with('-') { -1 } else { 1 };
    let value = trimmed.trim_start_matches(['+', '-']);
    let mut parts = value.split(':');
    let hours: i32 = parts
        .next()
        .ok_or("timezone")?
        .parse()
        .map_err(|_| "timezone hours")?;
    let minutes: i32 = parts
        .next()
        .unwrap_or("0")
        .parse()
        .map_err(|_| "timezone minutes")?;
    let total = sign * (hours * 3600 + minutes * 60);
    chrono::FixedOffset::east_opt(total).ok_or_else(|| "timezone offset".into())
}

fn resolve_timezone(
    agent_cfg: &AgentConfig,
    override_value: Option<String>,
) -> chrono::FixedOffset {
    let raw = override_value.or_else(|| agent_cfg.timezone.clone());
    raw.and_then(|v| parse_timezone_offset(&v).ok())
        .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap())
}

fn should_run_daily(
    last: &mut Option<chrono::NaiveDate>,
    now: chrono::DateTime<chrono::FixedOffset>,
    hour: u32,
    minute: u32,
) -> bool {
    let date = now.date_naive();
    if now.time().hour() != hour || now.time().minute() != minute {
        return false;
    }
    if last.as_ref().is_some_and(|d| *d == date) {
        return false;
    }
    *last = Some(date);
    true
}

fn should_run_weekly(
    last: &mut Option<chrono::NaiveDate>,
    now: chrono::DateTime<chrono::FixedOffset>,
    weekday: chrono::Weekday,
    hour: u32,
    minute: u32,
) -> bool {
    if now.weekday() != weekday {
        return false;
    }
    should_run_daily(last, now, hour, minute)
}

fn schedule_prompt(kind: &str) -> String {
    match kind {
        "daily_overview" => "Generate the Daily Overview. Sweep inbox (email_list), identify conflicts, and list top priorities. Include \"Needs Your Action\" items.".to_string(),
        "daily_recap" => "Generate the Daily Recap. Summarize what changed in inbox and calendar, actions taken, and pending follow-ups.".to_string(),
        "weekly_overview" => "Generate the Weekly Overview. List top priorities and key meetings. Flag conflicts and follow-ups.".to_string(),
        "weekly_recap" => "Generate the Weekly Recap. Summarize meetings handled, logistics, and outstanding items.".to_string(),
        _ => "Generate an executive summary.".to_string(),
    }
}

fn run_schedule_loop(
    mv2: PathBuf,
    workspace: Option<PathBuf>,
    timezone: Option<String>,
    telegram_token: Option<String>,
    telegram_chat_id: Option<String>,
    model_hook: Option<String>,
    max_steps: usize,
    log: bool,
    log_commit_interval: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    // No external lock — Vault::open_read_only acquires a shared flock() on the .mv2
    // which allows concurrent readers. Writes upgrade to exclusive automatically.
    let mut mem_read = Some(Vault::open_read_only(&mv2)?);
    let config = load_capsule_config(mem_read.as_mut().unwrap()).unwrap_or_default();
    let agent_cfg = config.agent.clone().unwrap_or_default();
    let tz = resolve_timezone(&agent_cfg, timezone);
    let workspace = resolve_workspace(workspace, &agent_cfg);
    let telegram_token = telegram_token
        .or(agent_cfg.telegram_token)
        .or_else(|| env_optional("TELEGRAM_BOT_TOKEN"));
    let telegram_chat_id = telegram_chat_id
        .or(agent_cfg.telegram_chat_id)
        .or_else(|| env_optional("AETHERVAULT_TELEGRAM_CHAT_ID"));

    let agent_config = build_bridge_agent_config(
        mv2.clone(),
        model_hook,
        None,
        false,
        None,
        8,
        12_000,
        max_steps,
        log,
        log_commit_interval,
    )?;

    let mut last_daily_overview = None;
    let mut last_daily_recap = None;
    let mut last_weekly_overview = None;
    let mut last_weekly_recap = None;

    loop {
        let now = chrono::Utc::now().with_timezone(&tz);
        let mut tasks = Vec::new();
        if should_run_daily(&mut last_daily_overview, now, 8, 30) {
            tasks.push("daily_overview");
        }
        if should_run_daily(&mut last_daily_recap, now, 15, 30) {
            tasks.push("daily_recap");
        }
        if should_run_weekly(&mut last_weekly_overview, now, chrono::Weekday::Mon, 8, 15) {
            tasks.push("weekly_overview");
        }
        if should_run_weekly(&mut last_weekly_recap, now, chrono::Weekday::Fri, 15, 15) {
            tasks.push("weekly_recap");
        }

        for task in tasks {
            let mut prompt = schedule_prompt(task);
            if let Some(ws) = &workspace {
                prompt.push_str(&format!("\n\nWorkspace: {}", ws.display()));
            }
            let session = format!("schedule:{task}");
            let result = run_agent_for_bridge(&agent_config, &prompt, session, None, None, None);
            if let Ok(output) = result {
                if let Some(text) = output.final_text {
                    if let (Some(token), Some(chat_id)) =
                        (telegram_token.as_ref(), telegram_chat_id.as_ref())
                    {
                        let agent = ureq::AgentBuilder::new()
                            .timeout_connect(Duration::from_secs(10))
                            .timeout_write(Duration::from_secs(10))
                            .timeout_read(Duration::from_secs(20))
                            .build();
                        let base_url = match std::env::var("TELEGRAM_API_BASE") {
        Ok(base) => format!("{base}/bot{token}"),
        Err(_) => format!("https://api.telegram.org/bot{token}"),
    };
                        if let Ok(chat_id) = chat_id.parse::<i64>() {
                            let _ = telegram_send_message(&agent, &base_url, chat_id, &text);
                        }
                    }
                }
            }
        }

        thread::sleep(Duration::from_secs(30));
    }
}

fn run_watch_loop(
    mv2: PathBuf,
    workspace: Option<PathBuf>,
    timezone: Option<String>,
    model_hook: Option<String>,
    max_steps: usize,
    log: bool,
    log_commit_interval: usize,
    poll_seconds: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut mem_read = Some(Vault::open_read_only(&mv2)?);
    let config = load_capsule_config(mem_read.as_mut().unwrap()).unwrap_or_default();
    let agent_cfg = config.agent.clone().unwrap_or_default();
    let tz = resolve_timezone(&agent_cfg, timezone);
    let workspace = resolve_workspace(workspace, &agent_cfg);
    let agent_config = build_bridge_agent_config(
        mv2.clone(),
        model_hook,
        None,
        false,
        None,
        8,
        12_000,
        max_steps,
        log,
        log_commit_interval,
    )?;

    loop {
        let now = chrono::Utc::now().with_timezone(&tz);
        let mut mem = open_or_create(&mv2)?;
        let mut triggers = load_triggers(&mut mem);
        let mut updated = false;

        for trigger in triggers.iter_mut() {
            if !trigger.enabled {
                continue;
            }
            match trigger.kind.as_str() {
                "email" => {
                    let query = match &trigger.query {
                        Some(q) if !q.trim().is_empty() => q.clone(),
                        _ => continue,
                    };
                    let token = match get_oauth_token(&mv2, "google") {
                        Ok(token) => token,
                        Err(_) => continue,
                    };
                    let agent = ureq::AgentBuilder::new()
                        .timeout_connect(Duration::from_secs(10))
                        .timeout_read(Duration::from_secs(20))
                        .build();
                    let mut url =
                        "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults=1"
                            .to_string();
                    url.push_str("&q=");
                    url.push_str(&urlencoding::encode(&query));
                    let resp = agent
                        .get(&url)
                        .set("authorization", &format!("Bearer {}", token))
                        .call();
                    let payload = match resp {
                        Ok(resp) => resp.into_json::<serde_json::Value>().unwrap_or_default(),
                        Err(_) => continue,
                    };
                    let id = payload
                        .get("messages")
                        .and_then(|m| m.as_array())
                        .and_then(|arr| arr.get(0))
                        .and_then(|m| m.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    if let Some(id) = id {
                        if trigger.last_seen.as_deref() != Some(&id) {
                            trigger.last_seen = Some(id.clone());
                            trigger.last_fired = Some(now.to_rfc3339());
                            updated = true;
                            let mut prompt = trigger.prompt.clone().unwrap_or_else(|| {
                                "New email received. Review and take action.".to_string()
                            });
                            prompt.push_str(&format!(
                                "\n\nQuery: {query}\nMessage ID: {id}\nUse gmail_read to inspect."
                            ));
                            if let Some(ws) = &workspace {
                                prompt.push_str(&format!("\nWorkspace: {}", ws.display()));
                            }
                            let session = format!("trigger:email:{}", trigger.id);
                            let _ =
                                run_agent_for_bridge(&agent_config, &prompt, session, None, None, None);
                        }
                    }
                }
                "calendar_free" => {
                    let start = match &trigger.start {
                        Some(s) => s.clone(),
                        None => continue,
                    };
                    let end = match &trigger.end {
                        Some(e) => e.clone(),
                        None => continue,
                    };
                    let token = match get_oauth_token(&mv2, "google") {
                        Ok(token) => token,
                        Err(_) => continue,
                    };
                    let agent = ureq::AgentBuilder::new()
                        .timeout_connect(Duration::from_secs(10))
                        .timeout_read(Duration::from_secs(20))
                        .build();
                    let url = format!(
                        "https://www.googleapis.com/calendar/v3/calendars/primary/events?timeMin={}&timeMax={}&maxResults=1&singleEvents=true",
                        urlencoding::encode(&start),
                        urlencoding::encode(&end)
                    );
                    let resp = agent
                        .get(&url)
                        .set("authorization", &format!("Bearer {}", token))
                        .call();
                    let payload = match resp {
                        Ok(resp) => resp.into_json::<serde_json::Value>().unwrap_or_default(),
                        Err(_) => continue,
                    };
                    let has_events = payload
                        .get("items")
                        .and_then(|v| v.as_array())
                        .map(|arr| !arr.is_empty())
                        .unwrap_or(false);
                    if !has_events {
                        let fired_today = trigger
                            .last_fired
                            .as_deref()
                            .and_then(|v| v.split('T').next())
                            .map(|d| d == now.date_naive().to_string())
                            .unwrap_or(false);
                        if !fired_today {
                            trigger.last_fired = Some(now.to_rfc3339());
                            updated = true;
                            let mut prompt = trigger.prompt.clone().unwrap_or_else(|| {
                                "Calendar is free in the requested window. Schedule task."
                                    .to_string()
                            });
                            prompt.push_str(&format!(
                                "\n\nWindow: {start} → {end}\nNo events detected."
                            ));
                            if let Some(ws) = &workspace {
                                prompt.push_str(&format!("\nWorkspace: {}", ws.display()));
                            }
                            let session = format!("trigger:calendar:{}", trigger.id);
                            let _ =
                                run_agent_for_bridge(&agent_config, &prompt, session, None, None, None);
                        }
                    }
                }
                _ => {}
            }
        }

        if updated {
            let _ = save_triggers(&mut mem, &triggers);
        }
        thread::sleep(Duration::from_secs(poll_seconds));
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
                // Check for embedded image markers: [AV_IMAGE:media_type:base64data]
                if content.contains("[AV_IMAGE:") {
                    let mut blocks: Vec<serde_json::Value> = Vec::new();
                    let mut remaining = content.as_str();
                    while let Some(start) = remaining.find("[AV_IMAGE:") {
                        // Text before the marker
                        let before = &remaining[..start];
                        if !before.trim().is_empty() {
                            blocks.push(serde_json::json!({"type": "text", "text": before.trim()}));
                        }
                        let after_prefix = &remaining[start + 10..]; // skip "[AV_IMAGE:"
                        if let Some(end) = after_prefix.find(']') {
                            let marker_content = &after_prefix[..end];
                            // marker_content = "media_type:base64data"
                            if let Some(colon) = marker_content.find(':') {
                                let media_type = &marker_content[..colon];
                                let b64_data = &marker_content[colon + 1..];
                                blocks.push(serde_json::json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
                                        "data": b64_data
                                    }
                                }));
                            }
                            remaining = &after_prefix[end + 1..];
                        } else {
                            remaining = after_prefix;
                            break;
                        }
                    }
                    if !remaining.trim().is_empty() {
                        blocks.push(serde_json::json!({"type": "text", "text": remaining.trim()}));
                    }
                    if blocks.is_empty() {
                        blocks.push(serde_json::json!({"type": "text", "text": ""}));
                    }
                    out.push(serde_json::json!({"role": "user", "content": blocks}));
                } else {
                    out.push(serde_json::json!({
                        "role": "user",
                        "content": [{"type": "text", "text": content}]
                    }));
                }
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

fn to_anthropic_tools(
    tools: &[serde_json::Value],
    cache_control: Option<serde_json::Value>,
) -> Vec<serde_json::Value> {
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
        if let Some(cache) = cache_control.clone() {
            entry.insert("cache_control".to_string(), cache);
        }
        out.push(serde_json::Value::Object(entry));
    }
    out
}

fn parse_claude_response(
    payload: &serde_json::Value,
) -> Result<AgentHookResponse, Box<dyn std::error::Error>> {
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
                let args = block
                    .get("input")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
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

fn call_claude(
    request: &AgentHookRequest,
) -> Result<AgentHookResponse, Box<dyn std::error::Error>> {
    let api_key = env_required("ANTHROPIC_API_KEY")?;
    let model = env_required("ANTHROPIC_MODEL")?;
    let base_url = env_optional("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string());
    let max_tokens = env_u64("ANTHROPIC_MAX_TOKENS", 8192)?;
    let temperature = env_optional("ANTHROPIC_TEMPERATURE")
        .map(|v| v.parse::<f64>())
        .transpose()
        .map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid ANTHROPIC_TEMPERATURE")
        })?;
    let top_p = env_optional("ANTHROPIC_TOP_P")
        .map(|v| v.parse::<f64>())
        .transpose()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid ANTHROPIC_TOP_P"))?;
    let timeout = env_f64("ANTHROPIC_TIMEOUT", 60.0)?;
    let max_retries = env_usize("ANTHROPIC_MAX_RETRIES", 2)?;
    let retry_base = env_f64("ANTHROPIC_RETRY_BASE", 0.5)?;
    let retry_max = env_f64("ANTHROPIC_RETRY_MAX", 4.0)?;
    let version = env_optional("ANTHROPIC_VERSION").unwrap_or_else(|| "2023-06-01".to_string());
    let beta = env_optional("ANTHROPIC_BETA");
    let token_efficient = env_bool("ANTHROPIC_TOKEN_EFFICIENT", false);
    let mut beta_values: Vec<String> = Vec::new();
    if let Some(b) = beta {
        for item in b.split(',') {
            let trimmed = item.trim();
            if !trimmed.is_empty() {
                beta_values.push(trimmed.to_string());
            }
        }
    }
    if token_efficient {
        beta_values.push("token-efficient-tools-2025-02-19".to_string());
    }

    let system = merge_system_messages(&request.messages);
    let use_prompt_cache = env_bool("ANTHROPIC_PROMPT_CACHE", false);
    let cache_ttl = env_optional("ANTHROPIC_PROMPT_CACHE_TTL");
    let cache_control = if use_prompt_cache {
        let mut obj = serde_json::Map::new();
        obj.insert("type".to_string(), serde_json::json!("ephemeral"));
        if let Some(ttl) = cache_ttl {
            if !ttl.trim().is_empty() {
                obj.insert("ttl".to_string(), serde_json::json!(ttl));
            }
        }
        Some(serde_json::Value::Object(obj))
    } else {
        None
    };
    let mut payload = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": to_anthropic_messages(&request.messages),
    });
    if !system.is_empty() {
        if let Some(cache) = cache_control.clone() {
            payload["system"] = serde_json::json!([{
                "type": "text",
                "text": system,
                "cache_control": cache
            }]);
        } else {
            payload["system"] = serde_json::json!(system);
        }
    }
    let tools = to_anthropic_tools(&request.tools, cache_control.clone());
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
        if !beta_values.is_empty() {
            request = request.set("anthropic-beta", &beta_values.join(","));
        }

        let response = request.send_json(payload.clone());
        match response {
            Ok(resp) => {
                body = Some(resp.into_string()?);
                break;
            }
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = parse_retry_after(&resp);
                let text = resp.into_string().unwrap_or_default();
                if attempt < max_retries && retryable(code) {
                    let mut delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                    if let Some(retry_after) = retry_after {
                        delay = delay.max(retry_after);
                    }
                    let jitter = jitter_ratio() * 0.2;
                    delay *= 1.0 + jitter;
                    thread::sleep(Duration::from_secs_f64(delay));
                    continue;
                }
                return Err(format!("Anthropic API error: {code} {text}").into());
            }
            Err(ureq::Error::Transport(err)) => {
                if attempt < max_retries {
                    let mut delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                    let jitter = jitter_ratio() * 0.2;
                    delay *= 1.0 + jitter;
                    thread::sleep(Duration::from_secs_f64(delay));
                    continue;
                }
                return Err(format!("Anthropic API request failed: {err}").into());
            }
        }
    }

    // If primary model failed, try fallback model
    if body.is_none() {
        if let Ok(fallback_model) = std::env::var("ANTHROPIC_FALLBACK_MODEL") {
            eprintln!("Primary model failed, trying fallback: {fallback_model}");
            payload["model"] = serde_json::json!(fallback_model);
            for attempt in 0..=1 {
                let mut request = agent
                    .post(&base_url)
                    .set("content-type", "application/json")
                    .set("x-api-key", &api_key)
                    .set("anthropic-version", &version);
                if !beta_values.is_empty() {
                    request = request.set("anthropic-beta", &beta_values.join(","));
                }
                match request.send_json(payload.clone()) {
                    Ok(resp) => {
                        body = Some(resp.into_string()?);
                        break;
                    }
                    Err(ureq::Error::Status(code, resp)) => {
                        let text = resp.into_string().unwrap_or_default();
                        if attempt == 1 {
                            return Err(format!("Fallback model also failed: {code} {text}").into());
                        }
                        thread::sleep(Duration::from_secs(1));
                    }
                    Err(ureq::Error::Transport(err)) => {
                        if attempt == 1 {
                            return Err(format!("Fallback model transport error: {err}").into());
                        }
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        }
    }

    // If both primary and fallback model failed, try Vertex proxy as last resort.
    if body.is_none() {
        let vertex_url = env_optional("VERTEX_FALLBACK_URL")
            .unwrap_or_else(|| "http://localhost:11436/v1/messages".to_string());
        let vertex_enabled = env_optional("VERTEX_FALLBACK").unwrap_or_else(|| "1".to_string()) == "1";
        if vertex_enabled {
            eprintln!("Anthropic direct failed, falling back to Vertex proxy at {vertex_url}");
            payload["model"] = serde_json::json!(model);
            let vertex_key = env_optional("VERTEX_API_KEY").unwrap_or_else(|| api_key.clone());
            for attempt in 0..=max_retries {
                let mut request = agent
                    .post(&vertex_url)
                    .set("content-type", "application/json")
                    .set("x-api-key", &vertex_key)
                    .set("anthropic-version", &version);
                if !beta_values.is_empty() {
                    request = request.set("anthropic-beta", &beta_values.join(","));
                }
                match request.send_json(payload.clone()) {
                    Ok(resp) => {
                        body = Some(resp.into_string()?);
                        break;
                    }
                    Err(ureq::Error::Status(code, resp)) => {
                        let text = resp.into_string().unwrap_or_default();
                        if attempt == max_retries {
                            return Err(format!("Vertex fallback also failed: {code} {text}").into());
                        }
                        let delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                        thread::sleep(Duration::from_secs_f64(delay));
                    }
                    Err(ureq::Error::Transport(err)) => {
                        if attempt == max_retries {
                            return Err(format!("Vertex fallback transport error: {err}").into());
                        }
                        let delay = (retry_base * 2.0_f64.powi(attempt as i32)).min(retry_max);
                        thread::sleep(Duration::from_secs_f64(delay));
                    }
                }
            }
        }
    }

    let body = body.ok_or("All API endpoints failed (Anthropic direct + Vertex fallback)")?;
    let payload: serde_json::Value = serde_json::from_str(&body)?;
    parse_claude_response(&payload)
}

fn run_claude_hook() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    if input.trim().is_empty() {
        return Err("Claude hook received empty input".into());
    }
    let req: AgentHookRequest = serde_json::from_str(&input)?;
    let response = call_claude(&req)?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn call_agent_hook(hook: &HookSpec, request: &AgentHookRequest) -> Result<AgentMessage, String> {
    let is_builtin = match &hook.command {
        CommandSpec::String(cmd) => {
            let cmd = cmd.trim().to_ascii_lowercase();
            cmd == "builtin:claude" || cmd == "claude"
        }
        CommandSpec::Array(items) => items
            .first()
            .map(|cmd| cmd.trim().to_ascii_lowercase())
            .map(|cmd| cmd == "builtin:claude" || cmd == "claude")
            .unwrap_or(false),
    };
    if is_builtin {
        return call_claude(request)
            .map(|resp| resp.message)
            .map_err(|e| format!("claude hook: {e}"));
    }

    let cmd = command_spec_to_vec(&hook.command);
    let timeout = hook.timeout_ms.unwrap_or(300000);
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
    let system_text = if let Some(path) = system_file {
        Some(fs::read_to_string(path)?)
    } else {
        system
    };

    let prompt_for_session = prompt_text.clone();
    let session_for_save = session.clone();
    let output = run_agent_with_prompt(
        mv2,
        prompt_text,
        session,
        model_hook,
        system_text,
        no_memory,
        context_query,
        context_results,
        context_max_bytes,
        max_steps,
        log_commit_interval,
        log,
        None,
    )?;

    // Save session turns for CLI agent continuity (mirrors Telegram bridge behaviour)
    if let Some(ref sess_id) = session_for_save {
        let mut turns = load_session_turns(sess_id, 8);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        turns.push(SessionTurn {
            role: "user".to_string(),
            content: prompt_for_session,
            timestamp: now,
        });
        if let Some(ref reply) = output.final_text {
            turns.push(SessionTurn {
                role: "assistant".to_string(),
                content: reply.clone(),
                timestamp: now,
            });
        }
        save_session_turns(sess_id, &turns, 8);
    }

    if json {
        let payload = AgentSession {
            session: output.session,
            context: output.context,
            messages: output.messages,
            tool_results: output.tool_results,
        };
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if let Some(text) = output.final_text {
        println!("{text}");
    }
    Ok(())
}

fn default_system_prompt() -> String {
    [
        "You are AetherVault, a high-performance personal AI assistant.",
        "Be proactive, concrete, and concise. Prefer action over discussion.",
        "",
        "## Action Protocol",
        "For routine actions (reading, searching): execute immediately, summarize after.",
        "For significant actions (writing, creating): state your plan in one sentence, then execute.",
        "For complex multi-step tasks: outline 2-3 bullet points, then execute step by step.",
        "For irreversible actions (deleting, sending, deploying): describe consequences, wait for confirmation.",
        "",
        "## Tools",
        "Tools load dynamically — call tool_search when you need a capability not currently available.",
        "When multiple independent tool calls are needed, request them all at once for parallel execution.",
        "Sensitive actions require approval. If a tool returns `approval required: <id>`, ask the user to approve or reject.",
        "Use subagent_invoke or subagent_batch for specialist work when it improves quality or speed.",
        "",
        "## Error Recovery",
        "When a tool fails, use reflect to record what went wrong, then retry differently.",
        "Never retry the same failing call. If stuck after 2 attempts, ask the user for guidance.",
        "",
        "## Critical Reminders",
        "Investigate before answering — search memory before making claims.",
        "Match the user's energy. Be concise when they're concise, detailed when they need detail.",
        "For irreversible actions, always confirm first.",
    ]
    .join("\n")
}

/// Estimate token count for messages (rough: chars / 4).
fn estimate_tokens(messages: &[AgentMessage]) -> usize {
    messages.iter().map(|m| {
        m.content.as_ref().map(|c| c.len()).unwrap_or(0) / 4
    }).sum()
}

/// Compact messages when context is getting large.
/// Keeps the system message (index 0) and last `keep_recent` messages verbatim.
/// Summarizes everything in between into a single system message.
fn compact_messages(
    messages: &mut Vec<AgentMessage>,
    hook: &HookSpec,
    keep_recent: usize,
) -> Result<(), String> {
    if messages.len() <= keep_recent + 2 {
        return Ok(()); // Nothing to compact
    }
    let system_msg = messages[0].clone();
    let to_summarize: Vec<_> = messages[1..messages.len() - keep_recent].to_vec();
    let recent: Vec<_> = messages[messages.len() - keep_recent..].to_vec();

    // Build a summary request
    let summary_text: String = to_summarize.iter().filter_map(|m| {
        let role = &m.role;
        m.content.as_ref().map(|c| {
            let preview = if c.len() > 300 { &c[..300] } else { c.as_str() };
            format!("[{role}] {preview}")
        })
    }).collect::<Vec<_>>().join("\n");

    let summary_prompt = format!(
        "Summarize this conversation concisely. Preserve: key decisions, file paths, unresolved issues, user preferences. Discard: verbose tool outputs, redundant context.\n\n{summary_text}"
    );

    let summary_request = AgentHookRequest {
        messages: vec![
            AgentMessage {
                role: "system".to_string(),
                content: Some("You are a conversation summarizer. Output only the summary, nothing else. Be concise — use bullet points.".to_string()),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
            },
            AgentMessage {
                role: "user".to_string(),
                content: Some(summary_prompt),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
            },
        ],
        tools: Vec::new(),
        session: None,
    };

    let summary_response = call_agent_hook(hook, &summary_request)?;
    let summary = summary_response.content.unwrap_or_else(|| "(compaction failed)".to_string());

    // Rebuild messages: system + compaction notice + recent
    *messages = Vec::new();
    messages.push(system_msg);
    messages.push(AgentMessage {
        role: "user".to_string(),
        content: Some(format!("[Context compacted. Summary of prior conversation:]\n{summary}")),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });
    messages.push(AgentMessage {
        role: "assistant".to_string(),
        content: Some("Understood. I have the context from the summary above. Continuing.".to_string()),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });
    messages.extend(recent);
    Ok(())
}

fn run_agent_with_prompt(
    mv2: PathBuf,
    prompt_text: String,
    session: Option<String>,
    model_hook: Option<String>,
    system_override: Option<String>,
    no_memory: bool,
    context_query: Option<String>,
    context_results: usize,
    context_max_bytes: usize,
    max_steps: usize,
    log_commit_interval: usize,
    log: bool,
    progress: Option<Arc<Mutex<AgentProgress>>>,
) -> Result<AgentRunOutput, Box<dyn std::error::Error>> {
    if prompt_text.trim().is_empty() {
        return Err("agent prompt is empty".into());
    }

    // No external lock — Vault handles shared/exclusive internally.
    let mut mem_read = Some(Vault::open_read_only(&mv2)?);
    let config = load_capsule_config(mem_read.as_mut().unwrap()).unwrap_or_default();
    let agent_cfg = config.agent.clone().unwrap_or_default();
    let hook_cfg = config.hooks.clone().unwrap_or_default();
    let model_spec = resolve_hook_spec(
        model_hook,
        300000,
        agent_cfg.model_hook.clone().or(hook_cfg.llm),
        None,
    )
    .ok_or("agent requires --model-hook or config.agent.model_hook or config.hooks.llm")?;

    let mut system_prompt = if let Some(system) = system_override {
        system
    } else if let Some(system) = agent_cfg.system.clone() {
        system
    } else {
        // Load from workspace SYSTEM.md, fall back to inline default
        let system_path = resolve_workspace(None, &agent_cfg)
            .map(|ws| ws.join("SYSTEM.md"))
            .filter(|p| p.exists());
        if let Some(path) = system_path {
            fs::read_to_string(&path).unwrap_or_else(|_| default_system_prompt())
        } else {
            default_system_prompt()
        }
    };

    if agent_cfg.onboarding_complete == Some(false) {
        system_prompt.push_str(
            "\n\n# Onboarding\nYou are in onboarding mode. Guide the user to connect email, calendar, and messaging integrations. Verify tool access. When complete, append a note to MEMORY.md and ask the user to run `aethervault config set --key index` to set `agent.onboarding_complete=true`.",
        );
    }

    if let Some(workspace) = resolve_workspace(None, &agent_cfg) {
        if workspace.exists() {
            let workspace_context = load_workspace_context(&workspace);
            if !workspace_context.trim().is_empty() {
                system_prompt.push_str("\n\n# Workspace Context\n");
                system_prompt.push_str(&workspace_context);
            }
        }
    }

    if let Some(global_context) = config.context {
        if !global_context.trim().is_empty() {
            system_prompt.push_str("\n\n# Global Context\n");
            system_prompt.push_str(&global_context);
        }
    }

    // --- KV-Cache Breakpoint ---
    // Everything above is stable within a session (identity, tools, workspace, global context).
    // Everything below is dynamic per-turn (memory, KG, session, reminders).

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
            collection: session.as_ref().map(|s| format!("agent-log/{s}")),
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


    // Release the read handle after initialization. Tool calls re-open on demand via
    // with_read_mem/with_write_mem. This prevents a long-lived shared flock from blocking
    // sibling subagents that need exclusive access for writes.
    mem_read = None;

    // Knowledge Graph entity auto-injection
    let kg_path = std::path::PathBuf::from("/root/.aethervault/data/knowledge-graph.json");
    if kg_path.exists() {
        if let Some(kg) = load_kg_graph(&kg_path) {
            let matched = find_kg_entities(&prompt_text, &kg);
            if !matched.is_empty() {
                let kg_context = build_kg_context(&matched, &kg);
                if !kg_context.trim().is_empty() {
                    system_prompt.push_str("\n\n# Knowledge Graph Context\n");
                    system_prompt.push_str("(Automatically matched entities from the knowledge graph)\n\n");
                    system_prompt.push_str(&kg_context);
                }
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

    // Insert session history as proper user/assistant messages (not in system prompt)
    if let Some(ref sess_id) = session {
        let session_turns = load_session_turns(sess_id, 8);
        for turn in &session_turns {
            messages.push(AgentMessage {
                role: turn.role.clone(),
                content: Some(if turn.content.len() > 500 {
                    format!("{}...", &turn.content[..500])
                } else {
                    turn.content.clone()
                }),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
            });
        }
    }

    messages.push(AgentMessage {
        role: "user".to_string(),
        content: Some(prompt_text.clone()),
        tool_calls: Vec::new(),
        name: None,
        tool_call_id: None,
        is_error: None,
    });

    let tool_catalog = tool_definitions_json();
    let tool_map = tool_catalog_map(&tool_catalog);
    let mut active_tools = base_tool_names();
    let mut tools = tools_from_active(&tool_map, &active_tools);
    let mut tool_results: Vec<AgentToolResult> = Vec::new();
    let should_log = log || agent_cfg.log.unwrap_or(false);
    let mut final_text = None;

    // Buffer log entries in memory and batch-flush them. This avoids holding an exclusive
    // lock on the capsule for the entire agent session, which would block all concurrent
    // subagents. The Vault is opened exclusively only during flush (milliseconds), then
    // immediately dropped to release the lock.
    let mut log_buffer: Vec<AgentLogEntry> = Vec::new();
    let mut mem_write: Option<Vault> = None;

    let flush_log_buffer = |mv2: &Path, buffer: &mut Vec<AgentLogEntry>, mem_read: &mut Option<Vault>| {
        if buffer.is_empty() {
            return Ok(()) as Result<(), Box<dyn std::error::Error>>;
        }
        // Drop any read handle so we can acquire exclusive.
        *mem_read = None;
        let mut mem = Vault::open(mv2)?;
        for entry in buffer.drain(..) {
            let _ = append_agent_log_uncommitted(&mut mem, &entry);
        }
        mem.commit()?;
        // `mem` is dropped here, releasing the exclusive lock immediately.
        Ok(())
    };

    if should_log {
        let entry = AgentLogEntry {
            session: session.clone(),
            role: "user".to_string(),
            text: prompt_text.clone(),
            meta: None,
            ts_utc: Some(Utc::now().timestamp()),
        };
        log_buffer.push(entry);
        if log_buffer.len() >= effective_log_commit_interval {
            flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
        }
    }

    let mut completed = false;
    for step in 0..effective_max_steps {
        // Update progress: thinking phase
        if let Some(ref prog) = progress {
            if let Ok(mut p) = prog.lock() {
                p.step = step;
                p.phase = "thinking".to_string();
            }
        }

        // Auto-compact when context exceeds threshold (80% of ~128K token window)
        let token_estimate = estimate_tokens(&messages);
        if token_estimate > 100_000 {
            eprintln!("[harness] context at ~{token_estimate} tokens, compacting...");
            if let Err(e) = compact_messages(&mut messages, &model_spec, 6) {
                eprintln!("[harness] compaction failed: {e}");
            }
        }

        let request = AgentHookRequest {
            messages: messages.clone(),
            tools: tools.clone(),
            session: session.clone(),
        };
        let message = call_agent_hook(&model_spec, &request)?;
        if let Some(content) = message.content.clone() {
            final_text = Some(content.clone());
            // Update progress: text preview
            if let Some(ref prog) = progress {
                if let Ok(mut p) = prog.lock() {
                    p.text_preview = Some(content.chars().take(100).collect());
                }
            }
            if should_log {
                let entry = AgentLogEntry {
                    session: session.clone(),
                    role: "assistant".to_string(),
                    text: content,
                    meta: None,
                    ts_utc: Some(Utc::now().timestamp()),
                };
                log_buffer.push(entry);
                if log_buffer.len() >= effective_log_commit_interval {
                    flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
                }
            }
        }
        let tool_calls = message.tool_calls.clone();
        messages.push(message);
        if tool_calls.is_empty() {
            completed = true;
            break;
        }

        // Validate all tool calls before execution
        for call in &tool_calls {
            if call.id.trim().is_empty() {
                return Err("tool call is missing an id".into());
            }
            if call.name.trim().is_empty() {
                return Err("tool call is missing a name".into());
            }
        }

        let max_tool_output = 8000; // chars (~2000 tokens)

        // Update progress: tool execution phase
        if let Some(ref prog) = progress {
            if let Ok(mut p) = prog.lock() {
                let names: Vec<&str> = tool_calls.iter().map(|c| c.name.as_str()).collect();
                p.phase = format!("tool:{}", names.join(","));
            }
        }

        if tool_calls.len() == 1 {
            // Single tool call — execute directly (no thread overhead)
            let call = &tool_calls[0];
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

            // Truncate large tool outputs to prevent context blowout
            let result = if result.output.len() > max_tool_output && !result.is_error {
                ToolExecution {
                    output: format!(
                        "{}\n\n[Output truncated: {} chars total, showing first {}. Use a more specific query for full results.]",
                        &result.output[..max_tool_output],
                        result.output.len(),
                        max_tool_output
                    ),
                    details: result.details,
                    is_error: result.is_error,
                }
            } else {
                result
            };

            let tool_content =
                format_tool_message_content(&call.name, &result.output, &result.details);
            tool_results.push(AgentToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                output: result.output.clone(),
                details: result.details.clone(),
                is_error: result.is_error,
            });
            messages.push(AgentMessage {
                role: "tool".to_string(),
                content: if tool_content.is_empty() { None } else { Some(tool_content) },
                tool_calls: Vec::new(),
                name: Some(call.name.clone()),
                tool_call_id: Some(call.id.clone()),
                is_error: Some(result.is_error),
            });

            if call.name == "tool_search" && !result.is_error {
                if let Some(results_arr) = result.details.get("results").and_then(|v| v.as_array()) {
                    let mut changed = false;
                    for item in results_arr {
                        if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                            if active_tools.insert(name.to_string()) {
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        tools = tools_from_active(&tool_map, &active_tools);
                    }
                }
            }

            if should_log {
                log_buffer.push(AgentLogEntry {
                    session: session.clone(),
                    role: "tool".to_string(),
                    text: result.output,
                    meta: Some(result.details),
                    ts_utc: Some(Utc::now().timestamp()),
                });
                if log_buffer.len() >= effective_log_commit_interval {
                    flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
                }
            }

            if matches!(call.name.as_str(), "put" | "log" | "feedback") && !result.is_error {
                flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
            }
        } else {
            // Multiple tool calls — execute in parallel
            let results: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = tool_calls.iter().map(|call| {
                    let mv2 = &mv2;
                    s.spawn(move || {
                        let mut local_mem_read: Option<Vault> = None;
                        let mut local_mem_write: Option<Vault> = None;
                        let result = match execute_tool_with_handles(
                            &call.name,
                            call.args.clone(),
                            mv2,
                            false,
                            &mut local_mem_read,
                            &mut local_mem_write,
                        ) {
                            Ok(r) => r,
                            Err(err) => ToolExecution {
                                output: format!("Tool error: {err}"),
                                details: serde_json::json!({ "error": err }),
                                is_error: true,
                            },
                        };
                        (call, result)
                    })
                }).collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });

            for (call, result) in results {
                // Truncate large tool outputs to prevent context blowout
                let result = if result.output.len() > max_tool_output && !result.is_error {
                    ToolExecution {
                        output: format!(
                            "{}\n\n[Output truncated: {} chars total, showing first {}.]",
                            &result.output[..max_tool_output],
                            result.output.len(),
                            max_tool_output
                        ),
                        details: result.details,
                        is_error: result.is_error,
                    }
                } else {
                    result
                };

                let tool_content = format_tool_message_content(&call.name, &result.output, &result.details);
                tool_results.push(AgentToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    output: result.output.clone(),
                    details: result.details.clone(),
                    is_error: result.is_error,
                });
                messages.push(AgentMessage {
                    role: "tool".to_string(),
                    content: if tool_content.is_empty() { None } else { Some(tool_content) },
                    tool_calls: Vec::new(),
                    name: Some(call.name.clone()),
                    tool_call_id: Some(call.id.clone()),
                    is_error: Some(result.is_error),
                });

                if call.name == "tool_search" && !result.is_error {
                    if let Some(results_arr) = result.details.get("results").and_then(|v| v.as_array()) {
                        let mut changed = false;
                        for item in results_arr {
                            if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                                if active_tools.insert(name.to_string()) {
                                    changed = true;
                                }
                            }
                        }
                        if changed {
                            tools = tools_from_active(&tool_map, &active_tools);
                        }
                    }
                }

                if should_log {
                    log_buffer.push(AgentLogEntry {
                        session: session.clone(),
                        role: "tool".to_string(),
                        text: result.output,
                        meta: Some(result.details),
                        ts_utc: Some(Utc::now().timestamp()),
                    });
                    if log_buffer.len() >= effective_log_commit_interval {
                        flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
                    }
                }

                if matches!(call.name.as_str(), "put" | "log" | "feedback") && !result.is_error {
                    flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
                }
            }
        }

        // Mid-loop system reminders
        let step_num = messages.iter().filter(|m| m.role == "assistant").count();
        let token_est = estimate_tokens(&messages);
        let mut reminders = Vec::new();

        if token_est > 80_000 {
            reminders.push("Context is large. Be concise in your responses and tool calls.");
        }
        if step_num > effective_max_steps * 3 / 4 {
            reminders.push("You are approaching the step limit. Focus on completing the current task.");
        }
        // Check if the last tool result was an error
        if messages.last().map(|m| m.is_error == Some(true)).unwrap_or(false) {
            reminders.push("The previous tool call failed. Use reflect to analyze what went wrong, then try a different approach. Do not retry the same call.");
        }

        if !reminders.is_empty() {
            messages.push(AgentMessage {
                role: "user".to_string(),
                content: Some(format!("[System Reminder] {}", reminders.join(" "))),
                tool_calls: Vec::new(),
                name: None,
                tool_call_id: None,
                is_error: None,
            });
        }
    }

    if should_log {
        flush_log_buffer(&mv2, &mut log_buffer, &mut mem_read)?;
    }

    if !completed {
        // Extract the last assistant message for context on what was in progress
        let last_action = messages.iter().rev()
            .find(|m| m.role == "assistant")
            .and_then(|m| m.content.as_ref())
            .map(|c| c.chars().take(200).collect::<String>())
            .unwrap_or_else(|| "(no context available)".to_string());
        return Err(format!(
            "Agent used all {effective_max_steps} steps without finishing. \
            Last action: {last_action}"
        )
        .into());
    }

    Ok(AgentRunOutput {
        session,
        context: context_pack,
        messages,
        tool_results,
        final_text,
    })
}

fn resolve_mv2_path(cli_mv2: Option<PathBuf>) -> PathBuf {
    if let Some(path) = cli_mv2 {
        return path;
    }
    if let Some(value) = env_optional("AETHERVAULT_MV2") {
        return PathBuf::from(value);
    }
    PathBuf::from("./data/knowledge.mv2")
}

fn resolve_bridge_model_hook(cli: Option<String>) -> Option<String> {
    if cli.is_some() {
        return cli;
    }
    if env_optional("ANTHROPIC_API_KEY").is_some() && env_optional("ANTHROPIC_MODEL").is_some() {
        return Some("builtin:claude".to_string());
    }
    None
}

fn build_bridge_agent_config(
    mv2: PathBuf,
    model_hook: Option<String>,
    system: Option<String>,
    no_memory: bool,
    context_query: Option<String>,
    context_results: usize,
    context_max_bytes: usize,
    max_steps: usize,
    log: bool,
    log_commit_interval: usize,
) -> Result<BridgeAgentConfig, Box<dyn std::error::Error>> {
    let model_hook = resolve_bridge_model_hook(model_hook);
    let system = system;
    let no_memory = no_memory;
    let context_query = context_query;
    let context_results = context_results;
    let context_max_bytes = context_max_bytes;
    let max_steps = max_steps;
    let log_commit_interval = log_commit_interval.max(1);
    let log = log;
    let session_prefix = String::new();

    Ok(BridgeAgentConfig {
        mv2,
        model_hook,
        system,
        no_memory,
        context_query,
        context_results,
        context_max_bytes,
        max_steps,
        log,
        log_commit_interval,
        session_prefix,
    })
}

fn run_agent_for_bridge(
    config: &BridgeAgentConfig,
    prompt: &str,
    session: String,
    system_override: Option<String>,
    model_hook_override: Option<String>,
    progress: Option<Arc<Mutex<AgentProgress>>>,
) -> Result<AgentRunOutput, String> {
    let (tx, rx) = mpsc::channel();
    let prompt_text = prompt.to_string();
    let mv2 = config.mv2.clone();
    let model_hook = model_hook_override.or_else(|| config.model_hook.clone());
    let system_text = system_override.or_else(|| config.system.clone());
    let no_memory = config.no_memory;
    let context_query = config.context_query.clone();
    let context_results = config.context_results;
    let context_max_bytes = config.context_max_bytes;
    let max_steps = config.max_steps;
    let log_commit_interval = config.log_commit_interval;
    let log = config.log;

    thread::spawn(move || {
        let result = run_agent_with_prompt(
            mv2,
            prompt_text,
            Some(session),
            model_hook,
            system_text,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log_commit_interval,
            log,
            progress,
        )
        .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });

    // No timeout — let the agent run as long as it needs.
    // The agent is bounded by max_steps, not wall-clock time.
    // Long-running tasks (dev work, swarms, batch processing) can take hours.
    rx.recv().map_err(|err| format!("Agent channel error: {err}"))?.map_err(|e| e)
}

#[derive(Debug, Deserialize)]
struct TelegramUpdateResponse {
    ok: bool,
    #[serde(default)]
    result: Vec<TelegramUpdate>,
}

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    #[serde(default)]
    message: Option<TelegramMessage>,
    #[serde(default)]
    edited_message: Option<TelegramMessage>,
    #[serde(default)]
    channel_post: Option<TelegramMessage>,
    #[serde(default)]
    callback_query: Option<TelegramCallbackQuery>,
}

#[derive(Debug, Deserialize)]
struct TelegramUser {
    id: i64,
    #[serde(default)]
    is_bot: Option<bool>,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramSticker {
    #[serde(default)]
    emoji: Option<String>,
    #[serde(default)]
    set_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramContact {
    phone_number: String,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramLocation {
    longitude: f64,
    latitude: f64,
}

#[derive(Debug, Deserialize)]
struct TelegramCallbackQuery {
    id: String,
    #[serde(default)]
    from: Option<TelegramUser>,
    #[serde(default)]
    message: Option<Box<TelegramMessage>>,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramPhotoSize {
    file_id: String,
    #[serde(default)]
    file_size: Option<i64>,
    #[serde(default)]
    width: Option<i64>,
    #[serde(default)]
    height: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TelegramVoice {
    file_id: String,
    #[serde(default)]
    duration: Option<i64>,
    #[serde(default)]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramAudio {
    file_id: String,
    #[serde(default)]
    duration: Option<i64>,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramDocument {
    file_id: String,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    chat: TelegramChat,
    #[serde(default)]
    message_id: Option<i64>,
    #[serde(default)]
    from: Option<TelegramUser>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    caption: Option<String>,
    #[serde(default)]
    photo: Option<Vec<TelegramPhotoSize>>,
    #[serde(default)]
    voice: Option<TelegramVoice>,
    #[serde(default)]
    audio: Option<TelegramAudio>,
    #[serde(default)]
    document: Option<TelegramDocument>,
    #[serde(default)]
    sticker: Option<TelegramSticker>,
    #[serde(default)]
    contact: Option<TelegramContact>,
    #[serde(default)]
    location: Option<TelegramLocation>,
    #[serde(default)]
    forward_from: Option<TelegramUser>,
    #[serde(default)]
    forward_from_chat: Option<TelegramChat>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
}

fn telegram_download_file_bytes(agent: &ureq::Agent, base_url: &str, file_id: &str) -> Option<(Vec<u8>, String)> {
    let url = format!("{base_url}/getFile");
    let payload = serde_json::json!({"file_id": file_id});
    let resp = agent.post(&url)
        .set("content-type", "application/json")
        .send_json(payload).ok()?;
    let data: serde_json::Value = resp.into_json().ok()?;
    let file_path = data["result"]["file_path"].as_str()?;
    // Build download URL: need token from base_url and correct API base
    let token_part = base_url.split("/bot").last()?;
    let api_base = if let Ok(base) = std::env::var("TELEGRAM_API_BASE") {
        base
    } else {
        "https://api.telegram.org".to_string()
    };
    let download_url = format!("{api_base}/file/bot{token_part}/{file_path}");
    let dl_resp = agent.get(&download_url).call().ok()?;
    let content_type = dl_resp.header("content-type")
        .unwrap_or("application/octet-stream").to_string();
    let mut bytes = Vec::new();
    dl_resp.into_reader().take(20_000_000).read_to_end(&mut bytes).ok()?;
    if bytes.is_empty() { return None; }
    Some((bytes, content_type))
}

fn transcribe_audio_deepgram(audio_bytes: &[u8], mime_type: &str) -> Option<String> {
    let api_key = std::env::var("DEEPGRAM_API_KEY").ok()?;
    if api_key.trim().is_empty() { return None; }
    let agent = ureq::AgentBuilder::new()
        .timeout_read(Duration::from_secs(60))
        .timeout_connect(Duration::from_secs(10))
        .build();
    let resp = agent.post("https://api.deepgram.com/v1/listen?model=nova-2&smart_format=true")
        .set("Authorization", &format!("Token {api_key}"))
        .set("Content-Type", mime_type)
        .send_bytes(audio_bytes).ok()?;
    let data: serde_json::Value = resp.into_json().ok()?;
    let transcript = data["results"]["channels"][0]["alternatives"][0]["transcript"]
        .as_str()
        .map(|s| s.to_string())?;
    if transcript.trim().is_empty() { return None; }
    Some(transcript)
}

fn guess_image_media_type(ct: &str, file_path: &str) -> String {
    if ct.starts_with("image/") { return ct.to_string(); }
    if file_path.ends_with(".jpg") || file_path.ends_with(".jpeg") { return "image/jpeg".to_string(); }
    if file_path.ends_with(".png") { return "image/png".to_string(); }
    if file_path.ends_with(".webp") { return "image/webp".to_string(); }
    if file_path.ends_with(".gif") { return "image/gif".to_string(); }
    "image/jpeg".to_string()
}

/// Extract content from a Telegram update. Returns (chat_id, message_id, text).
/// For photos, the text will contain an [AV_IMAGE:base64:media_type:DATA] marker.
/// For voice/audio, the transcription is prepended to any caption/text.
fn extract_telegram_content(update: &TelegramUpdate, agent: &ureq::Agent, base_url: &str) -> Option<(i64, Option<i64>, String)> {
    // Handle callback queries (inline keyboard presses)
    if let Some(cb) = &update.callback_query {
        if let Some(data) = &cb.data {
            let chat_id = cb.message.as_ref().map(|m| m.chat.id).unwrap_or(0);
            let user_name = cb.from.as_ref()
                .and_then(|u| u.first_name.clone())
                .unwrap_or_else(|| "User".to_string());
            let msg_id = cb.message.as_ref().and_then(|m| m.message_id);
            return Some((chat_id, msg_id, format!("[Callback button pressed by {user_name}]: {data}")));
        }
    }

    let msg = update
        .message
        .as_ref()
        .or(update.edited_message.as_ref())
        .or(update.channel_post.as_ref())?;
    let chat_id = msg.chat.id;
    let msg_id = msg.message_id;
    let base_text = msg.text.clone()
        .or_else(|| msg.caption.clone())
        .unwrap_or_default();
    let user_name = msg.from.as_ref()
        .and_then(|u| u.first_name.clone())
        .unwrap_or_else(|| "User".to_string());

    // Handle forwarded messages
    if let Some(fwd) = &msg.forward_from {
        let fwd_name = fwd.first_name.clone().unwrap_or_else(|| "someone".to_string());
        let fwd_text = if base_text.trim().is_empty() {
            format!("[Forwarded message from {fwd_name} — no text content]")
        } else {
            format!("[Forwarded message from {fwd_name}]:\n{base_text}")
        };
        return Some((chat_id, msg_id, fwd_text));
    }
    if let Some(fwd_chat) = &msg.forward_from_chat {
        let fwd_text = format!("[Forwarded from chat {}]:\n{base_text}", fwd_chat.id);
        return Some((chat_id, msg_id, fwd_text));
    }

    // Handle stickers
    if let Some(sticker) = &msg.sticker {
        let emoji = sticker.emoji.clone().unwrap_or_else(|| "unknown".to_string());
        let set_name = sticker.set_name.clone().unwrap_or_default();
        let sticker_text = format!("[{user_name} sent a sticker: {emoji} from set '{set_name}']");
        return Some((chat_id, msg_id, sticker_text));
    }

    // Handle contacts
    if let Some(contact) = &msg.contact {
        let name = contact.first_name.clone().unwrap_or_else(|| "Unknown".to_string());
        let last = contact.last_name.clone().unwrap_or_default();
        let phone = &contact.phone_number;
        let contact_text = format!("[{user_name} shared a contact: {name} {last}, phone: {phone}]");
        return Some((chat_id, msg_id, contact_text));
    }

    // Handle locations
    if let Some(loc) = &msg.location {
        let loc_text = format!(
            "[{user_name} shared a location: latitude {:.6}, longitude {:.6}]\nPlease describe this location or look it up.",
            loc.latitude, loc.longitude
        );
        return Some((chat_id, msg_id, loc_text));
    }

    // Handle photos: download largest, base64 encode, create image marker
    if let Some(photos) = &msg.photo {
        if !photos.is_empty() {
            // Telegram sends multiple sizes; pick the largest (last in array)
            let best = photos.iter().max_by_key(|p| p.file_size.unwrap_or(0))?;
            if let Some((bytes, ct)) = telegram_download_file_bytes(agent, base_url, &best.file_id) {
                let media_type = guess_image_media_type(&ct, &best.file_id);
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let marker = format!("[AV_IMAGE:{}:{}]", media_type, b64);
                let text = if base_text.trim().is_empty() {
                    format!("{marker}
Describe what you see in this image.")
                } else {
                    format!("{marker}
{base_text}")
                };
                return Some((chat_id, msg_id, text));
            }
            // Download failed, fall through to caption/text
            let text = if base_text.trim().is_empty() {
                "[User sent a photo but it could not be downloaded]".to_string()
            } else {
                format!("[User sent a photo but it could not be downloaded]
{base_text}")
            };
            return Some((chat_id, msg_id, text));
        }
    }

    // Handle voice messages
    if let Some(voice) = &msg.voice {
        let mime = voice.mime_type.clone().unwrap_or_else(|| "audio/ogg".to_string());
        if let Some((bytes, _ct)) = telegram_download_file_bytes(agent, base_url, &voice.file_id) {
            if let Some(transcript) = transcribe_audio_deepgram(&bytes, &mime) {
                let text = if base_text.trim().is_empty() {
                    format!("[Voice message transcription]: {transcript}")
                } else {
                    format!("[Voice message transcription]: {transcript}

User also wrote: {base_text}")
                };
                return Some((chat_id, msg_id, text));
            }
            return Some((chat_id, msg_id, "[User sent a voice message but transcription failed]".to_string()));
        }
        return Some((chat_id, msg_id, "[User sent a voice message but it could not be downloaded]".to_string()));
    }

    // Handle audio files
    if let Some(audio) = &msg.audio {
        let mime = audio.mime_type.clone().unwrap_or_else(|| "audio/mpeg".to_string());
        let title_note = audio.title.as_deref().map(|t| format!(" (title: {t})")).unwrap_or_default();
        if let Some((bytes, _ct)) = telegram_download_file_bytes(agent, base_url, &audio.file_id) {
            if let Some(transcript) = transcribe_audio_deepgram(&bytes, &mime) {
                let text = format!("[Audio{title_note} transcription]: {transcript}");
                return Some((chat_id, msg_id, text));
            }
            return Some((chat_id, msg_id, format!("[User sent an audio file{title_note} but transcription failed]")));
        }
        return Some((chat_id, msg_id, format!("[User sent an audio file{title_note} but it could not be downloaded]")));
    }

    // Handle documents (text-based ones)
    if let Some(doc) = &msg.document {
        let fname = doc.file_name.clone().unwrap_or_else(|| "unknown".to_string());
        let mime = doc.mime_type.clone().unwrap_or_default();
        let is_text = mime.starts_with("text/")
            || mime == "application/json"
            || mime == "application/xml"
            || fname.ends_with(".txt") || fname.ends_with(".md")
            || fname.ends_with(".json") || fname.ends_with(".csv")
            || fname.ends_with(".py") || fname.ends_with(".rs")
            || fname.ends_with(".js") || fname.ends_with(".ts")
            || fname.ends_with(".sh") || fname.ends_with(".yaml")
            || fname.ends_with(".yml") || fname.ends_with(".toml");
        if is_text {
            if let Some((bytes, _ct)) = telegram_download_file_bytes(agent, base_url, &doc.file_id) {
                if let Ok(text_content) = String::from_utf8(bytes) {
                    let truncated = if text_content.len() > 50000 {
                        format!("{}\n... (truncated, {} total chars)", &text_content[..50000], text_content.len())
                    } else {
                        text_content
                    };
                    let text = format!("[Document: {fname}]\n```\n{truncated}\n```\n\n{base_text}");
                    return Some((chat_id, msg_id, text));
                }
            }
        }
        // Non-text document or download failed
        let text = if base_text.trim().is_empty() {
            format!("[User sent a document: {fname} ({mime}). This file type is not supported for direct reading.]")
        } else {
            format!("[User sent a document: {fname} ({mime})]
{base_text}")
        };
        return Some((chat_id, msg_id, text));
    }

    // Plain text message
    if base_text.trim().is_empty() {
        return None;
    }
    Some((chat_id, msg_id, base_text))
}

fn split_text_chunks(text: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        if count >= max_chars {
            chunks.push(current);
            current = String::new();
            count = 0;
        }
        current.push(ch);
        count += 1;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

fn telegram_send_message_returning_id(
    agent: &ureq::Agent,
    base_url: &str,
    chat_id: i64,
    text: &str,
) -> Option<i64> {
    let url = format!("{base_url}/sendMessage");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
    });
    match agent.post(&url).set("content-type", "application/json").send_json(payload) {
        Ok(resp) => {
            if let Ok(body) = resp.into_json::<serde_json::Value>() {
                body.get("result")
                    .and_then(|r| r.get("message_id"))
                    .and_then(|v| v.as_i64())
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

fn telegram_edit_message(
    agent: &ureq::Agent,
    base_url: &str,
    chat_id: i64,
    message_id: i64,
    text: &str,
) {
    let url = format!("{base_url}/editMessageText");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "text": text,
    });
    let _ = agent.post(&url).set("content-type", "application/json").send_json(payload);
}

fn telegram_delete_message(agent: &ureq::Agent, base_url: &str, chat_id: i64, message_id: i64) {
    let url = format!("{base_url}/deleteMessage");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "message_id": message_id,
    });
    let _ = agent.post(&url).set("content-type", "application/json").send_json(payload);
}

fn telegram_send_typing(agent: &ureq::Agent, base_url: &str, chat_id: i64) {
    let url = format!("{base_url}/sendChatAction");
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "action": "typing"
    });
    let _ = agent.post(&url)
        .set("content-type", "application/json")
        .send_json(payload);
}

fn telegram_answer_callback(agent: &ureq::Agent, base_url: &str, callback_id: &str, text: Option<&str>) {
    let url = format!("{base_url}/answerCallbackQuery");
    let mut payload = serde_json::json!({"callback_query_id": callback_id});
    if let Some(t) = text {
        payload["text"] = serde_json::json!(t);
    }
    let _ = agent.post(&url)
        .set("content-type", "application/json")
        .send_json(payload);
}

fn escape_markdown_v2(text: &str) -> String {
    let special = ['_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!'];
    let mut out = String::with_capacity(text.len() * 2);
    let mut in_code_block = false;
    let mut in_inline_code = false;
    for ch in text.chars() {
        if ch == '`' {
            in_inline_code = !in_inline_code;
            out.push(ch);
            continue;
        }
        if in_inline_code || in_code_block {
            out.push(ch);
            continue;
        }
        if special.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn telegram_send_message(
    agent: &ureq::Agent,
    base_url: &str,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    telegram_send_message_ext(agent, base_url, chat_id, text, None)
}

fn telegram_send_message_ext(
    agent: &ureq::Agent,
    base_url: &str,
    chat_id: i64,
    text: &str,
    reply_to: Option<i64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{base_url}/sendMessage");
    let chunks = split_text_chunks(text, 3900);
    for (i, chunk) in chunks.iter().enumerate() {
        // Try Markdown first, fall back to plain text
        let mut payload = serde_json::json!({
            "chat_id": chat_id,
            "text": chunk,
            "parse_mode": "Markdown"
        });
        // Only reply to original on first chunk
        if i == 0 {
            if let Some(mid) = reply_to {
                payload["reply_to_message_id"] = serde_json::json!(mid);
                payload["allow_sending_without_reply"] = serde_json::json!(true);
            }
        }
        let response = agent
            .post(&url)
            .set("content-type", "application/json")
            .send_json(payload);
        match response {
            Ok(_) => {},
            Err(_) => {
                // Markdown failed, retry as plain text
                let mut plain_payload = serde_json::json!({
                    "chat_id": chat_id,
                    "text": chunk
                });
                if i == 0 {
                    if let Some(mid) = reply_to {
                        plain_payload["reply_to_message_id"] = serde_json::json!(mid);
                        plain_payload["allow_sending_without_reply"] = serde_json::json!(true);
                    }
                }
                let fallback = agent
                    .post(&url)
                    .set("content-type", "application/json")
                    .send_json(plain_payload);
                if let Err(err) = fallback {
                    return Err(format!("Telegram send error: {err}").into());
                }
            }
        }
    }
    Ok(())
}

fn spawn_agent_run(
    agent_config: &BridgeAgentConfig,
    chat_id: i64,
    reply_to_id: Option<i64>,
    user_text: &str,
    session: String,
    completion_tx: &mpsc::Sender<CompletionEvent>,
    http_agent: &ureq::Agent,
    base_url: &str,
) -> Arc<Mutex<AgentProgress>> {
    let progress = Arc::new(Mutex::new(AgentProgress {
        step: 0,
        max_steps: agent_config.max_steps,
        phase: "starting".to_string(),
        text_preview: None,
        started_at: std::time::Instant::now(),
    }));

    // Worker thread
    let worker_progress = progress.clone();
    let worker_config = agent_config.clone();
    let worker_prompt = user_text.to_string();
    let worker_session = session;
    let worker_tx = completion_tx.clone();
    thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_agent_for_bridge(
                &worker_config,
                &worker_prompt,
                worker_session,
                None,
                None,
                Some(worker_progress.clone()),
            )
        }));
        // Mark done
        if let Ok(mut p) = worker_progress.lock() {
            p.phase = "done".to_string();
        }
        let event = match result {
            Ok(agent_result) => CompletionEvent {
                chat_id,
                reply_to_id,
                result: agent_result.map_err(|e| e.to_string()),
            },
            Err(panic_info) => {
                let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "agent panicked".to_string()
                };
                CompletionEvent {
                    chat_id,
                    reply_to_id,
                    result: Err(format!("Agent crashed: {msg}")),
                }
            }
        };
        let _ = worker_tx.send(event);
    });

    // Progress reporter thread
    let prog_ref = progress.clone();
    let prog_agent = http_agent.clone();
    let prog_url = base_url.to_string();
    thread::spawn(move || {
        let mut progress_msg_id: Option<i64> = None;
        loop {
            thread::sleep(Duration::from_secs(30));
            let (phase, step, max_steps, elapsed, done) = {
                match prog_ref.lock() {
                    Ok(p) => (
                        p.phase.clone(),
                        p.step,
                        p.max_steps,
                        p.started_at.elapsed().as_secs(),
                        p.phase == "done",
                    ),
                    Err(_) => break,
                }
            };
            if done {
                // Clean up progress message
                if let Some(mid) = progress_msg_id {
                    telegram_delete_message(&prog_agent, &prog_url, chat_id, mid);
                }
                break;
            }
            let status = format!(
                "Working... step {}/{}, {} ({}s elapsed)",
                step + 1,
                max_steps,
                phase,
                elapsed
            );
            match progress_msg_id {
                Some(mid) => {
                    telegram_edit_message(&prog_agent, &prog_url, chat_id, mid, &status);
                }
                None => {
                    progress_msg_id =
                        telegram_send_message_returning_id(&prog_agent, &prog_url, chat_id, &status);
                    // Fall back to typing indicator if send fails
                    if progress_msg_id.is_none() {
                        telegram_send_typing(&prog_agent, &prog_url, chat_id);
                    }
                }
            }
        }
    });

    progress
}

fn handle_telegram_completion(
    event: CompletionEvent,
    http_agent: &ureq::Agent,
    base_url: &str,
    agent_config: &BridgeAgentConfig,
    active_runs: &mut HashMap<i64, ActiveRun>,
    completion_tx: &mpsc::Sender<CompletionEvent>,
) {
    let chat_id = event.chat_id;
    let reply_to_id = event.reply_to_id;

    let output = match event.result {
        Ok(result) => {
            let mut text = result.final_text.unwrap_or_default();
            if text.trim().is_empty() {
                text = "Done.".to_string();
            }
            text
        }
        Err(err) => {
            let detail = err.chars().take(500).collect::<String>();
            format!(
                "Something went wrong while processing your request.\n\n\
                Error: {detail}\n\n\
                This wasn't your fault. I can retry if you send the message again, \
                or you can rephrase if you'd like to try a different approach."
            )
        }
    };

    // Save conversation turns for session continuity
    let session_id = format!("{}telegram:{chat_id}", agent_config.session_prefix);
    {
        let mut turns = load_session_turns(&session_id, 8);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        // We don't have the original user_text here in the completion event,
        // but session turns were already saved by the agent run itself via the session.
        turns.push(SessionTurn {
            role: "assistant".to_string(),
            content: output.clone(),
            timestamp: now,
        });
        save_session_turns(&session_id, &turns, 8);
    }

    if let Err(err) = telegram_send_message_ext(http_agent, base_url, chat_id, &output, reply_to_id) {
        eprintln!("Telegram send failed: {err}");
    }

    // Check for queued messages
    if let Some(run) = active_runs.get_mut(&chat_id) {
        if let Some((queued_text, queued_reply_id)) = run.queued_messages.first().cloned() {
            run.queued_messages.remove(0);
            let session = format!("{}telegram:{chat_id}", agent_config.session_prefix);

            // Save the queued user message to session turns
            {
                let mut turns = load_session_turns(&session, 8);
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                turns.push(SessionTurn {
                    role: "user".to_string(),
                    content: queued_text.clone(),
                    timestamp: now,
                });
                save_session_turns(&session, &turns, 8);
            }

            let progress = spawn_agent_run(
                agent_config,
                chat_id,
                queued_reply_id,
                &queued_text,
                session,
                completion_tx,
                http_agent,
                base_url,
            );
            run.progress = progress;
        } else {
            active_runs.remove(&chat_id);
        }
    }
}

fn run_telegram_bridge(
    token: String,
    poll_timeout: u64,
    poll_limit: usize,
    agent_config: BridgeAgentConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let base_url = match std::env::var("TELEGRAM_API_BASE") {
        Ok(base) => format!("{base}/bot{token}"),
        Err(_) => format!("https://api.telegram.org/bot{token}"),
    };
    let (_config, _subagent_specs) = {
        let mut mem = Vault::open_read_only(&agent_config.mv2)?;
        let config = load_capsule_config(&mut mem).unwrap_or_default();
        let subagent_specs = load_subagents_from_config(&config);
        (config, subagent_specs)
    };
    let http_agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(poll_timeout.saturating_add(10)))
        .build();

    let mut active_runs: HashMap<i64, ActiveRun> = HashMap::new();
    let (completion_tx, completion_rx) = mpsc::channel::<CompletionEvent>();

    let mut offset: Option<i64> = None;
    loop {
        // 1. Drain completions (non-blocking)
        while let Ok(event) = completion_rx.try_recv() {
            handle_telegram_completion(
                event,
                &http_agent,
                &base_url,
                &agent_config,
                &mut active_runs,
                &completion_tx,
            );
        }

        // 2. Long-poll getUpdates
        let mut request = http_agent
            .get(&format!("{base_url}/getUpdates"))
            .query("timeout", &poll_timeout.to_string())
            .query("limit", &poll_limit.to_string());
        if let Some(last) = offset {
            request = request.query("offset", &(last + 1).to_string());
        }

        let response = request.call();
        let payload = match response {
            Ok(resp) => resp.into_json::<TelegramUpdateResponse>(),
            Err(err) => {
                eprintln!("Telegram poll error: {err}");
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        };

        let update = match payload {
            Ok(update) => update,
            Err(err) => {
                eprintln!("Telegram decode error: {err}");
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        if !update.ok {
            eprintln!("Telegram API returned ok=false");
            thread::sleep(Duration::from_secs(2));
            continue;
        }

        // 3. Process updates
        for entry in update.result {
            offset = Some(entry.update_id);

            // Handle callback queries (inline keyboard presses)
            if let Some(cb) = &entry.callback_query {
                telegram_answer_callback(&http_agent, &base_url, &cb.id, Some("Processing..."));
            }

            let Some((chat_id, reply_to_id, user_text)) = extract_telegram_content(&entry, &http_agent, &base_url) else {
                continue;
            };
            if let Some(output) = try_handle_approval_chat(&agent_config.mv2, &user_text) {
                if let Err(err) = telegram_send_message(&http_agent, &base_url, chat_id, &output) {
                    eprintln!("Telegram send failed: {err}");
                }
                continue;
            }

            // Check if there's already an active run for this chat
            if let Some(run) = active_runs.get_mut(&chat_id) {
                // Queue the message and acknowledge
                run.queued_messages.push((user_text, reply_to_id));
                let ack = format!(
                    "Still working on your previous request. Queued \u{2014} I'll handle it next. \
                    ({} message{} waiting)",
                    run.queued_messages.len(),
                    if run.queued_messages.len() == 1 { "" } else { "s" }
                );
                if let Err(err) = telegram_send_message(&http_agent, &base_url, chat_id, &ack) {
                    eprintln!("Telegram ack send failed: {err}");
                }
                continue;
            }

            // No active run — spawn a new one
            telegram_send_typing(&http_agent, &base_url, chat_id);

            let session = format!("{}telegram:{chat_id}", agent_config.session_prefix);

            // Save user message to session turns
            {
                let mut turns = load_session_turns(&session, 8);
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                turns.push(SessionTurn {
                    role: "user".to_string(),
                    content: user_text.clone(),
                    timestamp: now,
                });
                save_session_turns(&session, &turns, 8);
            }

            let progress = spawn_agent_run(
                &agent_config,
                chat_id,
                reply_to_id,
                &user_text,
                session,
                &completion_tx,
                &http_agent,
                &base_url,
            );

            active_runs.insert(chat_id, ActiveRun {
                progress,
                queued_messages: Vec::new(),
            });
        }
    }
}

fn escape_xml(input: &str) -> String {
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

fn run_whatsapp_bridge(
    bind: String,
    port: u16,
    agent_config: BridgeAgentConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{bind}:{port}");
    let server = Server::http(&addr)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("server: {e}")))?;
    eprintln!("WhatsApp bridge listening on http://{addr}");
    let (config, subagent_specs) = {
        let mut mem = Vault::open_read_only(&agent_config.mv2)?;
        let config = load_capsule_config(&mut mem).unwrap_or_default();
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
            output = "Done.".to_string();
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

fn parse_json_body(request: &mut tiny_http::Request) -> Result<serde_json::Value, String> {
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .map_err(|e| format!("read body: {e}"))?;
    if body.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&body).map_err(|e| format!("json: {e}"))
}

fn run_webhook_bridge(
    name: &str,
    bind: String,
    port: u16,
    agent_config: BridgeAgentConfig,
    extract_event: fn(&serde_json::Value) -> Option<(String, String)>,
    reply: fn(&BridgeAgentConfig, &str) -> Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{bind}:{port}");
    let server = Server::http(&addr)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("server: {e}")))?;
    eprintln!("{name} bridge listening on http://{addr}");

    for mut request in server.incoming_requests() {
        if *request.method() != Method::Post {
            let response = Response::from_string("ok");
            let _ = request.respond(response);
            continue;
        }
        let payload = parse_json_body(&mut request).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(challenge) = payload.get("challenge").and_then(|v| v.as_str()) {
            let response = Response::from_string(challenge.to_string());
            let _ = request.respond(response);
            continue;
        }
        let Some((session_key, text)) = extract_event(&payload) else {
            let response = Response::from_string("ok");
            let _ = request.respond(response);
            continue;
        };
        if let Some(output) = try_handle_approval_chat(&agent_config.mv2, &text) {
            if let Some(response_text) = reply(&agent_config, &output) {
                let response = Response::from_string(response_text);
                let _ = request.respond(response);
            } else {
                let response = Response::from_string("ok");
                let _ = request.respond(response);
            }
            continue;
        }
        let session = format!("{}{}", agent_config.session_prefix, session_key);
        let result = run_agent_for_bridge(&agent_config, &text, session, None, None, None);
        let output = match result {
            Ok(output) => output.final_text.unwrap_or_else(|| "Done.".to_string()),
            Err(err) => format!("Agent error: {err}"),
        };
        if let Some(response_text) = reply(&agent_config, &output) {
            let response = Response::from_string(response_text);
            let _ = request.respond(response);
        } else {
            let response = Response::from_string("ok");
            let _ = request.respond(response);
        }
    }
    Ok(())
}

fn payload_session_fallback(prefix: &str, payload: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    format!("{prefix}:{}", blake3_hash(&bytes).to_hex())
}

fn extract_slack_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload
        .get("event")
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            payload
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })?;
    let channel = payload
        .get("event")
        .and_then(|v| v.get("channel"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let user = payload
        .get("event")
        .and_then(|v| v.get("user"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session = if channel != "unknown" || user != "unknown" {
        format!("slack:{channel}:{user}")
    } else {
        payload_session_fallback("slack", payload)
    };
    Some((session, text))
}

fn extract_discord_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload.get("content").and_then(|v| v.as_str())?.to_string();
    let channel = payload
        .get("channel_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let user = payload
        .get("author")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session = if channel != "unknown" || user != "unknown" {
        format!("discord:{channel}:{user}")
    } else {
        payload_session_fallback("discord", payload)
    };
    Some((session, text))
}

fn extract_teams_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            payload
                .get("body")
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })?;
    let convo = payload
        .get("conversation")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let from = payload
        .get("from")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session = if convo != "unknown" || from != "unknown" {
        format!("teams:{convo}:{from}")
    } else {
        payload_session_fallback("teams", payload)
    };
    Some((session, text))
}

fn extract_signal_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload.get("text").and_then(|v| v.as_str())?.to_string();
    let source = payload
        .get("source")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("from").and_then(|v| v.as_str()))
        .unwrap_or("unknown");
    let session = if source != "unknown" {
        format!("signal:{source}")
    } else {
        payload_session_fallback("signal", payload)
    };
    Some((session, text))
}

fn extract_matrix_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload.get("text").and_then(|v| v.as_str())?.to_string();
    let room = payload
        .get("room_id")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("room").and_then(|v| v.as_str()))
        .unwrap_or("unknown");
    let sender = payload
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session = if room != "unknown" || sender != "unknown" {
        format!("matrix:{room}:{sender}")
    } else {
        payload_session_fallback("matrix", payload)
    };
    Some((session, text))
}

fn extract_imessage_event(payload: &serde_json::Value) -> Option<(String, String)> {
    let text = payload.get("text").and_then(|v| v.as_str())?.to_string();
    let from = payload
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session = if from != "unknown" {
        format!("imessage:{from}")
    } else {
        payload_session_fallback("imessage", payload)
    };
    Some((session, text))
}

fn reply_none(_: &BridgeAgentConfig, _: &str) -> Option<String> {
    None
}

fn reply_slack(_: &BridgeAgentConfig, text: &str) -> Option<String> {
    Some(serde_json::json!({ "text": text }).to_string())
}

fn run_bridge(command: BridgeCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        BridgeCommand::Telegram {
            mv2,
            token,
            poll_timeout,
            poll_limit,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let token = token
                .or_else(|| env_optional("TELEGRAM_BOT_TOKEN"))
                .ok_or("Missing TELEGRAM_BOT_TOKEN")?;
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_telegram_bridge(token, poll_timeout, poll_limit, config)
        }
        BridgeCommand::Whatsapp {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_whatsapp_bridge(bind, port, config)
        }
        BridgeCommand::Slack {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge(
                "slack",
                bind,
                port,
                config,
                extract_slack_event,
                reply_slack,
            )
        }
        BridgeCommand::Discord {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge(
                "discord",
                bind,
                port,
                config,
                extract_discord_event,
                reply_none,
            )
        }
        BridgeCommand::Teams {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge("teams", bind, port, config, extract_teams_event, reply_none)
        }
        BridgeCommand::Signal {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
            sender: _,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge(
                "signal",
                bind,
                port,
                config,
                extract_signal_event,
                reply_none,
            )
        }
        BridgeCommand::Matrix {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
            room: _,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge(
                "matrix",
                bind,
                port,
                config,
                extract_matrix_event,
                reply_none,
            )
        }
        BridgeCommand::IMessage {
            mv2,
            bind,
            port,
            model_hook,
            system,
            no_memory,
            context_query,
            context_results,
            context_max_bytes,
            max_steps,
            log,
            log_commit_interval,
        } => {
            let mv2 = resolve_mv2_path(mv2);
            let config = build_bridge_agent_config(
                mv2,
                model_hook,
                system,
                no_memory,
                context_query,
                context_results,
                context_max_bytes,
                max_steps,
                log,
                log_commit_interval,
            )?;
            run_webhook_bridge(
                "imessage",
                bind,
                port,
                config,
                extract_imessage_event,
                reply_none,
            )
        }
    }
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

                let existing_checksum = mem.frame_by_uri(&uri).ok().map(|frame| frame.checksum);

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
                    options
                        .extra_metadata
                        .insert("size_bytes".into(), size_bytes);
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
                .next_back()
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
                        if mem
                            .update_frame(frame_id, None, options, Some(embedding))
                            .is_ok()
                        {
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

        Command::Bootstrap {
            mv2,
            workspace,
            timezone,
            force,
        } => {
            let workspace = workspace
                .or_else(|| env_optional("AETHERVAULT_WORKSPACE").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACE_DIR));
            bootstrap_workspace(&mv2, &workspace, timezone, force)?;
            println!(
                "bootstrapped workspace at {} (mv2: {})",
                workspace.display(),
                mv2.display()
            );
            Ok(())
        }

        Command::Schedule {
            mv2,
            workspace,
            timezone,
            telegram_token,
            telegram_chat_id,
            model_hook,
            max_steps,
            log,
            log_commit_interval,
        } => run_schedule_loop(
            mv2,
            workspace,
            timezone,
            telegram_token,
            telegram_chat_id,
            model_hook,
            max_steps,
            log,
            log_commit_interval,
        ),

        Command::Watch {
            mv2,
            workspace,
            timezone,
            model_hook,
            max_steps,
            log,
            log_commit_interval,
            poll_seconds,
        } => run_watch_loop(
            mv2,
            workspace,
            timezone,
            model_hook,
            max_steps,
            log,
            log_commit_interval,
            poll_seconds,
        ),

        Command::Connect {
            mv2,
            provider,
            bind,
            port,
            redirect_base,
        } => run_oauth_broker(mv2, provider, bind, port, redirect_base),

        Command::Approve { mv2, id, execute } => {
            let output = approve_and_maybe_execute(&mv2, &id, execute)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            println!("{output}");
            Ok(())
        }

        Command::Reject { mv2, id } => {
            let output =
                reject_approval(&mv2, &id).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            println!("{output}");
            Ok(())
        }

        Command::Bridge { command } => run_bridge(command),

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

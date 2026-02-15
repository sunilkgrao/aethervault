#[allow(unused_imports)]
use std::path::PathBuf;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aethervault")]
#[command(about = "Hybrid retrieval over single-file .mv2 capsules", long_about = None)]
#[command(version)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
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
pub(crate) enum HookCommand {
    /// Anthropic Claude hook (stdio JSON)
    Claude,
}

#[derive(Subcommand)]
pub(crate) enum BridgeCommand {
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
pub(crate) enum ConfigCommand {
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

# PRD: Phase 10 — Agent Interface (CLI + Library)

## Overview

Build the user-facing CLI with all subcommands (`search`, `ingest`, `status`, `schema`, `clusters`, `get`, `watch`) and the public library API that exposes the same operations as typed async function calls. This phase wires together all previous phases into a complete, usable tool optimized for AI agents.

## Problem Statement

All the core functionality (parsing, chunking, embedding, indexing, search, clustering) exists as internal modules, but there's no way for users or agents to invoke them. The CLI provides a shell-friendly interface with JSON output for agent parsing, while the library API provides a typed Rust interface for programmatic integration into agent loops.

## Goals

- All operations available as CLI subcommands: `mdvdb search`, `mdvdb ingest`, `mdvdb status`, `mdvdb schema`, `mdvdb clusters`, `mdvdb get`, `mdvdb watch`
- JSON output mode for all commands (default for agents) with `--json` flag
- Human-readable output mode as fallback
- Exit codes: 0 = success, 1 = error (with message on stderr)
- Library API mirrors CLI operations as typed async functions returning structured objects
- Shell completions generation via `clap`
- Verbosity control: `-v` (info), `-vv` (debug), `-vvv` (trace)

## Non-Goals

- No interactive mode or REPL
- No GUI or web interface
- No remote server or network API
- No plugin system
- No shell aliases or convenience wrappers

## Technical Design

### CLI Subcommands

```
mdvdb search <query> [--limit N] [--min-score F] [--filter KEY=VALUE]... [--json]
mdvdb ingest [--full] [--file PATH] [--json]
mdvdb status [--json]
mdvdb schema [--json]
mdvdb clusters [--json]
mdvdb get <file-path> [--json]
mdvdb watch [--json]
mdvdb init
```

### Data Model Changes

**CLI argument structs (via clap derive):**

```rust
#[derive(clap::Parser)]
#[command(name = "mdvdb", about = "Markdown Vector Database")]
pub struct Cli {
    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Project root directory (default: current directory)
    #[arg(long, default_value = ".")]
    pub root: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Semantic search over indexed markdown files
    Search(SearchArgs),
    /// Index or re-index markdown files
    Ingest(IngestArgs),
    /// Show index status and health
    Status(StatusArgs),
    /// Display metadata schema
    Schema(SchemaArgs),
    /// List document clusters
    Clusters(ClustersArgs),
    /// Get a specific document's content and metadata
    Get(GetArgs),
    /// Watch for file changes and re-index automatically
    Watch(WatchArgs),
    /// Initialize a new .markdownvdb config file
    Init,
}
```

**Search subcommand:**

```rust
#[derive(clap::Args)]
pub struct SearchArgs {
    /// Natural-language search query
    pub query: String,

    /// Maximum results to return
    #[arg(short, long)]
    pub limit: Option<usize>,

    /// Minimum similarity score (0.0–1.0)
    #[arg(long)]
    pub min_score: Option<f64>,

    /// Metadata filters (KEY=VALUE format, repeatable)
    #[arg(short, long)]
    pub filter: Vec<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}
```

**Ingest subcommand:**

```rust
#[derive(clap::Args)]
pub struct IngestArgs {
    /// Force full re-index (ignore content hashes)
    #[arg(long)]
    pub full: bool,

    /// Ingest a specific file only
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}
```

### Library API

```rust
// src/lib.rs — public API

pub struct MarkdownVdb {
    config: Config,
    project_root: PathBuf,
    index: Arc<Index>,
    provider: Arc<dyn EmbeddingProvider>,
}

impl MarkdownVdb {
    /// Initialize from project root (loads config, opens/creates index)
    pub async fn open(project_root: impl Into<PathBuf>) -> Result<Self>;

    /// Semantic search
    pub async fn search(&self, query: SearchQuery) -> Result<Vec<SearchResult>>;

    /// Full ingestion
    pub async fn ingest(&self) -> Result<IngestResult>;

    /// Ingest a single file
    pub async fn ingest_file(&self, path: &Path) -> Result<()>;

    /// Get index status
    pub fn status(&self) -> Result<IndexStatus>;

    /// Get metadata schema
    pub fn schema(&self) -> Result<Schema>;

    /// Get cluster summaries
    pub fn clusters(&self) -> Result<Option<Vec<ClusterSummary>>>;

    /// Get a specific document's full content and metadata
    pub fn get_document(&self, path: &Path) -> Result<DocumentInfo>;

    /// Start file watching (blocks until cancelled)
    pub async fn watch(&self, cancel: tokio::sync::CancellationToken) -> Result<()>;
}

#[derive(serde::Serialize)]
pub struct ClusterSummary {
    pub id: usize,
    pub label: String,
    pub document_count: usize,
    pub keywords: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct DocumentInfo {
    pub path: String,
    pub frontmatter: Option<serde_json::Value>,
    pub headings: Vec<Heading>,
    pub content: String,
    pub chunk_count: usize,
    pub file_size: u64,
    pub indexed_at: u64,
}
```

### JSON Output Format

All CLI commands with `--json` output a JSON object to stdout:

**Search:**
```json
{
  "results": [
    {
      "score": 0.87,
      "chunk": {
        "chunk_id": "docs/guide.md#3",
        "heading_hierarchy": ["Getting Started", "Installation"],
        "content": "To install, run...",
        "start_line": 45,
        "end_line": 62
      },
      "file": {
        "path": "docs/guide.md",
        "frontmatter": {"title": "Guide", "tags": ["tutorial"]},
        "file_size": 4096
      }
    }
  ],
  "query": "how to install",
  "total_results": 1
}
```

**Status:**
```json
{
  "document_count": 342,
  "chunk_count": 1587,
  "last_updated": "2024-01-15T10:30:00Z",
  "index_file_size": 15728640,
  "embedding_config": {
    "provider": "openai",
    "model": "text-embedding-3-small",
    "dimensions": 1536
  }
}
```

**Errors (stderr):**
```json
{"error": "Index not found. Run 'mdvdb ingest' first."}
```

### Human-Readable Output Format

When `--json` is not specified, output is formatted for human reading:

**Search:**
```
Found 3 results for "how to install"

  1. docs/guide.md (score: 0.87)
     Getting Started > Installation
     Lines 45–62
     "To install, run cargo install mdvdb..."

  2. README.md (score: 0.72)
     Quick Start
     Lines 12–28
     "The fastest way to get started is..."
```

**Status:**
```
Index: .markdownvdb.index (15 MB)
Documents: 342 | Chunks: 1,587
Last updated: 2024-01-15 10:30:00
Embedding: openai / text-embedding-3-small (1536d)
```

### Migration Strategy

Not applicable — this wires together existing functionality.

## Implementation Steps

1. **Refactor `src/main.rs`** — Replace the placeholder CLI with full clap setup:
   - Define `Cli`, `Command`, and all argument structs using clap derive macros
   - Parse args with `Cli::parse()`
   - Initialize logging with `logging::init(cli.verbose)`
   - Resolve `project_root` from `cli.root` (canonicalize)
   - Load config with `Config::load(&project_root)`
   - Match on `cli.command` and dispatch to handler functions
   - Wrap everything in `anyhow::Result` for human-readable error display

2. **Implement search command handler** — `async fn cmd_search(args, config, root)`:
   - Open index with `Index::open_or_create()`
   - Create embedding provider with `create_provider(&config)`
   - Parse `--filter` args: split each on `=`, create `MetadataFilter::Equals` entries
   - Build `SearchQuery` from args (use config defaults for unspecified limit/min_score)
   - Call `search::search()`
   - If `--json`: serialize results with `serde_json::to_string_pretty()` → stdout
   - If not `--json`: format as human-readable text → stdout
   - Exit 0 on success, 1 on error

3. **Implement ingest command handler** — `async fn cmd_ingest(args, config, root)`:
   - Open index
   - Create provider
   - If `--file`: call `ingest::ingest_file()`
   - If `--full`: call `ingest::ingest_full()` (ignore content hashes)
   - Else: call `ingest::ingest_full()` (normal, with hash comparison)
   - Output: files indexed, skipped, removed, chunks created, API calls made
   - JSON or human-readable format

4. **Implement status command handler** — `fn cmd_status(args, config, root)`:
   - Open index (error if doesn't exist: "Index not found. Run 'mdvdb ingest' first.")
   - Call `index.status()`
   - Format `last_updated` timestamp as ISO 8601
   - JSON or human-readable format

5. **Implement schema command handler** — `fn cmd_schema(args, config, root)`:
   - Open index
   - Call `index.get_schema()`
   - List all fields with their types, descriptions, occurrence counts, and sample values
   - JSON or human-readable format

6. **Implement clusters command handler** — `fn cmd_clusters(args, config, root)`:
   - Open index
   - Call `index.get_clusters()`
   - If None: "Clustering is not enabled or no data has been indexed."
   - List clusters with labels, document counts, and keywords
   - JSON or human-readable format

7. **Implement get command handler** — `fn cmd_get(args, config, root)`:
   - Open index
   - Call `index.get_file()` for the specified path
   - If not found: error "File not found in index: {path}"
   - Return full content, frontmatter, headings, chunk count, metadata
   - JSON or human-readable format

8. **Implement watch command handler** — `async fn cmd_watch(args, config, root)`:
   - Open index
   - Create provider
   - Create `Watcher::new()`
   - Set up signal handler for Ctrl+C → cancel the `CancellationToken`
   - Call `watcher.watch(cancel_token)`
   - Output: "Watching {dirs} for changes... (Ctrl+C to stop)"

9. **Implement init command** — `fn cmd_init(root)`:
   - Check if `.markdownvdb` already exists → error "Config file already exists"
   - Copy `.markdownvdb.example` content to `.markdownvdb` with comments
   - Output: "Created .markdownvdb config file"

10. **Build the `MarkdownVdb` library API** — Refactor `src/lib.rs`:
    - Define `MarkdownVdb` struct that owns config, index, and provider
    - `open()`: loads config, creates provider, opens/creates index
    - Each method delegates to the internal modules (search, ingest, index, etc.)
    - All methods return `Result<T>` with typed library errors (not anyhow)
    - Re-export key types: `SearchQuery`, `SearchResult`, `MetadataFilter`, `IngestResult`, `Schema`, `IndexStatus`, `ClusterSummary`, `DocumentInfo`

11. **Add shell completions** — In `src/main.rs`:
    - Add a `completions` hidden subcommand: `mdvdb completions bash|zsh|fish`
    - Use `clap_complete::generate()` to output shell completion scripts
    - Document in `--help` output

12. **Write CLI integration tests** — Create `tests/cli_test.rs`:
    - Use `assert_cmd` crate for CLI testing (add to dev-dependencies)
    - Test: `mdvdb status` with no index returns exit code 1 and error message
    - Test: `mdvdb init` creates `.markdownvdb` file
    - Test: `mdvdb ingest --json` with test fixtures produces valid JSON output
    - Test: `mdvdb search "query" --json` produces valid JSON with result array
    - Test: `mdvdb schema --json` produces valid JSON with fields array
    - Test: `mdvdb get docs/test.md --json` produces valid JSON with file info
    - Test: `mdvdb search "query" --limit 3` returns at most 3 results
    - Test: `mdvdb search "query" --filter status=draft` applies filter
    - Test: exit code 0 on success, 1 on error for all commands
    - Test: `--json` output is parseable with `serde_json::from_str()`

13. **Write library API tests** — Create `tests/api_test.rs`:
    - Test: `MarkdownVdb::open()` with valid project root succeeds
    - Test: `MarkdownVdb::search()` returns `Vec<SearchResult>`
    - Test: `MarkdownVdb::ingest()` returns `IngestResult` with counts
    - Test: `MarkdownVdb::status()` returns `IndexStatus`
    - Test: `MarkdownVdb::schema()` returns `Schema` with fields
    - Test: `MarkdownVdb::clusters()` returns `Option<Vec<ClusterSummary>>`
    - Test: `MarkdownVdb::get_document()` returns `DocumentInfo`
    - Use mock provider and temp directories

## Validation Criteria

- [ ] `mdvdb search "how to install" --json` returns valid JSON with results array
- [ ] `mdvdb search "query" --limit 5` returns at most 5 results
- [ ] `mdvdb search "query" --min-score 0.8` excludes results below 0.8
- [ ] `mdvdb search "query" --filter status=draft` filters by frontmatter field
- [ ] `mdvdb ingest --json` outputs file/chunk/API-call counts as JSON
- [ ] `mdvdb ingest --full` re-embeds all files regardless of content hash
- [ ] `mdvdb ingest --file docs/test.md` re-indexes only that file
- [ ] `mdvdb status --json` outputs document count, chunk count, last updated, embedding config
- [ ] `mdvdb status` without index returns exit code 1 and error message
- [ ] `mdvdb schema --json` outputs all inferred fields with types and sample values
- [ ] `mdvdb clusters --json` outputs cluster labels, counts, and keywords
- [ ] `mdvdb get docs/test.md --json` returns full file content and metadata
- [ ] `mdvdb get nonexistent.md` returns exit code 1 and "not found" error
- [ ] `mdvdb watch` starts watching and responds to Ctrl+C gracefully
- [ ] `mdvdb init` creates `.markdownvdb` config file
- [ ] `mdvdb init` when file exists returns exit code 1 and error
- [ ] All commands return exit code 0 on success, 1 on error
- [ ] Error messages are printed to stderr, not stdout
- [ ] JSON output is valid and parseable by `jq`
- [ ] Human-readable output is clean and informative
- [ ] `MarkdownVdb::open()` library API loads config and opens index
- [ ] Library API methods return typed `Result<T>`, not strings
- [ ] `-v` shows info logs, `-vv` shows debug logs
- [ ] `cargo test` passes all CLI and API tests
- [ ] `cargo clippy` reports no warnings

## Anti-Patterns to Avoid

- **Do NOT mix stdout and stderr** — Results and JSON go to stdout. Error messages, warnings, and progress logs go to stderr. This is critical for agents that pipe stdout.
- **Do NOT use `println!` for output** — Use `serde_json::to_writer(io::stdout(), &result)` for JSON mode. For human mode, use a formatting function. `println!` mixed with tracing can interleave output.
- **Do NOT duplicate business logic in CLI handlers** — Handlers call library functions, then format output. All logic lives in the library; the CLI is just a thin shell.
- **Do NOT use `unwrap()` in CLI handlers** — Use `anyhow::Context` for error context. Every error path should produce a helpful message.
- **Do NOT make the library API depend on clap** — `clap` is a CLI dependency only. The library uses its own types (`SearchQuery`, etc.) that are independent of CLI argument parsing.
- **Do NOT forget to set exit codes** — `std::process::exit(1)` on error. Agents rely on exit codes for scripting.

## Patterns to Follow

- **CLI thin layer:** Each `cmd_*` function does: parse args → call library → format output → set exit code. No business logic in the CLI layer.
- **Output formatting:** Match on `args.json`: true → `serde_json::to_string_pretty()`, false → custom human-readable format function.
- **Error handling:** CLI uses `anyhow` for error display; library uses `thiserror` for typed errors. The boundary is in `main.rs`.
- **Library API pattern:** `MarkdownVdb` as the top-level entry point, owns all state, provides async methods. Mirrors the CLI 1:1.
- **Testing:** `assert_cmd` crate for CLI integration tests (runs the binary as a subprocess); standard Rust tests for library API.
- **Clap derive pattern:** `#[derive(Parser)]` on the top-level struct, `#[derive(Subcommand)]` on the command enum, `#[derive(Args)]` on each subcommand's arguments.

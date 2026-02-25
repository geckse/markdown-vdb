# PRD: Phase 1 — Project Foundation & Configuration

## Overview

Bootstrap the Rust project with all dependencies, build the configuration system that reads `.markdownvdb` dotenv files with shell environment overrides, define the error handling hierarchy, and set up structured logging. This phase produces no user-facing functionality but establishes the foundation every subsequent phase builds on.

## Problem Statement

No project structure exists yet. Before any feature work can begin, the Cargo workspace, dependency tree, configuration loading, error types, and logging infrastructure must be in place. Every subsequent phase depends on config values (embedding provider, source dirs, chunk sizes, etc.) and consistent error handling.

## Goals

- Cargo project compiles with all dependencies from TECH.md declared
- Configuration system loads values from shell env → `.markdownvdb` file → built-in defaults (in that priority order)
- All config variables from PROJECT.md §Configuration are supported with their documented defaults
- Typed error hierarchy covers all known error categories
- Structured logging with configurable verbosity is functional
- Project layout follows idiomatic Rust module structure

## Non-Goals

- No CLI subcommands yet (Phase 10 adds the full CLI; this phase only sets up `clap` scaffolding)
- No markdown parsing, embedding, indexing, or search
- No index file creation
- No file watching

## Technical Design

### Project Structure

```
markdown-vdb/
├── Cargo.toml
├── src/
│   ├── main.rs              # Entry point, clap setup, tokio runtime
│   ├── lib.rs               # Public library re-exports
│   ├── config.rs            # Configuration loading and validation
│   ├── error.rs             # Error types (thiserror)
│   └── logging.rs           # Tracing subscriber setup
├── tests/
│   └── config_test.rs       # Config integration tests
├── .markdownvdb.example     # Example config file
└── .gitignore
```

### Data Model Changes

**`Config` struct** — central configuration loaded once at startup:

```rust
pub struct Config {
    // Embedding
    pub embedding_provider: EmbeddingProviderType, // enum: OpenAI, Ollama, Custom
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub embedding_batch_size: usize,
    pub openai_api_key: Option<String>,
    pub ollama_host: String,
    pub embedding_endpoint: Option<String>,

    // Source & Index
    pub source_dirs: Vec<PathBuf>,
    pub index_file: PathBuf,
    pub ignore_patterns: Vec<String>,
    pub watch_enabled: bool,
    pub watch_debounce_ms: u64,

    // Chunking
    pub chunk_max_tokens: usize,
    pub chunk_overlap_tokens: usize,

    // Clustering
    pub clustering_enabled: bool,
    pub clustering_rebalance_threshold: usize,

    // Search
    pub search_default_limit: usize,
    pub search_min_score: f64,
}

pub enum EmbeddingProviderType {
    OpenAI,
    Ollama,
    Custom,
}
```

**`Error` enum** — typed library errors:

```rust
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Index not found at {path}")]
    IndexNotFound { path: PathBuf },

    #[error("Index corrupted: {0}")]
    IndexCorrupted(String),

    #[error("Embedding provider error: {0}")]
    EmbeddingProvider(String),

    #[error("Markdown parse error in {path}: {message}")]
    MarkdownParse { path: PathBuf, message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("File watching error: {0}")]
    Watch(String),

    #[error("Lock acquisition timeout")]
    LockTimeout,
}
```

### Interface Changes

**`Config::load()` — configuration loading function:**

```rust
impl Config {
    /// Load configuration with resolution order:
    /// 1. Shell environment (highest priority)
    /// 2. .markdownvdb file in project root
    /// 3. Built-in defaults (lowest priority)
    pub fn load(project_root: &Path) -> Result<Self, Error>;
}
```

**`logging::init()` — logging initialization:**

```rust
pub fn init(verbosity: u8) -> Result<(), Error>;
// verbosity: 0 = warn, 1 = info, 2 = debug, 3+ = trace
```

### Migration Strategy

Not applicable — this is a greenfield project.

## Implementation Steps

1. **Initialize Cargo project** — Run `cargo init --name mdvdb` in the project root. Add all dependencies from TECH.md to `Cargo.toml`:
   - `clap = { version = "4", features = ["derive"] }`
   - `tokio = { version = "1", features = ["rt", "macros", "fs"] }`
   - `serde = { version = "1", features = ["derive"] }`
   - `serde_json = "1"`
   - `serde_yaml = "0.9"`
   - `dotenvy = "0.15"`
   - `thiserror = "2"`
   - `anyhow = "1"`
   - `tracing = "0.1"`
   - `tracing-subscriber = { version = "0.3", features = ["env-filter"] }`
   - `pulldown-cmark = "0.12"`
   - `reqwest = { version = "0.12", features = ["json"] }`
   - `usearch = "2"`
   - `rkyv = { version = "0.8", features = ["validation"] }`
   - `memmap2 = "0.9"`
   - `notify = "7"`
   - `notify-debouncer-full = "0.4"`
   - `sha2 = "0.10"`
   - `parking_lot = "0.12"`
   - `linfa = "0.7"`
   - `linfa-clustering = "0.7"`
   - `tiktoken-rs = "0.6"`
   - `ignore = "0.4"`
   Update `.gitignore` to include `/target`, `.markdownvdb.index`, and `.DS_Store`.

2. **Create `src/error.rs`** — Define the `Error` enum as shown in Technical Design. Implement `From` conversions for `std::io::Error`. All library functions return `Result<T, Error>`.

3. **Create `src/config.rs`** — Implement `Config::load(project_root)`:
   - Call `dotenvy::from_path(project_root.join(".markdownvdb"))` (ignore error if file missing)
   - Read each `MDVDB_*` variable from `std::env::var()` (which now includes dotenv values)
   - Parse into typed fields with defaults from PROJECT.md §Configuration
   - `MDVDB_SOURCE_DIRS` splits on commas into `Vec<PathBuf>`
   - `MDVDB_IGNORE_PATTERNS` splits on commas into `Vec<String>`
   - `MDVDB_EMBEDDING_PROVIDER` maps to the `EmbeddingProviderType` enum (case-insensitive)
   - Validate: `embedding_dimensions > 0`, `chunk_max_tokens > chunk_overlap_tokens`, `search_min_score` in `[0.0, 1.0]`
   - Return `Error::Config` with a descriptive message on validation failure

4. **Create `src/logging.rs`** — Implement `init(verbosity)`:
   - Use `tracing_subscriber::fmt()` with `EnvFilter`
   - Map verbosity levels: 0 → `warn`, 1 → `info`, 2 → `debug`, 3+ → `trace`
   - Allow `RUST_LOG` env var to override (standard tracing behavior)
   - Format: timestamps, module paths, span context

5. **Create `src/lib.rs`** — Public library root that re-exports:
   - `pub mod config;`
   - `pub mod error;`
   - `pub mod logging;`
   - `pub use error::Error;`
   - `pub type Result<T> = std::result::Result<T, Error>;`

6. **Create `src/main.rs`** — Minimal CLI entry point:
   - `#[derive(clap::Parser)]` struct with a `verbosity` flag (`-v`, `-vv`, etc.)
   - Empty `#[derive(clap::Subcommand)]` enum (subcommands added in Phase 10)
   - `#[tokio::main]` async main that: initializes logging, loads config, and prints a status message
   - Wrap top-level errors with `anyhow` for human-readable CLI error messages

7. **Create `.markdownvdb.example`** — Copy the example config from PROJECT.md §Configuration into this file with comments explaining each variable.

8. **Write config tests** — Create `tests/config_test.rs`:
   - Test: default values are applied when no env vars or file exist
   - Test: `.markdownvdb` file values override defaults
   - Test: shell env vars override file values
   - Test: comma-separated `MDVDB_SOURCE_DIRS` parses correctly
   - Test: invalid `MDVDB_EMBEDDING_DIMENSIONS=0` returns `Error::Config`
   - Test: unknown `MDVDB_EMBEDDING_PROVIDER` value returns `Error::Config`
   - Use `temp_dir` for isolation between tests

## Validation Criteria

- [ ] `cargo build` compiles without errors or warnings
- [ ] `cargo test` passes all config tests
- [ ] Running `./target/debug/mdvdb` with no config file prints a status message using default config values
- [ ] Running with `MDVDB_EMBEDDING_PROVIDER=ollama ./target/debug/mdvdb` picks up the env override
- [ ] Creating a `.markdownvdb` file with `MDVDB_SOURCE_DIRS=docs,notes` and loading config returns `vec!["docs", "notes"]`
- [ ] Setting `MDVDB_EMBEDDING_DIMENSIONS=0` returns a config validation error
- [ ] Setting `MDVDB_CHUNK_MAX_TOKENS=10` and `MDVDB_CHUNK_OVERLAP_TOKENS=20` returns a config validation error (overlap > max)
- [ ] Logging at `-vv` verbosity shows debug-level messages
- [ ] `cargo clippy` reports no warnings

## Anti-Patterns to Avoid

- **Do NOT use `lazy_static` or global mutable state for config** — Pass `Config` as a parameter to functions that need it. Global state makes testing impossible and creates hidden dependencies.
- **Do NOT parse config values inside business logic** — All env var reading and parsing happens in `Config::load()`. Other modules receive a typed `Config` struct, never raw strings.
- **Do NOT use `unwrap()` or `expect()` in library code** — All fallible operations return `Result<T, Error>`. Only `main.rs` may use `anyhow` for top-level error display.
- **Do NOT add placeholder modules for future phases** — Only create files that have real content. Empty `mod embedding;` stubs add confusion.

## Patterns to Follow

- **Error pattern:** Use `thiserror` derive macros for the library `Error` enum; wrap with `anyhow` only at the CLI boundary in `main.rs`
- **Config pattern:** Use `dotenvy` for file loading, `std::env::var` for reading (which sees both shell and dotenv values), manual parsing with defaults
- **Module pattern:** Each concern gets its own file (`config.rs`, `error.rs`, `logging.rs`), re-exported from `lib.rs`
- **Test pattern:** Integration tests in `tests/` directory, unit tests as `#[cfg(test)] mod tests` within source files for internal logic

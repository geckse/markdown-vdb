# Markdown VDB

A filesystem-native vector database built around Markdown files. Rust, zero infrastructure, optimized for AI agents.

All 10 implementation phases are complete and passing (306 tests, clippy clean).

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Agent Interface                       │
│              CLI (clap) + Library API                    │
│         mdvdb search | ingest | status | watch          │
├──────────┬──────────┬───────────┬───────────────────────┤
│  Search  │  Schema  │ Clustering│   File Watcher        │
│  Engine  │  System  │  (linfa)  │   (notify)            │
├──────────┴──────────┴───────────┴───────────────────────┤
│                   Index Storage                         │
│        usearch (HNSW) + rkyv (metadata) + memmap2       │
│          parking_lot::RwLock (concurrency)               │
├──────────┬──────────────────────────────────────────────┤
│ Embedding│   OpenAI | Ollama | Custom (reqwest)         │
│ Providers│   Batch processing + content-hash skip       │
├──────────┴──────────────────────────────────────────────┤
│                  Chunking Engine                        │
│      Heading-split + token size guard (tiktoken-rs)     │
├─────────────────────────────────────────────────────────┤
│              Markdown Parsing & Discovery                │
│    pulldown-cmark + serde_yaml + ignore + sha2          │
├─────────────────────────────────────────────────────────┤
│               Foundation & Configuration                │
│         dotenvy + thiserror + anyhow + tracing          │
└─────────────────────────────────────────────────────────┘
```

## Project Structure

```
src/
├── main.rs              # CLI entry point (clap + anyhow)
├── lib.rs               # Public library API (MarkdownVdb)
├── config.rs            # Config loading: shell env → .markdownvdb → defaults
├── error.rs             # Error enum (thiserror)
├── logging.rs           # Tracing subscriber setup
├── discovery.rs         # File scanning with ignore patterns
├── parser.rs            # Markdown parsing: frontmatter, headings, body
├── chunker.rs           # Heading-based chunking + token size guard
├── search.rs            # Query pipeline, metadata filtering, results
├── schema.rs            # Auto-infer + overlay schema system
├── clustering.rs        # K-means, nearest-centroid, rebalancing, TF-IDF labels
├── watcher.rs           # Filesystem watcher (notify + debouncer)
├── ingest.rs            # Full + incremental ingestion pipeline
├── embedding/
│   ├── mod.rs           # EmbeddingProvider trait + factory
│   ├── provider.rs      # Trait definition
│   ├── openai.rs        # OpenAI-compatible provider
│   ├── ollama.rs        # Ollama provider
│   ├── batch.rs         # Concurrent batch orchestration (up to 4) + hash skip
│   └── mock.rs          # Mock provider for testing
└── index/
    ├── mod.rs           # Index public API
    ├── types.rs         # StoredChunk, StoredFile, IndexMetadata (rkyv)
    ├── storage.rs       # File I/O: header + rkyv region + usearch region
    └── state.rs         # Runtime operations with RwLock concurrency

tests/
├── api_test.rs          # Library API integration tests (9 tests)
├── cli_test.rs          # CLI binary integration tests (14 tests)
├── chunker_test.rs      # Chunking pipeline tests
├── clustering_test.rs   # K-means clustering tests
├── config_test.rs       # Configuration loading tests
├── discovery_test.rs    # File discovery tests
├── embedding_test.rs    # Embedding provider tests
├── index_test.rs        # Index storage tests
├── ingest_test.rs       # Ingestion pipeline tests
├── parser_test.rs       # Markdown parsing tests
├── schema_test.rs       # Schema inference tests
├── search_test.rs       # Search engine tests
└── watcher_test.rs      # File watcher tests

docs/prds/               # PRD specifications for all 10 phases (reference)
```

## Core Design Decisions

- **Config:** Dotenv-style `.markdownvdb` file, NOT TOML/YAML. Resolution: shell env > file > defaults
- **Index directory:** `.markdownvdb/` contains `index` (binary: `[64B header][rkyv metadata][usearch HNSW]`) + `fts/` (Tantivy BM25 segments). Configured via `MDVDB_INDEX_DIR`.
- **Paths:** ALL file paths in the index are relative to project root. Never absolute.
- **Errors:** `thiserror` for typed library errors, `anyhow` only at CLI boundary in `main.rs`
- **Concurrency:** `parking_lot::RwLock` (not std). Read lock for queries, write lock only during upsert.
- **Writes:** Always atomic — write to `.tmp`, fsync, rename. Never write directly to index file.
- **Frontmatter:** Read-only. The system NEVER writes to markdown files. All computed data lives in the index.
- **Embeddings:** Trait-based pluggable providers. Batch-first (up to 4 concurrent). Skip unchanged files via SHA-256 hash.
- **Chunking:** Primary split by headings, secondary token-count size guard. Deterministic `"path#index"` IDs.
- **Clustering:** Document-level vectors (averaged chunk vectors per file). K-means with cross-cluster TF-IDF keyword extraction.
- **CLI output:** stdout for data (JSON with `--json`, human-readable otherwise), stderr for errors/logs. Search JSON uses wrapped format: `{"results": [...], "query": "...", "total_results": N}`.

## Key Conventions

- Return `Result<T, Error>` from all library functions — never `unwrap()` in library code
- Pass `Config` as parameter — no global mutable state, no `lazy_static`
- All env var reading happens in `Config::load()` — other modules receive typed config
- Derive `serde::Serialize` on all API response types for JSON output
- Derive `rkyv::Archive`/`Serialize`/`Deserialize` on all types stored in the index
- Use `tracing::info!`/`debug!`/`error!` for logging, never `println!` in library code
- Keep clippy clean — `cargo clippy --all-targets` must pass with zero warnings

## Testing Requirements

**Every change must have automated tests. No exceptions.**

- Every new feature, bug fix, or behavioral change MUST include tests that verify the change works
- Every existing feature that is modified MUST have its existing tests updated if behavior changes, and new tests added for new behavior
- `cargo test` must pass with zero failures before any change is considered complete
- Unit tests go in `#[cfg(test)] mod tests` blocks in the source file
- Integration tests go in `tests/` — one file per module (e.g., `tests/search_test.rs` for the search engine)
- CLI tests use `std::process::Command` with `env!("CARGO_BIN_EXE_mdvdb")` and validate JSON output structure
- API tests use `mock_config()` with `EmbeddingProviderType::Mock` (8 dimensions) — no API keys needed
- Use `tempfile::TempDir` for filesystem isolation in all tests that touch files
- Do NOT skip writing tests to save time. Untested code is unfinished code.

## Public API (lib.rs)

The `MarkdownVdb` struct is the main entry point:

```rust
MarkdownVdb::open(root)                    // Open with auto-loaded config
MarkdownVdb::open_with_config(root, cfg)   // Open with explicit config
MarkdownVdb::init(path)                    // Create .markdownvdb config file

vdb.ingest(options)     // Index markdown files (full or incremental)
vdb.search(query)       // Semantic search with filters
vdb.status()            // Index stats (doc/chunk/vector counts)
vdb.schema()            // Inferred metadata schema
vdb.clusters()          // Document clusters with labels
vdb.get_document(path)  // Single document info + frontmatter
vdb.watch(cancel)       // File watcher with CancellationToken
vdb.config()            // Access current config
```

Key re-exports: `Config`, `SearchQuery`, `SearchResult`, `MetadataFilter`, `Schema`, `SchemaField`, `FieldType`, `ClusterInfo`, `ClusterState`, `IndexStatus`, `IngestOptions`, `IngestResult`.

## Development Workflow

```bash
cargo test               # Run all 306 tests
cargo clippy --all-targets  # Lint (must be clean)
cargo build --release    # Release build
cargo run -- ingest      # Test ingest locally
cargo run -- search "query" --json  # Test search
```

## Technology Stack

| Layer | Crate | Purpose |
|---|---|---|
| Runtime | `tokio` | Async I/O for embeddings, file watching |
| CLI | `clap` | Derive-based subcommands, completions |
| Markdown | `pulldown-cmark` | Streaming heading-aware parsing |
| Frontmatter | `serde_yaml` | Dynamic YAML → JSON metadata |
| Tokenizer | `tiktoken-rs` | Accurate token counting for chunks |
| Embeddings | `reqwest` | HTTP client for OpenAI/Ollama APIs |
| Vectors | `usearch` | Sub-ms HNSW nearest neighbor search |
| Serialization | `rkyv` | Zero-copy deserialization from mmap |
| Memory mapping | `memmap2` | On-demand index loading via OS page cache |
| File watching | `notify` + `notify-debouncer-full` | Cross-platform FS events + debouncing |
| Concurrency | `parking_lot` | Fast RwLock for read-heavy workloads |
| Clustering | `linfa` + `linfa-clustering` | K-means + nearest-centroid assignment |
| Async streams | `futures` | Concurrent batch embedding (buffer_unordered) |
| File scanning | `ignore` | Gitignore-native directory traversal |
| Hashing | `sha2` | Content change detection (SHA-256) |
| Config | `dotenvy` | Dotenv-style `.markdownvdb` config |
| Errors | `thiserror` / `anyhow` | Typed lib errors, ergonomic CLI errors |
| Serialization | `serde` + `serde_json` | JSON output, request/response bodies |
| Logging | `tracing` + `tracing-subscriber` | Structured, async-aware, spans |

## PRD Reference

Full specifications for all 10 phases live in `docs/prds/`. These document the design intent and acceptance criteria for each subsystem.

| Phase | PRD | Summary |
|---|---|---|
| 1 | `phase-1-foundation-config.md` | Cargo project, config, errors, logging |
| 2 | `phase-2-markdown-parsing.md` | File discovery, ignore rules, frontmatter, headings, SHA-256 |
| 3 | `phase-3-chunking-engine.md` | Heading split, token size guard, overlap, chunk metadata |
| 4 | `phase-4-embedding-providers.md` | Provider trait, OpenAI, Ollama, batch, mock |
| 5 | `phase-5-index-storage.md` | Index file format, rkyv, usearch, memmap, RwLock |
| 6 | `phase-6-semantic-search.md` | Query pipeline, filters, section-level results |
| 7 | `phase-7-metadata-schema.md` | Auto-infer, overlay YAML, schema introspection |
| 8 | `phase-8-file-watching.md` | FS watcher, debounce, incremental re-index, ingest pipeline |
| 9 | `phase-9-clustering.md` | K-means, nearest-centroid, rebalance, keyword labels |
| 10 | `phase-10-cli-library.md` | CLI subcommands, JSON output, MarkdownVdb library API |

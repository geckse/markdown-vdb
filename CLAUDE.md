# Markdown VDB

A filesystem-native vector database built around Markdown files. Rust, zero infrastructure, optimized for AI agents.

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
| File scanning | `ignore` | Gitignore-native directory traversal |
| Hashing | `sha2` | Content change detection (SHA-256) |
| Config | `dotenvy` | Dotenv-style `.markdownvdb` config |
| Errors | `thiserror` / `anyhow` | Typed lib errors, ergonomic CLI errors |
| Serialization | `serde` + `serde_json` | JSON output, request/response bodies |
| Logging | `tracing` + `tracing-subscriber` | Structured, async-aware, spans |

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
├── clustering.rs        # K-means, nearest-centroid, rebalancing
├── watcher.rs           # Filesystem watcher (notify + debouncer)
├── ingest.rs            # Full + incremental ingestion pipeline
├── embedding/
│   ├── mod.rs           # EmbeddingProvider trait + factory
│   ├── provider.rs      # Trait definition
│   ├── openai.rs        # OpenAI-compatible provider
│   ├── ollama.rs        # Ollama provider
│   ├── batch.rs         # Batch orchestration + hash skip
│   └── mock.rs          # Mock provider for testing
└── index/
    ├── mod.rs           # Index public API
    ├── types.rs         # StoredChunk, StoredFile, IndexMetadata (rkyv)
    ├── storage.rs       # File I/O: header + rkyv region + usearch region
    └── state.rs         # Runtime operations with RwLock concurrency
```

## Core Design Decisions

- **Language:** Rust — performance-critical (sub-100ms queries), memory-mapped I/O, strong concurrency
- **Config:** Dotenv-style `.markdownvdb` file, NOT TOML/YAML. Resolution: shell env > file > defaults
- **Index format:** Single binary file `[64B header][rkyv metadata][usearch HNSW]` — memory-mapped, portable
- **Paths:** ALL file paths in the index are relative to project root. Never absolute.
- **Errors:** `thiserror` for typed library errors, `anyhow` only at CLI boundary in `main.rs`
- **Concurrency:** `parking_lot::RwLock` (not std). Read lock for queries, write lock only during upsert.
- **Writes:** Always atomic — write to `.tmp`, fsync, rename. Never write directly to index file.
- **Frontmatter:** Read-only. The system NEVER writes to markdown files. All computed data lives in the index.
- **Embeddings:** Trait-based pluggable providers. Batch-first. Skip unchanged files via SHA-256 hash.
- **Chunking:** Primary split by headings, secondary token-count size guard. Deterministic `"path#index"` IDs.
- **CLI output:** stdout for data (JSON with `--json`, human-readable otherwise), stderr for errors/logs.

## Performance Targets

- Cold index load: < 500ms for 10k documents
- Warm search query: < 100ms
- HNSW vector search: sub-1ms (usearch)
- Embedding skip: $0 cost for unchanged files (content hash)

## Key Conventions

- Return `Result<T, Error>` from all library functions — never `unwrap()` in library code
- Pass `Config` as parameter — no global mutable state, no `lazy_static`
- All env var reading happens in `Config::load()` — other modules receive typed config
- Derive `serde::Serialize` on all API response types for JSON output
- Derive `rkyv::Archive`/`Serialize`/`Deserialize` on all types stored in the index
- Tests: integration tests in `tests/`, unit tests as `#[cfg(test)] mod tests` in source files
- Use `tracing::info!`/`debug!`/`error!` for logging, never `println!` in library code

## Implementation Phases

Full PRDs in `docs/prds/`. Execute with `/execute-prd docs/prds/<file>`.

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

**Critical path:** 1 → 2 → 3 → 4 → 5 → 6 → 10
**Parallel after Phase 5:** 7, 8, 9

## PRD Conventions

- PRD files live in `docs/prds/` with kebab-case names
- Every PRD follows `docs/prds/base-template.md`
- PRDs must be self-contained — a fresh session with only the PRD implements the feature
- Implementation steps reference specific files and are actionable
- Validation criteria must be testable (not "works well")
- Generate PRDs with `/generate-prd <description>`
- Execute PRDs with `/execute-prd docs/prds/<file>`

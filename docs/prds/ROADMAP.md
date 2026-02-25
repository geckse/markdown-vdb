# Markdown VDB — Implementation Roadmap

A filesystem-native vector database built entirely around Markdown files. Zero infrastructure, markdown-first, optimized for AI agent access.

---

## Architecture Overview

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

---

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
| File watching | `notify` | Cross-platform FS events + debouncing |
| Concurrency | `parking_lot` | Fast RwLock for read-heavy workloads |
| Clustering | `linfa` | K-means + nearest-centroid assignment |
| File scanning | `ignore` | Gitignore-native directory traversal |
| Hashing | `sha2` | Content change detection |
| Config | `dotenvy` | Dotenv-style `.markdownvdb` config |
| Errors | `thiserror` / `anyhow` | Typed lib errors, ergonomic CLI errors |

---

## Phases

### Phase 1 — Foundation & Configuration
> [phase-1-foundation-config.md](phase-1-foundation-config.md)

Bootstrap the Rust project. Config system reads `.markdownvdb` dotenv files with shell env overrides and built-in defaults. Error hierarchy and structured logging established.

**Produces:** Compiling project, `Config::load()`, `Error` enum, `logging::init()`
**Key decisions:** dotenvy for config (not TOML/YAML), thiserror for library errors, anyhow at CLI boundary

---

### Phase 2 — Markdown Parsing & File Discovery
> [phase-2-markdown-parsing.md](phase-2-markdown-parsing.md)

Recursive file scanner with layered ignore rules (`.gitignore` + 15 built-in patterns + user custom). Markdown parser extracts frontmatter, heading structure, and body content. SHA-256 content hashing for change detection.

**Produces:** `FileDiscovery::discover()`, `parse_markdown_file()`, `MarkdownFile` struct
**Key decisions:** `ignore` crate for traversal (not manual `read_dir`), pulldown-cmark event stream for headings, relative paths only

---

### Phase 3 — Chunking Engine
> [phase-3-chunking-engine.md](phase-3-chunking-engine.md)

Hybrid chunking: primary split by headings (preserving document structure), secondary split by token count for oversized sections with configurable overlap. Each chunk carries heading hierarchy breadcrumb and line range.

**Produces:** `chunk_document()`, `Chunk` struct with `id`, `heading_hierarchy`, `start_line`/`end_line`
**Key decisions:** Heading-based (not sentence-based) primary split, tiktoken-rs for accurate token counting, deterministic `"path#index"` chunk IDs

---

### Phase 4 — Embedding Provider System
> [phase-4-embedding-providers.md](phase-4-embedding-providers.md)

Pluggable `EmbeddingProvider` trait with OpenAI and Ollama implementations. Batch processing (N chunks per API call). Content-hash comparison skips re-embedding unchanged files.

**Produces:** `EmbeddingProvider` trait, `OpenAIProvider`, `OllamaProvider`, `embed_chunks()` batch orchestrator, `MockProvider` for testing
**Key decisions:** Trait-based abstraction, batch-first API, retry on 429 only (not 401), no API key in logs

---

### Phase 5 — Index Storage & Memory Mapping
> [phase-5-index-storage.md](phase-5-index-storage.md)

Single portable index file: `[header][rkyv metadata][usearch HNSW vectors]`. Memory-mapped via memmap2 for fast on-demand loading. usearch HNSW for sub-millisecond vector search. Concurrent access via parking_lot RwLock.

**Produces:** `Index` struct with `open()`, `upsert()`, `remove_file()`, `save()`, atomic writes, concurrent read/write support
**Key decisions:** rkyv zero-copy (not serde_json), usearch HNSW (not brute-force), atomic rename for crash safety, relative paths for portability

**Performance targets:** Cold load < 500ms (10k docs), warm query < 100ms

---

### Phase 6 — Semantic Search & Metadata Filtering
> [phase-6-semantic-search.md](phase-6-semantic-search.md)

Query pipeline: embed query → HNSW nearest neighbors → metadata filter → section-level results. Filters support exact match, list membership, numeric range, and field existence. Builder-pattern query construction.

**Produces:** `search()`, `SearchQuery` (builder pattern), `MetadataFilter` enum, `SearchResult` with chunk + file context
**Key decisions:** 3x over-fetching to compensate for filter losses, AND logic for multiple filters, cosine similarity scoring

---

### Phase 7 — Metadata Schema System
> [phase-7-metadata-schema.md](phase-7-metadata-schema.md)

Auto-infer schema from frontmatter (field names, types, sample values). Optional `.markdownvdb.schema.yml` overlay for annotations, type overrides, and allowed values. Schema is documentation, not enforcement.

**Produces:** `Schema::infer()`, `Schema::merge()`, `SchemaField` with type/description/samples, schema persisted in index
**Key decisions:** Infer + overlay pattern, Date detection via regex, 20-value sample cap, no strict enforcement

---

### Phase 8 — File Watching & Incremental Updates
> [phase-8-file-watching.md](phase-8-file-watching.md)

Filesystem watcher with debouncing. On change: hash comparison → parse → chunk → embed → upsert. Stale entry cleanup. Full ingest pipeline for initial indexing and CLI `mdvdb ingest` command.

**Produces:** `Watcher::watch()`, `ingest_full()`, `ingest_file()`, `IngestResult` with counts
**Key decisions:** notify + debouncer (not polling), async event bridging via mpsc channel, write lock only during upsert (not during embedding)

---

### Phase 9 — Clustering
> [phase-9-clustering.md](phase-9-clustering.md)

K-means clustering on document-level vectors. Incremental nearest-centroid assignment for new files. Periodic rebalancing after N new documents. Auto-generated labels from TF-IDF keywords. Cluster data stored only in index (never in frontmatter).

**Produces:** `Clusterer::cluster_all()`, `assign_to_nearest()`, `maybe_rebalance()`, `ClusterInfo` with labels/keywords
**Key decisions:** `k = sqrt(N/2)` heuristic, document-level (not chunk-level) vectors, non-fatal failures, index-only storage

---

### Phase 10 — Agent Interface (CLI + Library)
> [phase-10-cli-library.md](phase-10-cli-library.md)

Full CLI: `mdvdb search|ingest|status|schema|clusters|get|watch|init`. JSON output for agents, human-readable fallback. Library API via `MarkdownVdb` struct with typed async methods mirroring every CLI command.

**Produces:** Complete `mdvdb` binary, `MarkdownVdb` library entry point, shell completions, exit codes
**Key decisions:** CLI is thin layer over library, stdout for data / stderr for errors, `--json` flag on all commands

---

## Dependency Graph

```
  ┌───┐
  │ 1 │ Foundation & Config
  └─┬─┘
    │
  ┌─▼─┐
  │ 2 │ Parsing & Discovery
  └─┬─┘
    │
  ┌─▼─┐
  │ 3 │ Chunking Engine
  └─┬─┘
    │
  ┌─▼─┐
  │ 4 │ Embedding Providers
  └─┬─┘
    │
  ┌─▼─┐
  │ 5 │ Index Storage ◄────────────────────┐
  └─┬─┘                                    │
    │                                       │
  ┌─▼─┐   ┌───┐                          ┌─┴─┐
  │ 6 │   │ 7 │ Schema (uses 2,5)        │ 9 │ Clustering (uses 5)
  └─┬─┘   └─┬─┘                          └─┬─┘
    │        │        ┌───┐                 │
    │        │        │ 8 │ Watcher         │
    │        │        └─┬─┘ (uses 2-5)     │
    │        │          │                   │
  ┌─▼────────▼──────────▼───────────────────▼─┐
  │                   10                       │
  │          CLI + Library Interface            │
  └────────────────────────────────────────────┘
```

**Critical path:** 1 → 2 → 3 → 4 → 5 → 6 → 10
**Parallel after Phase 5:** Phases 7, 8, 9 can be developed concurrently

---

## Execution

To implement any phase:

```
/execute-prd docs/prds/phase-N-<name>.md
```

Each PRD is self-contained — a fresh session with only that PRD has all the context needed to implement the phase.

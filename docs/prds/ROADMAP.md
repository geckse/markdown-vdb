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
| Async streams | `futures` | Concurrent batch embedding |
| File scanning | `ignore` | Gitignore-native directory traversal |
| Hashing | `sha2` | Content change detection |
| Config | `dotenvy` | Dotenv-style `.markdownvdb` + `.env` config |
| Errors | `thiserror` / `anyhow` | Typed lib errors, ergonomic CLI errors |

---

## Sprint 1 — Core Engine (Phases 1–10) ✅

All 10 phases complete. 309 tests passing, clippy clean.

### Phase 1 — Foundation & Configuration ✅
> [phase-1-foundation-config.md](phase-1-foundation-config.md)

Bootstrap the Rust project. Config system reads `.markdownvdb` dotenv files with shell env overrides and built-in defaults. Error hierarchy and structured logging established.

### Phase 2 — Markdown Parsing & File Discovery ✅
> [phase-2-markdown-parsing.md](phase-2-markdown-parsing.md)

Recursive file scanner with layered ignore rules (`.gitignore` + 15 built-in patterns + user custom). Markdown parser extracts frontmatter, heading structure, and body content. SHA-256 content hashing for change detection.

### Phase 3 — Chunking Engine ✅
> [phase-3-chunking-engine.md](phase-3-chunking-engine.md)

Hybrid chunking: primary split by headings (preserving document structure), secondary split by token count for oversized sections with configurable overlap. Each chunk carries heading hierarchy breadcrumb and line range.

### Phase 4 — Embedding Provider System ✅
> [phase-4-embedding-providers.md](phase-4-embedding-providers.md)

Pluggable `EmbeddingProvider` trait with OpenAI and Ollama implementations. Concurrent batch processing (up to 4 simultaneous API calls). Content-hash comparison skips re-embedding unchanged files.

### Phase 5 — Index Storage & Memory Mapping ✅
> [phase-5-index-storage.md](phase-5-index-storage.md)

Single portable index file: `[header][rkyv metadata][usearch HNSW vectors]`. Memory-mapped via memmap2 for fast on-demand loading. usearch HNSW for sub-millisecond vector search. Concurrent access via parking_lot RwLock.

### Phase 6 — Semantic Search & Metadata Filtering ✅
> [phase-6-semantic-search.md](phase-6-semantic-search.md)

Query pipeline: embed query → HNSW nearest neighbors → metadata filter → section-level results. Filters support exact match, list membership, numeric range, and field existence. Builder-pattern query construction.

### Phase 7 — Metadata Schema System ✅
> [phase-7-metadata-schema.md](phase-7-metadata-schema.md)

Auto-infer schema from frontmatter (field names, types, sample values). Optional `.markdownvdb.schema.yml` overlay for annotations, type overrides, and allowed values. Schema persisted in index and updated during ingest and watch events.

### Phase 8 — File Watching & Incremental Updates ✅
> [phase-8-file-watching.md](phase-8-file-watching.md)

Filesystem watcher with debouncing. On change: hash comparison → parse → chunk → embed → upsert → schema inference. Stale entry cleanup. Full ingest pipeline with clustering integration.

### Phase 9 — Clustering ✅
> [phase-9-clustering.md](phase-9-clustering.md)

K-means clustering on document-level vectors (averaged chunk vectors per file). Integrated into ingest pipeline. Incremental nearest-centroid assignment for single-file ingests. Cross-cluster TF-IDF keyword extraction. Auto-generated labels.

### Phase 10 — Agent Interface (CLI + Library) ✅
> [phase-10-cli-library.md](phase-10-cli-library.md)

Full CLI: `mdvdb search|ingest|status|schema|clusters|get|watch|init`. Wrapped JSON output for agents, human-readable fallback. Library API via `MarkdownVdb` struct. 14 CLI tests + 9 API tests.

---

## Sprint 1.5 — Polish (Phase 11) ✅

### Phase 11 — Environment Variable & Config Loading ✅
> [phase-11-environment-vars-and-config.md](phase-11-environment-vars-and-config.md)

`.env` file loaded as fallback config source. Priority: shell env > `.markdownvdb` > `.env` > defaults. Shared secrets like `OPENAI_API_KEY` live in `.env` (gitignored) and are picked up automatically.

---

## Sprint 2 — CLI Polish & Search Power (Phases 12–14)

### Phase 12 — Making CLI Great
> [phase-12-cli-great.md](phase-12-cli-great.md)

Transform the CLI from plain text into a polished terminal experience. New `src/format.rs` module centralizes all human-readable formatting with colored output, ASCII art branding, score bars, distribution bars, progress spinners (`indicatif`), and humanized values ("2 hours ago", "1.5 MB"). TTY-aware with `--no-color` flag and `NO_COLOR` env var support. JSON output remains completely untouched.

**New crate dependencies:** `colored`, `indicatif`

**Key additions:**
- ASCII logo for `mdvdb` with no subcommand and `--version`
- Score bars next to search results, distribution bars for clusters, occurrence bars for schema
- Progress spinner during ingest (TTY only)
- Humanized timestamps and file sizes in human output
- Surface cluster keywords, document frontmatter, and file metadata that were previously JSON-only

---

### Phase 13 — File Tree Index & Path-Scoped Search
> [phase-13-file-tree-path-scoped-search.md](phase-13-file-tree-path-scoped-search.md)

New `mdvdb tree` command showing ASCII tree view of indexed documents with colored file-state indicators (indexed/modified/new/deleted). Path-scoped search via `--path docs/api/` restricts results to a directory subtree. `path_components` field added to search results for hierarchical context. All computed on-the-fly from existing index data — no new stored structures.

**No new crate dependencies.** Uses `sha2` (existing) for hash comparison and `std::io::IsTerminal` for TTY detection.

**Key additions:**
- `mdvdb tree [--path <prefix>] [--json]` — file tree with state indicators
- `mdvdb search --path <prefix>` — restrict search to subtree
- `path_components: Vec<String>` on `SearchResultFile`
- `vdb.file_tree()` library API method
- New `src/tree.rs` module with `FileState`, `FileTreeNode`, `FileTree` types

---

### Phase 14 — Hybrid Search (Semantic + Lexical BM25)
> [phase-14-hybrid-search.md](phase-14-hybrid-search.md)

Add fast BM25 lexical search via Tantivy alongside existing HNSW semantic search, combined through Reciprocal Rank Fusion (RRF). Default mode is hybrid (both signals). Lexical-only mode works without any API key — pure local, sub-millisecond search.

**New crate dependency:** `tantivy = "0.22"`

**Key additions:**
- New `src/fts.rs` module wrapping Tantivy — schema, upsert, remove, search, commit
- `.markdownvdb.fts/` directory for Tantivy segment files (separate from binary index)
- `SearchMode` enum: `Hybrid` (default), `Semantic`, `Lexical`
- CLI flags: `--mode hybrid|semantic|lexical` + shorthand `--semantic`/`--lexical`
- RRF fusion: `score(doc) = Σ 1/(k + rank)` with k=60 (industry standard)
- Markdown stripping via `pulldown-cmark` before FTS indexing
- Heading hierarchy boosted 1.5x in BM25 scoring
- Config: `MDVDB_SEARCH_MODE`, `MDVDB_FTS_INDEX_DIR`, `MDVDB_SEARCH_RRF_K`

---

### Phase 15 — Link Graph & Backlinks
> [phase-15-link-graph.md](phase-15-link-graph.md)

Extract internal markdown links (`[text](path.md)` and `[[wikilinks]]`) during parsing to build a persistent link graph. Backlink queries, orphan detection, broken link checking, and optional link-aware search boosting. Three new CLI commands: `links`, `backlinks`, `orphans`.

**No new crate dependencies.** Uses `pulldown-cmark` (existing) for link extraction.

**Key additions:**
- Link extraction in parser: `[text](path.md)` + `[[wikilink]]` syntax
- `LinkGraph` stored in `IndexMetadata` (follows Schema/ClusterState pattern)
- `mdvdb links <file>` — outgoing links + incoming backlinks with tree rendering
- `mdvdb backlinks <file>` — files that link TO this file
- `mdvdb orphans` — files with no links in or out
- `mdvdb search --boost-links` — boost results that are link neighbors of top hits
- Broken link detection with `[broken]` state badges
- New `src/links.rs` module with `LinkGraph`, `LinkEntry`, `ResolvedLink`, `LinkState` types

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
  └──────────────┬──────────────────────────┘
                 │
               ┌─▼──┐
               │ 11 │ Config (.env fallback)
               └─┬──┘
                 │
               ┌─▼──┐
               │ 12 │ CLI Polish (format.rs, colors, spinners)
               └─┬──┘
                 │
          ┌──────┼──────┐
        ┌─▼──┐ ┌─▼──┐ ┌─▼──┐
        │ 13 │ │ 14 │ │ 15 │ Link Graph
        │Tree│ │BM25│ │    │ (uses 2, 5)
        └────┘ └────┘ └────┘
```

**Sprint 1 critical path:** 1 → 2 → 3 → 4 → 5 → 6 → 10 ✅
**Sprint 1.5:** 11 (config polish) ✅
**Sprint 2:** 12 first (CLI formatting used by 13+). 13, 14, and 15 are independent of each other after 12.

---

## Execution

To implement any phase:

```
/execute-prd docs/prds/phase-N-<name>.md
```

Each PRD is self-contained — a fresh session with only that PRD has all the context needed to implement the phase.

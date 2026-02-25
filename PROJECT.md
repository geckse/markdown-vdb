# Markdown VDB — Project Specification

A filesystem-native vector database built entirely around Markdown files. Designed for AI agents that need fast semantic search over local knowledge bases.

---

## Core Principles

- **Zero infrastructure** — no servers, no containers, everything lives on the filesystem
- **Markdown-first** — `.md` files are the source of truth, not rows in a table
- **Agent-oriented** — optimized for speed and programmatic access by AI agents
- **Non-destructive** — never alters markdown content or frontmatter

---

## Must-Have Functionality

### 1. Ingestion

- Recursively scan one or more source folders for `.md` files
- Respect `.gitignore` rules automatically; additionally support `MDVDB_IGNORE_PATTERNS` in config for extra excludes (glob syntax)
- Apply **built-in default ignore patterns** for directories that commonly contain markdown but should never be indexed:
  - `.claude/` — Claude Code agent memory and context
  - `.cursor/` — Cursor IDE settings
  - `.vscode/` — VSCode settings
  - `.idea/` — JetBrains IDE settings
  - `.git/` — git internals
  - `node_modules/` — npm dependencies
  - `.obsidian/` — Obsidian app config
  - `__pycache__/` — Python cache
  - `.next/`, `.nuxt/`, `.svelte-kit/` — framework build output
  - `target/` — Rust/Maven build output
  - `dist/`, `build/`, `out/` — general build output
- Built-in ignores are always applied and cannot be overridden
- Parse each file into: frontmatter metadata, heading structure, and body content
- Generate vector embeddings from file content (see §3 Chunking, §4 Embedding Providers)
- Compute a **SHA-256 content hash** per file and store it in the index — skip re-embedding when content hasn't actually changed
- Store all index data (vectors, metadata snapshots, chunk mappings, file references, content hashes) in a **single portable index file** on disk
- All file paths stored as **relative to the project root** (where `.markdownvdb` lives)
- Support re-ingestion of individual files without rebuilding the full index
- Handle file deletions — remove stale entries from the index automatically

### 2. File Watching

- Watch designated folders for filesystem events (create, modify, rename, delete)
- On change, compare content hash against the stored hash — only re-embed if content actually changed
- Debounce rapid successive changes to the same file
- Work across nested subdirectories

### 3. Chunking

Documents are split using a **hybrid strategy** for optimal search quality:

- **Primary split: by headings** — each heading (h1–h6) starts a new chunk, preserving the document's natural structure
- **Secondary split: size guard** — sections exceeding a max token limit are sub-split into overlapping fixed-size chunks
- Each chunk retains a reference to its parent file, heading hierarchy, and line range
- Short files that fall under the token limit remain a single chunk

### 4. Embedding Providers

Embeddings are **pluggable** — the user configures their provider in the `.markdownvdb` config file:

- Ship with a sensible default provider (API-based)
- Support local inference (e.g. Ollama) as an alternative provider
- Provider config specifies: endpoint, model name, dimensions, auth (if needed)
- The system defines a standardized embedding interface — any provider that implements it can be swapped in
- Provider choice is transparent to the rest of the system; all internal code works with raw vectors regardless of source

### 5. Semantic Search

- Accept a natural-language query and return the top-N most relevant results
- **Results are section-level**: each result returns the matched chunk's heading, content excerpt, line range, parent file path, relevance score, and full file metadata
- Agents get both precision (the specific section) and context (which file it belongs to)
- **Must be fast** — sub-100ms query response on warm index with 10k+ documents
- Support configurable result count and similarity threshold

### 6. Index Loading & Caching

- Index is loaded **on-demand** per invocation — no persistent daemon required
- Use memory-mapped files or a shared cache layer so that the first invocation pays the load cost but subsequent calls within a reasonable window are near-instant
- Target: cold load under 500ms for a 10k-doc index; warm queries under 100ms

### 7. Concurrency

- Multiple readers can query the index simultaneously
- Writes (re-indexing from watcher or manual ingest) acquire an exclusive write lock
- Queries arriving during a write either wait briefly or read from the last consistent snapshot
- No data corruption under concurrent access

### 8. Metadata & Filtering

- Read YAML frontmatter from each markdown file as structured metadata
- Support filtering search results by metadata fields (exact match, range, list membership)
- Combine semantic search with metadata filters in a single query
- Metadata filters must not degrade query performance significantly

### 9. Metadata Schema

The schema uses an **infer + overlay** approach:

- **Auto-infer (baseline)**: on ingest, scan all frontmatter and automatically discover field names, infer types (string, number, boolean, list, date), and collect observed values
- **User overlay (optional)**: an optional `.markdownvdb.schema.yml` file lets users annotate, override, or extend the inferred schema — add descriptions, constrain allowed values, mark fields as required, or define fields that don't exist yet
- The merged schema (inferred + overlay) is persisted in the index so agents can introspect it
- Agents can query the schema to discover what filterable fields exist before searching
- Schema acts as documentation, not strict enforcement — files missing fields are still indexed

### 10. Clustering

- Automatically group semantically similar documents into clusters
- Assign new documents to the nearest existing cluster on ingest
- Periodically rebalance clusters to prevent drift as the corpus evolves
- Cluster assignments are stored **only in the index file** — frontmatter is never modified
- Expose cluster summaries (label, document count, representative keywords) so agents can browse topics before searching

### 11. Frontmatter Contract

Every indexed markdown file should support (but not require) frontmatter like:

```yaml
---
title: "Document Title"
tags: [tag1, tag2]
# ...additional user-defined fields per schema
---
```

- The system **reads** all frontmatter fields into metadata
- The system **never writes** to markdown files — all computed data (clusters, vectors, scores) lives in the index
- Unknown/extra fields are preserved and indexed as-is

### 12. Index File

- All vector data, metadata snapshots, chunk mappings, cluster assignments, content hashes, and schema live in a single index file
- The file must be portable — moving it with the markdown folder to another machine should just work
- File paths stored as relative to the project root
- Index file is the only artifact the system produces; deleting it triggers a full re-index on next run
- Must support memory-mapping for fast on-demand loading (see §6)

### 13. Agent Interface

The system exposes two interfaces — a **library** for programmatic use and a **CLI** for shell-based agents and scripting:

#### Operations

- **Search** — semantic query with optional metadata filters; returns section-level results with file context
- **Inspect schema** — list available metadata fields and types
- **List clusters** — get all cluster labels with document counts and summaries
- **Get document** — retrieve full content + metadata for a specific file
- **Get status** — check index health, document count, last updated timestamp
- **Ingest** — trigger manual (re-)ingestion of one or more files or full rebuild

#### CLI

- All operations available as subcommands (e.g. `mdvdb search "query"`, `mdvdb status`, `mdvdb ingest`)
- Output as JSON for easy parsing by agents
- Exit codes for scripting (0 = success, non-zero = error with message on stderr)

#### Library

- Importable module with the same operations as typed function calls
- Returns structured objects, not strings
- Async-friendly for integration into agent loops

---

## Configuration

The `.markdownvdb` file in the project root serves as both config and environment file (dotenv-style). All settings are defined as environment variables with sane defaults — if the file doesn't exist, the system runs with defaults only.

### Resolution Order

1. Shell environment (highest priority — always wins)
2. `.markdownvdb` file
3. Built-in defaults (lowest priority)

### Variables & Defaults

#### Embedding Provider

| Variable | Default | Description |
|---|---|---|
| `MDVDB_EMBEDDING_PROVIDER` | `openai` | Provider type: `openai`, `ollama`, `custom` |
| `MDVDB_EMBEDDING_MODEL` | `text-embedding-3-small` | Model name for embedding generation |
| `MDVDB_EMBEDDING_DIMENSIONS` | `1536` | Vector dimensions (must match model output) |
| `MDVDB_EMBEDDING_BATCH_SIZE` | `100` | Number of chunks per embedding request |
| `OPENAI_API_KEY` | — | API key for OpenAI-compatible providers |
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama server endpoint |
| `MDVDB_EMBEDDING_ENDPOINT` | — | Custom endpoint URL (when provider is `custom`) |

#### Source & Index

| Variable | Default | Description |
|---|---|---|
| `MDVDB_SOURCE_DIRS` | `.` | Comma-separated list of folders to scan (relative to project root) |
| `MDVDB_INDEX_FILE` | `.markdownvdb.index` | Path to the index file |
| `MDVDB_IGNORE_PATTERNS` | — | Comma-separated glob patterns to exclude (in addition to `.gitignore` and built-in ignores) |
| `MDVDB_WATCH` | `true` | Enable/disable file watching |
| `MDVDB_WATCH_DEBOUNCE_MS` | `300` | Debounce interval for file change events |

#### Chunking

| Variable | Default | Description |
|---|---|---|
| `MDVDB_CHUNK_MAX_TOKENS` | `512` | Max tokens per chunk before sub-splitting |
| `MDVDB_CHUNK_OVERLAP_TOKENS` | `50` | Token overlap between sub-split chunks |

#### Clustering

| Variable | Default | Description |
|---|---|---|
| `MDVDB_CLUSTERING_ENABLED` | `true` | Enable/disable automatic clustering |
| `MDVDB_CLUSTERING_REBALANCE_THRESHOLD` | `50` | Number of new documents before triggering rebalance |

#### Search

| Variable | Default | Description |
|---|---|---|
| `MDVDB_SEARCH_DEFAULT_LIMIT` | `10` | Default number of results returned |
| `MDVDB_SEARCH_MIN_SCORE` | `0.0` | Minimum similarity score threshold |

### Example `.markdownvdb`

```env
# Embedding — use local Ollama
MDVDB_EMBEDDING_PROVIDER=ollama
MDVDB_EMBEDDING_MODEL=nomic-embed-text
MDVDB_EMBEDDING_DIMENSIONS=768
OLLAMA_HOST=http://localhost:11434

# Source
MDVDB_SOURCE_DIRS=docs,notes,wiki
MDVDB_INDEX_FILE=.markdownvdb.index
MDVDB_IGNORE_PATTERNS=drafts/**,archive/**

# Chunking
MDVDB_CHUNK_MAX_TOKENS=1024

# Clustering
MDVDB_CLUSTERING_REBALANCE_THRESHOLD=100
```

---

## Non-Goals (for now)

- No GUI or web interface
- No multi-user or access control
- No cloud sync or remote storage
- No support for non-markdown file types
- No query language — filters are structured objects, not SQL/DSL
- No frontmatter modification — the system is strictly read-only on source files

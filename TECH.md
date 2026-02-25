# Markdown VDB — Technology Decisions

Rust toolchain mapped to each functional area of the project. Every choice justifies **why this crate** for **this requirement**.

---

## CLI — `clap`

The agent interface is CLI-first (`mdvdb search`, `mdvdb status`, `mdvdb ingest`).

- **Why `clap`**: derive macros turn struct definitions into full CLI parsers — subcommands, flags, validation, help text, and shell completions all generated from code. It's the de facto standard in Rust CLI tooling.
- JSON output mode (`--json`) is just `serde_json::to_string` on the response structs — no extra work.

```
clap = { version = "4", features = ["derive"] }
```

---

## Async Runtime — `tokio`

Embedding API calls, file watching, and concurrent index access all need async.

- **Why `tokio`**: the Rust async ecosystem is built around it. `reqwest`, `notify`, and most libraries assume Tokio. Single-threaded runtime is enough here — we're I/O-bound (HTTP calls, file reads), not CPU-bound.
- The CLI entry point uses `#[tokio::main]` and everything flows from there.

```
tokio = { version = "1", features = ["rt", "macros", "fs"] }
```

---

## Markdown Parsing — `pulldown-cmark`

Ingestion needs to parse markdown into heading structure, body content, and understand document hierarchy for chunking.

- **Why `pulldown-cmark`**: streaming event-based CommonMark parser. Emits `Start(Heading)`, `Text`, `End(Heading)` events that map directly to our chunking strategy — split on heading events, accumulate text between them. Zero allocations for events we don't care about.
- Fast enough to parse thousands of files during full re-index without being the bottleneck.

```
pulldown-cmark = "0.12"
```

---

## Frontmatter / YAML — `serde_yaml`

Every markdown file can have YAML frontmatter that becomes filterable metadata.

- **Why `serde_yaml`**: deserializes YAML into `serde_json::Value` (or typed structs). We need dynamic typing since frontmatter fields are user-defined and schema-flexible. Parsing into `Value` lets us store heterogeneous metadata without knowing the shape ahead of time.
- Frontmatter extraction is just splitting on `---` delimiters before passing to `serde_yaml`.

```
serde_yaml = "0.9"
```

---

## Embedding Providers — `reqwest`

Pluggable embedding providers (OpenAI, Ollama, custom) all speak HTTP.

- **Why `reqwest`**: async HTTP client with connection pooling, timeouts, retries. OpenAI and Ollama both expose REST APIs — same `reqwest::Client` with different base URLs and headers.
- The embedding provider trait is simple: `async fn embed(&self, texts: Vec<String>) -> Vec<Vec<f32>>`. Each provider implements it with `reqwest` calls internally.
- Batch support (`MDVDB_EMBEDDING_BATCH_SIZE`) is just chunking the input vec and making parallel requests.

```
reqwest = { version = "0.12", features = ["json"] }
```

---

## Vector Search — `usearch`

The core performance requirement: sub-100ms semantic search over 10k+ document chunks.

- **Why `usearch`**: production-grade HNSW (Hierarchical Navigable Small World) index with native Rust bindings. Sub-millisecond approximate nearest neighbor search on 10k vectors — not sub-100ms, sub-1ms. Built-in support for **memory-mapped indexes** — the HNSW graph lives on disk and is mmap'd directly, meaning cold start loads the index without reading the entire file into RAM. This single crate satisfies both §5 (fast search) and §6 (memory-mapped caching).
- Supports cosine similarity, inner product, and L2 distance.
- Vectors can be added and removed incrementally without rebuilding — matches our file-watching incremental update requirement.

```
usearch = "2"
```

---

## Index Serialization — `rkyv`

The index file stores metadata, chunk mappings, content hashes, cluster assignments, and schema alongside the vector index.

- **Why `rkyv`**: zero-copy deserialization. The serialized bytes can be memory-mapped and accessed as native Rust structs **without deserialization**. No parsing step, no allocation — just cast the mmap'd region and read. This is what makes cold start under 500ms realistic for 10k docs.
- `usearch` handles its own vector index persistence. `rkyv` handles everything else: metadata, chunk text, file paths, hashes, schema, clusters.
- The index file is a concatenation: `[rkyv metadata region][usearch HNSW region]` with a header pointing to each section's offset.

```
rkyv = { version = "0.8", features = ["validation"] }
```

---

## Memory Mapping — `memmap2`

On-demand index loading without reading the full file into memory.

- **Why `memmap2`**: safe Rust wrapper around `mmap`. Maps the index file into the process's virtual address space — the OS loads pages on demand as they're accessed. Combined with `rkyv` (zero-copy) and `usearch` (native mmap), the entire query path avoids copying data.
- Subsequent invocations within a reasonable window hit the OS page cache — effectively free.

```
memmap2 = "0.9"
```

---

## File Watching — `notify`

Incremental re-indexing on file changes.

- **Why `notify`**: cross-platform filesystem event library (FSEvents on macOS, inotify on Linux, ReadDirectoryChanges on Windows). Emits create/modify/rename/delete events on watched directories, recursively.
- Debouncing is built in via `notify-debouncer-full` — set `MDVDB_WATCH_DEBOUNCE_MS` and duplicate events within that window are collapsed.

```
notify = "7"
notify-debouncer-full = "0.4"
```

---

## Content Hashing — `sha2`

Skip re-embedding when a file is saved but content hasn't changed.

- **Why `sha2`**: pure Rust SHA-256 implementation. Fast enough to hash thousands of markdown files during ingestion without being noticeable. The hash is compared against the stored hash in the index — if identical, skip the embedding API call entirely.
- This is the difference between "save file = $0" and "save file = embedding API cost" for unchanged files.

```
sha2 = "0.10"
```

---

## Concurrency — `parking_lot`

Multiple readers querying while the watcher writes.

- **Why `parking_lot`**: drop-in replacement for `std::sync::RwLock` that's faster (no syscall overhead for uncontended locks), never poisons, and has a smaller footprint. Read-heavy workload (many queries, rare writes) is exactly where `RwLock` excels.
- Write lock is acquired only during index updates (re-embedding a changed file). Readers either wait briefly or proceed with the pre-update snapshot.

```
parking_lot = "0.12"
```

---

## Clustering — `linfa`

Automatic document grouping with incremental assignment and periodic rebalance.

- **Why `linfa`**: Rust's ML framework, modeled after scikit-learn. `linfa-clustering` provides k-means (for rebalance) and nearest-centroid assignment (for incremental). The workflow:
  1. **Initial cluster**: run k-means on all document vectors → assign centroids
  2. **Incremental ingest**: assign new document to nearest existing centroid (no recluster)
  3. **Rebalance** (after N new docs): re-run k-means on all vectors → update centroids and assignments
- Centroids are stored in the index file. Nearest-centroid lookup is a single vector comparison per cluster — trivially fast.

```
linfa = "0.7"
linfa-clustering = "0.7"
```

---

## Token Counting — `tiktoken-rs`

Chunking needs to know when a section exceeds the max token limit.

- **Why `tiktoken-rs`**: Rust port of OpenAI's tokenizer. Accurate token counts that match what the embedding model actually sees. Using word count or character count as a proxy is unreliable — "don't" is 1 word but 2 tokens.
- Used only during chunking, not on the query path, so it doesn't affect search latency.

```
tiktoken-rs = "0.6"
```

---

## Config / Dotenv — `dotenvy`

The `.markdownvdb` file is a dotenv-style env file.

- **Why `dotenvy`**: maintained fork of the original `dotenv` crate. Reads key-value pairs from a file, respects shell environment overrides (our resolution order: shell > file > defaults). No TOML, no YAML config — just flat env vars as specified.

```
dotenvy = "0.15"
```

---

## Ignore Patterns — `ignore`

Respect `.gitignore` and custom `MDVDB_IGNORE_PATTERNS` during ingestion.

- **Why `ignore`**: built by the ripgrep author. Natively parses `.gitignore` files and applies glob matching during directory traversal. Faster than walking the full tree and filtering after — it prunes ignored directories entirely.
- Custom ignore patterns from config are added as additional rules on top.

```
ignore = "0.4"
```

---

## Schema File — `serde_yaml` (reused)

The optional `.markdownvdb.schema.yml` overlay file.

- Already using `serde_yaml` for frontmatter — same crate handles the schema file. Deserialize into a `Schema` struct that merges with the auto-inferred schema from the index.

---

## Error Handling — `thiserror` + `anyhow`

- **`thiserror`** for the library layer: typed errors with `#[derive(Error)]` that consumers can match on (e.g., `IndexNotFound`, `EmbeddingProviderError`, `InvalidConfig`).
- **`anyhow`** for the CLI layer: wraps any error into a printable chain with context. CLI exits with code 1 and a human-readable message on stderr.

```
thiserror = "2"
anyhow = "1"
```

---

## Serialization — `serde` + `serde_json`

The backbone for JSON CLI output, API request/response serialization, and internal data structures.

- Every struct that touches I/O derives `Serialize`/`Deserialize`. JSON output for the CLI is just `serde_json::to_string_pretty`.

```
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

---

## Logging — `tracing`

Observable ingestion, search, and watch operations.

- **Why `tracing`**: structured, async-aware logging. Spans track "ingesting file X" or "searching query Y" with timing. The CLI controls verbosity (`-v`, `-vv`). In library mode, consumers attach their own subscriber.

```
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

---

## Summary

| Requirement | Crate | Why |
|---|---|---|
| CLI interface | `clap` | Derive-based, subcommands, completions |
| Async runtime | `tokio` | Ecosystem standard, I/O-bound workload |
| Markdown parsing | `pulldown-cmark` | Streaming events, heading-aware chunking |
| YAML frontmatter | `serde_yaml` | Dynamic typing for user-defined fields |
| HTTP / embeddings | `reqwest` | Async, pooled, works with any REST API |
| Vector search | `usearch` | Sub-ms HNSW, native mmap, incremental |
| Index serialization | `rkyv` | Zero-copy deserialization from mmap |
| Memory mapping | `memmap2` | On-demand loading, OS page cache |
| File watching | `notify` | Cross-platform, built-in debouncer |
| Content hashing | `sha2` | Skip unchanged files, save API costs |
| Concurrency | `parking_lot` | Fast RwLock for read-heavy workloads |
| Clustering | `linfa` | K-means + nearest-centroid assignment |
| Token counting | `tiktoken-rs` | Accurate token limits for chunking |
| Config / dotenv | `dotenvy` | Flat env file with shell override |
| Ignore patterns | `ignore` | Gitignore-native, prunes on traversal |
| Error handling | `thiserror` + `anyhow` | Typed lib errors, ergonomic CLI errors |
| Serialization | `serde` + `serde_json` | JSON output, request/response bodies |
| Logging | `tracing` | Structured, async-aware, spans |

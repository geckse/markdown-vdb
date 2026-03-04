# PRD: Phase 19 — Concurrent Access & Read-Only Mode

## Overview

Enable multiple `mdvdb` processes to safely access the same index simultaneously. A long-running watcher process holds exclusive write locks on the Tantivy FTS index. Read-only CLI commands (search, status, get, links, backlinks, tree, etc.) must run concurrently without blocking, even while the watcher is active. This is critical for the Electron desktop app, which spawns `mdvdb watch` as a background child process and issues read-only CLI commands for UI interactions.

## Problem Statement

Tantivy's `IndexWriter` acquires an exclusive file lock (`directory.lock`) on the FTS index directory. Before this phase, every `MarkdownVdb::open_with_config()` call — including for read-only operations — created an `IndexWriter`, attempting to acquire this exclusive lock. When the watcher process held the lock, any concurrent CLI invocation (e.g., `mdvdb get notes/readme.md --json`) would block indefinitely waiting for the lock, causing the desktop app to freeze.

The root issue: no distinction between read and write access modes for the FTS index and the `MarkdownVdb` API.

## Goals

- Read-only CLI commands run instantly, even when a watcher process holds the FTS write lock
- The watcher (and ingest) retain exclusive write access to the FTS index
- No data corruption or race conditions between concurrent readers and a single writer
- Desktop app can spawn `mdvdb watch` and issue read-only commands in parallel without freezing
- All existing tests continue to pass; new tests verify concurrent access

## Non-Goals

- Multiple concurrent writers — only one process may write at a time (enforced by Tantivy's lock)
- Distributed access across machines or network filesystems
- Hot-reloading of index changes in a running read-only process (readers see a snapshot)
- Changes to the vector index (`usearch`) locking — it uses in-process `parking_lot::RwLock` which is already per-process and doesn't conflict

## Technical Design

### FTS Index: Read-Only Mode

Add `FtsIndex::open_readonly()` that opens the Tantivy index without acquiring a writer. Read operations (`search`, `num_docs`) work normally. Write operations (`upsert_chunks`, `remove_file`, `commit`, `delete_all`) return an error.

```rust
pub struct FtsIndex {
    index: Index,
    fields: FtsFields,
    writer: Option<parking_lot::Mutex<IndexWriter>>,  // None in read-only mode
}

impl FtsIndex {
    /// Opens with exclusive writer lock (for ingest/watch).
    pub fn open_or_create(path: &Path) -> Result<Self>;

    /// Opens without writer lock (for search/status/get/etc.).
    pub fn open_readonly(path: &Path) -> Result<Self>;
}
```

### MarkdownVdb: Read-Only Constructor

Add `MarkdownVdb::open_readonly()` and `open_readonly_with_config()` that use `FtsIndex::open_readonly` internally. Identical to the regular constructors except for the FTS open mode.

```rust
impl MarkdownVdb {
    /// Full read-write access (for ingest, watch).
    pub fn open(root: &Path) -> Result<Self>;
    pub fn open_with_config(root: PathBuf, config: Config) -> Result<Self>;

    /// Read-only access (for search, status, get, tree, links, etc.).
    pub fn open_readonly(root: &Path) -> Result<Self>;
    pub fn open_readonly_with_config(root: PathBuf, config: Config) -> Result<Self>;
}
```

### CLI Command Classification

| Mode | Commands |
|---|---|
| **Read-write** (`open_with_config`) | `ingest`, `watch` |
| **Read-only** (`open_readonly_with_config`) | `search`, `status`, `schema`, `clusters`, `tree`, `get`, `links`, `backlinks`, `orphans`, `doctor` |
| **No index needed** | `init`, `config`, `completions` |

### NDJSON Watch Output Fix

The `mdvdb watch --json` command must emit single-line JSON (NDJSON) on stdout, not pretty-printed multi-line JSON. The desktop app's `WatcherManager` parses stdout line-by-line — multi-line JSON causes the parser to never see a complete object, so the watcher appears stuck in "starting" state.

### Migration Strategy

Fully backward compatible. Existing indexes work unchanged. The only behavioral change: read-only commands no longer acquire the FTS writer lock. No on-disk format changes.

## Implementation Steps

1. **Make `FtsIndex.writer` optional** — In `src/fts.rs`, change `writer: parking_lot::Mutex<IndexWriter>` to `writer: Option<parking_lot::Mutex<IndexWriter>>`. Update `open_or_create` to wrap the writer in `Some(...)`.

2. **Add `FtsIndex::open_readonly()`** — Same as `open_or_create` but sets `writer: None`. Opens the Tantivy index for reading only.

3. **Guard write methods** — In `upsert_chunks`, `remove_file`, `commit`, `delete_all`: unwrap the `Option` with an error message if `None` (read-only mode).

4. **Add `MarkdownVdb::open_readonly()` and `open_readonly_with_config()`** — Mirror the existing constructors but call `FtsIndex::open_readonly` instead of `open_or_create`. Extract shared validation logic into a private `finish_open()` helper to avoid duplication.

5. **Update `src/main.rs`** — Change all read-only CLI commands to use `MarkdownVdb::open_readonly_with_config()`. Keep `ingest` and `watch` using `open_with_config()`.

6. **Fix NDJSON output in watch command** — In `src/main.rs`, change the initial watch status message from `serde_json::to_writer_pretty` to `serde_json::to_string` + `println!` so it emits a single line.

7. **Write concurrent access tests** — Add tests verifying:
   - `FtsIndex::open_readonly` can open an index while `open_or_create` holds the writer
   - Write operations on a read-only `FtsIndex` return appropriate errors
   - `MarkdownVdb::open_readonly` succeeds when another process holds the FTS lock
   - Read-only search returns correct results from a pre-built index

8. **Write NDJSON output test** — Verify `mdvdb watch --json` emits valid single-line JSON as its first output.

## Validation Criteria

- [ ] `FtsIndex::open_readonly()` opens without blocking, even when another process holds the writer lock
- [ ] `FtsIndex::open_readonly()` `search()` and `num_docs()` return correct results
- [ ] Write operations on a read-only `FtsIndex` return `Error::Fts("FTS index opened in read-only mode")`
- [ ] `mdvdb get <file> --json` returns instantly while `mdvdb watch` is running in another process
- [ ] `mdvdb search <query> --json` returns results while `mdvdb watch` is running
- [ ] `mdvdb tree --json` returns instantly while `mdvdb watch` is running
- [ ] `mdvdb status --json` returns instantly while `mdvdb watch` is running
- [ ] `mdvdb watch --json` emits a single-line NDJSON object as its first stdout line
- [ ] Desktop app: clicking a file in the tree loads content without freezing
- [ ] Desktop app: watcher state transitions to "running" correctly
- [ ] `cargo test` passes all tests (including new concurrent access tests)
- [ ] `cargo clippy --all-targets` reports zero warnings

## Anti-Patterns to Avoid

- **Do NOT acquire a Tantivy writer for read-only operations** — This is the root cause of the freeze. Tantivy's writer lock is process-exclusive and file-based. Any process that calls `index.writer()` will block if another process already holds it.
- **Do NOT use pretty-printed JSON on stdout for long-running commands** — When the consuming process parses NDJSON line-by-line, multi-line JSON objects are never seen as complete. Always use single-line `serde_json::to_string` for streaming JSON output.
- **Do NOT share a single `MarkdownVdb` instance between read and write paths** — The constructor determines the access mode. Don't add runtime mode-switching; it creates confusing state. Pick the right constructor for the use case.
- **Do NOT add locking at the CLI layer** — The locking is handled by Tantivy's directory lock (for FTS writes) and `parking_lot::RwLock` (for vector index in-process access). Adding another lock layer creates deadlock risk.

## Patterns to Follow

- **Constructor variants for access mode:** Follow the Rust convention of offering `open` vs `open_readonly` constructors (similar to `std::fs::File::open` vs `File::create`). The caller picks the right one.
- **Option-wrapped writer:** Using `Option<Mutex<IndexWriter>>` is idiomatic Rust for "sometimes present" resources. Guard methods with `.as_ref().ok_or_else(...)` for clear error messages.
- **Shared constructor logic:** Use a private `finish_open()` method to avoid duplicating config validation, dimension checking, and logging between the read-write and read-only constructors. See `src/lib.rs::finish_open()`.
- **Error propagation:** Write operations on read-only instances return `Error::Fts(...)` — not panics. The caller can handle this gracefully.
- **CLI command classification:** Explicitly categorize each CLI command as read-only or read-write in `main.rs`. This makes the access pattern obvious in code review.

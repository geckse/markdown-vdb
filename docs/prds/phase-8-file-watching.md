# PRD: Phase 8 — File Watching & Incremental Updates

## Overview

Implement the filesystem watcher that monitors source directories for changes (create, modify, rename, delete) to `.md` files, debounces rapid events, compares content hashes to skip unchanged files, and triggers incremental re-indexing. This keeps the index up-to-date without full rebuilds.

## Problem Statement

After initial ingestion, markdown files change — users edit, create, rename, and delete files continuously. Without a watcher, the index becomes stale and search results reference outdated content. Full re-indexing after every change is too slow for large vaults (10k+ files). Incremental updates — where only changed files are re-embedded — keep the index fresh while minimizing embedding API costs.

## Goals

- Watch all configured `MDVDB_SOURCE_DIRS` for filesystem events (create, modify, rename, delete)
- Debounce rapid successive changes to the same file (`MDVDB_WATCH_DEBOUNCE_MS`)
- On change: compute content hash and compare against stored hash — only re-embed if content actually changed
- Re-index individual changed files without rebuilding the full index
- Handle file deletions — remove stale entries from the index automatically
- Handle file renames — treat as delete + create
- Work across nested subdirectories
- Respect all ignore patterns (`.gitignore`, built-in, custom) for watched events
- Watchable via `MDVDB_WATCH=true` (default) or disabled with `MDVDB_WATCH=false`

## Non-Goals

- No persistent daemon mode — the watcher runs as long as the CLI process is alive (e.g., `mdvdb watch`)
- No webhook or callback notification system
- No batched re-indexing (each file is processed individually after debounce)
- No watching for config file changes
- No watching for schema overlay file changes

## Technical Design

### Data Model Changes

**`Watcher` struct:**

```rust
pub struct Watcher {
    config: Config,
    project_root: PathBuf,
    index: Arc<Index>,
    provider: Arc<dyn EmbeddingProvider>,
    discovery: FileDiscovery,
}
```

**`FileEvent` enum:**

```rust
pub enum FileEvent {
    Created(PathBuf),   // relative path
    Modified(PathBuf),
    Deleted(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}
```

### Interface Changes

```rust
impl Watcher {
    pub fn new(
        config: Config,
        project_root: PathBuf,
        index: Arc<Index>,
        provider: Arc<dyn EmbeddingProvider>,
    ) -> Self;

    /// Start watching — blocks until cancelled or error
    /// Processes events via the provided callback (for testability)
    pub async fn watch(&self, cancel: tokio::sync::CancellationToken) -> Result<()>;

    /// Process a single file event (public for testing)
    pub async fn handle_event(&self, event: FileEvent) -> Result<()>;
}
```

### Event Processing Pipeline

```
1. notify emits raw filesystem event
2. notify-debouncer-full collapses events within MDVDB_WATCH_DEBOUNCE_MS window
3. Filter: only .md files, only files passing ignore rules
4. Classify event: Created | Modified | Deleted | Renamed
5. For Created/Modified:
   a. Read file and compute SHA-256 hash
   b. Compare against stored hash in index (via get_file_hashes())
   c. If hash unchanged → skip (log at debug level)
   d. If hash changed or file is new:
      - Parse file (parser.rs)
      - Chunk file (chunker.rs)
      - Embed chunks (embedding/batch.rs)
      - Upsert into index
      - Update schema inference for new frontmatter fields
      - Save index to disk
6. For Deleted:
   a. Remove file's entries from index
   b. Save index to disk
7. For Renamed:
   a. Remove old path's entries
   b. Process new path as Created
   c. Save index to disk
```

### Migration Strategy

Not applicable — new functionality only.

## Implementation Steps

1. **Create `src/watcher.rs`** — Implement the watcher module:
   - Define `Watcher`, `FileEvent` structs
   - Constructor stores `Arc` references to shared `Index` and `EmbeddingProvider`
   - Create a `FileDiscovery` instance for ignore-pattern checking

2. **Implement `Watcher::watch()`:**
   - Create a `notify_debouncer_full::new_debouncer()` with:
     - Timeout: `Duration::from_millis(config.watch_debounce_ms)`
     - Tick rate: `None` (use default)
   - Add watchers for each directory in `config.source_dirs` using `debouncer.watch(path, RecursiveMode::Recursive)`
   - Use a `tokio::sync::mpsc` channel to bridge the synchronous `notify` callback to async processing
   - In the notify callback: filter to `.md` files, check against ignore patterns using `FileDiscovery`, send `FileEvent` to the channel
   - In the async loop: receive events from channel, call `handle_event()` for each
   - Check `CancellationToken` on each iteration to support graceful shutdown
   - Log at info level: `"Watching {n} directories for changes"`

3. **Implement `Watcher::handle_event()`:**
   - Match on `FileEvent`:
   - **Created / Modified**:
     1. Read file content from disk
     2. Compute SHA-256 hash
     3. Call `index.get_file(&relative_path)` to get stored hash
     4. If stored hash exists and matches → `tracing::debug!("Skipping unchanged file: {}", path)` and return
     5. Parse with `parse_markdown_file()`
     6. Chunk with `chunk_document()`
     7. Embed changed chunks with `embed_chunks()` (passing current hash vs stored hash)
     8. Call `index.upsert()` with new chunks and embeddings
     9. Call `index.save()`
     10. `tracing::info!("Re-indexed: {}", path)`
   - **Deleted**:
     1. Call `index.remove_file(&relative_path)`
     2. Call `index.save()`
     3. `tracing::info!("Removed from index: {}", path)`
   - **Renamed**:
     1. Call `index.remove_file(&from_path)`
     2. Process `to_path` as a Created event
     3. `tracing::info!("Renamed in index: {} → {}", from, to)`

4. **Implement ignore-pattern filtering for events** — Add a method to `FileDiscovery`:
   - `pub fn should_index(&self, relative_path: &Path) -> bool`
   - Checks: is `.md` extension, not matched by `.gitignore`, not matched by built-in ignores, not matched by custom `MDVDB_IGNORE_PATTERNS`
   - Reuses the same `ignore` crate logic from Phase 2

5. **Implement the full ingestion pipeline** — Create `src/ingest.rs`:
   - `pub async fn ingest_full(config, project_root, index, provider) -> Result<IngestResult>`:
     1. Discover all `.md` files
     2. Get existing file hashes from index
     3. For each file: parse → chunk → collect
     4. Embed all new/changed chunks in batches
     5. Upsert all results into index
     6. Remove stale entries (files in index but not on disk)
     7. Infer and merge schema
     8. Save index
   - `pub async fn ingest_file(config, project_root, index, provider, path) -> Result<()>`:
     - Single-file version for watcher use
   - Return `IngestResult { files_indexed, files_skipped, files_removed, chunks_created, api_calls }`

6. **Update `src/lib.rs`** — Add `pub mod watcher;` and `pub mod ingest;`

7. **Write watcher unit tests** — In `src/watcher.rs` `#[cfg(test)] mod tests`:
   - Test: `handle_event(Created)` with new file parses, chunks, embeds, and upserts
   - Test: `handle_event(Modified)` with unchanged hash skips re-embedding
   - Test: `handle_event(Modified)` with changed hash re-embeds and upserts
   - Test: `handle_event(Deleted)` removes file entries from index
   - Test: `handle_event(Renamed)` removes old path and adds new path
   - Test: events for non-`.md` files are ignored
   - Test: events for files matching ignore patterns are ignored
   - Use mock `EmbeddingProvider` and a temp directory with a real `Index`

8. **Write ingest integration tests** — Create `tests/ingest_test.rs`:
   - Test: full ingest of a directory with 5 `.md` files creates correct index
   - Test: second ingest with no changes skips all files (0 API calls)
   - Test: second ingest after modifying 1 file re-embeds only that file
   - Test: ingest after deleting a file removes it from the index
   - Test: `IngestResult` counts are accurate
   - Use mock `EmbeddingProvider` and temp directories

9. **Write watcher integration test** — Create `tests/watcher_test.rs`:
   - Test: start watcher, create a new `.md` file, verify it appears in index within 2 seconds
   - Test: start watcher, modify an existing file, verify index is updated
   - Test: start watcher, delete a file, verify it's removed from index
   - Test: `CancellationToken` gracefully stops the watcher
   - Use `tokio::time::timeout` to prevent tests from hanging

## Validation Criteria

- [ ] Watcher detects new `.md` files in watched directories within `MDVDB_WATCH_DEBOUNCE_MS + 100ms`
- [ ] Watcher detects modifications to existing `.md` files
- [ ] Watcher detects file deletions and removes entries from index
- [ ] Watcher handles file renames as delete-old + create-new
- [ ] Debouncing: saving a file 5 times in 100ms triggers only 1 re-index
- [ ] Content hash comparison: saving a file without changing content does NOT trigger re-embedding
- [ ] Content hash comparison: changing file content DOES trigger re-embedding
- [ ] Non-`.md` file changes are ignored
- [ ] Files matching ignore patterns (`.gitignore`, built-in, custom) are ignored
- [ ] Watcher works across nested subdirectories
- [ ] `MDVDB_WATCH=false` disables watching entirely
- [ ] Full ingest on 100 files with mock provider completes without error
- [ ] Second ingest with no changes makes 0 embedding API calls
- [ ] Stale entries (files deleted from disk) are removed during full ingest
- [ ] Graceful shutdown via `CancellationToken` works without data loss
- [ ] `cargo test` passes all watcher and ingest tests
- [ ] `cargo clippy` reports no warnings

## Anti-Patterns to Avoid

- **Do NOT re-embed every file on every save** — Always compare content hashes first. Most file saves (formatting, whitespace) don't change semantic content enough to warrant an API call.
- **Do NOT process events synchronously in the notify callback** — The `notify` callback runs on a dedicated thread. Send events to an async channel and process them in the tokio runtime. Blocking in the callback can cause event loss.
- **Do NOT save the index after every single chunk operation** — Save after the complete file event is processed (all chunks upserted). Intermediate saves waste I/O.
- **Do NOT watch individual files** — Watch directories with `RecursiveMode::Recursive`. Watching individual files breaks when files are deleted and recreated (new inode).
- **Do NOT ignore rename events** — A rename changes the file's path in the index. If ignored, the old path becomes stale and the new path is never indexed.
- **Do NOT hold the write lock during embedding API calls** — Embedding is slow (network I/O). Read file hashes → release lock → embed → reacquire write lock → upsert. This prevents blocking readers during API calls.

## Patterns to Follow

- **Async bridging:** `notify` is sync, the rest of the system is async. Use `tokio::sync::mpsc` channel to bridge events from the sync callback to the async processing loop.
- **Cancellation:** Use `tokio::sync::CancellationToken` for graceful shutdown — the watcher checks it on each event loop iteration.
- **Lock minimization:** Acquire write lock only for the actual upsert/remove operations, not for file reading, parsing, chunking, or embedding.
- **Error handling:** Log and continue on individual file processing errors (don't crash the watcher). Use `tracing::error!` for failures, `tracing::info!` for successful operations.
- **Module structure:** `src/watcher.rs` for the filesystem watcher, `src/ingest.rs` for the ingestion pipeline (shared between watcher events and CLI `mdvdb ingest` command).

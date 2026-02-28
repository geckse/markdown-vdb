# PRD: Interactive Ingest Progress & CLI Improvements

## Overview

Replace the static "Ingesting markdown files..." spinner with a rich, interactive progress display that shows per-file status, percentage completion, file counts, and current activity. Rename `--full` to `--reindex` (clearer intent). Add `--preview` dry-run mode that estimates work without calling APIs. Add live event reporting to the watch process. Support Ctrl+C to cancel ingestion gracefully.

## Problem Statement

The current ingest command shows only a spinner with a static message — no file names, no progress percentage, no file counts. Users have zero visibility into what's happening during ingestion, which can take minutes for large repositories. The `--full` flag name is unclear (full of what?). The watch process is equally opaque — events are processed silently unless `-v` is used, but verbose mode dumps noisy tracing output unsuitable for interactive use.

## Goals

- Show real-time per-file progress during ingestion: current file, phase (parsing/embedding/saving), files processed vs total, percentage, elapsed time
- Rename `--full` to `--reindex` (keep `--full` as hidden alias for backward compatibility)
- Add `--preview` dry-run mode: discover, parse, chunk, count tokens — report what would happen without calling the embedding API
- Add a progress callback mechanism so the library can report progress without depending on UI crates
- Show live event feedback during watch (file changed, indexed, errors) in a user-friendly format
- Support Ctrl+C to cancel ingestion mid-process (graceful shutdown)
- Preserve JSON mode behavior (no interactive output, structured result only)

## Non-Goals

- No TUI framework (no ratatui/crossterm full-screen UI) — use indicatif multi-progress bars only
- No changes to the ingest pipeline logic itself (discovery, parsing, chunking, embedding, upsert order stays the same)
- No watch progress bars (watch shows event log, not progress bars)
- No changes to the embedding batch concurrency model

## Technical Design

### Data Model Changes

**New `IngestProgress` callback type** in `src/lib.rs`:

```rust
/// Progress events emitted during ingestion.
#[derive(Debug, Clone)]
pub enum IngestPhase {
    /// Scanning for markdown files.
    Discovering,
    /// Parsing and chunking file N of total.
    Parsing { current: usize, total: usize, path: String },
    /// Skipping unchanged file.
    Skipped { current: usize, total: usize, path: String },
    /// Embedding chunks (batch N of total batches).
    Embedding { current_batch: usize, total_batches: usize, chunks_done: usize, chunks_total: usize },
    /// Saving index and FTS.
    Saving,
    /// Clustering documents.
    Clustering,
    /// Removing stale files.
    Cleaning { removed: usize },
    /// Complete.
    Done,
}

/// Callback type for progress reporting. Takes a reference to avoid cloning overhead.
pub type ProgressCallback = Box<dyn Fn(&IngestPhase) + Send + Sync>;
```

**Updated `IngestOptions`:**

```rust
pub struct IngestOptions {
    /// Force re-embedding of all files (reindex).
    pub full: bool,
    /// Ingest a specific file only.
    pub file: Option<PathBuf>,
    /// Optional progress callback for real-time reporting.
    pub progress: Option<ProgressCallback>,
}
```

Note: `IngestOptions` currently derives `Default` and `Clone`. Since `ProgressCallback` is not `Clone`, we need to either:
- Remove `Clone` from `IngestOptions` (preferred — it's only constructed once in `main.rs`)
- Or wrap in `Arc`. Removing `Clone` is simpler and matches actual usage.

**New `IngestPreview` result type:**

```rust
/// Result of a preview (dry-run) estimation.
#[derive(Debug, Clone, Serialize)]
pub struct IngestPreview {
    /// Total markdown files discovered.
    pub total_files: usize,
    /// Files that would be re-embedded (new or changed).
    pub files_to_index: usize,
    /// Files that would be skipped (unchanged hash).
    pub files_to_skip: usize,
    /// Files in the index that no longer exist on disk.
    pub files_to_remove: usize,
    /// Total chunks that would be created from changed files.
    pub chunks_to_create: usize,
    /// Total tokens across all chunks to embed.
    pub total_tokens: usize,
    /// Estimated API calls (chunks_to_create / batch_size, rounded up).
    pub estimated_api_calls: usize,
    /// Per-file breakdown.
    pub files: Vec<PreviewFileInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewFileInfo {
    pub path: String,
    pub status: PreviewFileStatus,
    pub chunks: usize,
    pub tokens: usize,
}

#[derive(Debug, Clone, Serialize)]
pub enum PreviewFileStatus {
    New,
    Changed,
    Unchanged,
    Deleted,
}
```

### Interface Changes

**`--reindex` replaces `--full`, `--preview` added in `src/main.rs`:**

```rust
struct IngestArgs {
    /// Force re-indexing of all files (re-embed everything)
    #[arg(long)]
    reindex: bool,

    /// Alias for --reindex (deprecated)
    #[arg(long, hide = true)]
    full: bool,

    /// Preview what would happen without calling embedding APIs
    #[arg(long)]
    preview: bool,
    // ...
}
```

Map both to `options.full = args.reindex || args.full`.

**Progress display in `src/main.rs`:**

The CLI creates an `indicatif::MultiProgress` with:
1. A main progress bar showing `[current/total] filename... phase` with percentage
2. A status line below showing elapsed time and skip/fail counts

The progress callback updates these bars on each `IngestPhase` event.

**Watch live output:**

When not in JSON mode, the watch command prints each event as it happens:

```
  ● Watching for changes
  →  docs/  notes/

  [12:34:05] ✓ docs/guide.md (3 chunks, 45ms)
  [12:34:12] ✓ notes/ideas.md (2 chunks, 32ms)
  [12:34:15] ✗ notes/broken.md — parse error: invalid frontmatter
  [12:34:20] − old-file.md (removed)
```

### New Commands / API / UI

**Ingest preview output (`mdvdb ingest --preview`):**

```
  ● Ingest Preview

  Files discovered:   42
  To index:           8  (3 new, 5 changed)
  To skip:            33 (unchanged)
  To remove:          1  (deleted from disk)

  Chunks to create:   64
  Tokens to embed:    ~28,450
  Estimated API calls: 1  (batch size: 100)

  Changed files:
    + notes/new-idea.md           4 chunks   ~1,820 tokens
    + notes/another-new.md        2 chunks   ~940 tokens
    + notes/third-new.md          3 chunks   ~1,200 tokens
    ~ docs/architecture.md        8 chunks   ~3,600 tokens
    ~ docs/getting-started.md     6 chunks   ~2,800 tokens
    ~ docs/api-reference.md       12 chunks  ~5,400 tokens
    ~ guides/deployment.md        5 chunks   ~2,100 tokens
    ~ guides/troubleshooting.md   4 chunks   ~1,800 tokens

  Run `mdvdb ingest` to proceed or `mdvdb ingest --reindex` to re-embed all files.
```

JSON mode (`--preview --json`) outputs the `IngestPreview` struct directly.

**Ingest progress display (interactive terminal):**

```
  Ingesting markdown files...

  ████████████████░░░░  80% [16/20]  docs/architecture.md
  ┊ Embedding chunks (batch 3/4)...
  ┊ 14 indexed · 2 skipped · 0 failed · 12.3s elapsed

  Press ESC to cancel
```

When complete:

```
  ✓ Ingestion complete

  Files indexed:  16
  Files skipped:  2
  Files removed:  1
  Chunks created: 128
  API calls:      8

  Completed in 14.2s
```

**Watch live event output:**

```
  ● Watching for changes (ESC or Ctrl+C to stop)
  →  docs/  notes/

  12:34:05  ✓  docs/guide.md          3 chunks  45ms
  12:34:12  ✓  notes/ideas.md         2 chunks  32ms
  12:34:15  ✗  notes/broken.md        parse error: invalid frontmatter
  12:34:20  −  old-file.md            removed
  12:34:25  ↻  renamed.md → new.md    2 chunks  38ms
```

### Migration Strategy

- `--full` remains as a hidden alias for `--reindex` — no breaking change
- Progress callback is `Option<ProgressCallback>` — existing callers (tests, lib users) pass `None` and see no difference
- `IngestOptions` loses `Clone` derive — check all call sites (only `main.rs` constructs it, tests use `Default`)

## Implementation Steps

0. **CRITICAL BUG FIX: Fix HNSW key mismatch after reindex in `src/index/state.rs`** — There is a critical bug that causes semantic search to return no results after a full reindex (and potentially misattributes results after any save/load cycle).

   **Root cause:** `Index::open()` (state.rs:33-61) reconstructs the `id_to_key` mapping by sequentially enumerating `metadata.chunks.keys()`:
   ```rust
   for (idx, chunk_id) in metadata.chunks.keys().enumerate() {
       let key = idx as u64; // assigns 0, 1, 2, ...
       id_to_key.insert(chunk_id.clone(), key);
   }
   ```
   This assumes HNSW vectors are stored with keys 0, 1, 2, ... matching the enumeration order. This assumption breaks in two ways:

   **Problem A (--full / reindex):** During `upsert()`, old vectors are removed with `hnsw.remove(old_key)` and new vectors are added with `hnsw.add(next_key, ...)` where `next_key` increments monotonically. After a full reindex of N files, old keys 0..N are removed and new keys N+1..2N are created. On save/load, `open()` assigns keys 0..N but HNSW has vectors at N+1..2N. `search_vectors()` returns keys from HNSW (N+1..2N) but `key_to_id` only maps 0..N → all results are silently dropped.

   **Problem B (any save/load):** `HashMap::keys()` iteration order is non-deterministic. Even without reindex, chunk IDs may be assigned to wrong HNSW keys after deserialization, causing search results to return wrong chunks with wrong similarity scores.

   **Fix: Compact HNSW keys during `save()`** — Before writing the index, rebuild the HNSW with sequential keys matching a deterministic chunk ordering:
   ```rust
   // In save(), before write_index:
   // 1. Collect all (chunk_id, vector) pairs in deterministic order
   let dims = state.metadata.embedding_config.dimensions;
   let mut chunk_ids: Vec<&String> = state.metadata.chunks.keys().collect();
   chunk_ids.sort(); // deterministic order

   let mut new_hnsw = storage::create_hnsw(dims)?;
   new_hnsw.reserve(chunk_ids.len().max(10))?;
   let mut new_id_to_key = HashMap::new();

   for (idx, chunk_id) in chunk_ids.iter().enumerate() {
       let key = idx as u64;
       if let Some(&old_key) = state.id_to_key.get(*chunk_id) {
           let mut buf = vec![0.0f32; dims];
           if state.hnsw.get(old_key, &mut buf).is_ok() {
               new_hnsw.add(key, &buf)?;
               new_id_to_key.insert((*chunk_id).clone(), key);
           }
       }
   }

   // 2. Replace state
   state.hnsw = new_hnsw;
   state.id_to_key = new_id_to_key;
   state.next_key = chunk_ids.len() as u64;
   ```
   Also update `open()` to sort `metadata.chunks.keys()` before enumeration (matching the sort order used in save), so the key assignment is deterministic.

   **Test for the fix** (add to `tests/api_test.rs`):
   ```rust
   #[tokio::test]
   async fn test_search_works_after_full_reindex() {
       let (_dir, vdb) = setup_project();

       // Initial ingest
       vdb.ingest(IngestOptions::default()).await.unwrap();

       // Verify search works
       let query = SearchQuery::new("test");
       let results = vdb.search(query).await.unwrap();
       assert!(!results.is_empty(), "search should work after initial ingest");

       // Full reindex
       let opts = IngestOptions { full: true, file: None };
       vdb.ingest(opts).await.unwrap();

       // Verify search STILL works after reindex
       let query = SearchQuery::new("test");
       let results = vdb.search(query).await.unwrap();
       assert!(!results.is_empty(), "search must work after full reindex");
   }
   ```

   Also add a save/load roundtrip test in `tests/index_test.rs` that verifies search returns correct chunks after save + reopen.

1. **Add `IngestPreview`, `PreviewFileInfo`, `PreviewFileStatus`, `IngestPhase` enum, and `ProgressCallback` type to `src/lib.rs`** — Define the progress event enum and callback type alias. Export them from `lib.rs`. Remove `Clone` from `IngestOptions` derive list (keep `Debug`, `Default`). Add `progress: Option<ProgressCallback>` field to `IngestOptions`. Implement `Default` manually to set `progress: None`.

2. **Wire progress callbacks into `MarkdownVdb::ingest()` in `src/lib.rs`** — At each pipeline phase, call the progress callback if present:
   - After discovery: `Discovering`
   - In the per-file parse loop: `Parsing { current, total, path }` for changed files, `Skipped { current, total, path }` for unchanged
   - Before/during batch embedding: `Embedding { current_batch, total_batches, ... }`
   - Before `index.save()`: `Saving`
   - Before clustering: `Clustering`
   - After stale removal: `Cleaning { removed }`
   - At end: `Done`

   Add a helper in the ingest method:
   ```rust
   let report = |phase: &IngestPhase| {
       if let Some(ref cb) = options.progress {
           cb(phase);
       }
   };
   ```

3. **Add `MarkdownVdb::preview()` method to `src/lib.rs`** — This method performs discovery, parsing, chunking, and token counting without calling the embedding API. It reuses the same pipeline as `ingest()` up to the embedding step:
   - Discover files (same as ingest)
   - Get existing hashes from index
   - For each discovered file: parse, compare hash, chunk if changed
   - Count tokens per chunk using `chunker::count_tokens()` (already public)
   - Identify stale files (in index but not on disk)
   - Build and return `IngestPreview` with per-file breakdown
   - The method is synchronous (no async needed since no API calls)

   ```rust
   impl MarkdownVdb {
       pub fn preview(&self, reindex: bool) -> Result<IngestPreview> { ... }
   }
   ```

4. **Add batch progress callback to `embed_chunks()` in `src/embedding/batch.rs`** — Add an optional `on_batch: Option<&(dyn Fn(usize, usize) + Send + Sync)>` parameter (batch_index, total_batches). Call it after each batch completes. The caller in `lib.rs` bridges this to the `IngestPhase::Embedding` variant. Since `embed_chunks` uses `buffer_unordered` (async concurrent), the callback needs to be called from the sequential collection loop (lines 123-129), not from inside the async closure.

5. **Rename `--full` to `--reindex` and add `--preview` in `src/main.rs`** — Change `IngestArgs`:
   - Rename the `full` field to `reindex` with `#[arg(long)]`
   - Add `#[arg(long, hide = true)] full: bool` as hidden alias
   - Add `#[arg(long)] preview: bool`
   - Map: `options.full = args.reindex || args.full`
   - In the handler: if `args.preview`, call `vdb.preview(options.full)` instead of `vdb.ingest()`. Output via `format::print_ingest_preview()` or JSON serialize.
   - Update help text to say "Force re-indexing of all files"

6. **Build interactive progress display in `src/main.rs`** — Replace the simple spinner with an `indicatif::MultiProgress` setup:
   - Create a `ProgressBar` with `ProgressStyle` template showing `[current/total] filename percentage`
   - Create a second status bar below for elapsed time and counters
   - Build a `ProgressCallback` closure that updates bars based on `IngestPhase` events
   - Pass the callback into `IngestOptions { progress: Some(...) }`
   - Only create interactive progress when `IsTerminal` is true and not JSON mode
   - On `Done`, clear progress bars and print final `print_ingest_result()`

7. **Add Ctrl+C cancellation support to `MarkdownVdb::ingest()`** — Add `cancel: Option<CancellationToken>` to `IngestOptions`. Set up Ctrl+C handler in `main.rs` that triggers the token (same pattern as the watch command at `src/main.rs:405-410`). Check `cancel.is_cancelled()` at the start of each file in the parse loop and between embedding batches. On cancellation, break out of the loop, save whatever was indexed so far, and return the partial result. Add a `cancelled: bool` field to `IngestResult`.

8. **Add live event output to watch command in `src/main.rs` and `src/format.rs`** — Currently the watch handler in `main.rs` just prints "Watching for changes" and blocks on `vdb.watch(cancel)`. The watcher processes events internally with no output.

   Option A: Add a callback/channel to `Watcher` that emits events to the CLI for display.
   Option B: Return events from the watch loop for the CLI to print.

   **Recommended: Option A** — Add an `on_event` callback to `Watcher::new()` or `Watcher::watch()`:
   ```rust
   pub type WatchEventCallback = Box<dyn Fn(&WatchEventReport) + Send + Sync>;

   pub struct WatchEventReport {
       pub path: String,
       pub event_type: WatchEventType, // Indexed, Deleted, Renamed, Failed
       pub chunks: Option<usize>,
       pub duration_ms: u64,
       pub error: Option<String>,
   }
   ```

   The CLI creates a callback that prints formatted lines:
   - `format::print_watch_event(report)` — formats a single event line with timestamp, icon, path, details

9. **Wire watch event callback in `src/watcher.rs`** — Store the optional callback in the `Watcher` struct. Call it after each successful `process_file()`, after each `remove_file()`, and on errors. Time each operation with `std::time::Instant`.

10. **Add `print_watch_event()` and `print_ingest_preview()` to `src/format.rs`** — `print_watch_event()` formats a single watch event line with local time (HH:MM:SS), colored status icon (✓/✗/−/↻), file path, chunk count, and duration. `print_ingest_preview()` formats the preview output: summary section (files discovered, to index, to skip, to remove), token/chunk/API call estimates, and per-file breakdown with `+` for new, `~` for changed, `-` for deleted. Use the existing `Colorize` patterns.

11. **Add elapsed time to `print_ingest_result()` in `src/format.rs`** — Add a `duration` field to `IngestResult` (or pass it separately). Show "Completed in X.Xs" at the bottom of the result output.

12. **Migrate shell completions to `clap_complete`** — The current completions are hardcoded strings that are already stale (missing `links`, `backlinks`, `orphans` commands, and no subcommand-level flags). Replace the manual `Completions` command handler with `clap_complete` auto-generation:
   - Uncomment `clap_complete = "4"` in `Cargo.toml` (already present as a TODO comment)
   - Replace the `ShellType` enum with `clap_complete::Shell` (which already implements `ValueEnum`)
   - Replace the hardcoded match arms with `clap_complete::generate(shell, &mut Cli::command(), "mdvdb", &mut std::io::stdout())`
   - This automatically includes all subcommands, flags (`--reindex`, `--preview`, `--json`, `--filter`, etc.), and argument types
   - Remove the ~60 lines of manual completion scripts

13. **Move `--json` to a global flag** — Currently every subcommand args struct repeats `#[arg(long)] json: bool`. Move it to the top-level `Cli` struct alongside `--verbose`, `--no-color`, and `--root`:
   ```rust
   struct Cli {
       /// Output as JSON instead of human-readable format
       #[arg(long, global = true)]
       json: bool,
       // ... existing global flags ...
   }
   ```
   - Remove `json: bool` from all individual `*Args` structs (`SearchArgs`, `IngestArgs`, `StatusArgs`, etc.)
   - Update all handlers to use `cli.json` instead of `args.json`
   - This also means `mdvdb --json status` and `mdvdb status --json` both work (clap global flag behavior)
   - The existing `no_color` conditional at lines 290-292 already follows this pattern

14. **Add tests** — Test cases:
    - Unit test: `IngestPhase` enum variants are constructable
    - Integration test: ingest with progress callback receives expected phase sequence (Discovering → Parsing... → Embedding... → Saving → Done)
    - Integration test: `--reindex` flag works same as `--full`
    - Integration test: `--full` still works (backward compat)
    - Integration test: preview returns correct file counts, chunk counts, and token estimates
    - Integration test: preview with `reindex=true` marks all files as changed
    - CLI test: `mdvdb ingest --reindex` exits 0
    - CLI test: `mdvdb ingest --full` exits 0 (hidden alias)
    - CLI test: `mdvdb ingest --preview` outputs preview summary
    - CLI test: `mdvdb ingest --preview --json` outputs valid JSON matching `IngestPreview`
    - Unit test: `WatchEventReport` formatting

## Validation Criteria

- [ ] `cargo test` passes with zero failures
- [ ] `cargo clippy --all-targets` passes with zero warnings
- [ ] `mdvdb ingest` shows interactive progress bar with file names, counts, and percentage in terminal
- [ ] `mdvdb ingest --json` shows no progress output, only final JSON result
- [ ] `mdvdb ingest --reindex` forces re-embedding of all files
- [ ] `mdvdb ingest --full` still works (hidden alias, same behavior)
- [ ] `mdvdb ingest --preview` shows file counts, chunk counts, token estimates, and per-file breakdown without calling embedding API
- [ ] `mdvdb ingest --preview --json` outputs valid JSON `IngestPreview` struct
- [ ] `mdvdb ingest --preview --reindex` treats all files as changed in the estimate
- [ ] Ctrl+C during ingest saves partial progress and shows "cancelled" message
- [ ] `mdvdb watch` shows per-file event lines with timestamps when files change
- [ ] `mdvdb watch --json` shows no event lines (or structured JSON events)
- [ ] Progress callback is optional — passing `None` produces no output (library users unaffected)
- [ ] Elapsed time shown in final ingest result

## Anti-Patterns to Avoid

- **Do NOT pull in a TUI framework** (ratatui, crossterm for full-screen) — indicatif's `MultiProgress` is sufficient for progress bars. Only add `crossterm` if implementing ESC handling, and even then consider Ctrl+C first.
- **Do NOT put indicatif/UI code in library modules** — The `src/lib.rs` ingest method should only call the progress callback. All indicatif bar creation and styling lives in `src/main.rs`. The library stays UI-agnostic.
- **Do NOT break the JSON output contract** — `--json` mode must output only the final `IngestResult` JSON to stdout. Progress output goes to stderr or is suppressed entirely.
- **Do NOT use `println!` for progress** — Use indicatif bars which handle terminal clearing correctly. Raw `println!` during progress bars causes display corruption.
- **Do NOT make `ProgressCallback` required** — It must be `Option` so library consumers and tests aren't forced to provide one.
- **Do NOT change the ingest pipeline order** — This PRD is about visibility, not logic changes. Discovery → parse → hash-check → chunk → embed → upsert → cleanup → save → cluster stays the same.
- **Do NOT add a `Clone` bound on the callback** — Use `Option<ProgressCallback>` without requiring `Clone` on `IngestOptions`. Manually implement `Default` instead.

## Patterns to Follow

- **Spinner pattern** (`src/main.rs:299-313`) — The existing `indicatif::ProgressBar::new_spinner()` pattern shows where progress UI is created and torn down. Replace this with `MultiProgress`.
- **Callback pattern** — Use `Box<dyn Fn(&IngestPhase) + Send + Sync>` matching the `EmbeddingProvider` trait pattern of `Box<dyn ... + Send + Sync>`.
- **Format function pattern** (`src/format.rs:237-285`) — `print_ingest_result()` shows the colored output style. New progress and event formatting should match this aesthetic.
- **Watch event pattern** (`src/watcher.rs:145-168`) — `handle_event()` shows how events are dispatched. Add callback invocation after each event's processing.
- **Terminal detection** (`src/main.rs:299`) — `std::io::IsTerminal::is_terminal(&std::io::stdout())` is already used to decide interactive vs non-interactive output. Reuse this gate.
- **CancellationToken pattern** (`src/main.rs:405-410`) — The watch command already uses `tokio_util::sync::CancellationToken` with `tokio::signal::ctrl_c()`. Reuse this pattern for ingest cancellation.
- **Hidden CLI alias** — Use `#[arg(long, hide = true)]` for the deprecated `--full` flag (clap pattern for backward compat).

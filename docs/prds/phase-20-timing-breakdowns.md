# PRD: Phase 20 — Timing Breakdowns for Ingest & Search

## Overview

Add structured timing breakdowns to mdvdb's ingest and search pipelines, exposing how much time is spent in each phase (embedding API calls, HNSW search, BM25 search, parsing, index writes). Timings are only included when verbosity is enabled (`-v` or higher) — in both JSON and human-readable output. This enables fair benchmarking by separating API latency from engine performance.

## Problem Statement

mdvdb's ingest reports a single `duration_secs` and search returns results with no timing information. In benchmarks, a search that takes 307ms appears slow — but 285ms of that is the OpenAI API call to embed the query, and only 3ms is the actual HNSW vector search. Similarly, an 18s ingest is 94% embedding API time and 3% engine time.

Without per-phase timing, users cannot:
- Identify whether performance bottlenecks are in the API, parsing, indexing, or search engine
- Fairly compare mdvdb against other vector databases (which may not include embedding in their benchmarks)
- Tune their setup (e.g., switch to a faster embedding provider, increase batch size)

## Goals

- Add `IngestTimings` struct with per-phase durations to `IngestResult`
- Add `SearchTimings` struct with per-phase durations to search output
- Timings only appear when verbosity is set (`-v` or higher)
  - In JSON mode with `-v`: include `timings` field in JSON output
  - In human-readable mode with `-v`: print timing breakdown to stderr
  - Without `-v`: no timings in output (clean default)
- Zero overhead when not using timings (`Instant` is near-free)
- Backward compatible — timings are additive and opt-in

## Non-Goals

- Profiling-level instrumentation (per-chunk, per-batch timings) — too noisy
- Histogram / percentile tracking across multiple queries — that's the benchmark suite's job
- Memory usage tracking — only wall-clock time
- Adding timing to the `watch` command or file watcher events
- Always-on timing in JSON output — must require `-v`

## Technical Design

### New Types

**`IngestTimings`** — added to `src/lib.rs`:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct IngestTimings {
    /// Time spent discovering markdown files.
    pub discover_secs: f64,
    /// Time spent parsing files, computing hashes, and chunking.
    pub parse_secs: f64,
    /// Time spent calling the embedding provider API.
    pub embed_secs: f64,
    /// Time spent upserting chunks into the vector index and FTS.
    pub upsert_secs: f64,
    /// Time spent saving the index to disk and committing FTS.
    pub save_secs: f64,
    /// Total wall-clock time (equals duration_secs).
    pub total_secs: f64,
}
```

**`SearchTimings`** — added to `src/search.rs`:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct SearchTimings {
    /// Time spent calling the embedding provider to embed the query.
    pub embed_secs: f64,
    /// Time spent in HNSW vector search (usearch). 0 if lexical-only.
    pub vector_search_secs: f64,
    /// Time spent in BM25 lexical search (tantivy). 0 if semantic-only.
    pub lexical_search_secs: f64,
    /// Time spent in RRF fusion + score normalization. 0 if not hybrid.
    pub fusion_secs: f64,
    /// Time spent assembling results (filtering, decay, link boosting).
    pub assemble_secs: f64,
    /// Total wall-clock time.
    pub total_secs: f64,
}
```

### Data Model Changes

**`IngestResult`** in `src/lib.rs` (line 144-163) — add optional field:

```rust
pub struct IngestResult {
    // ... existing fields unchanged ...
    pub duration_secs: f64,
    pub timings: Option<IngestTimings>,  // NEW — populated by library, displayed by CLI only with -v
    pub cancelled: bool,
}
```

### Interface Changes

**`search::search()`** in `src/search.rs` (line 259-371) — change return type:

Current:
```rust
pub async fn search(...) -> Result<Vec<SearchResult>>
```

New:
```rust
pub async fn search(...) -> Result<(Vec<SearchResult>, SearchTimings)>
```

**`MarkdownVdb::search()`** in `src/lib.rs` (line 945-963) — change return type:

Current:
```rust
pub async fn search(&self, query: SearchQuery) -> Result<Vec<SearchResult>>
```

New:
```rust
pub async fn search(&self, query: SearchQuery) -> Result<(Vec<SearchResult>, SearchTimings)>
```

**`semantic_search()`** in `src/search.rs` (line 374-384) — return embed + search times:

```rust
async fn semantic_search(...) -> Result<(Vec<(String, f64)>, f64, f64)>
// Returns: (candidates, embed_secs, vector_search_secs)
```

### CLI Changes — Verbosity-Gated

**Key principle:** Timings are always computed internally (near-zero cost), but only surfaced when `-v` is passed.

**Search JSON output with `-v`** — `SearchOutput` in `src/main.rs` gains optional timings:

```rust
#[derive(serde::Serialize)]
struct SearchOutput {
    results: Vec<SearchResult>,
    query: String,
    total_results: usize,
    mode: SearchMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    timings: Option<SearchTimings>,  // NEW — only Some when -v
}
```

**Search human-readable output with `-v`** — print to stderr:

```
  [timing] embed=285ms hnsw=3ms bm25=0ms assemble=1ms total=289ms
```

**Ingest JSON output with `-v`** — `IngestResult.timings` is `Option<IngestTimings>`:
- The library always populates `timings: Some(...)`
- CLI sets `#[serde(skip_serializing_if = "Option::is_none")]` — but since the library always fills it, CLI should conditionally clear it to `None` before serializing when verbosity is 0

Alternative (cleaner): The library populates `timings: Some(...)` always. The CLI wraps `IngestResult` in its own output struct that conditionally includes timings based on verbosity, similar to `SearchOutput`.

**Ingest human-readable output with `-v`** — print to stderr:

```
  [timing] discover=10ms parse=350ms embed=17200ms upsert=420ms save=150ms total=18300ms
```

### Benchmark Integration

The benchmark scripts invoke mdvdb with `-v` to get timings in JSON output, then parse and display them in the report.

### Migration Strategy

No migration needed. Changes are additive:
- `IngestResult.timings` is `Option<IngestTimings>` with `skip_serializing_if`
- `SearchOutput.timings` is `Option<SearchTimings>` with `skip_serializing_if`
- Consumers not using `-v` see identical output to before

## Implementation Steps

1. **Add `SearchTimings` struct** — `src/search.rs`: Define the struct with `Debug, Clone, Serialize` derives. Place it near the top alongside `SearchResult`.

2. **Instrument `semantic_search()`** — `src/search.rs` line 374-384: Wrap `provider.embed_batch()` with `Instant` to capture `embed_secs`. Wrap `index.search_vectors()` to capture `vector_search_secs`. Return both times alongside candidates.

3. **Instrument `lexical_search()`** — `src/search.rs` line 387-397: Wrap `fts_index.search()` to capture `lexical_search_secs`. Return time alongside candidates.

4. **Instrument `search()` main function** — `src/search.rs` line 259-371:
   - Capture total start time
   - For each mode: unpack phase times from instrumented helpers
   - For hybrid: semantic + lexical run in parallel via `tokio::join!`, so `embed_secs` comes from semantic path
   - Wrap score normalization + RRF fusion to capture `fusion_secs`
   - Wrap `assemble_results()` to capture `assemble_secs`
   - Build `SearchTimings`, return as tuple `(results, timings)`

5. **Update `MarkdownVdb::search()`** — `src/lib.rs` line 945-963: Change return type. Pass through timings.

6. **Add `IngestTimings` struct** — `src/lib.rs`: Define with `Debug, Clone, Serialize` derives. Add `pub timings: Option<IngestTimings>` to `IngestResult`.

7. **Instrument `MarkdownVdb::ingest()`** — `src/lib.rs` line 547-942:
   - `discover_secs`: Wrap lines 564-579
   - `parse_secs`: Wrap lines 640-711
   - `embed_secs`: Wrap lines 732-740
   - `upsert_secs`: Wrap lines 754-872 (upsert + FTS rebuild + cleanup + links)
   - `save_secs`: Wrap lines 877-925 (save + clustering)
   - `total_secs`: From existing `start_time`
   - Always populate `timings: Some(IngestTimings { ... })`

8. **Update CLI search handler** — `src/main.rs`:
   - Destructure `(results, timings)` from `vdb.search()`
   - Pass verbosity level down; if `-v`, include `timings: Some(timings)` in `SearchOutput`; else `timings: None`
   - In human-readable mode with `-v`, print timing line to stderr
   - Add `#[serde(skip_serializing_if = "Option::is_none")]` on `SearchOutput.timings`

9. **Update CLI ingest handler** — `src/main.rs`:
   - Wrap `IngestResult` in a CLI-level struct (or clone and modify) that conditionally includes timings based on verbosity
   - In human-readable mode with `-v`, print timing breakdown to stderr
   - Use `#[serde(skip_serializing_if = "Option::is_none")]` so timings only appear in JSON with `-v`

10. **Update re-exports** — `src/lib.rs`: Add `IngestTimings` and `SearchTimings` (via `search::SearchTimings`) to public re-exports.

11. **Update tests** — All callers of `vdb.search()` need to handle the new tuple return. Key files:
    - `tests/search_test.rs`
    - `tests/api_test.rs`
    - `tests/cli_test.rs` (verify timings absent without `-v`, present with `-v`)
    - Any other test calling `vdb.search()` or `search::search()`

12. **Update benchmark scripts** —
    - `benchmark/scripts/mdvdb_bench.py`: Pass `-v` when invoking mdvdb CLI. Parse `timings` from JSON output.
    - `benchmark/scripts/report.py`: Add timing breakdown section showing embed vs engine time for mdvdb.

## Files Modified

| File | Change |
|---|---|
| `src/search.rs` | Add `SearchTimings`. Instrument `semantic_search()`, `lexical_search()`, `search()`. Change return type. |
| `src/lib.rs` | Add `IngestTimings`. Add to `IngestResult`. Instrument `ingest()`. Change `search()` return type. Update re-exports. |
| `src/main.rs` | Add `timings` (optional) to `SearchOutput`. Print timing breakdowns at `-v`. Handle tuple return. Verbosity-gate ingest timings. |
| `tests/search_test.rs` | Handle `(Vec<SearchResult>, SearchTimings)` return |
| `tests/api_test.rs` | Handle `(Vec<SearchResult>, SearchTimings)` return |
| `tests/cli_test.rs` | Verify timings absent without `-v`, present with `-v -json` |
| `benchmark/scripts/mdvdb_bench.py` | Add `-v` flag, parse `timings` from JSON |
| `benchmark/scripts/report.py` | Add timing breakdown section |

## Validation Criteria

- [ ] `cargo test` passes — all existing + new tests
- [ ] `cargo clippy --all-targets` — zero warnings
- [ ] `mdvdb ingest --json` (no `-v`) — output does NOT contain `timings`
- [ ] `mdvdb ingest --json -v` — output contains `timings` with all phase fields
- [ ] `mdvdb search "query" --json` (no `-v`) — output does NOT contain `timings`
- [ ] `mdvdb search "query" --json -v` — output contains `timings` with all phase fields
- [ ] `mdvdb search "query" --json -v --mode hybrid` — non-zero `lexical_search_secs` and `fusion_secs`
- [ ] `mdvdb search "query" --json -v --mode lexical` — zero `embed_secs` and `vector_search_secs`
- [ ] `mdvdb ingest -v` — prints timing breakdown to stderr in human-readable format
- [ ] `mdvdb search "query" -v` — prints timing line to stderr
- [ ] Individual phase times sum approximately to `total_secs`
- [ ] `embed_secs` dominates total for API-based providers
- [ ] `embed_secs` is near-zero for mock provider in tests
- [ ] `benchmark/run.sh` with updated scripts shows timing breakdown in report
- [ ] Default output (no `-v`) is identical to current behavior

## Anti-Patterns to Avoid

- **Do not use `SystemTime` for timing** — Use `std::time::Instant` which is monotonic. `SystemTime` is for wall-clock timestamps, not duration measurement.

- **Do not include timings in default output** — Timings must be gated behind `-v`. Clean default output is a project principle.

- **Do not add timings to `SearchResult` (per-result)** — Timings are per-query, not per-result. They belong on the response wrapper.

- **Do not change the library API to require verbosity** — The library always computes and returns timings (near-zero cost). The CLI decides whether to display them based on verbosity.

- **Do not time individual chunks or batches** — Phase-level granularity (embed, search, assemble) is the right abstraction.

- **Do not create a new `SearchResponse` wrapper struct in the library** — Use a simple tuple `(Vec<SearchResult>, SearchTimings)` to minimize API surface change.

## Patterns to Follow

- **`Instant::now()` + `.elapsed()`:** Already used in `lib.rs:548` for `start_time`. Same pattern for all phase timings.

- **`#[derive(Debug, Clone, Serialize)]`:** Standard derives for all public types. See `IngestResult`, `SearchResult` in `src/lib.rs` and `src/search.rs`.

- **`#[serde(skip_serializing_if = "Option::is_none")]`:** Used in `SearchQuery` fields. Apply same pattern for optional `timings`.

- **Stderr for non-data output:** Convention from `CLAUDE.md`: stdout for data, stderr for logs. Timing lines go to stderr.

- **Public re-exports:** New public types must be re-exported from `src/lib.rs` — see existing `pub use search::{...}` block.

- **Tuple returns from internal functions:** Simple and idiomatic for adding secondary return values without wrapper structs.

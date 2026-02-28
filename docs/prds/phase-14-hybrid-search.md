# PRD: Phase 14 — Hybrid Search (Semantic + Lexical BM25)

## Overview

Add fast BM25 lexical search via Tantivy alongside the existing HNSW semantic search, combined through Reciprocal Rank Fusion (RRF). The CLI defaults to hybrid mode (both signals) with `--mode`, `--semantic`, and `--lexical` flags for control. Lexical mode requires no API keys — pure local, sub-millisecond search.

## Problem Statement

mdvdb currently only supports semantic search (embed query → usearch HNSW → cosine similarity). While semantic search excels at meaning-based retrieval, it fails on exact keyword matches — searching for a function name like `parse_config`, a specific error message, or a technical term returns poor results because embeddings don't preserve exact tokens. Users coming from grep/ripgrep expect keyword precision alongside semantic understanding.

Additionally, semantic search always requires an embedding API call (network latency + API key), making it unusable for quick local lookups.

## Goals

- Sub-millisecond BM25 keyword search via Tantivy (mmap-backed, Rust-native)
- Hybrid mode (default) that combines semantic + lexical via RRF for best-of-both-worlds results
- Lexical-only mode that works without any API key — pure local search
- Incremental FTS indexing in sync with the existing vector index
- CLI flags: `--mode hybrid|semantic|lexical` + shorthand `--semantic`/`--lexical`
- Zero regression to existing semantic search behavior

## Non-Goals

- Indexing frontmatter metadata in FTS — frontmatter is already filterable via `--filter`
- Fuzzy matching or typo correction — BM25 with stemming covers the main use case
- Custom BM25 parameter tuning (k1, b) — Tantivy defaults are well-tuned
- Merging the FTS index into the single binary index file — Tantivy needs its own directory format
- Re-ranking or cross-encoder reranking — RRF is sufficient for v1

## Technical Design

### Architecture

```
Query
  │
  ├── Semantic path: embed → usearch HNSW → cosine similarity [0,1]
  │                  (existing, runs via tokio)
  │
  ├── Lexical path:  parse → Tantivy BM25 → saturation normalize → [0,1]
  │                  (sub-ms, local only, no API key)
  │
  └── Hybrid path:   both in parallel → RRF Fusion → max normalize → [0,1]
                     score(doc) = Σ 1/(k + rank), then ÷ max_possible
```

Both paths run in parallel via `tokio::join!` in hybrid mode. The semantic path dominates latency (network-bound embedding call), while lexical search completes in <1ms.

### Data Model Changes

**New on-disk artifact:** `.markdownvdb.fts/` directory (Tantivy segment files). Separate from the existing `.markdownvdb.index` binary. Tantivy manages its own segment merging, compression, and mmap.

**Tantivy schema** (per chunk, same granularity as vector index):

| Field | Tantivy Type | Options | Purpose |
|---|---|---|---|
| `chunk_id` | `STRING` | Stored, not tokenized | Join key with vector results (`"path.md#0"`) |
| `source_path` | `STRING` | Stored, not tokenized | File-level delete operations |
| `content` | `TEXT` | Indexed (`en_stem` tokenizer), **not stored** | BM25 body search |
| `heading_hierarchy` | `TEXT` | Indexed (`en_stem` tokenizer), **not stored** | Heading search (boosted 1.5x) |

Content is NOT stored in Tantivy because it already exists in the rkyv metadata region of the vector index. This keeps the FTS index small (~20-30% of raw text size).

**Markdown stripping:** Before indexing in Tantivy, markdown syntax is stripped using `pulldown_cmark` (already a dependency) — single-pass streaming extraction of `Text` and `Code` events, discarding formatting characters (`#`, `*`, `` ` ``, `[]()`, etc.).

**New `Config` fields:**

| Field | Env Var | Default | Type | Purpose |
|---|---|---|---|---|
| `fts_index_dir` | `MDVDB_FTS_INDEX_DIR` | `.markdownvdb.fts` | `PathBuf` | Tantivy segment directory |
| `search_default_mode` | `MDVDB_SEARCH_MODE` | `hybrid` | `SearchMode` | Default search mode |
| `search_rrf_k` | `MDVDB_SEARCH_RRF_K` | `60.0` | `f64` | RRF smoothing constant |
| `bm25_norm_k` | `MDVDB_BM25_NORM_K` | `1.5` | `f64` | BM25 saturation normalization midpoint |

Validation: `search_rrf_k` must be > 0. `bm25_norm_k` must be > 0.

### Interface Changes

**SearchQuery** gains a `mode: SearchMode` field (default `Hybrid`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum SearchMode {
    #[default]
    Hybrid,
    Semantic,
    Lexical,
}
```

Builder method: `SearchQuery::new("query").with_mode(SearchMode::Lexical)`.

**`search::search()` signature** adds FTS index, RRF k, and BM25 normalization parameters:

```rust
pub async fn search(
    query: &SearchQuery,
    index: &Index,
    provider: &dyn EmbeddingProvider,
    fts_index: Option<&FtsIndex>,
    rrf_k: f64,
    bm25_norm_k: f64,
) -> Result<Vec<SearchResult>>
```

**`MarkdownVdb` struct** gains `fts_index: Arc<FtsIndex>` field. Public API (`MarkdownVdb::search()`) remains the same signature — FTS is passed internally.

**SearchResult** struct is unchanged. The `score` field is always normalized to `[0, 1]`:
- Semantic mode: Cosine similarity (absolute, unchanged)
- Lexical mode: BM25 score normalized via saturation `score / (score + bm25_norm_k)`. Maps unbounded BM25 `(0, ∞)` → `(0, 1)` with diminishing returns. A BM25 score equal to `bm25_norm_k` maps to 0.5.
- Hybrid mode: RRF score normalized by theoretical maximum `num_lists / (rrf_k + 1)`. A document ranked #1 in both lists → 1.0. Found by only one retriever → ~0.5.

### Score Normalization

All three modes produce scores in `[0, 1]` with meaningful absolute interpretation:

**Semantic (no normalization needed):** Cosine similarity is inherently `[0, 1]`.

**Lexical — BM25 Saturation Normalization:**

```
normalized = score / (score + bm25_norm_k)
```

Uses a saturation curve that maps `(0, ∞)` → `(0, 1)`. The `bm25_norm_k` parameter (default 1.5, configurable via `MDVDB_BM25_NORM_K`) controls the midpoint — a BM25 score equal to `bm25_norm_k` maps to 0.5. This preserves absolute meaning: a poor keyword match scores low, a strong match scores high. Unlike min-max normalization, a bad result set does not artificially inflate scores.

| Raw BM25 | Normalized (k=1.5) | Meaning |
|----------|---------------------|---------|
| 0.5 | 0.25 | Weak keyword match |
| 1.5 | 0.50 | Moderate match |
| 3.0 | 0.67 | Good match |
| 6.0 | 0.80 | Strong match |
| 15.0 | 0.91 | Excellent match |

**Hybrid — RRF Theoretical Maximum Normalization:**

```
normalized = rrf_score / (num_lists / (rrf_k + 1))
```

Divides by the maximum possible RRF score (achieved when a document is ranked #1 in every retriever list). With the default `rrf_k=60` and 2 retrievers, the max is `2/61 ≈ 0.0328`.

| Scenario | Raw RRF | Normalized | Meaning |
|----------|---------|------------|---------|
| #1 in both lists | 0.0328 | 1.00 | Top-ranked by both signals |
| #1 in one list only | 0.0164 | 0.50 | Found by one retriever only |
| #5 in both lists | 0.0308 | 0.94 | Strong match on both signals |
| #10 in one list only | 0.0143 | 0.44 | Moderate match, one signal |

Both normalization functions are monotonically increasing, so **ranking order is always preserved**.

### New Commands / API / UI

**CLI flags on `search` subcommand:**

```
--mode <MODE>   Search mode: hybrid, semantic, or lexical [default: hybrid]
--semantic      Shorthand for --mode semantic (conflicts with --lexical, --mode)
--lexical       Shorthand for --mode lexical (conflicts with --semantic, --mode)
```

Resolution logic: `--semantic` flag → Semantic, `--lexical` flag → Lexical, else `--mode` value (default hybrid).

**JSON output** gains a `mode` field:
```json
{
  "results": [...],
  "query": "search terms",
  "total_results": 5,
  "mode": "hybrid"
}
```

### New Module: `src/fts.rs`

```rust
pub struct FtsIndex { /* tantivy::Index, IndexReader, field handles */ }
pub struct FtsChunkData { pub chunk_id: String, pub content: String, pub headings: Vec<String> }
pub struct FtsResult { pub chunk_id: String, pub bm25_score: f32 }

impl FtsIndex {
    pub fn open_or_create(path: &Path) -> Result<Self>;
    pub fn upsert_chunks(&self, source_path: &str, chunks: &[FtsChunkData]) -> Result<()>;
    pub fn remove_file(&self, source_path: &str) -> Result<()>;
    pub fn search(&self, query_text: &str, limit: usize) -> Result<Vec<FtsResult>>;
    pub fn commit(&self) -> Result<()>;
}

fn strip_markdown(content: &str) -> String; // pulldown-cmark Text/Code extraction
```

**Upsert:** Delete all docs matching `source_path` term → add new docs. Commit deferred to caller for batching.

**Search:** `QueryParser` on `content` (boost 1.0) + `heading_hierarchy` (boost 1.5) → `TopDocs` collector.

### RRF Implementation (in `src/search.rs`)

```rust
pub fn reciprocal_rank_fusion(
    list_a: &[(String, f64)],  // (chunk_id, score) — e.g. semantic results
    list_b: &[(String, f64)],  // (chunk_id, score) — e.g. lexical results
    k: f64,                    // smoothing constant (default 60)
) -> Vec<(String, f64)>        // (chunk_id, rrf_score) sorted desc
```

Formula: `score(doc) = Σ 1/(k + rank)` summed across both lists (1-indexed ranks). Original scores are discarded — only rank position matters. Items appearing in both lists accumulate scores from each. Higher `k` = gentler blending; lower `k` = top results amplified. Default `k=60` is the industry standard (used by Azure AI Search, Elasticsearch, MongoDB).

After RRF fusion, scores are normalized by the theoretical maximum (`num_lists / (k + 1)`) to produce meaningful `[0, 1]` values.

### Search Pipeline (mode branching in `search::search()`)

- **Semantic:** embed query → HNSW → cosine similarity `[0, 1]` → filter → results
- **Lexical:** Tantivy BM25 → **saturation normalize** → `[0, 1]` → look up metadata → filter → results. **No embedding API call.**
- **Hybrid:** `tokio::join!` runs both in parallel → RRF fusion → **max normalize** → `[0, 1]` → look up metadata → filter → results. Over-fetch 5x from each source (vs 3x for single-mode) to give RRF enough candidates.

**Metadata filtering** (via `--filter`) applied after normalization in all modes, same AND logic as today.

**min_score:** Applied to the normalized score in all modes. Since all modes now produce `[0, 1]` scores, `--min-score` works consistently. For example, `--min-score 0.3` is meaningful in hybrid mode (filters out results found by only one retriever with poor ranking).

### Migration Strategy

- **Existing vector index:** Unchanged format. Zero migration needed.
- **FTS index:** Created fresh on first `ingest` after upgrade. No embedding calls needed — reads content from rkyv metadata and indexes into Tantivy.
- **Consistency guard:** If FTS index has 0 documents but vector index has >0, auto-rebuild FTS from stored chunks during ingest.
- **Default mode:** `Hybrid`. Users wanting old behavior can set `MDVDB_SEARCH_MODE=semantic`.
- **Existing API consumers:** `SearchQuery::new()` defaults to `Hybrid`. `SearchResult` struct unchanged.

## Implementation Steps

1. **Config changes** — `src/config.rs`: Add `fts_index_dir`, `search_default_mode`, `search_rrf_k` fields. Env var parsing, validation (`rrf_k > 0`). Update `.markdownvdb.example`. Update `mock_config()` in tests. Add config tests in `tests/config_test.rs`.

2. **Error variant** — `src/error.rs`: Add `Fts(String)` variant. Map all Tantivy errors to this.

3. **SearchMode enum** — `src/search.rs`: Add `SearchMode` enum (Hybrid/Semantic/Lexical). Add `mode` field to `SearchQuery` with `with_mode()` builder. Default `Hybrid`.

4. **FTS module** — NEW `src/fts.rs`: Full Tantivy integration — schema, open/create, upsert, remove, search, commit, `strip_markdown()`. Register as `pub mod fts;` in `src/lib.rs`. Unit tests for all functions including markdown stripping.

5. **RRF fusion** — `src/search.rs`: Implement `reciprocal_rank_fusion()` function. Unit tests for single list, overlapping lists, disjoint lists, k parameter effect.

6. **Score normalization** — `src/search.rs`: Implement `normalize_bm25_scores()` (saturation: `score/(score+k)`) and `normalize_rrf_scores()` (divide by theoretical max `num_lists/(k+1)`). Applied after mode branching, before filtering. Unit tests for known values, order preservation, edge cases.

7. **Search pipeline rewrite** — `src/search.rs`: Update `search()` signature to accept `Option<&FtsIndex>` + `rrf_k` + `bm25_norm_k`. Add mode branching logic. Parallel execution for hybrid via `tokio::join!`. Apply normalization after retrieval.

8. **Library integration** — `src/lib.rs`: Add `fts_index: Arc<FtsIndex>` to `MarkdownVdb`. Open/create in constructor. Integrate FTS indexing into `ingest()` — upsert chunks after vector upsert, remove stale files, commit after save. Pass FTS index + `bm25_norm_k` to `search()`. Re-export `SearchMode`.

9. **Watcher integration** — `src/watcher.rs`: Add `fts_index: Arc<FtsIndex>` field. Update file change/delete handlers to also update FTS index.

10. **CLI changes** — `src/main.rs`: Add `--mode` flag (`SearchModeArg` clap enum), `--semantic` and `--lexical` boolean shorthand flags with `conflicts_with_all`. Add resolution logic. Add `mode` field to `SearchOutput` JSON.

11. **Integration tests** — NEW `tests/fts_test.rs` (FTS-specific). Extend `tests/search_test.rs` (hybrid/semantic/lexical modes, score normalization), `tests/api_test.rs` (FTS index creation, hybrid search), `tests/cli_test.rs` (`--mode`, `--semantic`, `--lexical` flags, JSON mode field), `tests/config_test.rs` (new defaults, validation).

12. **Documentation** — Update `CLAUDE.md` architecture diagram. Update config table and CLI docs.

## Dependency

```toml
# Cargo.toml
tantivy = "0.22"
```

No other new crates. `pulldown-cmark` (existing) handles markdown stripping.

## Files Modified

| File | Change |
|---|---|
| `Cargo.toml` | Add `tantivy = "0.22"` |
| `src/error.rs` | Add `Fts(String)` variant |
| `src/config.rs` | 4 new fields (`fts_index_dir`, `search_default_mode`, `search_rrf_k`, `bm25_norm_k`) + env parsing + validation |
| `src/search.rs` | `SearchMode` enum, `mode` on `SearchQuery`, RRF fn, `normalize_bm25_scores()`, `normalize_rrf_scores()`, mode branching with normalization |
| `src/fts.rs` | **NEW** — Tantivy wrapper module |
| `src/lib.rs` | `pub mod fts`, `fts_index` field, integrate into ingest + search |
| `src/watcher.rs` | `fts_index` field, update handlers |
| `src/main.rs` | `--mode` + `--semantic`/`--lexical` flags, JSON `mode` field |
| `.markdownvdb.example` | New config keys |
| `tests/fts_test.rs` | **NEW** — FTS integration tests |
| `tests/config_test.rs` | New default/validation tests |
| `tests/search_test.rs` | Hybrid/semantic/lexical mode tests |
| `tests/api_test.rs` | FTS creation + hybrid search tests |
| `tests/cli_test.rs` | Mode flag + shorthand tests |

## Validation Criteria

- [x] `cargo test` passes — all existing tests pass, plus new tests (434 total)
- [x] `cargo clippy --all-targets` passes with zero warnings
- [x] `mdvdb ingest` creates `.markdownvdb.fts/` directory with Tantivy segments
- [x] `mdvdb search "query"` returns hybrid results (default) with normalized `[0, 1]` scores
- [x] `mdvdb search "query" --semantic` returns semantic-only results (cosine similarity)
- [x] `mdvdb search "query" --lexical` returns BM25-only results **without needing OPENAI_API_KEY**, scores normalized to `[0, 1]`
- [x] `mdvdb search "query" --mode lexical --json` includes `"mode": "lexical"` in output
- [x] `--semantic` and `--lexical` conflict with each other and with `--mode`
- [x] Hybrid results combine signals: a keyword-exact match ranks higher than semantic-only
- [x] Incremental ingest updates FTS index for changed files only
- [x] Full ingest (`--full`) rebuilds FTS index from scratch
- [x] File watcher updates FTS index on file changes
- [x] FTS auto-rebuilds from rkyv metadata if out of sync with vector index
- [x] All three modes produce `[0, 1]` scores — CLI score bar renders correctly for all modes
- [x] `--min-score` works consistently across all modes (no longer effectively disabled for hybrid)
- [x] Hybrid score of 1.0 = ranked #1 by both semantic and lexical; ~0.5 = found by only one signal
- [x] Lexical score of 0.5 = BM25 match at the saturation midpoint (configurable via `MDVDB_BM25_NORM_K`)

## Anti-Patterns to Avoid

- **Do not store content in Tantivy** — Content already exists in rkyv metadata. Storing it again doubles disk usage for zero benefit. Look up content from the vector index after getting chunk_ids from Tantivy.

- **Do not call the embedding provider in lexical mode** — The entire point of lexical search is local, zero-API-call operation. Guard the embed call behind a mode check.

- **Do not normalize scores before fusion** — RRF uses ranks, not scores. BM25 and cosine scores are only normalized *after* fusion/retrieval for user-facing output. The fusion step itself is scale-agnostic.

- **Do not commit Tantivy per-file during bulk ingest** — Commit once at the end. Per-file commits destroy indexing performance due to segment creation overhead.

- **Do not block on FTS during semantic search** — In semantic-only mode, don't touch the FTS index at all. No read, no lock, no overhead.

- **Do not merge FTS into the single binary index file** — Tantivy needs its own directory format for segment merging and mmap. Fighting this creates complexity for zero benefit.

## Patterns to Follow

- **Existing index open/create in `MarkdownVdb::open_with_config()`** — FTS index follows the same pattern: open if exists, create if not.

- **Existing upsert loop in `MarkdownVdb::ingest()`** — FTS upsert piggybacks on the same loop, using the same chunks and source paths.

- **Existing atomic save pattern** — Tantivy commit is atomic (segment-based). Call `fts_index.commit()` after `index.save()`.

- **Existing `mock_config()` in tests** — Extend with FTS defaults. Use `tempfile::TempDir` for FTS index path in tests.

- **Existing CLI flag style** — `--mode` follows the same `#[arg(long)]` pattern as `--limit`, `--min-score`. Shorthand flags follow clap `conflicts_with_all` pattern.

- **Error mapping** — All Tantivy errors → `Error::Fts(msg)`, same pattern as other error variants.

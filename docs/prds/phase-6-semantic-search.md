# PRD: Phase 6 — Semantic Search & Metadata Filtering

## Overview

Implement the query pipeline that accepts a natural-language query, embeds it, searches the HNSW vector index, applies optional metadata filters, and returns section-level results with full file context. This is the primary user-facing capability — the reason the system exists.

## Problem Statement

Users (primarily AI agents) need to find specific information within large markdown knowledge bases. Keyword search fails when the user describes what they want in different words than the document uses. Semantic search bridges this gap by matching on meaning, not keywords. Results must be section-level (the specific chunk that matched) with full context (which file, what metadata) so agents can act on them immediately.

## Goals

- Accept a natural-language query string and return top-N most relevant results
- Results are section-level: each result includes the matched chunk's heading, content excerpt, line range, parent file path, relevance score, and full file metadata
- Support configurable result count (`MDVDB_SEARCH_DEFAULT_LIMIT`) and minimum similarity threshold (`MDVDB_SEARCH_MIN_SCORE`)
- Support metadata filters: exact match, range comparisons, list membership
- Combine semantic search with metadata filters in a single query
- Sub-100ms query response on warm index with 10k+ document chunks
- Return results sorted by relevance score (highest first)

## Non-Goals

- No full-text keyword search (this is a vector database, not a text search engine)
- No query language or DSL — filters are structured objects
- No query expansion or rewriting
- No result caching (the OS page cache via mmap is the cache)
- No cross-file deduplication of results

## Technical Design

### Data Model Changes

**`SearchQuery` struct:**

```rust
pub struct SearchQuery {
    /// Natural-language query text
    pub query: String,
    /// Maximum number of results to return
    pub limit: usize,
    /// Minimum cosine similarity score (0.0 to 1.0)
    pub min_score: f64,
    /// Optional metadata filters (all must match — AND logic)
    pub filters: Vec<MetadataFilter>,
}

impl SearchQuery {
    pub fn new(query: impl Into<String>) -> Self;
    pub fn with_limit(self, limit: usize) -> Self;
    pub fn with_min_score(self, min_score: f64) -> Self;
    pub fn with_filter(self, filter: MetadataFilter) -> Self;
}
```

**`MetadataFilter` enum:**

```rust
pub enum MetadataFilter {
    /// Field equals value: frontmatter.field == value
    Equals { field: String, value: serde_json::Value },
    /// Field is in list: frontmatter.field IN [values]
    In { field: String, values: Vec<serde_json::Value> },
    /// Field >= min AND field <= max (for numbers and dates)
    Range { field: String, min: Option<serde_json::Value>, max: Option<serde_json::Value> },
    /// Field exists in frontmatter
    Exists { field: String },
}
```

**`SearchResult` struct:**

```rust
pub struct SearchResult {
    /// Relevance score (cosine similarity, 0.0 to 1.0)
    pub score: f64,
    /// The matched chunk
    pub chunk: SearchResultChunk,
    /// The parent file's metadata
    pub file: SearchResultFile,
}

pub struct SearchResultChunk {
    pub chunk_id: String,
    pub heading_hierarchy: Vec<String>,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
}

pub struct SearchResultFile {
    pub path: String,
    pub frontmatter: Option<serde_json::Value>,
    pub file_size: u64,
}
```

### Interface Changes

```rust
/// Execute a semantic search query against the index
pub async fn search(
    index: &Index,
    provider: &dyn EmbeddingProvider,
    query: SearchQuery,
) -> Result<Vec<SearchResult>>;
```

### Query Pipeline

```
1. Embed the query text → query_vector (single embedding API call)
2. Search usearch HNSW index for nearest neighbors:
   - Request limit * 3 candidates (over-fetch to compensate for filter losses)
   - Returns Vec<(key, distance)> sorted by distance
3. Convert usearch keys to chunk IDs via id_to_key reverse mapping
4. For each candidate:
   a. Look up StoredChunk and StoredFile in metadata
   b. Convert distance to cosine similarity score
   c. If score < min_score → discard
   d. If filters are specified → check each filter against file frontmatter
   e. If all filters pass → include in results
5. Take first `limit` results
6. Return Vec<SearchResult> sorted by score descending
```

### Filter Evaluation

Filters operate on the parent file's frontmatter (parsed from JSON string in `StoredFile.frontmatter`):

- **Equals**: `frontmatter[field] == value` (JSON value equality)
- **In**: `frontmatter[field]` is in `values` list, OR if `frontmatter[field]` is an array, any element matches any value
- **Range**: `min <= frontmatter[field] <= max` (numeric/string comparison)
- **Exists**: `frontmatter.contains_key(field)`

All filters use AND logic — every filter must pass for the result to be included.

### Migration Strategy

Not applicable — new functionality only.

## Implementation Steps

1. **Create `src/search.rs`** — Implement the search module:
   - Define `SearchQuery`, `MetadataFilter`, `SearchResult`, `SearchResultChunk`, `SearchResultFile` structs
   - Derive `serde::Serialize` on result types (for JSON CLI output)
   - Implement `SearchQuery::new()` with builder pattern (`.with_limit()`, `.with_min_score()`, `.with_filter()`)

2. **Implement the `search()` function:**
   - Embed the query text: call `provider.embed_batch(&[query.query.clone()])` to get a single embedding vector
   - Acquire a read lock on the index
   - Call `usearch::Index::search(&query_vector, limit * 3)` to get over-fetched candidates
   - Convert distances to cosine similarity scores: `score = 1.0 - distance` (for cosine metric, usearch returns distance = 1 - similarity)
   - For each candidate, look up the chunk and file metadata
   - Apply `min_score` threshold filter
   - Apply metadata filters (call `evaluate_filters()`)
   - Collect passing results up to `limit`
   - Sort by score descending
   - Return `Vec<SearchResult>`

3. **Implement `evaluate_filters()`** — Private function:
   - Takes `&[MetadataFilter]` and `Option<&serde_json::Value>` (the file's frontmatter)
   - If frontmatter is `None` and any filter is specified → return false (except `Exists` which correctly returns false)
   - For each filter, evaluate against the frontmatter JSON value:
     - `Equals`: navigate to `frontmatter[field]`, compare with `==`
     - `In`: navigate to `frontmatter[field]`, check if value is in the list. If the field value is itself an array, check intersection.
     - `Range`: navigate to `frontmatter[field]`, parse as f64 for numeric comparison, or as string for lexicographic comparison
     - `Exists`: check if `frontmatter[field]` exists and is not null
   - Return true only if ALL filters pass

4. **Add index search method** — Extend `Index` in `src/index/state.rs`:
   - Add `pub fn search_vectors(&self, query: &[f32], limit: usize) -> Result<Vec<(String, f64)>>`:
     - Acquires read lock
     - Calls `hnsw.search(query, limit)` to get `(key, distance)` pairs
     - Maps keys back to chunk IDs via reverse `id_to_key` lookup
     - Returns `Vec<(chunk_id, score)>`
   - Add `pub fn get_chunk(&self, chunk_id: &str) -> Result<Option<StoredChunk>>`: read lock, lookup in metadata
   - Add `pub fn get_file_metadata(&self, path: &str) -> Result<Option<StoredFile>>`: read lock, lookup in metadata

5. **Update `src/lib.rs`** — Add `pub mod search;`

6. **Write search unit tests** — In `src/search.rs` `#[cfg(test)] mod tests`:
   - Test: search with mock provider returns results sorted by score
   - Test: `min_score` threshold filters out low-relevance results
   - Test: `limit` caps the number of returned results
   - Test: empty index returns empty results
   - Test: query embedding failure returns `Error::EmbeddingProvider`

7. **Write filter tests** — In `src/search.rs` tests:
   - Test: `Equals` filter matches exact frontmatter value
   - Test: `Equals` filter rejects non-matching value
   - Test: `In` filter matches when value is in list
   - Test: `In` filter works with array frontmatter fields (intersection)
   - Test: `Range` filter matches values within range
   - Test: `Range` filter with only `min` (no max) works
   - Test: `Range` filter with only `max` (no min) works
   - Test: `Exists` filter returns true when field exists
   - Test: `Exists` filter returns false when field is missing
   - Test: multiple filters use AND logic (all must pass)
   - Test: filters on file without frontmatter return no results

8. **Write integration test** — Create `tests/search_test.rs`:
   - Create an index with mock embeddings for 10 test chunks
   - Search with a query that should match specific chunks
   - Verify: correct chunks returned, scores are reasonable, metadata is complete
   - Search with metadata filters and verify filtered results
   - Test performance: searching 10k mock chunks completes in under 100ms

## Validation Criteria

- [ ] `search()` returns results sorted by relevance score (highest first)
- [ ] Each result contains: score, chunk content, heading hierarchy, line range, file path, frontmatter
- [ ] `limit=5` returns at most 5 results
- [ ] `min_score=0.8` excludes results with score below 0.8
- [ ] `Equals` filter on `tags: [rust]` matches files with `tags: [rust, cli]` (list contains value)
- [ ] `In` filter on `status` with values `["draft", "review"]` matches files with `status: draft`
- [ ] `Range` filter on `year` with min=2023, max=2025 matches files with `year: 2024`
- [ ] `Exists` filter on `author` excludes files without an `author` field
- [ ] Multiple filters are AND'd — all must pass for a result to be included
- [ ] Empty query string returns error, not results
- [ ] Search on empty index returns empty results (no error)
- [ ] Query embedding uses exactly 1 API call (single text)
- [ ] Over-fetching (limit * 3) compensates for filter losses so that applying filters still yields up to `limit` results
- [ ] Warm search on 10k chunks completes in under 100ms
- [ ] `cargo test` passes all search tests
- [ ] `cargo clippy` reports no warnings

## Anti-Patterns to Avoid

- **Do NOT embed the query synchronously** — Use `async` embedding to avoid blocking the tokio runtime. The embedding API call is the single largest latency contributor to search.
- **Do NOT fetch exact `limit` candidates from HNSW** — Over-fetch by 3x to account for results that will be discarded by `min_score` or metadata filters. Fetching exactly `limit` with aggressive filters could return 0 results when matches exist.
- **Do NOT parse frontmatter JSON on every filter evaluation** — Parse `StoredFile.frontmatter` once per candidate, then evaluate all filters against the parsed `serde_json::Value`.
- **Do NOT use string comparison for numeric range filters** — Detect numeric values and compare as f64. String comparison gives wrong results: `"9" > "10"` lexicographically.
- **Do NOT clone large strings in results** — Use references where possible; only clone into `SearchResult` at the final assembly step.

## Patterns to Follow

- **Builder pattern:** `SearchQuery::new("query").with_limit(10).with_filter(...)` for ergonomic query construction
- **Data flow:** Query string → embedding → HNSW search → candidate enrichment → filter → result assembly — each step is a clear transformation
- **Error handling:** `Error::EmbeddingProvider` for embedding failures, `Error::IndexNotFound` if index not loaded
- **Serialization:** Derive `serde::Serialize` on all result types so Phase 10 (CLI) can output them as JSON
- **Read lock scope:** Acquire the index read lock once, do all lookups, release — don't acquire/release per candidate

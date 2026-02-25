# PRD: Phase 4 — Embedding Provider System

## Overview

Build the pluggable embedding provider system with a standardized trait interface, concrete implementations for OpenAI and Ollama, batch processing support, and content-hash-based skip logic. This phase converts text chunks into vector embeddings — the core data that powers semantic search.

## Problem Statement

Semantic search requires vector embeddings — numerical representations of text that capture meaning. Different users prefer different embedding providers (cloud-based OpenAI for quality, local Ollama for privacy/cost). The system needs a unified interface that makes the provider choice transparent to all downstream code, while supporting batch processing to minimize API calls and content hashing to avoid redundant re-embedding.

## Goals

- Define an `EmbeddingProvider` trait that all providers implement
- Implement OpenAI-compatible provider (works with OpenAI API and compatible endpoints)
- Implement Ollama provider for local inference
- Support custom endpoints via configurable URL
- Batch embedding: process `MDVDB_EMBEDDING_BATCH_SIZE` chunks per API request
- Content hash comparison: skip re-embedding when file content hasn't changed
- All providers return `Vec<f32>` vectors of dimension `MDVDB_EMBEDDING_DIMENSIONS`
- Async-friendly for integration into the tokio runtime

## Non-Goals

- No local model inference (only HTTP-based providers)
- No embedding model download or management
- No vector storage (Phase 5)
- No caching of embeddings beyond content-hash skip logic
- No provider auto-detection — user must configure explicitly

## Technical Design

### Data Model Changes

**`EmbeddingProvider` trait:**

```rust
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a batch of text strings into vectors
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Return the dimensionality of the embedding vectors
    fn dimensions(&self) -> usize;

    /// Return the provider name for logging
    fn name(&self) -> &str;
}
```

**`OpenAIProvider` struct:**

```rust
pub struct OpenAIProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    dimensions: usize,
    endpoint: String, // default: "https://api.openai.com/v1/embeddings"
}
```

**`OllamaProvider` struct:**

```rust
pub struct OllamaProvider {
    client: reqwest::Client,
    host: String,       // default: "http://localhost:11434"
    model: String,
    dimensions: usize,
}
```

**`EmbeddingResult` struct** — maps chunks to their embeddings:

```rust
pub struct EmbeddingResult {
    /// Chunk ID → embedding vector
    pub embeddings: HashMap<String, Vec<f32>>,
    /// Chunk IDs that were skipped (content unchanged)
    pub skipped: Vec<String>,
    /// Number of API calls made
    pub api_calls: usize,
}
```

### Interface Changes

**Provider factory:**

```rust
/// Create the appropriate embedding provider based on config
pub fn create_provider(config: &Config) -> Result<Box<dyn EmbeddingProvider>>;
```

**Batch embedding orchestrator:**

```rust
/// Embed all chunks, skipping those whose source file hash hasn't changed
pub async fn embed_chunks(
    provider: &dyn EmbeddingProvider,
    chunks: &[Chunk],
    existing_hashes: &HashMap<PathBuf, String>, // path → previous content hash
    current_hashes: &HashMap<PathBuf, String>,  // path → current content hash
    batch_size: usize,
) -> Result<EmbeddingResult>;
```

### API Request/Response Formats

**OpenAI Embedding API:**
```json
// POST https://api.openai.com/v1/embeddings
// Header: Authorization: Bearer <api_key>
{
    "input": ["text1", "text2", ...],
    "model": "text-embedding-3-small",
    "dimensions": 1536
}
// Response:
{
    "data": [
        {"embedding": [0.1, 0.2, ...], "index": 0},
        {"embedding": [0.3, 0.4, ...], "index": 1}
    ]
}
```

**Ollama Embedding API:**
```json
// POST http://localhost:11434/api/embed
{
    "model": "nomic-embed-text",
    "input": ["text1", "text2", ...]
}
// Response:
{
    "embeddings": [[0.1, 0.2, ...], [0.3, 0.4, ...]]
}
```

### Migration Strategy

Not applicable — no prior data exists.

## Implementation Steps

1. **Add `async-trait` dependency** — Add `async-trait = "0.1"` to `Cargo.toml` (needed for async trait methods).

2. **Create `src/embedding/mod.rs`** — Define the module structure:
   - `pub mod provider;` — trait definition
   - `pub mod openai;` — OpenAI provider
   - `pub mod ollama;` — Ollama provider
   - `pub mod batch;` — batch orchestration and hash comparison
   - Re-export: `pub use provider::EmbeddingProvider;`

3. **Create `src/embedding/provider.rs`** — Define the `EmbeddingProvider` trait as shown in Technical Design. Include the `create_provider(config)` factory function that matches on `config.embedding_provider` and constructs the appropriate provider.

4. **Create `src/embedding/openai.rs`** — Implement `OpenAIProvider`:
   - Constructor takes `api_key`, `model`, `dimensions`, and optional `endpoint` (defaults to `https://api.openai.com/v1/embeddings`)
   - `embed_batch()`: POST to the endpoint with JSON body `{"input": texts, "model": model, "dimensions": dimensions}`, parse response, extract embedding vectors sorted by `index` field
   - Set `Authorization: Bearer {api_key}` header
   - Handle errors: 401 (invalid key) → `Error::EmbeddingProvider("Invalid API key")`, 429 (rate limit) → retry with exponential backoff (max 3 retries), 5xx → `Error::EmbeddingProvider` with status
   - Validate response: check that returned vector count matches input count, check that each vector has the expected dimensions

5. **Create `src/embedding/ollama.rs`** — Implement `OllamaProvider`:
   - Constructor takes `host`, `model`, `dimensions`
   - `embed_batch()`: POST to `{host}/api/embed` with JSON body `{"model": model, "input": texts}`, parse response, extract embeddings array
   - Handle errors: connection refused → `Error::EmbeddingProvider("Cannot connect to Ollama at {host}")`, model not found → `Error::EmbeddingProvider("Model {model} not found")`
   - Validate response: check vector count and dimensions match expectations

6. **Create `src/embedding/batch.rs`** — Implement the batch embedding orchestrator:
   - `embed_chunks()` function:
     1. Group chunks by source file path
     2. For each file, compare `current_hashes[path]` with `existing_hashes[path]` — if identical, add all chunks from that file to the `skipped` list
     3. Collect non-skipped chunks into batches of `batch_size`
     4. For each batch, call `provider.embed_batch()` concurrently (up to 4 concurrent batches to avoid rate limits)
     5. Map results back to chunk IDs
     6. Return `EmbeddingResult` with all embeddings, skipped IDs, and API call count
   - Use `tokio::task::JoinSet` or `futures::stream::FuturesUnordered` for concurrent batch processing
   - Log progress: `tracing::info!("Embedding batch {}/{}: {} chunks", batch_num, total_batches, batch.len())`

7. **Update `src/lib.rs`** — Add `pub mod embedding;`

8. **Write provider unit tests** — In each provider file, add `#[cfg(test)] mod tests`:
   - OpenAI: test request body format, test response parsing, test error handling for 401/429/5xx (use mock HTTP server or test against serialized responses)
   - Ollama: test request body format, test response parsing, test connection error handling
   - Use `serde_json` to verify request/response serialization

9. **Write batch orchestrator tests** — In `src/embedding/batch.rs` tests:
   - Test: all chunks embedded when no existing hashes (nothing to skip)
   - Test: chunks from unchanged files (matching hashes) are skipped entirely
   - Test: chunks from changed files (different hashes) are embedded
   - Test: mixed scenario (some files changed, some not) produces correct split
   - Test: batch size of 2 with 5 chunks produces 3 API calls
   - Test: empty chunk list returns empty results
   - Use a mock `EmbeddingProvider` implementation that returns deterministic vectors

10. **Create mock provider for testing** — Add `src/embedding/mock.rs`:
    - `MockProvider` that implements `EmbeddingProvider`
    - Returns deterministic vectors (e.g., hash of input text mapped to floats)
    - Tracks call count for batch size verification
    - Useful for all downstream testing (Phase 5, 6) without real API calls

## Validation Criteria

- [ ] `create_provider(config)` returns `OpenAIProvider` when `MDVDB_EMBEDDING_PROVIDER=openai`
- [ ] `create_provider(config)` returns `OllamaProvider` when `MDVDB_EMBEDDING_PROVIDER=ollama`
- [ ] `create_provider(config)` returns error for unknown provider type
- [ ] OpenAI provider sends correct request format with `Authorization: Bearer` header
- [ ] OpenAI provider parses response and returns vectors in correct order (sorted by `index`)
- [ ] Ollama provider sends correct request format to `{host}/api/embed`
- [ ] Returned vectors have exactly `MDVDB_EMBEDDING_DIMENSIONS` dimensions
- [ ] Batch size of 100 with 250 chunks makes 3 API calls (100 + 100 + 50)
- [ ] Chunks from files with unchanged content hashes are skipped (0 API calls for those)
- [ ] Chunks from files with changed content hashes are re-embedded
- [ ] Rate limit (429) response triggers retry with backoff (up to 3 retries)
- [ ] Invalid API key (401) returns `Error::EmbeddingProvider` immediately (no retry)
- [ ] `MockProvider` returns deterministic vectors for testing
- [ ] `cargo test` passes all embedding tests
- [ ] `cargo clippy` reports no warnings

## Anti-Patterns to Avoid

- **Do NOT embed one chunk at a time** — Always use batch processing. Embedding 1000 chunks individually makes 1000 API calls; batching at 100 makes 10. This is the difference between 30 seconds and 0.3 seconds.
- **Do NOT hardcode API endpoints** — OpenAI-compatible APIs (Azure, local proxies) use different base URLs. The endpoint must come from config.
- **Do NOT ignore the `dimensions` field in OpenAI requests** — Some models (text-embedding-3-small) support flexible dimensions. Always pass the configured dimensions to get consistent vector sizes.
- **Do NOT block the tokio runtime with synchronous HTTP calls** — Use `reqwest`'s async API. Blocking calls inside async context will deadlock.
- **Do NOT retry on 401 (auth) errors** — Auth errors are permanent. Only retry on transient errors (429, 5xx).
- **Do NOT store the API key in logs** — Use `tracing` but never log the `api_key` value. Log the provider name and endpoint, not credentials.

## Patterns to Follow

- **Trait-based abstraction:** All code outside this module interacts with `&dyn EmbeddingProvider`, never concrete types. This is what makes providers pluggable.
- **Module structure:** `embedding/mod.rs` as the public API, submodules for each provider — mirrors how Rust crates conventionally organize implementations behind a trait.
- **Error mapping:** Map `reqwest::Error` to `Error::EmbeddingProvider(msg)` at the provider boundary. Downstream code never sees HTTP-specific errors.
- **Async patterns:** Use `async fn` on all I/O operations. The `#[async_trait]` macro enables async methods in trait definitions.
- **Testing:** Mock provider for unit tests; integration tests against real providers are optional and gated behind feature flags or env vars.

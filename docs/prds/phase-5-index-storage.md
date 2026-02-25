# PRD: Phase 5 — Index Storage & Memory Mapping

## Overview

Build the single-file index that stores all vector data (via `usearch` HNSW), metadata snapshots, chunk mappings, file references, content hashes, and schema — all memory-mappable for fast on-demand loading. This phase also implements the concurrency model (read/write locks) that protects the index during concurrent access. The index file is the system's only artifact and must be portable.

## Problem Statement

The system needs a persistent storage layer that holds everything computed during ingestion: vector embeddings (for search), chunk metadata (for result display), file references (for context), and content hashes (for change detection). This must be a single portable file that supports memory-mapped access for fast cold starts (under 500ms for 10k docs) and warm queries (under 100ms). Multiple readers must be able to query simultaneously while writes acquire exclusive access.

## Goals

- Single index file at `MDVDB_INDEX_FILE` path containing all system state
- HNSW vector index via `usearch` for approximate nearest neighbor search
- Metadata serialized via `rkyv` for zero-copy deserialization from memory-mapped files
- Memory-mapped loading via `memmap2` — no full file read into RAM
- Index file format: `[header][rkyv metadata region][usearch HNSW region]`
- Support adding, updating, and removing individual entries without full rebuild
- Concurrent read access with exclusive write locking via `parking_lot::RwLock`
- All file paths stored as relative to project root
- Deleting the index file triggers full re-index on next run
- Cold load under 500ms for 10k-document index

## Non-Goals

- No search query logic (Phase 6)
- No clustering data storage (Phase 9 extends the index)
- No file watching (Phase 8)
- No remote storage or sync
- No index compression or compaction

## Technical Design

### Index File Format

The index file is a binary file with three sections:

```
[Header (64 bytes)]
  - Magic bytes: "MDVDB\x00" (6 bytes)
  - Version: u32 (4 bytes)
  - Metadata region offset: u64 (8 bytes)
  - Metadata region size: u64 (8 bytes)
  - HNSW region offset: u64 (8 bytes)
  - HNSW region size: u64 (8 bytes)
  - Reserved: zero-padded to 64 bytes

[rkyv Metadata Region]
  - Zero-copy serialized IndexMetadata struct

[usearch HNSW Region]
  - Native usearch binary index data
```

### Data Model Changes

**`IndexMetadata` struct** — everything except vectors, serialized with rkyv:

```rust
#[derive(Archive, Serialize, Deserialize)]
pub struct IndexMetadata {
    /// Mapping from chunk ID to chunk metadata
    pub chunks: HashMap<String, StoredChunk>,
    /// Mapping from file path (relative) to file metadata
    pub files: HashMap<String, StoredFile>,
    /// When the index was last updated (Unix timestamp)
    pub last_updated: u64,
    /// Total number of documents (files) indexed
    pub document_count: usize,
    /// Total number of chunks in the index
    pub chunk_count: usize,
    /// Embedding provider and model used (for consistency checks)
    pub embedding_config: EmbeddingConfig,
}

#[derive(Archive, Serialize, Deserialize)]
pub struct StoredChunk {
    pub source_path: String,
    pub heading_hierarchy: Vec<String>,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub chunk_index: usize,
    pub is_sub_split: bool,
}

#[derive(Archive, Serialize, Deserialize)]
pub struct StoredFile {
    pub relative_path: String,
    pub content_hash: String,
    pub frontmatter: Option<String>, // JSON-serialized frontmatter
    pub file_size: u64,
    pub chunk_ids: Vec<String>,
    pub indexed_at: u64, // Unix timestamp
}

#[derive(Archive, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
}
```

**`Index` struct** — the runtime index handle:

```rust
pub struct Index {
    /// Path to the index file
    path: PathBuf,
    /// Read/write lock protecting the index state
    lock: parking_lot::RwLock<IndexState>,
}

struct IndexState {
    /// Metadata region (deserialized or mmap'd)
    metadata: IndexMetadata,
    /// usearch HNSW index
    hnsw: usearch::Index,
    /// Mapping from chunk ID to usearch key (u64)
    id_to_key: HashMap<String, u64>,
    /// Next available usearch key
    next_key: u64,
}
```

### Interface Changes

```rust
impl Index {
    /// Open an existing index file (memory-mapped)
    pub fn open(path: &Path) -> Result<Self>;

    /// Create a new empty index
    pub fn create(path: &Path, embedding_config: EmbeddingConfig) -> Result<Self>;

    /// Open existing or create new
    pub fn open_or_create(path: &Path, embedding_config: EmbeddingConfig) -> Result<Self>;

    /// Add or update chunks with their embeddings (acquires write lock)
    pub fn upsert(
        &self,
        chunks: &[Chunk],
        embeddings: &HashMap<String, Vec<f32>>,
        files: &[MarkdownFile],
    ) -> Result<()>;

    /// Remove all chunks belonging to a file (acquires write lock)
    pub fn remove_file(&self, relative_path: &Path) -> Result<()>;

    /// Get stored file metadata (acquires read lock)
    pub fn get_file(&self, relative_path: &Path) -> Result<Option<StoredFile>>;

    /// Get all stored file paths and their content hashes (acquires read lock)
    pub fn get_file_hashes(&self) -> Result<HashMap<PathBuf, String>>;

    /// Get index status: document count, chunk count, last updated
    pub fn status(&self) -> Result<IndexStatus>;

    /// Flush in-memory changes to disk (acquires write lock)
    pub fn save(&self) -> Result<()>;

    /// Check if embedding config matches (provider, model, dimensions)
    pub fn check_config_compatibility(&self, config: &EmbeddingConfig) -> Result<()>;
}

pub struct IndexStatus {
    pub document_count: usize,
    pub chunk_count: usize,
    pub last_updated: u64,
    pub index_file_size: u64,
    pub embedding_config: EmbeddingConfig,
}
```

### Concurrency Model

- `Index` wraps all mutable state in `parking_lot::RwLock<IndexState>`
- Read operations (`get_file`, `get_file_hashes`, `status`) acquire a read lock — multiple readers can proceed concurrently
- Write operations (`upsert`, `remove_file`, `save`) acquire a write lock — exclusive access
- `parking_lot::RwLock` is chosen over `std::sync::RwLock` for performance (no syscall on uncontended acquire) and no poisoning
- The `save()` method serializes the full state to a temporary file, then atomically renames it to the index path (crash-safe)

### Migration Strategy

If the index file exists but has an incompatible version or mismatched embedding config (different model/dimensions), log a warning and trigger a full re-index. The old index file is renamed to `.markdownvdb.index.bak` before rebuilding.

## Implementation Steps

1. **Create `src/index/mod.rs`** — Define the module:
   - `pub mod storage;` — index file I/O (header, rkyv, usearch)
   - `pub mod state;` — `IndexState` and in-memory operations
   - `pub mod types;` — `StoredChunk`, `StoredFile`, `EmbeddingConfig`, `IndexStatus`
   - Re-export: `pub use storage::Index;`

2. **Create `src/index/types.rs`** — Define all data structures:
   - `StoredChunk`, `StoredFile`, `EmbeddingConfig`, `IndexStatus`, `IndexMetadata`
   - Derive `rkyv::Archive`, `rkyv::Serialize`, `rkyv::Deserialize` on types that go into the index
   - Derive `serde::Serialize` on `IndexStatus` for JSON output
   - Implement `From<&Chunk>` for `StoredChunk` and `From<&MarkdownFile>` for `StoredFile` conversions

3. **Create `src/index/storage.rs`** — Implement index file I/O:
   - `write_index(path, metadata, hnsw_index)`:
     1. Write to a temp file (same directory, `.tmp` suffix)
     2. Write the 64-byte header with magic, version, and region offsets
     3. Serialize `IndexMetadata` with `rkyv::to_bytes()` and write the rkyv region
     4. Save `usearch::Index` to a temp buffer and write the HNSW region
     5. Update header with actual offsets and sizes
     6. `fsync` the temp file
     7. Atomically rename temp file to the target path
   - `read_index(path)`:
     1. Memory-map the file with `memmap2::Mmap`
     2. Validate magic bytes and version
     3. Read header to get region offsets
     4. Access the rkyv region via `rkyv::access::<IndexMetadata>(&mmap[offset..offset+size])`
     5. Load the usearch HNSW region with `usearch::Index::load()` or from the mmap'd region
     6. Build the `id_to_key` mapping from metadata
     7. Return `IndexState`

4. **Create `src/index/state.rs`** — Implement `Index` with concurrency:
   - `Index::open(path)`: call `read_index`, wrap result in `RwLock`
   - `Index::create(path, config)`: create empty `IndexState` with new `usearch::Index`, wrap in `RwLock`
   - `Index::open_or_create(path, config)`: try `open`, fall back to `create` if file doesn't exist
   - `upsert()`: acquire write lock → for each chunk, add/update in `metadata.chunks`, update `metadata.files`, add vector to `hnsw` index with a u64 key, update `id_to_key` mapping
   - `remove_file()`: acquire write lock → remove all chunk entries for the file, remove vectors from `hnsw`, remove the file entry, update `id_to_key`
   - `get_file()`, `get_file_hashes()`, `status()`: acquire read lock → return data
   - `save()`: acquire write lock → call `write_index()`
   - `check_config_compatibility()`: compare stored `EmbeddingConfig` with provided config, return error if dimensions or model differ

5. **Set up `usearch` HNSW index** — Configure the usearch index:
   - Metric: `usearch::MetricKind::Cos` (cosine similarity)
   - Dimensions: from `embedding_config.dimensions`
   - Connectivity: 16 (HNSW M parameter — good default for 10k-100k vectors)
   - Expansion add: 128, Expansion search: 64 (standard HNSW defaults)
   - Use u64 keys that map to chunk IDs via `id_to_key` HashMap

6. **Update `src/lib.rs`** — Add `pub mod index;`

7. **Write storage tests** — Create `tests/index_test.rs`:
   - Test: create new index, save to disk, reopen and verify contents match
   - Test: upsert chunks with embeddings, save, reopen, verify chunk metadata accessible
   - Test: remove a file's chunks, save, reopen, verify chunks are gone
   - Test: `get_file_hashes()` returns correct hash mapping
   - Test: `status()` returns correct document and chunk counts
   - Test: header magic bytes are validated on open (corrupted file → `Error::IndexCorrupted`)
   - Test: version mismatch returns error
   - Test: embedding config mismatch detected by `check_config_compatibility()`
   - Test: atomic save (simulate crash — if temp file exists but rename didn't complete, original index is intact)
   - Test: index with 0 documents opens and reports empty status

8. **Write concurrency tests** — In `tests/index_test.rs`:
   - Test: multiple concurrent readers can query simultaneously (spawn threads)
   - Test: writer blocks readers during upsert
   - Test: reader gets consistent snapshot (no partial writes visible)
   - Use `std::thread::spawn` with `Arc<Index>` shared across threads

## Validation Criteria

- [ ] New index file is created at `MDVDB_INDEX_FILE` path with correct header magic bytes `"MDVDB\x00"`
- [ ] Index file can be opened via memory mapping without reading entire file into RAM
- [ ] Upserting 100 chunks with embeddings and saving produces a valid index file
- [ ] Reopening a saved index file recovers all chunk metadata, file metadata, and content hashes
- [ ] `remove_file()` removes all associated chunks and vectors
- [ ] Vectors are searchable via `usearch` after upsert (basic nearest-neighbor query returns correct result)
- [ ] `get_file_hashes()` returns the correct SHA-256 hash for each indexed file
- [ ] `status()` returns accurate `document_count`, `chunk_count`, and `last_updated`
- [ ] Corrupted index file (wrong magic bytes) returns `Error::IndexCorrupted`
- [ ] Embedding config mismatch (different dimensions) is detected on open
- [ ] Concurrent reads complete without blocking each other
- [ ] Write operations are exclusive (no concurrent writes)
- [ ] Atomic save: power loss during save leaves the previous valid index intact
- [ ] Index file is portable — moving it with markdown files to another directory works (relative paths)
- [ ] `cargo test` passes all index tests
- [ ] `cargo clippy` reports no warnings

## Anti-Patterns to Avoid

- **Do NOT serialize vectors with rkyv** — Vectors are stored in the `usearch` HNSW index, not in the rkyv metadata region. `usearch` handles its own serialization with native mmap support. Storing vectors twice wastes space and complicates updates.
- **Do NOT use `std::sync::RwLock`** — Use `parking_lot::RwLock` for better performance on uncontended paths and no lock poisoning. The standard library RwLock has higher overhead.
- **Do NOT write directly to the index file** — Always write to a temp file and atomically rename. Direct writes risk corruption on crash.
- **Do NOT use sequential u64 keys without a mapping** — `usearch` uses u64 keys internally, but chunks are identified by string IDs. Maintain the `id_to_key` HashMap to translate between them.
- **Do NOT load the full index into RAM for reads** — Use memory mapping so the OS loads only the pages actually accessed. This is critical for the cold-start performance target.
- **Do NOT store absolute paths in the index** — All paths must be relative to the project root. The index file must be portable.

## Patterns to Follow

- **File format:** Header + rkyv region + usearch region, with offsets in the header — this allows independent access to each region via mmap
- **Atomic writes:** Write to `.tmp` file → `fsync` → rename. This is the standard safe-write pattern for persistent data.
- **Concurrency:** `parking_lot::RwLock` around `IndexState` — read methods take `&self`, write methods take `&self` (lock is interior)
- **Error handling:** Return `Error::IndexNotFound` for missing file, `Error::IndexCorrupted` for invalid format, `Error::Io` for filesystem errors
- **Module structure:** `index/types.rs` for data structures, `index/storage.rs` for file I/O, `index/state.rs` for runtime operations — clean separation of concerns

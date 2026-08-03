use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use parking_lot::RwLock;
use rayon::prelude::*;
use usearch::Index as HnswIndex;

use tracing::debug;

use crate::chunker::Chunk;
use crate::clustering::{ClusterState, CustomClusterState};
use crate::error::{Error, Result};
use crate::index::storage::{self, WriteOptions};
use crate::index::types::{
    ComputedFieldEntry, EmbeddingConfig, IndexMetadata, IndexStatus, ScopedCounts, StoredChunk,
    StoredFile,
};
use crate::links::LinkGraph;
use crate::parser::MarkdownFile;
use crate::schema::Schema;

/// Information about a single chunk's vector embedding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChunkVectorInfo {
    /// The chunk ID (e.g. "path/to/file.md#0").
    pub chunk_id: String,
    /// Relative path to the source markdown file.
    pub source_path: String,
    /// Heading hierarchy leading to this chunk.
    pub heading_hierarchy: Vec<String>,
    /// 0-based index of this chunk within the file.
    pub chunk_index: usize,
    /// Chunk content length in bytes, used to size graph nodes without cloning
    /// the complete stored chunk for every node.
    pub content_len: usize,
    /// The embedding vector for this chunk.
    pub vector: Vec<f32>,
}

/// Interned output from a batch of chunk-vector searches.
///
/// Neighbor ids are owned once in `ids`; every match refers to that table by
/// index. This keeps large chunk graphs from allocating the same path-sized
/// string for every query result.
pub(crate) struct VectorSearchBatch {
    pub(crate) ids: Vec<String>,
    pub(crate) matches: Vec<Vec<(usize, f64)>>,
}

fn collect_vector_search_indices(
    hnsw: &HnswIndex,
    key_to_id_index: &HashMap<u64, usize>,
    query: &[f32],
    limit: usize,
    search_limit: usize,
) -> Result<Vec<(usize, f64)>> {
    let chunk_count = key_to_id_index.len();
    if limit == 0 || chunk_count == 0 {
        return Ok(Vec::new());
    }

    let results = hnsw
        .search(query, search_limit.min(hnsw.size()))
        .map_err(|e| Error::Serialization(format!("usearch search: {e}")))?;

    let mut output = Vec::with_capacity(results.keys.len());
    for (key, distance) in results.keys.iter().zip(results.distances.iter()) {
        if let Some(id_index) = key_to_id_index.get(key) {
            output.push((*id_index, 1.0 - *distance as f64));
        }
    }

    output.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    output.truncate(limit);
    Ok(output)
}

fn chunk_id_projection(state: &IndexState) -> (Vec<&str>, HashMap<u64, usize>) {
    // Key order is stable after index compaction and avoids making the string
    // table depend on HashMap iteration order. Edge ids are omitted up front.
    let mut keyed_ids: Vec<(u64, &str)> = state
        .id_to_key
        .iter()
        .filter(|(id, _)| !id.starts_with("edge:"))
        .map(|(id, key)| (*key, id.as_str()))
        .collect();
    keyed_ids.sort_unstable_by_key(|(key, _)| *key);

    let mut ids = Vec::with_capacity(keyed_ids.len());
    let mut key_to_id_index = HashMap::with_capacity(keyed_ids.len());
    for (key, id) in keyed_ids {
        let index = ids.len();
        ids.push(id);
        key_to_id_index.insert(key, index);
    }
    (ids, key_to_id_index)
}

/// Internal mutable state protected by the RwLock.
struct IndexState {
    metadata: IndexMetadata,
    hnsw: HnswIndex,
    id_to_key: HashMap<String, u64>,
    next_key: u64,
    dirty: bool,
}

/// Thread-safe handle to a memory-mapped index file.
pub struct Index {
    path: PathBuf,
    state: RwLock<IndexState>,
    write_options: WriteOptions,
}

impl Index {
    /// Replace the current in-memory vector generation with an empty one using
    /// a newly resolved embedding space. Nothing is persisted until `save`, so
    /// callers can finish all remote inference before invalidating the last
    /// good on-disk generation.
    pub fn reset_embedding_space(&self, config: &EmbeddingConfig) -> Result<()> {
        if config.dimensions == 0 {
            return Err(Error::Config(
                "cannot initialize an index with unresolved dimensions".into(),
            ));
        }
        let hnsw = storage::create_hnsw(
            config.dimensions,
            storage::scalar_kind_for(&self.write_options.quantization),
        )?;
        let mut state = self.state.write();
        state.metadata = IndexMetadata {
            chunks: HashMap::new(),
            files: HashMap::new(),
            embedding_config: config.clone(),
            last_updated: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
            schema: None,
            cluster_state: None,
            link_graph: None,
            file_mtimes: Some(HashMap::new()),
            scoped_schemas: None,
            custom_cluster_state: None,
        };
        state.hnsw = hnsw;
        state.id_to_key.clear();
        state.next_key = 0;
        state.dirty = true;
        Ok(())
    }

    /// Open an existing index file at the given path with default write options.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_options(path, WriteOptions::default())
    }

    /// Reload the persisted generation into this handle.
    ///
    /// Manual module commands acquire their cross-process run lock only after
    /// constructing `MarkdownVdb`. Reloading under that lock prevents a process
    /// which opened just before another module run completed from evaluating
    /// and later saving an obsolete in-memory generation.
    ///
    /// A dirty handle represents an unfinished transaction. Silently retaining
    /// it here would let the next transaction save that partial branch over a
    /// newer on-disk generation, so callers must explicitly discard/reopen it.
    pub fn reload_from_disk_if_clean(&self) -> Result<bool> {
        // Hold the write guard for the complete check/load/swap sequence.  A
        // watcher in this process must not make the handle dirty between the
        // cleanliness check and replacement with the on-disk generation.
        let mut state = self.state.write();
        if state.dirty {
            return Err(Error::IndexDirty {
                path: self.path.clone(),
            });
        }
        let refreshed = Self::open_with_options(&self.path, self.write_options.clone())?;
        *state = refreshed.state.into_inner();
        Ok(true)
    }

    /// Open an existing index file at the given path with explicit write options.
    pub fn open_with_options(path: &Path, write_options: WriteOptions) -> Result<Self> {
        let (metadata, hnsw) = storage::load_index(path)?;

        // Build id_to_key mapping and compute next_key from chunk IDs.
        // Sort chunk IDs alphabetically for deterministic key assignment,
        // ensuring reproducible mapping regardless of HashMap iteration order.
        let mut sorted_chunk_ids: Vec<&String> = metadata.chunks.keys().collect();
        sorted_chunk_ids.sort();

        let mut id_to_key = HashMap::new();
        for (idx, chunk_id) in sorted_chunk_ids.iter().enumerate() {
            id_to_key.insert((*chunk_id).clone(), idx as u64);
        }

        let mut next_key = sorted_chunk_ids.len() as u64;

        // Also load edge IDs from the link graph's semantic_edges map.
        // Edge vectors exist in the HNSW index from a prior save() but are
        // NOT in metadata.chunks, so we must reconstruct their id_to_key entries.
        if let Some(ref link_graph) = metadata.link_graph {
            if let Some(ref semantic_edges) = link_graph.semantic_edges {
                let mut sorted_edge_ids: Vec<&String> = semantic_edges.keys().collect();
                sorted_edge_ids.sort();
                for edge_id in sorted_edge_ids {
                    id_to_key.insert(edge_id.clone(), next_key);
                    next_key += 1;
                }
            }
        }

        // Safety: ensure next_key exceeds any key in the loaded HNSW.
        // After save() compaction, keys are assigned sequentially 0..total-1.
        // If metadata (chunks + semantic_edges) doesn't account for all entries
        // (e.g., orphaned edge vectors), next_key could be too low, causing
        // "duplicate key" errors on subsequent adds. Use hnsw.size() as a
        // lower bound since the max key is at most total-1 >= size-1.
        let hnsw_size = hnsw.size() as u64;
        if next_key < hnsw_size {
            debug!(
                computed = next_key,
                hnsw_size, "next_key adjusted to match HNSW size"
            );
            next_key = hnsw_size;
        }

        Ok(Self {
            path: path.to_path_buf(),
            state: RwLock::new(IndexState {
                metadata,
                hnsw,
                id_to_key,
                next_key,
                dirty: false,
            }),
            write_options,
        })
    }

    /// Create a new, empty index file at the given path with default write options.
    pub fn create(path: &Path, config: &EmbeddingConfig) -> Result<Self> {
        Self::create_with_options(path, config, WriteOptions::default())
    }

    /// Create a new, empty index file at the given path with explicit write options.
    pub fn create_with_options(
        path: &Path,
        config: &EmbeddingConfig,
        write_options: WriteOptions,
    ) -> Result<Self> {
        let metadata = IndexMetadata {
            chunks: HashMap::new(),
            files: HashMap::new(),
            embedding_config: EmbeddingConfig {
                provider: config.provider.clone(),
                model: config.model.clone(),
                dimensions: config.dimensions,
            },
            last_updated: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            schema: None,
            cluster_state: None,
            link_graph: None,
            file_mtimes: Some(HashMap::new()),
            scoped_schemas: None,
            custom_cluster_state: None,
        };

        let scalar_kind = storage::scalar_kind_for(&write_options.quantization);
        let hnsw = storage::create_hnsw(config.dimensions, scalar_kind)?;
        hnsw.reserve(10)
            .map_err(|e| Error::Serialization(format!("usearch reserve: {e}")))?;

        storage::write_index(path, &metadata, &hnsw, &write_options)?;

        Ok(Self {
            path: path.to_path_buf(),
            state: RwLock::new(IndexState {
                metadata,
                hnsw,
                id_to_key: HashMap::new(),
                next_key: 0,
                dirty: false,
            }),
            write_options,
        })
    }

    /// Open an existing index or create a new one if it doesn't exist.
    pub fn open_or_create(path: &Path, config: &EmbeddingConfig) -> Result<Self> {
        Self::open_or_create_with_options(path, config, WriteOptions::default())
    }

    /// Open an existing index or create a new one, with explicit write options.
    pub fn open_or_create_with_options(
        path: &Path,
        config: &EmbeddingConfig,
        write_options: WriteOptions,
    ) -> Result<Self> {
        Self::open_or_create_with_options_report(path, config, write_options)
            .map(|(index, _rebuilt)| index)
    }

    /// Open an index and report whether a missing/incompatible generation had
    /// to be recreated. Callers use this to invalidate companion stores such
    /// as FTS in the same generation.
    pub fn open_or_create_with_options_report(
        path: &Path,
        config: &EmbeddingConfig,
        write_options: WriteOptions,
    ) -> Result<(Self, bool)> {
        Self::open_or_create_with_options_report_and_rebuild_hook(
            path,
            config,
            write_options,
            || Ok(()),
        )
    }

    /// Open an index and invoke `before_rebuild` before an incompatible or
    /// missing generation is removed/recreated. Companion stores use the hook
    /// to persist a reconciliation marker without changing the index format.
    pub(crate) fn open_or_create_with_options_report_and_rebuild_hook(
        path: &Path,
        config: &EmbeddingConfig,
        write_options: WriteOptions,
        before_rebuild: impl FnOnce() -> Result<()>,
    ) -> Result<(Self, bool)> {
        match Self::open_with_options(path, write_options.clone()) {
            Ok(index) => Ok((index, false)),
            Err(Error::IndexNotFound { .. })
            | Err(Error::IndexVersionMismatch { .. })
            | Err(Error::IndexCorrupted(_)) => {
                before_rebuild()?;
                // Remove outdated/corrupted index file so we can recreate it
                if path.exists() {
                    let _ = std::fs::remove_file(path);
                }
                Self::create_with_options(path, config, write_options).map(|index| (index, true))
            }
            Err(e) => Err(e),
        }
    }

    /// Upsert a file and its chunks into the index.
    ///
    /// If the file already exists, its old chunks and vectors are removed first.
    /// Each chunk is assigned a sequential HNSW key, and the corresponding embedding
    /// vector is added to the HNSW index.
    pub fn upsert(
        &self,
        file: &MarkdownFile,
        chunks: &[Chunk],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        let mut state = self.state.write();
        let relative_path = crate::path_util::to_slash(&file.path);

        debug!(path = %relative_path, chunks = chunks.len(), "upserting file");

        // Remove old vector data if file already exists, but retain module
        // ownership until hooks replace it. This lets a removed/malformed
        // formula definition clean its previously materialized source field
        // even when the raw file was re-indexed in the same operation.
        let previous_computed_fields =
            if let Some(old_file) = state.metadata.files.remove(&relative_path) {
                for chunk_id in &old_file.chunk_ids {
                    if let Some(key) = state.id_to_key.remove(chunk_id) {
                        let _ = state.hnsw.remove(key);
                    }
                    state.metadata.chunks.remove(chunk_id);
                }
                old_file.computed_fields
            } else {
                HashMap::new()
            };

        // Ensure HNSW has capacity for new vectors.
        let current_size = state.hnsw.size();
        let needed = current_size + chunks.len();
        if needed > state.hnsw.capacity() {
            state
                .hnsw
                .reserve(needed.max(current_size * 2))
                .map_err(|e| Error::Serialization(format!("usearch reserve: {e}")))?;
        }

        // Insert new chunks.
        let mut stored_file = StoredFile::from(file);
        stored_file.computed_fields = previous_computed_fields;
        stored_file.reconcile_materialized_proofs();
        for (i, chunk) in chunks.iter().enumerate() {
            let key = state.next_key;
            state.next_key += 1;

            state
                .hnsw
                .add(key, &embeddings[i])
                .map_err(|e| Error::Serialization(format!("usearch add: {e}")))?;

            let stored_chunk = StoredChunk::from(chunk);
            state.metadata.chunks.insert(chunk.id.clone(), stored_chunk);
            state.id_to_key.insert(chunk.id.clone(), key);
            stored_file.chunk_ids.push(chunk.id.clone());
        }

        // Store file modification time in the mtime map.
        state
            .metadata
            .file_mtimes
            .get_or_insert_with(HashMap::new)
            .insert(relative_path.clone(), file.modified_at);

        state.metadata.files.insert(relative_path, stored_file);
        state.dirty = true;
        Ok(())
    }

    /// Add a newly discovered source document without chunking or embedding it.
    ///
    /// Manual computed-module runs need a collection-wide raw-source snapshot so
    /// a newly created relation target (or incoming relation owner) can
    /// participate before the next ingest.  The empty embedding-body hash is an
    /// intentional provisional sentinel: a later incremental ingest must not
    /// mistake this metadata-only entry for an already embedded document, even
    /// when its source hash has not changed in the meantime.
    ///
    /// Existing documents are rejected so this helper can never discard their
    /// chunks or vectors. Call [`Self::refresh_source_metadata`] for those.
    pub(crate) fn insert_unembedded_source_metadata(&self, file: &MarkdownFile) -> Result<()> {
        let mut state = self.state.write();
        let relative_path = crate::path_util::to_slash(&file.path);
        if state.metadata.files.contains_key(&relative_path) {
            return Err(Error::Config(format!(
                "cannot insert provisional source metadata for existing file `{relative_path}`"
            )));
        }

        let mut stored_file = StoredFile::from(file);
        stored_file.embedding_body_hash.clear();
        state
            .metadata
            .file_mtimes
            .get_or_insert_with(HashMap::new)
            .insert(relative_path.clone(), file.modified_at);
        state.metadata.files.insert(relative_path, stored_file);
        state.dirty = true;
        Ok(())
    }

    /// Upsert edge vectors into the HNSW index.
    ///
    /// Each edge is a `(edge_id, embedding)` pair where `edge_id` uses the format
    /// `"edge:source.md->target.md@offset"`. Old edge vectors with the same IDs are
    /// removed first. Edge vectors are NOT added to `metadata.chunks` — they only
    /// exist in the HNSW index and `id_to_key` mapping.
    pub fn upsert_edges(&self, edges: &[(String, Vec<f32>)]) -> Result<()> {
        let mut state = self.state.write();

        debug!(count = edges.len(), "upserting edge vectors");

        // Remove old edge vectors with the same IDs.
        for (edge_id, _) in edges {
            if let Some(key) = state.id_to_key.remove(edge_id) {
                let _ = state.hnsw.remove(key);
            }
        }

        // Ensure HNSW has capacity for new vectors.
        let current_size = state.hnsw.size();
        let needed = current_size + edges.len();
        if needed > state.hnsw.capacity() {
            state
                .hnsw
                .reserve(needed.max(current_size * 2))
                .map_err(|e| Error::Serialization(format!("usearch reserve: {e}")))?;
        }

        // Insert new edge vectors.
        for (edge_id, embedding) in edges {
            let key = state.next_key;
            state.next_key += 1;

            state
                .hnsw
                .add(key, embedding)
                .map_err(|e| Error::Serialization(format!("usearch add: {e}")))?;

            state.id_to_key.insert(edge_id.clone(), key);
        }

        state.dirty = true;
        Ok(())
    }

    /// Remove a file and all its chunks from the index.
    ///
    /// Returns `Ok(())` if the file is not found (no-op).
    pub fn remove_file(&self, relative_path: &str) -> Result<()> {
        let mut state = self.state.write();

        let file = match state.metadata.files.remove(relative_path) {
            Some(f) => f,
            None => return Ok(()),
        };

        debug!(path = %relative_path, chunks = file.chunk_ids.len(), "removing file");

        for chunk_id in &file.chunk_ids {
            if let Some(key) = state.id_to_key.remove(chunk_id) {
                let _ = state.hnsw.remove(key);
            }
            state.metadata.chunks.remove(chunk_id);
        }

        // Remove edge vectors where edge ID starts with "edge:{file_path}->".
        let edge_prefix = format!("edge:{}->", relative_path);
        let edge_ids_to_remove: Vec<String> = state
            .id_to_key
            .keys()
            .filter(|id| id.starts_with(&edge_prefix))
            .cloned()
            .collect();
        for edge_id in &edge_ids_to_remove {
            if let Some(key) = state.id_to_key.remove(edge_id) {
                let _ = state.hnsw.remove(key);
            }
        }

        // Remove mtime entry.
        if let Some(ref mut mtimes) = state.metadata.file_mtimes {
            mtimes.remove(relative_path);
        }

        state.dirty = true;
        Ok(())
    }

    /// Get a cloned copy of the stored file entry for the given path.
    pub fn get_file(&self, relative_path: &str) -> Option<StoredFile> {
        let state = self.state.read();
        state.metadata.files.get(relative_path).cloned()
    }

    /// Get a cloned map of computed fields for one indexed file.
    pub fn get_computed_fields(
        &self,
        relative_path: &str,
    ) -> Option<HashMap<String, ComputedFieldEntry>> {
        let state = self.state.read();
        state
            .metadata
            .files
            .get(relative_path)
            .map(|file| file.computed_fields.clone())
    }

    /// Replace all computed fields for one indexed file.
    ///
    /// The module runner prepares a complete patch before calling this method,
    /// keeping partially evaluated state out of the persisted index.
    pub fn replace_computed_fields(
        &self,
        relative_path: &str,
        computed_fields: HashMap<String, ComputedFieldEntry>,
    ) -> Result<()> {
        let mut state = self.state.write();
        let file =
            state
                .metadata
                .files
                .get_mut(relative_path)
                .ok_or_else(|| Error::FileNotInIndex {
                    path: PathBuf::from(relative_path),
                })?;
        file.computed_fields = computed_fields;
        file.reconcile_materialized_proofs();
        state.dirty = true;
        Ok(())
    }

    /// Refresh source metadata without touching chunks, vectors, or the body
    /// hash represented by those vectors.
    ///
    /// This is used for frontmatter-only edits, including Formula write-backs.
    /// Keeping the operation metadata-only is what prevents a watcher echo from
    /// spending embedding tokens.
    pub fn refresh_source_metadata(&self, file: &MarkdownFile) -> Result<()> {
        let relative_path = crate::path_util::to_slash(&file.path);
        let mut state = self.state.write();
        let stored = state
            .metadata
            .files
            .get_mut(&relative_path)
            .ok_or_else(|| Error::FileNotInIndex {
                path: PathBuf::from(&relative_path),
            })?;
        stored.content_hash = file.content_hash.clone();
        stored.frontmatter = file
            .frontmatter
            .as_ref()
            .and_then(|value| serde_json::to_string(value).ok());
        stored.file_size = file.file_size;
        stored.reconcile_materialized_proofs();
        state
            .metadata
            .file_mtimes
            .get_or_insert_with(HashMap::new)
            .insert(relative_path, file.modified_at);
        state.dirty = true;
        Ok(())
    }

    /// Commit a module-owned source rewrite and its bookkeeping as one index
    /// mutation while preserving chunks, vectors, and embedding identity.
    pub fn apply_module_source_state(
        &self,
        expected_content_hash: &str,
        file: &MarkdownFile,
        computed_fields: HashMap<String, ComputedFieldEntry>,
    ) -> Result<()> {
        let relative_path = crate::path_util::to_slash(&file.path);
        let mut state = self.state.write();
        let stored = state
            .metadata
            .files
            .get_mut(&relative_path)
            .ok_or_else(|| Error::FileNotInIndex {
                path: PathBuf::from(&relative_path),
            })?;
        if stored.content_hash != expected_content_hash {
            return Err(Error::SourceChanged {
                path: PathBuf::from(&relative_path),
            });
        }

        stored.content_hash = file.content_hash.clone();
        stored.frontmatter = file
            .frontmatter
            .as_ref()
            .and_then(|value| serde_json::to_string(value).ok());
        stored.file_size = file.file_size;
        stored.computed_fields = computed_fields;
        stored.reconcile_materialized_proofs();
        state
            .metadata
            .file_mtimes
            .get_or_insert_with(HashMap::new)
            .insert(relative_path, file.modified_at);
        state.dirty = true;
        Ok(())
    }

    /// Clear entries owned by `module` within an optional path scope.
    ///
    /// A scope matches either an exact indexed path or all descendants of a
    /// folder prefix. Returns the number of removed field entries.
    pub fn clear_computed_fields_for_module(&self, module: &str, scope: Option<&str>) -> usize {
        let mut state = self.state.write();
        let mut removed = 0usize;

        for (path, file) in &mut state.metadata.files {
            if scope.is_some_and(|scope| !crate::path_util::path_is_in_scope(path, scope)) {
                continue;
            }
            let before = file.computed_fields.len();
            file.computed_fields
                .retain(|_, entry| entry.module != module);
            removed += before - file.computed_fields.len();
        }

        if removed > 0 {
            state.dirty = true;
        }
        removed
    }

    /// Get a cloned snapshot of all indexed files for read-only module execution.
    pub fn get_all_files(&self) -> HashMap<String, StoredFile> {
        let state = self.state.read();
        state.metadata.files.clone()
    }

    /// Get a map of all file paths to their content hashes.
    pub fn get_file_hashes(&self) -> HashMap<String, String> {
        let state = self.state.read();
        state
            .metadata
            .files
            .iter()
            .map(|(path, file)| (path.clone(), file.content_hash.clone()))
            .collect()
    }

    /// Return a status snapshot of the index.
    pub fn status(&self) -> IndexStatus {
        let state = self.state.read();
        let file_size = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let edge_count = state
            .id_to_key
            .keys()
            .filter(|id| id.starts_with("edge:"))
            .count();

        IndexStatus {
            document_count: state.metadata.files.len(),
            chunk_count: state.metadata.chunks.len(),
            vector_count: state.hnsw.size(),
            edge_count,
            last_updated: state.metadata.last_updated,
            file_size,
            embedding_config: state.metadata.embedding_config.clone(),
        }
    }

    /// Count files, chunks, and edge vectors within a path scope.
    ///
    /// `prefix` is a slash-terminated folder prefix (e.g. `"blog/"`); `None`
    /// counts the whole index. Edge vectors are attributed to their SOURCE
    /// file, matching `remove_file`'s `"edge:{path}->"` ownership semantics.
    pub fn scoped_counts(&self, prefix: Option<&str>) -> ScopedCounts {
        let state = self.state.read();

        let in_scope =
            |path: &str| prefix.is_none_or(|scope| crate::path_util::path_is_in_scope(path, scope));

        let mut counts = ScopedCounts::default();
        for (path, file) in &state.metadata.files {
            if in_scope(path) {
                counts.files += 1;
                counts.chunks += file.chunk_ids.len();
            }
        }
        for id in state.id_to_key.keys() {
            // Edge ids are "edge:{source}->{target}@..."; a source path
            // containing "->" would mis-split here, which we accept.
            if let Some(rest) = id.strip_prefix("edge:") {
                let source = rest.split("->").next().unwrap_or(rest);
                if in_scope(source) {
                    counts.edges += 1;
                }
            }
        }
        counts
    }

    /// Search for nearest vectors, returning `(chunk_id, cosine_similarity_score)` pairs.
    ///
    /// Converts usearch distance to cosine similarity: `score = 1.0 - distance`.
    /// Results are sorted by score descending (most similar first).
    /// Edge vectors (IDs starting with `"edge:"`) are excluded from the
    /// returned candidate window.
    pub fn search_vectors(&self, query: &[f32], limit: usize) -> Result<Vec<(String, f64)>> {
        let state = self.state.read();

        if state.hnsw.size() == 0 {
            return Ok(Vec::new());
        }

        let (ids, key_to_id_index) = chunk_id_projection(&state);
        // Keep normal candidate windows bounded. A caller requesting the full
        // chunk corpus gets the full shared index so edge vectors cannot hide
        // the final candidates during progressive scoped retrieval.
        let requested = limit.min(ids.len());
        let search_limit = if requested == ids.len() {
            state.hnsw.size()
        } else {
            requested.saturating_mul(2).min(state.hnsw.size())
        };
        collect_vector_search_indices(&state.hnsw, &key_to_id_index, query, limit, search_limit)
            .map(|matches| {
                matches
                    .into_iter()
                    .map(|(id_index, score)| (ids[id_index].to_string(), score))
                    .collect()
            })
    }

    /// Search many chunk vectors while holding one index snapshot.
    ///
    /// Chunk graph construction issues one nearest-neighbor query per node.
    /// Reusing the key-to-id projection avoids rebuilding an O(n) hash map for
    /// every query, and independent HNSW reads run concurrently. Results stay
    /// in query order and otherwise match [`search_vectors`](Self::search_vectors).
    pub(crate) fn search_vectors_batch(
        &self,
        queries: &[&[f32]],
        limit: usize,
    ) -> Result<VectorSearchBatch> {
        let state = self.state.read();

        if state.hnsw.size() == 0 {
            return Ok(VectorSearchBatch {
                ids: Vec::new(),
                matches: vec![Vec::new(); queries.len()],
            });
        }

        let (ids, key_to_id_index) = chunk_id_projection(&state);

        let matches = queries
            .par_iter()
            // Graph construction performs one query per chunk; retain bounded
            // over-fetching here rather than scanning every semantic edge for
            // every node.
            .map(|query| {
                collect_vector_search_indices(
                    &state.hnsw,
                    &key_to_id_index,
                    query,
                    limit,
                    limit.saturating_mul(2),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let ids = ids.into_iter().map(str::to_string).collect();

        Ok(VectorSearchBatch { ids, matches })
    }

    /// Get a stored chunk by its ID.
    pub fn get_chunk(&self, chunk_id: &str) -> Option<StoredChunk> {
        let state = self.state.read();
        state.metadata.chunks.get(chunk_id).cloned()
    }

    /// Get stored file metadata by relative path.
    pub fn get_file_metadata(&self, path: &str) -> Option<StoredFile> {
        let state = self.state.read();
        state.metadata.files.get(path).cloned()
    }

    /// Get the filesystem modification time for a file, if available.
    pub fn get_file_mtime(&self, path: &str) -> Option<u64> {
        let state = self.state.read();
        state.metadata.file_mtimes.as_ref()?.get(path).copied()
    }

    /// Get all file modification times as a cloned HashMap.
    pub fn get_file_mtimes(&self) -> HashMap<String, u64> {
        let state = self.state.read();
        state.metadata.file_mtimes.clone().unwrap_or_default()
    }

    /// Search the HNSW index for the nearest neighbors to the query vector.
    ///
    /// Returns a list of `(chunk_id, distance)` pairs sorted by distance.
    /// Edge vectors (IDs starting with `"edge:"`) are post-filtered out.
    /// Over-fetches by 2x to compensate for filtered edge entries.
    pub fn search(&self, query: &[f32], limit: usize) -> Result<Vec<(String, f32)>> {
        let state = self.state.read();

        if state.hnsw.size() == 0 {
            return Ok(Vec::new());
        }

        // Over-fetch by 2x to compensate for edge vectors that will be filtered out.
        let over_fetch = limit * 2;
        let results = state
            .hnsw
            .search(query, over_fetch)
            .map_err(|e| Error::Serialization(format!("usearch search: {e}")))?;

        // Build reverse lookup: key → chunk_id.
        let key_to_id: HashMap<u64, &String> =
            state.id_to_key.iter().map(|(id, key)| (*key, id)).collect();

        let mut output = Vec::with_capacity(results.keys.len());
        for (key, distance) in results.keys.iter().zip(results.distances.iter()) {
            if let Some(chunk_id) = key_to_id.get(key) {
                // Post-filter out edge vectors.
                if chunk_id.starts_with("edge:") {
                    continue;
                }
                output.push(((*chunk_id).clone(), *distance));
            }
        }

        output.truncate(limit);
        Ok(output)
    }

    /// Get the current schema, if any.
    pub fn get_schema(&self) -> Option<Schema> {
        let state = self.state.read();
        state.metadata.schema.clone()
    }

    /// Get the current cluster state, if any.
    pub fn get_clusters(&self) -> Option<ClusterState> {
        let state = self.state.read();
        state.metadata.cluster_state.clone()
    }

    /// Compute document-level vectors by averaging chunk vectors per file.
    ///
    /// Returns a map from relative file path to its averaged embedding vector.
    /// Used by the clustering pipeline which operates at the document level.
    pub fn get_document_vectors(&self) -> HashMap<String, Vec<f32>> {
        let state = self.state.read();
        let dims = state.metadata.embedding_config.dimensions;
        let mut result: HashMap<String, Vec<f32>> = HashMap::new();

        for (path, file) in &state.metadata.files {
            let mut sum = vec![0.0f32; dims];
            let mut count = 0usize;

            for chunk_id in &file.chunk_ids {
                if let Some(&key) = state.id_to_key.get(chunk_id) {
                    let mut buf = vec![0.0f32; dims];
                    if state.hnsw.get(key, &mut buf).is_ok() {
                        for (s, v) in sum.iter_mut().zip(buf.iter()) {
                            *s += v;
                        }
                        count += 1;
                    }
                }
            }

            if count > 0 {
                let scale = 1.0 / count as f32;
                for s in &mut sum {
                    *s *= scale;
                }
                result.insert(path.clone(), sum);
            }
        }

        result
    }

    /// Get chunk-level vectors with metadata for graph visualization.
    ///
    /// Returns a vector of `ChunkVectorInfo` for every chunk that has a valid
    /// embedding in the HNSW index.
    pub fn get_chunk_vectors(&self) -> Vec<ChunkVectorInfo> {
        let state = self.state.read();
        let dims = state.metadata.embedding_config.dimensions;
        let mut result = Vec::new();

        for (chunk_id, chunk) in &state.metadata.chunks {
            if let Some(&key) = state.id_to_key.get(chunk_id) {
                let mut buf = vec![0.0f32; dims];
                if state.hnsw.get(key, &mut buf).is_ok() {
                    result.push(ChunkVectorInfo {
                        chunk_id: chunk_id.clone(),
                        source_path: chunk.source_path.clone(),
                        heading_hierarchy: chunk.heading_hierarchy.clone(),
                        chunk_index: chunk.chunk_index,
                        content_len: chunk.content.len(),
                        vector: buf,
                    });
                }
            }
        }

        result
    }

    /// Get concatenated chunk content for each document (for keyword extraction).
    ///
    /// Returns a map from relative file path to the combined text of all its chunks.
    pub fn get_document_contents(&self) -> HashMap<String, String> {
        let state = self.state.read();
        let mut result: HashMap<String, String> = HashMap::new();

        for (path, file) in &state.metadata.files {
            let mut content = String::new();
            for chunk_id in &file.chunk_ids {
                if let Some(chunk) = state.metadata.chunks.get(chunk_id) {
                    if !content.is_empty() {
                        content.push(' ');
                    }
                    content.push_str(&chunk.content);
                }
            }
            if !content.is_empty() {
                result.insert(path.clone(), content);
            }
        }

        result
    }

    /// Update (or clear) the cluster state.
    pub fn update_clusters(&self, cluster_state: Option<ClusterState>) {
        let mut state = self.state.write();
        state.metadata.cluster_state = cluster_state;
        state.dirty = true;
    }

    /// Get the current custom cluster state, if any.
    pub fn get_custom_clusters(&self) -> Option<CustomClusterState> {
        let state = self.state.read();
        state.metadata.custom_cluster_state.clone()
    }

    /// Update (or clear) the custom cluster state.
    pub fn update_custom_clusters(&self, custom_cluster_state: Option<CustomClusterState>) {
        let mut state = self.state.write();
        state.metadata.custom_cluster_state = custom_cluster_state;
        state.dirty = true;
    }

    /// Get the current link graph, if any.
    pub fn get_link_graph(&self) -> Option<LinkGraph> {
        let state = self.state.read();
        state.metadata.link_graph.clone()
    }

    /// Read the current link graph without cloning it.
    ///
    /// The callback runs while the index read lock is held, so it must not call
    /// methods that acquire the index write lock. This is intended for
    /// read-only projections such as the graph visualization payload, where a
    /// semantic link graph can contain many megabytes of repeated context
    /// strings and cloning the whole value would be unnecessarily expensive.
    pub(crate) fn with_link_graph<T>(&self, callback: impl FnOnce(Option<&LinkGraph>) -> T) -> T {
        let state = self.state.read();
        callback(state.metadata.link_graph.as_ref())
    }

    /// Update (or clear) the link graph.
    pub fn update_link_graph(&self, link_graph: Option<LinkGraph>) {
        let mut state = self.state.write();
        state.metadata.link_graph = link_graph;
        state.dirty = true;
    }

    /// Retrieve all edge vectors from the HNSW index.
    ///
    /// Filters `id_to_key` for IDs with the `"edge:"` prefix and retrieves
    /// their vectors from the HNSW index.
    pub fn get_edge_vectors(&self) -> HashMap<String, Vec<f32>> {
        let state = self.state.read();
        let dims = state.metadata.embedding_config.dimensions;
        let mut result = HashMap::new();

        for (id, &key) in &state.id_to_key {
            if id.starts_with("edge:") {
                let mut buf = vec![0.0f32; dims];
                if state.hnsw.get(key, &mut buf).is_ok() {
                    result.insert(id.clone(), buf);
                }
            }
        }

        result
    }

    /// Search for nearest edge vectors, returning `(edge_id, cosine_similarity_score)` pairs.
    ///
    /// Chunk vectors sharing the HNSW index are excluded from the returned
    /// candidate window. Results are sorted by score descending.
    pub fn search_edges(&self, query: &[f32], limit: usize) -> Result<Vec<(String, f64)>> {
        let state = self.state.read();

        if state.hnsw.size() == 0 || limit == 0 {
            return Ok(Vec::new());
        }

        let edge_count = state
            .id_to_key
            .keys()
            .filter(|id| id.starts_with("edge:"))
            .count();
        if edge_count == 0 {
            return Ok(Vec::new());
        }
        let requested = limit.min(edge_count);
        let search_limit = if requested == edge_count {
            state.hnsw.size()
        } else {
            requested.saturating_mul(2).min(state.hnsw.size())
        };
        let results = state
            .hnsw
            .search(query, search_limit)
            .map_err(|e| Error::Serialization(format!("usearch search: {e}")))?;

        // Build reverse lookup: key → id.
        let key_to_id: HashMap<u64, &String> =
            state.id_to_key.iter().map(|(id, key)| (*key, id)).collect();

        let mut output = Vec::new();
        for (key, distance) in results.keys.iter().zip(results.distances.iter()) {
            if let Some(id) = key_to_id.get(key) {
                if id.starts_with("edge:") {
                    let score = 1.0 - *distance as f64;
                    output.push(((*id).clone(), score));
                }
            }
        }

        // Sort by score descending.
        output.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        output.truncate(limit);

        Ok(output)
    }

    /// Get all indexed file paths as a HashSet.
    pub fn get_indexed_file_paths(&self) -> std::collections::HashSet<String> {
        let state = self.state.read();
        state.metadata.files.keys().cloned().collect()
    }

    /// Set (or clear) the metadata schema.
    pub fn set_schema(&self, schema: Option<Schema>) {
        let mut state = self.state.write();
        state.metadata.schema = schema;
        state.dirty = true;
    }

    /// Get all scoped schemas, if any.
    pub fn get_scoped_schemas(&self) -> Option<Vec<crate::schema::ScopedSchema>> {
        let state = self.state.read();
        state.metadata.scoped_schemas.clone()
    }

    /// Get the scoped schema for a specific path prefix, if any.
    pub fn get_scoped_schema(&self, prefix: &str) -> Option<crate::schema::ScopedSchema> {
        let state = self.state.read();
        state
            .metadata
            .scoped_schemas
            .as_ref()
            .and_then(|schemas| schemas.iter().find(|s| s.scope == prefix).cloned())
    }

    /// Set (or clear) the scoped schemas.
    pub fn set_scoped_schemas(&self, scoped_schemas: Option<Vec<crate::schema::ScopedSchema>>) {
        let mut state = self.state.write();
        state.metadata.scoped_schemas = scoped_schemas;
        state.dirty = true;
    }

    /// Persist the index to disk atomically.
    ///
    /// Compacts HNSW keys to sequential 0..N matching sorted chunk ID order,
    /// ensuring that after any number of save/load cycles, keys always match.
    ///
    /// Takes an exclusive advisory cross-process lock on a sibling
    /// `<index>.lock` file for the duration of the write critical section;
    /// a concurrent writer (e.g. `mdvdb watch`) yields [`Error::IndexBusy`].
    pub fn save(&self) -> Result<()> {
        let mut state = self.state.write();

        state.metadata.last_updated = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Compact HNSW keys: create a new index with sequential keys 0..N
        // matching alphabetically sorted chunk IDs, then edge IDs.
        let dims = state.metadata.embedding_config.dimensions;

        // Clean up orphaned edge entries from id_to_key.
        // Edges can become orphaned when links are removed from a file:
        // upsert_edges() only removes edges in the NEW list, but
        // update_link_graph() replaces semantic_edges entirely, leaving
        // stale edge entries in id_to_key that aren't tracked in metadata.
        // On reload, open_with_options() reconstructs id_to_key from
        // metadata.chunks + semantic_edges, so orphaned edges cause
        // next_key to be too low → duplicate key errors.
        {
            let tracked_edges: std::collections::HashSet<String> = state
                .metadata
                .link_graph
                .as_ref()
                .and_then(|lg| lg.semantic_edges.as_ref())
                .map(|se| se.keys().cloned().collect())
                .unwrap_or_default();

            let orphaned: Vec<String> = state
                .id_to_key
                .keys()
                .filter(|id| {
                    !state.metadata.chunks.contains_key(*id) && !tracked_edges.contains(*id)
                })
                .cloned()
                .collect();

            for id in &orphaned {
                if let Some(key) = state.id_to_key.remove(id) {
                    let _ = state.hnsw.remove(key);
                }
            }

            if !orphaned.is_empty() {
                debug!(count = orphaned.len(), "removed orphaned edge vectors");
            }
        }

        let mut sorted_chunk_ids: Vec<&String> = state.metadata.chunks.keys().collect();
        sorted_chunk_ids.sort();

        // Collect edge IDs (those in id_to_key but not in metadata.chunks).
        let mut sorted_edge_ids: Vec<String> = state
            .id_to_key
            .keys()
            .filter(|id| !state.metadata.chunks.contains_key(*id))
            .cloned()
            .collect();
        sorted_edge_ids.sort();

        let total = sorted_chunk_ids.len() + sorted_edge_ids.len();
        let scalar_kind = storage::scalar_kind_for(&self.write_options.quantization);
        let new_hnsw = storage::create_hnsw(dims, scalar_kind)?;
        if total > 0 {
            new_hnsw
                .reserve(total.max(10))
                .map_err(|e| Error::Serialization(format!("usearch reserve: {e}")))?;
        }

        let mut new_id_to_key = HashMap::new();
        let mut buf = vec![0.0f32; dims];
        let mut next = 0u64;

        for chunk_id in &sorted_chunk_ids {
            if let Some(&old_key) = state.id_to_key.get(*chunk_id) {
                if state.hnsw.get(old_key, &mut buf).is_ok() {
                    new_hnsw
                        .add(next, &buf)
                        .map_err(|e| Error::Serialization(format!("usearch add: {e}")))?;
                }
            }
            new_id_to_key.insert((*chunk_id).clone(), next);
            next += 1;
        }

        for edge_id in &sorted_edge_ids {
            if let Some(&old_key) = state.id_to_key.get(edge_id) {
                if state.hnsw.get(old_key, &mut buf).is_ok() {
                    new_hnsw
                        .add(next, &buf)
                        .map_err(|e| Error::Serialization(format!("usearch add: {e}")))?;
                }
            }
            new_id_to_key.insert(edge_id.clone(), next);
            next += 1;
        }

        state.hnsw = new_hnsw;
        state.id_to_key = new_id_to_key;
        state.next_key = next;

        // Advisory cross-process lock held ONLY for the write critical section
        // (dropped at the end of this scope; File drop releases the OS lock).
        {
            let _write_lock = acquire_write_lock(&self.path)?;
            storage::write_index(
                &self.path,
                &state.metadata,
                &state.hnsw,
                &self.write_options,
            )?;
        }
        state.dirty = false;

        debug!(path = %self.path.display(), "index saved");
        Ok(())
    }

    /// Check that the index's embedding configuration is compatible with the given config.
    ///
    /// Returns `Error::IndexCorrupted` if dimensions or model don't match.
    pub fn check_config_compatibility(&self, config: &EmbeddingConfig) -> Result<()> {
        let state = self.state.read();
        let existing = &state.metadata.embedding_config;

        if existing.dimensions != config.dimensions {
            return Err(Error::IndexCorrupted(format!(
                "dimension mismatch: index has {}, config has {}",
                existing.dimensions, config.dimensions
            )));
        }

        if existing.model != config.model {
            return Err(Error::IndexCorrupted(format!(
                "model mismatch: index has '{}', config has '{}'",
                existing.model, config.model
            )));
        }

        Ok(())
    }
}

/// Acquire an exclusive advisory lock on the sibling `<index>.lock` file
/// (the index file itself has no extension, so e.g. `.markdownvdb/index`
/// locks via `.markdownvdb/index.lock`).
///
/// Retries up to 10 times at 200ms intervals, then fails with
/// [`Error::IndexBusy`]. The lock is released when the returned `File` is
/// dropped (the OS releases advisory locks on close), so callers hold it
/// only for the write critical section by scoping the returned handle.
fn acquire_write_lock(index_path: &Path) -> Result<std::fs::File> {
    const ATTEMPTS: usize = 10;
    const RETRY_DELAY_MS: u64 = 200;

    let lock_path = index_path.with_extension("lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;

    for attempt in 0..ATTEMPTS {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                debug!(
                    lock = %lock_path.display(),
                    attempt = attempt + 1,
                    "index write lock busy, retrying"
                );
                // No point sleeping after the final failed attempt.
                if attempt + 1 < ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS));
                }
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(Error::Io(e)),
        }
    }

    Err(Error::IndexBusy {
        path: index_path.to_path_buf(),
    })
}

/// Test-only helpers for manipulating index state directly.
#[cfg(test)]
impl Index {
    /// Insert a file entry with just a path and hash (no chunks/vectors).
    pub fn insert_file_hash_for_test(&self, path: &str, hash: &str) {
        let mut state = self.state.write();
        state.metadata.files.insert(
            path.to_string(),
            StoredFile {
                relative_path: path.to_string(),
                content_hash: hash.to_string(),
                embedding_body_hash: hash.to_string(),
                chunk_ids: Vec::new(),
                frontmatter: None,
                file_size: 0,
                indexed_at: 0,
                computed_fields: HashMap::new(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config() -> EmbeddingConfig {
        EmbeddingConfig {
            provider: "OpenAI".to_string(),
            model: "test-model".to_string(),
            dimensions: 128,
        }
    }

    #[test]
    fn create_new_index() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();

        let index = Index::create(&path, &config).unwrap();
        assert!(path.exists());

        let state = index.state.read();
        assert!(state.metadata.chunks.is_empty());
        assert_eq!(state.next_key, 0);
        assert!(!state.dirty);
    }

    #[test]
    fn open_existing_index() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();

        Index::create(&path, &config).unwrap();
        let index = Index::open(&path).unwrap();

        let state = index.state.read();
        assert_eq!(state.metadata.embedding_config.dimensions, 128);
    }

    #[test]
    fn open_missing_returns_error() {
        let result = Index::open(Path::new("/nonexistent/index.bin"));
        assert!(matches!(result, Err(Error::IndexNotFound { .. })));
    }

    #[test]
    fn open_or_create_creates_when_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();

        let index = Index::open_or_create(&path, &config).unwrap();
        assert!(path.exists());

        let state = index.state.read();
        assert!(state.metadata.chunks.is_empty());
    }

    #[test]
    fn upsert_edges_adds_vectors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        let edges = vec![
            ("edge:a.md->b.md@0".to_string(), vec![1.0f32; 128]),
            ("edge:a.md->c.md@5".to_string(), vec![0.5f32; 128]),
        ];

        index.upsert_edges(&edges).unwrap();

        let state = index.state.read();
        // Edge vectors should be in HNSW and id_to_key but NOT in metadata.chunks.
        assert_eq!(state.hnsw.size(), 2);
        assert!(state.id_to_key.contains_key("edge:a.md->b.md@0"));
        assert!(state.id_to_key.contains_key("edge:a.md->c.md@5"));
        assert!(state.metadata.chunks.is_empty());
        assert!(state.dirty);
    }

    #[test]
    fn upsert_edges_replaces_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        let edges1 = vec![("edge:a.md->b.md@0".to_string(), vec![1.0f32; 128])];
        index.upsert_edges(&edges1).unwrap();

        // Upsert same ID with different vector.
        let edges2 = vec![("edge:a.md->b.md@0".to_string(), vec![0.5f32; 128])];
        index.upsert_edges(&edges2).unwrap();

        let state = index.state.read();
        // Should still have only 1 vector (old removed, new added).
        assert_eq!(state.hnsw.size(), 1);
        assert!(state.id_to_key.contains_key("edge:a.md->b.md@0"));
    }

    fn mk_file(path: &str) -> MarkdownFile {
        MarkdownFile {
            path: PathBuf::from(path),
            body: "hello".to_string(),
            frontmatter: None,
            headings: vec![],
            content_hash: format!("hash-{path}"),
            modified_at: 0,
            frontmatter_links: Vec::new(),
            file_size: 5,
            links: vec![],
        }
    }

    fn mk_chunk(path: &str, idx: usize) -> Chunk {
        Chunk {
            id: format!("{path}#{idx}"),
            content: "hello".to_string(),
            source_path: PathBuf::from(path),
            heading_hierarchy: vec![],
            chunk_index: idx,
            start_line: 1,
            end_line: 1,
            is_sub_split: false,
        }
    }

    fn computed_entry(module: &str, value_json: &str) -> ComputedFieldEntry {
        ComputedFieldEntry {
            module: module.to_string(),
            definition_fingerprint: format!("{module}-fingerprint"),
            input_fingerprint: None,
            dependency_snapshot: Default::default(),
            value_json: Some(value_json.to_string()),
            materialized_value_json: None,
            diagnostic: None,
        }
    }

    #[test]
    fn replace_and_get_computed_fields() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let index = Index::create(&path, &test_config()).unwrap();
        index.upsert(&mk_file("invoice.md"), &[], &[]).unwrap();

        let mut entry = computed_entry("formula", "12.50");
        entry.input_fingerprint = Some("formula-inputs".to_string());
        entry.dependency_snapshot =
            crate::index::types::ComputedDependencySnapshot::owner("invoice.md", "hash-invoice.md");
        let fields = HashMap::from([("total".to_string(), entry)]);
        index
            .replace_computed_fields("invoice.md", fields.clone())
            .unwrap();

        assert_eq!(index.get_computed_fields("invoice.md"), Some(fields));
        assert_eq!(index.get_all_files()["invoice.md"].computed_fields.len(), 1);
        assert!(matches!(
            index.replace_computed_fields("missing.md", HashMap::new()),
            Err(Error::FileNotInIndex { .. })
        ));
    }

    #[test]
    fn clear_computed_fields_for_module_honors_scope() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let index = Index::create(&path, &test_config()).unwrap();
        for file in ["invoices/a.md", "invoices/archive/b.md", "notes/c.md"] {
            index.upsert(&mk_file(file), &[], &[]).unwrap();
            index
                .replace_computed_fields(
                    file,
                    HashMap::from([
                        ("total".to_string(), computed_entry("formula", "10")),
                        ("other".to_string(), computed_entry("other", "\"kept\"")),
                    ]),
                )
                .unwrap();
        }

        assert_eq!(
            index.clear_computed_fields_for_module("formula", Some("invoices/")),
            2
        );
        assert_eq!(index.get_computed_fields("invoices/a.md").unwrap().len(), 1);
        assert_eq!(
            index.get_computed_fields("invoices/a.md").unwrap()["other"]
                .module
                .as_str(),
            "other"
        );
        assert_eq!(index.get_computed_fields("notes/c.md").unwrap().len(), 2);

        assert_eq!(index.clear_computed_fields_for_module("formula", None), 1);
        assert_eq!(index.clear_computed_fields_for_module("formula", None), 0);
    }

    #[test]
    fn raw_file_upsert_preserves_module_ownership_until_hooks_run() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let index = Index::create(&path, &test_config()).unwrap();
        let file = mk_file("invoice.md");
        index.upsert(&file, &[], &[]).unwrap();
        index
            .replace_computed_fields(
                "invoice.md",
                HashMap::from([("total".to_string(), computed_entry("formula", "10"))]),
            )
            .unwrap();

        index.upsert(&file, &[], &[]).unwrap();
        assert_eq!(
            index.get_computed_fields("invoice.md").unwrap()["total"]
                .value_json
                .as_deref(),
            Some("10")
        );
    }

    #[test]
    fn computed_fields_persist_across_save_and_open() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let index = Index::create(&path, &test_config()).unwrap();
        index.upsert(&mk_file("invoice.md"), &[], &[]).unwrap();
        let fields = HashMap::from([("total".to_string(), computed_entry("formula", "12.50"))]);
        index
            .replace_computed_fields("invoice.md", fields.clone())
            .unwrap();
        index.save().unwrap();

        let reopened = Index::open(&path).unwrap();
        assert_eq!(reopened.get_computed_fields("invoice.md"), Some(fields));
    }

    #[test]
    fn raw_replacement_permanently_revokes_materialized_ownership() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let index = Index::create(&path, &test_config()).unwrap();
        let mut materialized = mk_file("contact.md");
        materialized.content_hash = "computed-write".to_string();
        materialized.frontmatter = Some(serde_json::json!({
            "client_domain": "computed.example",
            "ordinary": "keep"
        }));
        index.upsert(&materialized, &[], &[]).unwrap();
        let mut proof = computed_entry("lookup_rollup", "\"computed.example\"");
        proof.materialized_value_json = Some("\"computed.example\"".to_string());
        index
            .replace_computed_fields(
                "contact.md",
                HashMap::from([("client_domain".to_string(), proof)]),
            )
            .unwrap();
        let owned = index.get_file("contact.md").unwrap();
        assert!(owned
            .materialized_field_matches("client_domain", &owned.computed_fields["client_domain"]));

        // A raw refresh represents an external/user-authored edit. Once the
        // semantic value differs, the old proof must be destroyed, not merely
        // made temporarily inactive.
        let mut user_replacement = materialized.clone();
        user_replacement.content_hash = "user-replacement".to_string();
        user_replacement.frontmatter = Some(serde_json::json!({
            "client_domain": "user.example",
            "ordinary": "keep"
        }));
        index.refresh_source_metadata(&user_replacement).unwrap();
        index.save().unwrap();
        drop(index);

        let reopened = Index::open(&path).unwrap();
        let replaced = reopened.get_file("contact.md").unwrap();
        assert!(replaced.computed_fields["client_domain"]
            .materialized_value_json
            .is_none());
        assert_eq!(
            replaced.effective_frontmatter().unwrap()["client_domain"],
            "user.example"
        );

        // Even if the user later writes bytes with the old computed semantic
        // value, equality cannot resurrect deletion/suppression authority.
        let mut coincidentally_equal = user_replacement;
        coincidentally_equal.content_hash = "user-equal-to-old-computed".to_string();
        coincidentally_equal.frontmatter = Some(serde_json::json!({
            "client_domain": "computed.example",
            "ordinary": "keep"
        }));
        reopened
            .refresh_source_metadata(&coincidentally_equal)
            .unwrap();
        reopened.save().unwrap();
        drop(reopened);

        let reopened_again = Index::open(&path).unwrap();
        let final_file = reopened_again.get_file("contact.md").unwrap();
        let final_entry = &final_file.computed_fields["client_domain"];
        assert!(final_entry.materialized_value_json.is_none());
        assert!(!final_file.materialized_field_matches("client_domain", final_entry));
        assert!(final_file.computed_values_json().is_empty());
        let effective = final_file.effective_frontmatter().unwrap();
        assert_eq!(effective["client_domain"], "computed.example");
        assert_eq!(effective["ordinary"], "keep");
    }

    #[test]
    fn metadata_and_module_source_sync_preserve_vectors_and_embedding_body_hash() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let index = Index::create(&path, &test_config()).unwrap();
        let original = mk_file("invoice.md");
        let original_body_hash = crate::parser::compute_content_hash(&original.body);
        index
            .upsert(&original, &[mk_chunk("invoice.md", 0)], &[vec![0.25; 128]])
            .unwrap();
        index
            .replace_computed_fields(
                "invoice.md",
                HashMap::from([("other".to_string(), computed_entry("other", "\"kept\""))]),
            )
            .unwrap();

        let mut metadata_edit = original.clone();
        metadata_edit.content_hash = "frontmatter-edit".to_string();
        metadata_edit.frontmatter = Some(serde_json::json!({"price": 3}));
        metadata_edit.file_size = 123;
        metadata_edit.modified_at = 456;
        index.refresh_source_metadata(&metadata_edit).unwrap();

        let refreshed = index.get_file("invoice.md").unwrap();
        assert_eq!(refreshed.content_hash, "frontmatter-edit");
        assert_eq!(refreshed.embedding_body_hash, original_body_hash);
        assert_eq!(refreshed.chunk_ids, vec!["invoice.md#0"]);
        assert_eq!(index.status().vector_count, 1);
        assert_eq!(
            refreshed.computed_fields["other"].value_json.as_deref(),
            Some("\"kept\"")
        );

        let mut formula_write = metadata_edit;
        formula_write.content_hash = "formula-write".to_string();
        formula_write.frontmatter = Some(serde_json::json!({"price": 3, "total": 6}));
        let fields = HashMap::from([
            ("other".to_string(), computed_entry("other", "\"kept\"")),
            ("total".to_string(), computed_entry("formula", "6")),
        ]);
        index
            .apply_module_source_state("frontmatter-edit", &formula_write, fields)
            .unwrap();

        let final_file = index.get_file("invoice.md").unwrap();
        assert_eq!(final_file.content_hash, "formula-write");
        assert_eq!(final_file.embedding_body_hash, original_body_hash);
        assert_eq!(final_file.chunk_ids, vec!["invoice.md#0"]);
        assert_eq!(index.status().vector_count, 1);
        assert_eq!(
            final_file.computed_fields["other"].value_json.as_deref(),
            Some("\"kept\"")
        );
    }

    #[test]
    fn status_edge_count_counts_only_edges() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let index = Index::create(&path, &test_config()).unwrap();

        index
            .upsert(
                &mk_file("a.md"),
                &[mk_chunk("a.md", 0), mk_chunk("a.md", 1)],
                &[vec![0.1f32; 128], vec![0.2f32; 128]],
            )
            .unwrap();
        index
            .upsert_edges(&[
                ("edge:a.md->b.md@0".to_string(), vec![0.9f32; 128]),
                ("edge:a.md->c.md@fm.client".to_string(), vec![0.8f32; 128]),
            ])
            .unwrap();

        let status = index.status();
        assert_eq!(status.chunk_count, 2);
        assert_eq!(status.edge_count, 2);
        assert_eq!(status.vector_count, 4);
        assert_eq!(status.vector_count, status.chunk_count + status.edge_count);
    }

    #[test]
    fn scoped_counts_filters_by_prefix() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let index = Index::create(&path, &test_config()).unwrap();

        index
            .upsert(
                &mk_file("blog/a.md"),
                &[mk_chunk("blog/a.md", 0), mk_chunk("blog/a.md", 1)],
                &[vec![0.1f32; 128], vec![0.2f32; 128]],
            )
            .unwrap();
        index
            .upsert(
                &mk_file("notes/b.md"),
                &[mk_chunk("notes/b.md", 0)],
                &[vec![0.3f32; 128]],
            )
            .unwrap();
        // Edges attributed to their source file.
        index
            .upsert_edges(&[
                (
                    "edge:blog/a.md->notes/b.md@0".to_string(),
                    vec![0.9f32; 128],
                ),
                (
                    "edge:notes/b.md->blog/a.md@0".to_string(),
                    vec![0.8f32; 128],
                ),
                (
                    "edge:notes/b.md->blog/a.md@fm.rel".to_string(),
                    vec![0.7f32; 128],
                ),
            ])
            .unwrap();

        let blog = index.scoped_counts(Some("blog/"));
        assert_eq!(
            blog,
            ScopedCounts {
                files: 1,
                chunks: 2,
                edges: 1
            }
        );

        let notes = index.scoped_counts(Some("notes/"));
        assert_eq!(
            notes,
            ScopedCounts {
                files: 1,
                chunks: 1,
                edges: 2
            }
        );

        let all = index.scoped_counts(None);
        assert_eq!(
            all,
            ScopedCounts {
                files: 2,
                chunks: 3,
                edges: 3
            }
        );

        let none = index.scoped_counts(Some("missing/"));
        assert_eq!(none, ScopedCounts::default());
    }

    #[test]
    fn upsert_edges_coexists_with_chunks() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        // Add a regular file with chunks.
        let file = MarkdownFile {
            path: PathBuf::from("test.md"),
            body: "hello".to_string(),
            frontmatter: None,
            headings: vec![],
            content_hash: "abc123".to_string(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
            file_size: 5,
            links: vec![],
        };
        let chunk = Chunk {
            id: "test.md#0".to_string(),
            content: "hello".to_string(),
            source_path: PathBuf::from("test.md"),
            heading_hierarchy: vec![],
            chunk_index: 0,
            start_line: 1,
            end_line: 1,
            is_sub_split: false,
        };
        index.upsert(&file, &[chunk], &[vec![0.1f32; 128]]).unwrap();

        // Now add edge vectors.
        let edges = vec![("edge:test.md->other.md@0".to_string(), vec![0.9f32; 128])];
        index.upsert_edges(&edges).unwrap();

        let state = index.state.read();
        assert_eq!(state.hnsw.size(), 2);
        assert_eq!(state.metadata.chunks.len(), 1);
        assert!(state.id_to_key.contains_key("test.md#0"));
        assert!(state.id_to_key.contains_key("edge:test.md->other.md@0"));
    }

    #[test]
    fn get_edge_vectors_returns_only_edges() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        // Add a regular chunk.
        let file = MarkdownFile {
            path: PathBuf::from("test.md"),
            body: "hello".to_string(),
            frontmatter: None,
            headings: vec![],
            content_hash: "abc123".to_string(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
            file_size: 5,
            links: vec![],
        };
        let chunk = Chunk {
            id: "test.md#0".to_string(),
            content: "hello".to_string(),
            source_path: PathBuf::from("test.md"),
            heading_hierarchy: vec![],
            chunk_index: 0,
            start_line: 1,
            end_line: 1,
            is_sub_split: false,
        };
        index.upsert(&file, &[chunk], &[vec![0.1f32; 128]]).unwrap();

        // Add edge vectors.
        let edges = vec![
            ("edge:a.md->b.md@0".to_string(), vec![1.0f32; 128]),
            ("edge:a.md->c.md@5".to_string(), vec![0.5f32; 128]),
        ];
        index.upsert_edges(&edges).unwrap();

        let edge_vectors = index.get_edge_vectors();
        assert_eq!(edge_vectors.len(), 2);
        assert!(edge_vectors.contains_key("edge:a.md->b.md@0"));
        assert!(edge_vectors.contains_key("edge:a.md->c.md@5"));
        // Should NOT contain the regular chunk.
        assert!(!edge_vectors.contains_key("test.md#0"));
    }

    #[test]
    fn get_edge_vectors_empty_when_no_edges() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        let edge_vectors = index.get_edge_vectors();
        assert!(edge_vectors.is_empty());
    }

    #[test]
    fn search_edges_filters_to_edge_ids() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        // Add a regular chunk.
        let file = MarkdownFile {
            path: PathBuf::from("test.md"),
            body: "hello".to_string(),
            frontmatter: None,
            headings: vec![],
            content_hash: "abc123".to_string(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
            file_size: 5,
            links: vec![],
        };
        let chunk = Chunk {
            id: "test.md#0".to_string(),
            content: "hello".to_string(),
            source_path: PathBuf::from("test.md"),
            heading_hierarchy: vec![],
            chunk_index: 0,
            start_line: 1,
            end_line: 1,
            is_sub_split: false,
        };
        index.upsert(&file, &[chunk], &[vec![0.1f32; 128]]).unwrap();

        // Add edge vectors.
        let edges = vec![
            ("edge:a.md->b.md@0".to_string(), vec![1.0f32; 128]),
            ("edge:a.md->c.md@5".to_string(), vec![0.8f32; 128]),
        ];
        index.upsert_edges(&edges).unwrap();

        // Search for edges similar to [1.0; 128].
        let query = vec![1.0f32; 128];
        let results = index.search_edges(&query, 10).unwrap();

        // All results should be edge IDs only.
        for (id, _score) in &results {
            assert!(id.starts_with("edge:"), "Expected edge ID, got: {id}");
        }
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_edges_respects_limit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        let edges = vec![
            ("edge:a.md->b.md@0".to_string(), vec![1.0f32; 128]),
            ("edge:a.md->c.md@5".to_string(), vec![0.8f32; 128]),
            ("edge:a.md->d.md@2".to_string(), vec![0.6f32; 128]),
        ];
        index.upsert_edges(&edges).unwrap();

        let query = vec![1.0f32; 128];
        let results = index.search_edges(&query, 1).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_edges_empty_index() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        let query = vec![1.0f32; 128];
        let results = index.search_edges(&query, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_vectors_filters_out_edge_ids() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        // Add a regular chunk.
        let file = MarkdownFile {
            path: PathBuf::from("test.md"),
            body: "hello".to_string(),
            frontmatter: None,
            headings: vec![],
            content_hash: "abc123".to_string(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
            file_size: 5,
            links: vec![],
        };
        let chunk = Chunk {
            id: "test.md#0".to_string(),
            content: "hello".to_string(),
            source_path: PathBuf::from("test.md"),
            heading_hierarchy: vec![],
            chunk_index: 0,
            start_line: 1,
            end_line: 1,
            is_sub_split: false,
        };
        index.upsert(&file, &[chunk], &[vec![1.0f32; 128]]).unwrap();

        // Add edge vectors.
        let edges = vec![("edge:test.md->other.md@0".to_string(), vec![1.0f32; 128])];
        index.upsert_edges(&edges).unwrap();

        // search_vectors should NOT return edge IDs.
        let query = vec![1.0f32; 128];
        let results = index.search_vectors(&query, 10).unwrap();
        for (id, _) in &results {
            assert!(
                !id.starts_with("edge:"),
                "search_vectors returned edge ID: {id}"
            );
        }
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "test.md#0");
    }

    #[test]
    fn batch_vector_search_matches_individual_queries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let index = Index::create(&path, &test_config()).unwrap();

        let mut first = vec![0.0f32; 128];
        first[0] = 1.0;
        let mut second = vec![0.0f32; 128];
        second[1] = 1.0;
        let mut third = vec![0.0f32; 128];
        third[0] = 0.8;
        third[1] = 0.2;

        index
            .upsert(&mk_file("a.md"), &[mk_chunk("a.md", 0)], &[first.clone()])
            .unwrap();
        index
            .upsert(&mk_file("b.md"), &[mk_chunk("b.md", 0)], &[second.clone()])
            .unwrap();
        index
            .upsert(&mk_file("c.md"), &[mk_chunk("c.md", 0)], &[third.clone()])
            .unwrap();
        index
            .upsert_edges(&[("edge:a.md->b.md@0".to_string(), first.clone())])
            .unwrap();

        let expected = vec![
            index.search_vectors(&first, 3).unwrap(),
            index.search_vectors(&second, 3).unwrap(),
            index.search_vectors(&third, 3).unwrap(),
        ];
        let queries = vec![first.as_slice(), second.as_slice(), third.as_slice()];
        let batch = index.search_vectors_batch(&queries, 3).unwrap();
        let actual: Vec<Vec<(String, f64)>> = batch
            .matches
            .into_iter()
            .map(|matches| {
                matches
                    .into_iter()
                    .map(|(id_index, score)| (batch.ids[id_index].clone(), score))
                    .collect()
            })
            .collect();

        assert_eq!(actual, expected);
        assert!(actual
            .iter()
            .flatten()
            .all(|(id, _)| !id.starts_with("edge:")));
    }

    #[test]
    fn chunk_vector_snapshot_includes_content_length() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let index = Index::create(&path, &test_config()).unwrap();
        let file = mk_file("sized.md");
        let mut chunk = mk_chunk("sized.md", 0);
        chunk.content = "a chunk with known bytes".to_string();

        index
            .upsert(&file, std::slice::from_ref(&chunk), &[vec![1.0f32; 128]])
            .unwrap();

        let vectors = index.get_chunk_vectors();
        assert_eq!(vectors.len(), 1);
        assert_eq!(vectors[0].content_len, chunk.content.len());
    }

    #[test]
    fn search_filters_out_edge_ids() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        let file = MarkdownFile {
            path: PathBuf::from("test.md"),
            body: "hello".to_string(),
            frontmatter: None,
            headings: vec![],
            content_hash: "abc123".to_string(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
            file_size: 5,
            links: vec![],
        };
        let chunk = Chunk {
            id: "test.md#0".to_string(),
            content: "hello".to_string(),
            source_path: PathBuf::from("test.md"),
            heading_hierarchy: vec![],
            chunk_index: 0,
            start_line: 1,
            end_line: 1,
            is_sub_split: false,
        };
        index.upsert(&file, &[chunk], &[vec![1.0f32; 128]]).unwrap();

        let edges = vec![("edge:test.md->other.md@0".to_string(), vec![1.0f32; 128])];
        index.upsert_edges(&edges).unwrap();

        let query = vec![1.0f32; 128];
        let results = index.search(&query, 10).unwrap();
        for (id, _) in &results {
            assert!(!id.starts_with("edge:"), "search returned edge ID: {id}");
        }
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn remove_file_cleans_up_edge_vectors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        // Add a file with chunks.
        let file = MarkdownFile {
            path: PathBuf::from("source.md"),
            body: "hello".to_string(),
            frontmatter: None,
            headings: vec![],
            content_hash: "abc123".to_string(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
            file_size: 5,
            links: vec![],
        };
        let chunk = Chunk {
            id: "source.md#0".to_string(),
            content: "hello".to_string(),
            source_path: PathBuf::from("source.md"),
            heading_hierarchy: vec![],
            chunk_index: 0,
            start_line: 1,
            end_line: 1,
            is_sub_split: false,
        };
        index.upsert(&file, &[chunk], &[vec![0.1f32; 128]]).unwrap();

        // Add edge vectors from this file.
        let edges = vec![
            ("edge:source.md->target.md@0".to_string(), vec![0.5f32; 128]),
            ("edge:source.md->other.md@3".to_string(), vec![0.6f32; 128]),
            // Edge from a different file — should NOT be removed.
            ("edge:other.md->source.md@0".to_string(), vec![0.7f32; 128]),
        ];
        index.upsert_edges(&edges).unwrap();

        // Remove the file.
        index.remove_file("source.md").unwrap();

        let state = index.state.read();
        // Chunks and file-sourced edges should be gone.
        assert!(!state.id_to_key.contains_key("source.md#0"));
        assert!(!state.id_to_key.contains_key("edge:source.md->target.md@0"));
        assert!(!state.id_to_key.contains_key("edge:source.md->other.md@3"));
        // Edge from other file should remain.
        assert!(state.id_to_key.contains_key("edge:other.md->source.md@0"));
        assert_eq!(state.hnsw.size(), 1);
    }

    #[test]
    fn save_load_round_trips_edge_vectors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        // Add a chunk.
        let file = MarkdownFile {
            path: PathBuf::from("test.md"),
            body: "hello".to_string(),
            frontmatter: None,
            headings: vec![],
            content_hash: "abc123".to_string(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
            file_size: 5,
            links: vec![],
        };
        let chunk = Chunk {
            id: "test.md#0".to_string(),
            content: "hello".to_string(),
            source_path: PathBuf::from("test.md"),
            heading_hierarchy: vec![],
            chunk_index: 0,
            start_line: 1,
            end_line: 1,
            is_sub_split: false,
        };
        index.upsert(&file, &[chunk], &[vec![0.1f32; 128]]).unwrap();

        // Add edge vectors.
        let edges = vec![("edge:test.md->other.md@0".to_string(), vec![1.0f32; 128])];
        index.upsert_edges(&edges).unwrap();

        // Store edge info in link_graph so open_with_options can reconstruct id_to_key.
        {
            use crate::links::{LinkGraph, SemanticEdge};
            let mut semantic_edges = HashMap::new();
            semantic_edges.insert(
                "edge:test.md->other.md@0".to_string(),
                SemanticEdge {
                    edge_id: "edge:test.md->other.md@0".to_string(),
                    source: "test.md".to_string(),
                    target: "other.md".to_string(),
                    context_text: "link context".to_string(),
                    line_number: 1,
                    strength: None,
                    relationship_type: None,
                    cluster_id: None,
                },
            );
            let lg = LinkGraph {
                forward: HashMap::new(),
                last_updated: 0,
                semantic_edges: Some(semantic_edges),
                edge_cluster_state: None,
            };
            index.update_link_graph(Some(lg));
        }

        // Save and reload.
        index.save().unwrap();
        let index2 = Index::open(&path).unwrap();

        // Edge should be in id_to_key after reload.
        let state2 = index2.state.read();
        assert!(state2.id_to_key.contains_key("edge:test.md->other.md@0"));
        assert!(state2.id_to_key.contains_key("test.md#0"));

        // Verify edge vector can be retrieved.
        drop(state2);
        let edge_vecs = index2.get_edge_vectors();
        assert_eq!(edge_vecs.len(), 1);
        assert!(edge_vecs.contains_key("edge:test.md->other.md@0"));
    }

    #[test]
    fn open_or_create_opens_when_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();

        Index::create(&path, &config).unwrap();
        let index = Index::open_or_create(&path, &config).unwrap();

        let state = index.state.read();
        assert_eq!(state.metadata.embedding_config.model, "test-model");
    }

    #[test]
    fn save_creates_sibling_lock_file() {
        let dir = TempDir::new().unwrap();
        // Match production: index file has no extension → sibling is "index.lock".
        let path = dir.path().join("index");
        let index = Index::create(&path, &test_config()).unwrap();

        index.save().unwrap();
        assert!(dir.path().join("index.lock").exists());
        // The lock is released after save; a subsequent save re-acquires it.
        index.save().unwrap();
    }

    #[test]
    fn save_fails_with_index_busy_while_lock_held_then_succeeds() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index");

        // Two handles on the same index file.
        let writer = Index::create(&path, &test_config()).unwrap();
        let other = Index::open(&path).unwrap();

        // Simulate the other handle being mid-write by holding the advisory
        // lock (exactly what its save() critical section holds).
        let held = acquire_write_lock(&path).unwrap();

        let result = writer.save();
        match result {
            Err(Error::IndexBusy { path: p }) => assert_eq!(p, path),
            other => panic!("expected IndexBusy while lock held, got {other:?}"),
        }

        // Dropping the file handle releases the OS lock; save now succeeds.
        drop(held);
        writer.save().unwrap();
        other.save().unwrap();
    }

    #[test]
    fn reload_rejects_an_unsaved_partial_branch() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index");
        let index = Index::create(&path, &test_config()).unwrap();
        index.upsert(&mk_file("partial.md"), &[], &[]).unwrap();

        let result = index.reload_from_disk_if_clean();
        assert!(matches!(result, Err(Error::IndexDirty { path: p }) if p == path));
        assert!(index.get_file("partial.md").is_some());
        assert!(Index::open(&path).unwrap().get_file("partial.md").is_none());
    }

    /// Regression test: orphaned edge vectors in HNSW cause "Duplicate keys"
    /// error after save/reload because next_key is computed from metadata
    /// (chunks + semantic_edges) which doesn't include orphaned edges.
    #[test]
    fn orphaned_edges_do_not_cause_duplicate_key_on_reload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();
        let index = Index::create(&path, &config).unwrap();

        // Add a chunk so the index isn't empty.
        let file = crate::parser::MarkdownFile {
            path: std::path::PathBuf::from("test.md"),
            content_hash: "abc".to_string(),
            frontmatter: None,
            headings: vec![],
            body: "hello".to_string(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
            file_size: 5,
            links: vec![],
        };
        let chunk = crate::chunker::Chunk {
            id: "test.md#0".to_string(),
            source_path: std::path::PathBuf::from("test.md"),
            content: "hello".to_string(),
            heading_hierarchy: vec![],
            chunk_index: 0,
            is_sub_split: false,
            start_line: 0,
            end_line: 1,
        };
        index.upsert(&file, &[chunk], &[vec![1.0f32; 128]]).unwrap();

        // Add edge vectors (simulating edge embedding during ingest).
        let edges = vec![
            ("edge:test.md->a.md@1".to_string(), vec![0.5f32; 128]),
            ("edge:test.md->b.md@2".to_string(), vec![0.3f32; 128]),
        ];
        index.upsert_edges(&edges).unwrap();

        // Update link graph with semantic_edges that include ONLY one of the
        // two edges (simulating a link being removed from the file).
        let mut semantic_edges = HashMap::new();
        semantic_edges.insert(
            "edge:test.md->a.md@1".to_string(),
            crate::links::SemanticEdge {
                edge_id: "edge:test.md->a.md@1".to_string(),
                source: "test.md".to_string(),
                target: "a.md".to_string(),
                context_text: "link to a".to_string(),
                line_number: 1,
                strength: None,
                relationship_type: None,
                cluster_id: None,
            },
        );
        // Note: edge:test.md->b.md@2 is intentionally missing from semantic_edges
        // (it was removed from the file). This is the "orphan".
        let graph = crate::links::LinkGraph {
            forward: HashMap::new(),
            last_updated: 0,
            semantic_edges: Some(semantic_edges),
            edge_cluster_state: None,
        };
        index.update_link_graph(Some(graph));

        // Save and reload. Before the fix, the orphaned edge would cause
        // next_key to be too low on reload, leading to duplicate key errors.
        index.save().unwrap();
        let reloaded = Index::open(&path).unwrap();

        // Upsert a new file — this should NOT fail with "duplicate key".
        let file2 = crate::parser::MarkdownFile {
            path: std::path::PathBuf::from("other.md"),
            content_hash: "def".to_string(),
            frontmatter: None,
            headings: vec![],
            body: "world".to_string(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
            file_size: 5,
            links: vec![],
        };
        let chunk2 = crate::chunker::Chunk {
            id: "other.md#0".to_string(),
            source_path: std::path::PathBuf::from("other.md"),
            content: "world".to_string(),
            heading_hierarchy: vec![],
            chunk_index: 0,
            is_sub_split: false,
            start_line: 0,
            end_line: 1,
        };
        reloaded
            .upsert(&file2, &[chunk2], &[vec![0.8f32; 128]])
            .expect("upsert should not fail with duplicate key error");
    }
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use parking_lot::RwLock;
use usearch::Index as HnswIndex;

use tracing::debug;

use crate::chunker::Chunk;
use crate::error::{Error, Result};
use crate::index::storage::{self, WriteOptions};
use crate::index::types::{EmbeddingConfig, IndexMetadata, IndexStatus, StoredChunk, StoredFile};
use crate::clustering::ClusterState;
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
    /// The embedding vector for this chunk.
    pub vector: Vec<f32>,
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
    /// Open an existing index file at the given path with default write options.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_options(path, WriteOptions::default())
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
                hnsw_size,
                "next_key adjusted to match HNSW size"
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
        match Self::open_with_options(path, write_options.clone()) {
            Ok(index) => Ok(index),
            Err(Error::IndexNotFound { .. })
            | Err(Error::IndexVersionMismatch { .. })
            | Err(Error::IndexCorrupted(_)) => {
                // Remove outdated/corrupted index file so we can recreate it
                if path.exists() {
                    let _ = std::fs::remove_file(path);
                }
                Self::create_with_options(path, config, write_options)
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
        let relative_path = file.path.to_string_lossy().to_string();

        debug!(path = %relative_path, chunks = chunks.len(), "upserting file");

        // Remove old data if file already exists.
        if let Some(old_file) = state.metadata.files.remove(&relative_path) {
            for chunk_id in &old_file.chunk_ids {
                if let Some(key) = state.id_to_key.remove(chunk_id) {
                    let _ = state.hnsw.remove(key);
                }
                state.metadata.chunks.remove(chunk_id);
            }
        }

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
        state.metadata.file_mtimes
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

        IndexStatus {
            document_count: state.metadata.files.len(),
            chunk_count: state.metadata.chunks.len(),
            vector_count: state.hnsw.size(),
            last_updated: state.metadata.last_updated,
            file_size,
            embedding_config: state.metadata.embedding_config.clone(),
        }
    }

    /// Search for nearest vectors, returning `(chunk_id, cosine_similarity_score)` pairs.
    ///
    /// Converts usearch distance to cosine similarity: `score = 1.0 - distance`.
    /// Results are sorted by score descending (most similar first).
    /// Edge vectors (IDs starting with `"edge:"`) are post-filtered out.
    /// Over-fetches by 2x to compensate for filtered edge entries.
    pub fn search_vectors(&self, query: &[f32], limit: usize) -> Result<Vec<(String, f64)>> {
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
                let score = 1.0 - *distance as f64;
                output.push(((*chunk_id).clone(), score));
            }
        }

        // Sort by score descending.
        output.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        output.truncate(limit);

        Ok(output)
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

    /// Get the current link graph, if any.
    pub fn get_link_graph(&self) -> Option<LinkGraph> {
        let state = self.state.read();
        state.metadata.link_graph.clone()
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
    /// Over-fetches by 2x from the HNSW index, then post-filters to only `"edge:"` prefix
    /// IDs, and truncates to the requested limit. Results are sorted by score descending.
    pub fn search_edges(&self, query: &[f32], limit: usize) -> Result<Vec<(String, f64)>> {
        let state = self.state.read();

        if state.hnsw.size() == 0 {
            return Ok(Vec::new());
        }

        let over_fetch = limit * 2;
        let results = state
            .hnsw
            .search(query, over_fetch)
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
        state.metadata.scoped_schemas.as_ref().and_then(|schemas| {
            schemas.iter().find(|s| s.scope == prefix).cloned()
        })
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

        storage::write_index(&self.path, &state.metadata, &state.hnsw, &self.write_options)?;
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
                chunk_ids: Vec::new(),
                frontmatter: None,
                file_size: 0,
                indexed_at: 0,
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

        let edges1 = vec![
            ("edge:a.md->b.md@0".to_string(), vec![1.0f32; 128]),
        ];
        index.upsert_edges(&edges1).unwrap();

        // Upsert same ID with different vector.
        let edges2 = vec![
            ("edge:a.md->b.md@0".to_string(), vec![0.5f32; 128]),
        ];
        index.upsert_edges(&edges2).unwrap();

        let state = index.state.read();
        // Should still have only 1 vector (old removed, new added).
        assert_eq!(state.hnsw.size(), 1);
        assert!(state.id_to_key.contains_key("edge:a.md->b.md@0"));
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
        let edges = vec![
            ("edge:test.md->other.md@0".to_string(), vec![0.9f32; 128]),
        ];
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
        let edges = vec![
            ("edge:test.md->other.md@0".to_string(), vec![1.0f32; 128]),
        ];
        index.upsert_edges(&edges).unwrap();

        // search_vectors should NOT return edge IDs.
        let query = vec![1.0f32; 128];
        let results = index.search_vectors(&query, 10).unwrap();
        for (id, _) in &results {
            assert!(!id.starts_with("edge:"), "search_vectors returned edge ID: {id}");
        }
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "test.md#0");
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

        let edges = vec![
            ("edge:test.md->other.md@0".to_string(), vec![1.0f32; 128]),
        ];
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
            ("edge:test.md->other.md@0".to_string(), vec![1.0f32; 128]),
        ];
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

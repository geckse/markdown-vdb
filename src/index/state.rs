use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use parking_lot::RwLock;
use usearch::Index as HnswIndex;

use tracing::debug;

use crate::chunker::Chunk;
use crate::error::{Error, Result};
use crate::index::storage;
use crate::clustering::ClusterState;
use crate::index::types::{EmbeddingConfig, IndexMetadata, IndexStatus, StoredChunk, StoredFile};
use crate::links::LinkGraph;
use crate::parser::MarkdownFile;
use crate::schema::Schema;

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
}

impl Index {
    /// Open an existing index file at the given path.
    pub fn open(path: &Path) -> Result<Self> {
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

        let next_key = sorted_chunk_ids.len() as u64;

        Ok(Self {
            path: path.to_path_buf(),
            state: RwLock::new(IndexState {
                metadata,
                hnsw,
                id_to_key,
                next_key,
                dirty: false,
            }),
        })
    }

    /// Create a new, empty index file at the given path.
    pub fn create(path: &Path, config: &EmbeddingConfig) -> Result<Self> {
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
        };

        let hnsw = storage::create_hnsw(config.dimensions)?;
        hnsw.reserve(10)
            .map_err(|e| Error::Serialization(format!("usearch reserve: {e}")))?;

        storage::write_index(path, &metadata, &hnsw)?;

        Ok(Self {
            path: path.to_path_buf(),
            state: RwLock::new(IndexState {
                metadata,
                hnsw,
                id_to_key: HashMap::new(),
                next_key: 0,
                dirty: false,
            }),
        })
    }

    /// Open an existing index or create a new one if it doesn't exist.
    pub fn open_or_create(path: &Path, config: &EmbeddingConfig) -> Result<Self> {
        match Self::open(path) {
            Ok(index) => Ok(index),
            Err(Error::IndexNotFound { .. }) => Self::create(path, config),
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
    pub fn search_vectors(&self, query: &[f32], limit: usize) -> Result<Vec<(String, f64)>> {
        let state = self.state.read();

        if state.hnsw.size() == 0 {
            return Ok(Vec::new());
        }

        let results = state
            .hnsw
            .search(query, limit)
            .map_err(|e| Error::Serialization(format!("usearch search: {e}")))?;

        // Build reverse lookup: key → chunk_id.
        let key_to_id: HashMap<u64, &String> =
            state.id_to_key.iter().map(|(id, key)| (*key, id)).collect();

        let mut output = Vec::with_capacity(results.keys.len());
        for (key, distance) in results.keys.iter().zip(results.distances.iter()) {
            if let Some(chunk_id) = key_to_id.get(key) {
                let score = 1.0 - *distance as f64;
                output.push(((*chunk_id).clone(), score));
            }
        }

        // Sort by score descending.
        output.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

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
    pub fn search(&self, query: &[f32], limit: usize) -> Result<Vec<(String, f32)>> {
        let state = self.state.read();

        if state.hnsw.size() == 0 {
            return Ok(Vec::new());
        }

        let results = state
            .hnsw
            .search(query, limit)
            .map_err(|e| Error::Serialization(format!("usearch search: {e}")))?;

        // Build reverse lookup: key → chunk_id.
        let key_to_id: HashMap<u64, &String> =
            state.id_to_key.iter().map(|(id, key)| (*key, id)).collect();

        let mut output = Vec::with_capacity(results.keys.len());
        for (key, distance) in results.keys.iter().zip(results.distances.iter()) {
            if let Some(chunk_id) = key_to_id.get(key) {
                output.push(((*chunk_id).clone(), *distance));
            }
        }

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
        // matching alphabetically sorted chunk IDs.
        let dims = state.metadata.embedding_config.dimensions;
        let mut sorted_chunk_ids: Vec<&String> = state.metadata.chunks.keys().collect();
        sorted_chunk_ids.sort();

        let new_hnsw = storage::create_hnsw(dims)?;
        let n = sorted_chunk_ids.len();
        if n > 0 {
            new_hnsw
                .reserve(n.max(10))
                .map_err(|e| Error::Serialization(format!("usearch reserve: {e}")))?;
        }

        let mut new_id_to_key = HashMap::new();
        let mut buf = vec![0.0f32; dims];
        for (new_key, chunk_id) in sorted_chunk_ids.iter().enumerate() {
            if let Some(&old_key) = state.id_to_key.get(*chunk_id) {
                if state.hnsw.get(old_key, &mut buf).is_ok() {
                    new_hnsw
                        .add(new_key as u64, &buf)
                        .map_err(|e| Error::Serialization(format!("usearch add: {e}")))?;
                }
            }
            new_id_to_key.insert((*chunk_id).clone(), new_key as u64);
        }

        state.hnsw = new_hnsw;
        state.id_to_key = new_id_to_key;
        state.next_key = n as u64;

        storage::write_index(&self.path, &state.metadata, &state.hnsw)?;
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
    fn open_or_create_opens_when_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.idx");
        let config = test_config();

        Index::create(&path, &config).unwrap();
        let index = Index::open_or_create(&path, &config).unwrap();

        let state = index.state.read();
        assert_eq!(state.metadata.embedding_config.model, "test-model");
    }
}

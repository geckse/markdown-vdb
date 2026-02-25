use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use parking_lot::RwLock;
use usearch::Index as HnswIndex;

use tracing::debug;

use crate::chunker::Chunk;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::index::storage;
use crate::index::types::{EmbeddingConfig, IndexMetadata, IndexStatus, StoredChunk, StoredFile};
use crate::parser::MarkdownFile;

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
        // Chunk keys in the HNSW are assigned sequentially starting from 0.
        let mut id_to_key = HashMap::new();
        let mut max_key: Option<u64> = None;

        for (idx, chunk_id) in metadata.chunks.keys().enumerate() {
            let key = idx as u64;
            id_to_key.insert(chunk_id.clone(), key);
            max_key = Some(max_key.map_or(key, |m: u64| m.max(key)));
        }

        let next_key = max_key.map_or(0, |k| k + 1);

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
    pub fn create(path: &Path, config: &Config) -> Result<Self> {
        let metadata = IndexMetadata {
            chunks: HashMap::new(),
            files: HashMap::new(),
            embedding_config: EmbeddingConfig {
                provider: format!("{:?}", config.embedding_provider),
                model: config.embedding_model.clone(),
                dimensions: config.embedding_dimensions,
            },
            last_updated: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };

        let hnsw = storage::create_hnsw(config.embedding_dimensions)?;
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
    pub fn open_or_create(path: &Path, config: &Config) -> Result<Self> {
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

    /// Persist the index to disk atomically.
    pub fn save(&self) -> Result<()> {
        let mut state = self.state.write();

        state.metadata.last_updated = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config() -> Config {
        // Build a minimal config for testing. We bypass Config::load to avoid env var issues.
        Config {
            embedding_provider: crate::config::EmbeddingProviderType::OpenAI,
            embedding_model: "test-model".to_string(),
            embedding_dimensions: 128,
            embedding_batch_size: 100,
            openai_api_key: None,
            ollama_host: "http://localhost:11434".to_string(),
            embedding_endpoint: None,
            source_dirs: vec![PathBuf::from(".")],
            index_file: PathBuf::from(".markdownvdb.index"),
            ignore_patterns: vec![],
            watch_enabled: false,
            watch_debounce_ms: 300,
            chunk_max_tokens: 512,
            chunk_overlap_tokens: 50,
            clustering_enabled: false,
            clustering_rebalance_threshold: 50,
            search_default_limit: 10,
            search_min_score: 0.0,
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

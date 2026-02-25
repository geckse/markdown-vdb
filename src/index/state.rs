use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use parking_lot::RwLock;
use usearch::Index as HnswIndex;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::index::storage;
use crate::index::types::{EmbeddingConfig, IndexMetadata};

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

pub mod chunker;
pub mod config;
pub mod discovery;
pub mod embedding;
pub mod error;
pub mod index;
pub mod logging;
pub mod parser;
pub mod schema;
pub mod search;

pub use error::Error;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

use std::path::{Path, PathBuf};

use serde::Serialize;
use tracing::info;

use crate::config::Config;
use crate::embedding::provider::{create_provider, EmbeddingProvider};
use crate::index::state::Index;
use crate::index::types::EmbeddingConfig;

/// Result of an ingestion operation.
#[derive(Debug, Clone, Serialize)]
pub struct IngestResult {
    /// Number of files ingested.
    pub files_ingested: usize,
    /// Number of chunks created.
    pub chunks_created: usize,
    /// Number of files skipped (unchanged).
    pub files_skipped: usize,
    /// Number of files that failed to ingest.
    pub files_failed: usize,
    /// Errors encountered during ingestion.
    pub errors: Vec<IngestError>,
}

/// A single ingestion error for a specific file.
#[derive(Debug, Clone, Serialize)]
pub struct IngestError {
    /// Path to the file that failed.
    pub path: String,
    /// Error message.
    pub message: String,
}

/// Information about an indexed document.
#[derive(Debug, Clone, Serialize)]
pub struct DocumentInfo {
    /// Relative path to the markdown file.
    pub path: String,
    /// SHA-256 content hash.
    pub content_hash: String,
    /// Number of chunks for this document.
    pub chunk_count: usize,
    /// File size in bytes.
    pub file_size: u64,
    /// Unix timestamp when indexed.
    pub indexed_at: u64,
}

/// Summary of a cluster.
#[derive(Debug, Clone, Serialize)]
pub struct ClusterSummary {
    /// Cluster identifier.
    pub id: usize,
    /// Number of chunks in this cluster.
    pub chunk_count: usize,
    /// Representative keywords or label.
    pub label: Option<String>,
}

/// Primary library API handle for markdown-vdb.
pub struct MarkdownVdb {
    /// Canonicalized project root directory.
    root: PathBuf,
    /// Loaded configuration.
    config: Config,
    /// Embedding provider instance.
    provider: Box<dyn EmbeddingProvider>,
    /// Vector index.
    index: Index,
}

impl MarkdownVdb {
    /// Open a markdown-vdb instance rooted at the given directory.
    ///
    /// Loads config from `.markdownvdb`, creates the embedding provider,
    /// and opens or creates the index file.
    pub fn open(root: &Path) -> Result<Self> {
        let root = root.canonicalize().map_err(|e| {
            Error::Config(format!(
                "cannot canonicalize root '{}': {e}",
                root.display()
            ))
        })?;

        let config = Config::load(&root)?;
        Self::open_with_config(root, config)
    }

    /// Open a markdown-vdb instance with an explicit configuration.
    ///
    /// Useful for testing or when configuration is constructed programmatically.
    pub fn open_with_config(root: PathBuf, config: Config) -> Result<Self> {
        let root = if root.is_relative() {
            root.canonicalize().map_err(|e| {
                Error::Config(format!(
                    "cannot canonicalize root '{}': {e}",
                    root.display()
                ))
            })?
        } else {
            root
        };

        let provider = create_provider(&config)?;

        let embedding_config = EmbeddingConfig {
            provider: format!("{:?}", config.embedding_provider),
            model: config.embedding_model.clone(),
            dimensions: config.embedding_dimensions,
        };

        let index_path = root.join(&config.index_file);
        let index = Index::open_or_create(&index_path, &embedding_config)?;

        // Check config compatibility: dimensions must match.
        let status = index.status();
        if status.embedding_config.dimensions != config.embedding_dimensions {
            return Err(Error::Config(format!(
                "index was created with {} dimensions but config specifies {}",
                status.embedding_config.dimensions, config.embedding_dimensions
            )));
        }

        info!(
            root = %root.display(),
            provider = provider.name(),
            dimensions = config.embedding_dimensions,
            "opened markdown-vdb"
        );

        Ok(Self {
            root,
            config,
            provider,
            index,
        })
    }

    /// Get a reference to the project root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Get a reference to the loaded configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get a reference to the index.
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// Get a reference to the embedding provider.
    pub fn provider(&self) -> &dyn EmbeddingProvider {
        self.provider.as_ref()
    }

    /// Execute a semantic search query against the index.
    pub async fn search(
        &self,
        query: search::SearchQuery,
    ) -> Result<Vec<search::SearchResult>> {
        search::search(&query, &self.index, self.provider.as_ref()).await
    }

    /// Return a status snapshot of the index.
    pub fn status(&self) -> index::types::IndexStatus {
        self.index.status()
    }

    /// Return the metadata schema, either from the index or inferred from discovered files.
    pub fn schema(&self) -> Result<schema::Schema> {
        // Return stored schema if available.
        if let Some(s) = self.index.get_schema() {
            return Ok(s);
        }

        // Otherwise infer from discovered files.
        let disco = discovery::FileDiscovery::new(&self.root, &self.config);
        let files = disco.discover()?;
        let mut parsed = Vec::new();
        for path in &files {
            match parser::parse_markdown_file(&self.root, path) {
                Ok(md) => parsed.push(md),
                Err(_) => continue,
            }
        }
        Ok(schema::Schema::infer(&parsed))
    }

    /// Get information about an indexed document by its relative path.
    pub fn get_document(&self, relative_path: &str) -> Result<DocumentInfo> {
        let file = self.index.get_file(relative_path).ok_or_else(|| {
            Error::FileNotInIndex {
                path: PathBuf::from(relative_path),
            }
        })?;

        Ok(DocumentInfo {
            path: relative_path.to_string(),
            content_hash: file.content_hash.clone(),
            chunk_count: file.chunk_ids.len(),
            file_size: file.file_size,
            indexed_at: file.indexed_at,
        })
    }
}

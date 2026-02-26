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

// Re-export key public types for convenience.
pub use config::Config;
pub use index::types::IndexStatus;
pub use schema::{FieldType, Schema, SchemaField};
pub use search::{MetadataFilter, SearchQuery, SearchResult, SearchResultChunk, SearchResultFile};

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::embedding::provider::{create_provider, EmbeddingProvider};
use crate::index::state::Index;
use crate::index::types::EmbeddingConfig;

/// Options controlling the ingestion pipeline.
#[derive(Debug, Clone, Default)]
pub struct IngestOptions {
    /// Force re-embedding of all files, ignoring content hashes.
    pub full: bool,
    /// Ingest only a single file (relative path).
    pub file: Option<PathBuf>,
}

/// Result of an ingestion operation.
#[derive(Debug, Clone, Serialize)]
pub struct IngestResult {
    /// Number of files indexed (new or changed).
    pub files_indexed: usize,
    /// Number of files skipped (unchanged).
    pub files_skipped: usize,
    /// Number of files removed from index (deleted from disk).
    pub files_removed: usize,
    /// Number of chunks created.
    pub chunks_created: usize,
    /// Number of API calls made to the embedding provider.
    pub api_calls: usize,
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

    /// Ingest markdown files into the index.
    ///
    /// Pipeline: discover → parse → hash-compare → chunk → embed → upsert → remove deleted → save.
    pub async fn ingest(&self, options: IngestOptions) -> Result<IngestResult> {
        let disco = discovery::FileDiscovery::new(&self.root, &self.config);

        // Discover files to process.
        let discovered = if let Some(ref single_file) = options.file {
            // Verify the file exists.
            let full = self.root.join(single_file);
            if !full.is_file() {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("file not found: {}", single_file.display()),
                )));
            }
            vec![single_file.clone()]
        } else {
            disco.discover()?
        };

        info!(files = discovered.len(), "discovered markdown files");

        // Get existing hashes from index for skip detection.
        let existing_hashes = self.index.get_file_hashes();
        let existing_paths: std::collections::HashSet<String> =
            existing_hashes.keys().cloned().collect();

        let mut result = IngestResult {
            files_indexed: 0,
            files_skipped: 0,
            files_removed: 0,
            chunks_created: 0,
            api_calls: 0,
            files_failed: 0,
            errors: Vec::new(),
        };

        // Parse all files and collect chunks + hashes.
        let mut all_batch_chunks: Vec<embedding::batch::Chunk> = Vec::new();
        let mut current_hashes: HashMap<PathBuf, String> = HashMap::new();
        let mut parsed_files: HashMap<PathBuf, (parser::MarkdownFile, Vec<chunker::Chunk>)> =
            HashMap::new();
        let mut discovered_paths: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for path in &discovered {
            let path_str = path.to_string_lossy().to_string();
            discovered_paths.insert(path_str.clone());

            // Parse the file.
            let md = match parser::parse_markdown_file(&self.root, path) {
                Ok(md) => md,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to parse");
                    result.files_failed += 1;
                    result.errors.push(IngestError {
                        path: path_str,
                        message: e.to_string(),
                    });
                    continue;
                }
            };

            // Check content hash for skip (unless --full).
            if !options.full {
                if let Some(existing) = existing_hashes.get(&path_str) {
                    if *existing == md.content_hash {
                        debug!(path = %path.display(), "unchanged, skipping");
                        result.files_skipped += 1;
                        current_hashes.insert(path.clone(), md.content_hash.clone());
                        continue;
                    }
                }
            }

            current_hashes.insert(path.clone(), md.content_hash.clone());

            // Chunk the document.
            let chunks = match chunker::chunk_document(
                &md,
                self.config.chunk_max_tokens,
                self.config.chunk_overlap_tokens,
            ) {
                Ok(c) => c,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to chunk");
                    result.files_failed += 1;
                    result.errors.push(IngestError {
                        path: path_str,
                        message: e.to_string(),
                    });
                    continue;
                }
            };

            // Convert to batch chunks for embedding.
            for chunk in &chunks {
                all_batch_chunks.push(embedding::batch::Chunk {
                    id: chunk.id.clone(),
                    source_path: chunk.source_path.clone(),
                    content: chunk.content.clone(),
                });
            }

            parsed_files.insert(path.clone(), (md, chunks));
        }

        // Embed all changed chunks.
        // For files we're re-embedding, we pass empty existing hashes so nothing is skipped.
        let embed_existing: HashMap<PathBuf, String> = HashMap::new();
        let embed_current: HashMap<PathBuf, String> = all_batch_chunks
            .iter()
            .map(|c| (c.source_path.clone(), "changed".to_string()))
            .collect();

        let embed_result = embedding::batch::embed_chunks(
            self.provider.as_ref(),
            &all_batch_chunks,
            &embed_existing,
            &embed_current,
            self.config.embedding_batch_size,
        )
        .await?;

        result.api_calls = embed_result.api_calls;

        // Upsert files with their embeddings.
        for (path, (md, chunks)) in &parsed_files {
            let embeddings: Vec<Vec<f32>> = chunks
                .iter()
                .map(|chunk| {
                    embed_result
                        .embeddings
                        .get(&chunk.id)
                        .cloned()
                        .unwrap_or_default()
                })
                .collect();

            self.index.upsert(md, chunks, &embeddings)?;
            result.files_indexed += 1;
            result.chunks_created += chunks.len();
            debug!(path = %path.display(), chunks = chunks.len(), "indexed");
        }

        // Remove files that no longer exist on disk (only for full discovery, not single file).
        if options.file.is_none() {
            for path_str in &existing_paths {
                if !discovered_paths.contains(path_str) {
                    self.index.remove_file(path_str)?;
                    result.files_removed += 1;
                    debug!(path = %path_str, "removed deleted file from index");
                }
            }
        }

        // Save the index to disk.
        self.index.save()?;

        info!(
            files_indexed = result.files_indexed,
            files_skipped = result.files_skipped,
            files_removed = result.files_removed,
            chunks_created = result.chunks_created,
            api_calls = result.api_calls,
            "ingestion complete"
        );

        Ok(result)
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

    /// Initialize a new markdown-vdb project by creating a `.markdownvdb` config file
    /// with default/example values.
    ///
    /// Returns `Error::ConfigAlreadyExists` if the file already exists.
    pub fn init(root: &Path) -> Result<()> {
        let config_path = root.join(".markdownvdb");
        if config_path.exists() {
            return Err(Error::ConfigAlreadyExists {
                path: config_path,
            });
        }

        let default_config = "\
# markdown-vdb configuration
# See https://github.com/example/markdown-vdb for documentation

# Embedding provider: openai, ollama, or custom
MDVDB_EMBEDDING_PROVIDER=openai
MDVDB_EMBEDDING_MODEL=text-embedding-3-small
MDVDB_EMBEDDING_DIMENSIONS=1536
MDVDB_EMBEDDING_BATCH_SIZE=100

# Source directories (comma-separated)
MDVDB_SOURCE_DIRS=.

# Index file location
MDVDB_INDEX_FILE=.markdownvdb.index

# Chunking
MDVDB_CHUNK_MAX_TOKENS=512
MDVDB_CHUNK_OVERLAP_TOKENS=50

# Search defaults
MDVDB_SEARCH_DEFAULT_LIMIT=10
MDVDB_SEARCH_MIN_SCORE=0.0

# File watching
MDVDB_WATCH=true
MDVDB_WATCH_DEBOUNCE_MS=300

# Clustering
MDVDB_CLUSTERING_ENABLED=true
MDVDB_CLUSTERING_REBALANCE_THRESHOLD=50
";

        std::fs::write(&config_path, default_config)?;
        info!(path = %config_path.display(), "created default config file");
        Ok(())
    }

    /// Start watching for file changes and re-index incrementally.
    ///
    /// This is a stub that returns an error until the watcher module is fully integrated.
    pub fn watch(&self) -> Result<()> {
        Err(Error::Watch(
            "watch is not yet implemented; use `ingest` for manual indexing".into(),
        ))
    }

    /// Return cluster summaries for the indexed documents.
    ///
    /// This is a stub that returns an empty list until clustering is fully integrated.
    pub fn clusters(&self) -> Result<Vec<ClusterSummary>> {
        Ok(Vec::new())
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

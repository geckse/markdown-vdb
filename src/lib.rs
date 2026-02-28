pub mod chunker;
pub mod clustering;
pub mod config;
pub mod discovery;
pub mod embedding;
pub mod error;
pub mod fts;
pub mod index;
pub mod logging;
pub mod parser;
pub mod ingest;
pub mod schema;
pub mod search;
pub mod tree;
pub mod watcher;

pub use error::Error;

// Re-export key public types for convenience.
pub use config::Config;
pub use index::types::IndexStatus;
pub use schema::{FieldType, Schema, SchemaField};
pub use search::{MetadataFilter, SearchMode, SearchQuery, SearchResult, SearchResultChunk, SearchResultFile};
// Additional re-exports for library consumers.
pub use clustering::{ClusterInfo, ClusterState};
pub use tree::{FileState, FileTree, FileTreeNode};

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::embedding::provider::{create_provider, EmbeddingProvider};
use crate::fts::FtsIndex;
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
    /// Frontmatter metadata, if present.
    pub frontmatter: Option<serde_json::Value>,
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
    /// Number of documents in this cluster.
    pub document_count: usize,
    /// Representative keywords or label.
    pub label: Option<String>,
    /// Top keywords extracted via TF-IDF.
    pub keywords: Vec<String>,
}

/// Primary library API handle for markdown-vdb.
pub struct MarkdownVdb {
    /// Canonicalized project root directory.
    root: PathBuf,
    /// Loaded configuration.
    config: Config,
    /// Embedding provider instance (Arc for sharing with watcher).
    provider: Arc<dyn EmbeddingProvider>,
    /// Vector index (Arc for sharing with watcher).
    index: Arc<Index>,
    /// Full-text search index (Arc for sharing with watcher).
    fts_index: Arc<FtsIndex>,
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

        let provider: Arc<dyn EmbeddingProvider> = Arc::from(create_provider(&config)?);

        let embedding_config = EmbeddingConfig {
            provider: format!("{:?}", config.embedding_provider),
            model: config.embedding_model.clone(),
            dimensions: config.embedding_dimensions,
        };

        let index_path = root.join(&config.index_file);
        let index = Arc::new(Index::open_or_create(&index_path, &embedding_config)?);

        let fts_path = root.join(&config.fts_index_dir);
        let fts_index = Arc::new(FtsIndex::open_or_create(&fts_path)?);

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
            fts_index,
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

    /// Get a shared reference to the index (for watcher integration).
    pub fn index_arc(&self) -> Arc<Index> {
        Arc::clone(&self.index)
    }

    /// Get a reference to the embedding provider.
    pub fn provider(&self) -> &dyn EmbeddingProvider {
        self.provider.as_ref()
    }

    /// Get a shared reference to the embedding provider (for watcher integration).
    pub fn provider_arc(&self) -> Arc<dyn EmbeddingProvider> {
        Arc::clone(&self.provider)
    }

    /// Get a reference to the full-text search index.
    pub fn fts_index(&self) -> &FtsIndex {
        &self.fts_index
    }

    /// Get a shared reference to the FTS index (for watcher integration).
    pub fn fts_index_arc(&self) -> Arc<FtsIndex> {
        Arc::clone(&self.fts_index)
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

        // Full ingest: clear FTS index for a clean rebuild.
        if options.full {
            debug!("full ingest: clearing FTS index for rebuild");
            self.fts_index.delete_all()?;
            self.fts_index.commit()?;
        }

        // Consistency guard: if FTS has 0 docs but vector index has docs,
        // force re-indexing of all files into FTS.
        let fts_doc_count = self.fts_index.num_docs().unwrap_or(0);
        let vector_doc_count = self.index.status().document_count;
        let fts_needs_rebuild = !options.full && fts_doc_count == 0 && vector_doc_count > 0;
        if fts_needs_rebuild {
            info!(
                vector_docs = vector_doc_count,
                "FTS index empty but vector index has documents — will rebuild FTS"
            );
        }

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

            // Upsert into FTS index.
            let fts_chunks: Vec<fts::FtsChunkData> = chunks
                .iter()
                .map(|c| fts::FtsChunkData {
                    chunk_id: c.id.clone(),
                    source_path: c.source_path.to_string_lossy().to_string(),
                    content: c.content.clone(),
                    heading_hierarchy: c.heading_hierarchy.join(" > "),
                })
                .collect();
            let path_str_fts = path.to_string_lossy().to_string();
            self.fts_index.upsert_chunks(&path_str_fts, &fts_chunks)?;

            result.files_indexed += 1;
            result.chunks_created += chunks.len();
            debug!(path = %path.display(), chunks = chunks.len(), "indexed");
        }

        // Consistency guard: rebuild FTS from stored chunks for files that
        // were skipped (already in vector index but missing from FTS).
        if fts_needs_rebuild {
            info!("rebuilding FTS index from stored chunks");
            let file_hashes = self.index.get_file_hashes();
            for path_str in file_hashes.keys() {
                // Skip files we already upserted above.
                if parsed_files.keys().any(|p| p.to_string_lossy() == *path_str) {
                    continue;
                }
                if let Some(file_entry) = self.index.get_file(path_str) {
                    let fts_chunks: Vec<fts::FtsChunkData> = file_entry
                        .chunk_ids
                        .iter()
                        .filter_map(|cid| {
                            self.index.get_chunk(cid).map(|sc| fts::FtsChunkData {
                                chunk_id: cid.clone(),
                                source_path: sc.source_path.clone(),
                                content: sc.content.clone(),
                                heading_hierarchy: sc.heading_hierarchy.join(" > "),
                            })
                        })
                        .collect();
                    if !fts_chunks.is_empty() {
                        self.fts_index.upsert_chunks(path_str, &fts_chunks)?;
                    }
                }
            }
            debug!("FTS rebuild complete");
        }

        // Remove files that no longer exist on disk (only for full discovery, not single file).
        if options.file.is_none() {
            for path_str in &existing_paths {
                if !discovered_paths.contains(path_str) {
                    self.index.remove_file(path_str)?;
                    self.fts_index.remove_file(path_str)?;
                    result.files_removed += 1;
                    debug!(path = %path_str, "removed deleted file from index");
                }
            }
        }

        // Commit FTS changes and save the vector index to disk.
        self.fts_index.commit()?;
        self.index.save()?;

        // Run clustering if enabled.
        if self.config.clustering_enabled {
            let clusterer = clustering::Clusterer::new(&self.config);
            let doc_vectors = self.index.get_document_vectors();
            let doc_contents = self.index.get_document_contents();

            if !doc_vectors.is_empty() {
                if let Some(ref single_file) = options.file {
                    // Single-file ingest: assign to nearest cluster + maybe rebalance.
                    if let Some(mut state) = self.index.get_clusters() {
                        let path_str = single_file.to_string_lossy().to_string();
                        if let Some(vec) = doc_vectors.get(&path_str) {
                            if let Err(e) = clusterer.assign_to_nearest(&mut state, &path_str, vec) {
                                warn!(error = %e, "failed to assign document to cluster");
                            } else {
                                // Attempt rebalance with all document vectors.
                                match clusterer.maybe_rebalance(&mut state, &doc_vectors, &doc_contents) {
                                    Ok(rebalanced) => {
                                        if rebalanced {
                                            info!("clusters rebalanced after single-file ingest");
                                        }
                                    }
                                    Err(e) => warn!(error = %e, "cluster rebalance failed"),
                                }
                                self.index.update_clusters(Some(state));
                                self.index.save()?;
                            }
                        }
                    }
                    // If no existing clusters, skip — full ingest will create them.
                } else {
                    // Full ingest: run full K-means clustering.
                    match clusterer.cluster_all(&doc_vectors, &doc_contents) {
                        Ok(state) => {
                            self.index.update_clusters(Some(state));
                            self.index.save()?;
                            info!("clustering complete after full ingest");
                        }
                        Err(e) => {
                            warn!(error = %e, "clustering failed (non-fatal)");
                        }
                    }
                }
            }
        }

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
        search::search(
            &query,
            &self.index,
            self.provider.as_ref(),
            Some(&self.fts_index),
            self.config.search_rrf_k,
        )
        .await
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
MDVDB_SEARCH_MODE=hybrid
MDVDB_SEARCH_RRF_K=60.0

# Full-text search index directory
MDVDB_FTS_INDEX_DIR=.markdownvdb.fts

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
    /// Blocks until the provided `cancel` token is triggered (e.g. Ctrl+C).
    pub async fn watch(&self, cancel: CancellationToken) -> Result<()> {
        let w = watcher::Watcher::new(
            self.config.clone(),
            &self.root,
            Arc::clone(&self.index),
            Arc::clone(&self.fts_index),
            Arc::clone(&self.provider),
        );
        w.watch(cancel).await
    }

    /// Return cluster summaries for the indexed documents.
    pub fn clusters(&self) -> Result<Vec<ClusterSummary>> {
        match self.index.get_clusters() {
            Some(state) => Ok(state
                .clusters
                .iter()
                .map(|c| ClusterSummary {
                    id: c.id,
                    document_count: c.members.len(),
                    label: if c.label.is_empty() {
                        None
                    } else {
                        Some(c.label.clone())
                    },
                    keywords: c.keywords.clone(),
                })
                .collect()),
            None => Ok(Vec::new()),
        }
    }

    /// Build a file tree showing sync state of all discovered files.
    ///
    /// Compares files on disk against the index to classify each as
    /// indexed, modified, new, or deleted.
    pub fn file_tree(&self) -> Result<tree::FileTree> {
        tree::build_file_tree(&self.root, &self.config, &self.index)
    }

    /// Get information about an indexed document by its relative path.
    pub fn get_document(&self, relative_path: &str) -> Result<DocumentInfo> {
        let file = self.index.get_file(relative_path).ok_or_else(|| {
            Error::FileNotInIndex {
                path: PathBuf::from(relative_path),
            }
        })?;

        // Parse frontmatter from stored JSON string.
        let frontmatter = file
            .frontmatter
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        Ok(DocumentInfo {
            path: relative_path.to_string(),
            content_hash: file.content_hash.clone(),
            frontmatter,
            chunk_count: file.chunk_ids.len(),
            file_size: file.file_size,
            indexed_at: file.indexed_at,
        })
    }
}

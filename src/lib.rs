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
pub mod links;
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
pub use links::{LinkEntry, LinkGraph, LinkQueryResult, LinkState, OrphanFile, ResolvedLink};
pub use tree::{FileState, FileTree, FileTreeNode};
// Ingest progress and preview types are defined in this file and automatically public.

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

/// Phase of the ingestion pipeline, reported via progress callbacks.
#[derive(Debug, Clone, Serialize)]
pub enum IngestPhase {
    /// Discovering markdown files on disk.
    Discovering,
    /// Parsing a file. `current` and `total` are 1-based counts, `path` is relative.
    Parsing { current: usize, total: usize, path: String },
    /// Skipping an unchanged file.
    Skipped { current: usize, total: usize, path: String },
    /// Embedding a batch of chunks.
    Embedding { batch: usize, total_batches: usize },
    /// Saving the index to disk.
    Saving,
    /// Running clustering.
    Clustering,
    /// Cleaning up removed files.
    Cleaning,
    /// Ingestion complete.
    Done,
}

/// Callback invoked to report ingestion progress.
pub type ProgressCallback = Box<dyn Fn(&IngestPhase) + Send + Sync>;

/// Status of a file in an ingest preview.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum PreviewFileStatus {
    /// File is new (not yet indexed).
    New,
    /// File has changed since last index.
    Changed,
    /// File is unchanged.
    Unchanged,
}

/// Information about a single file in an ingest preview.
#[derive(Debug, Clone, Serialize)]
pub struct PreviewFileInfo {
    /// Relative path to the file.
    pub path: String,
    /// Whether the file is new, changed, or unchanged.
    pub status: PreviewFileStatus,
    /// Number of chunks this file would produce.
    pub chunks: usize,
    /// Estimated token count for embedding.
    pub estimated_tokens: usize,
}

/// Preview of what an ingestion would do, without actually doing it.
#[derive(Debug, Clone, Serialize)]
pub struct IngestPreview {
    /// Per-file details.
    pub files: Vec<PreviewFileInfo>,
    /// Total number of files discovered.
    pub total_files: usize,
    /// Number of files that need processing (new + changed).
    pub files_to_process: usize,
    /// Number of files that are unchanged.
    pub files_unchanged: usize,
    /// Total chunks across all files to process.
    pub total_chunks: usize,
    /// Estimated total tokens for embedding.
    pub estimated_tokens: usize,
    /// Estimated number of API calls.
    pub estimated_api_calls: usize,
}

/// Options controlling the ingestion pipeline.
#[derive(Default)]
pub struct IngestOptions {
    /// Force re-embedding of all files, ignoring content hashes.
    pub full: bool,
    /// Ingest only a single file (relative path).
    pub file: Option<PathBuf>,
    /// Optional progress callback invoked during ingestion.
    pub progress: Option<ProgressCallback>,
    /// Optional cancellation token to abort ingestion early.
    pub cancel: Option<CancellationToken>,
}

impl std::fmt::Debug for IngestOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IngestOptions")
            .field("full", &self.full)
            .field("file", &self.file)
            .field("progress", &self.progress.as_ref().map(|_| "..."))
            .field("cancel", &self.cancel)
            .finish()
    }
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
    /// Wall-clock duration of the ingestion in seconds.
    pub duration_secs: f64,
    /// Whether the ingestion was cancelled before completion.
    pub cancelled: bool,
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

/// Result of a doctor diagnostic check.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorResult {
    /// Individual diagnostic checks.
    pub checks: Vec<DoctorCheck>,
    /// Number of checks that passed.
    pub passed: usize,
    /// Total number of checks.
    pub total: usize,
}

/// A single diagnostic check.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    /// Human-readable name of the check.
    pub name: String,
    /// Pass, Fail, or Warn.
    pub status: CheckStatus,
    /// Detail message describing the result.
    pub detail: String,
}

/// Status of a diagnostic check.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum CheckStatus {
    Pass,
    Fail,
    Warn,
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

        // Ensure the unified .markdownvdb directory exists.
        // If it's a legacy flat config file, migrate it first.
        let index_dir = root.join(".markdownvdb");
        if index_dir.is_file() {
            // Legacy: .markdownvdb was a flat config file. Move it aside,
            // create the directory, then move the config inside as .config.
            let tmp_config = root.join(".markdownvdb.migrating");
            std::fs::rename(&index_dir, &tmp_config)?;
            std::fs::create_dir_all(&index_dir)?;
            std::fs::rename(&tmp_config, index_dir.join(".config"))?;
            info!("migrated legacy .markdownvdb config file → .markdownvdb/.config");
        } else if !index_dir.exists() {
            std::fs::create_dir_all(&index_dir)?;
        }

        // Auto-migrate from old split layout (.markdownvdb.index file + .markdownvdb.fts/ dir).
        let legacy_index_file = root.join(".markdownvdb.index");
        let legacy_fts_dir = root.join(".markdownvdb.fts");
        if legacy_index_file.is_file() && !index_dir.join("index").exists() {
            info!("migrating legacy .markdownvdb.index → .markdownvdb/index");
            std::fs::rename(&legacy_index_file, index_dir.join("index"))?;
        }
        if legacy_fts_dir.is_dir() && !index_dir.join("fts").exists() {
            info!("migrating legacy .markdownvdb.fts/ → .markdownvdb/fts/");
            std::fs::rename(&legacy_fts_dir, index_dir.join("fts"))?;
        }

        let index_path = index_dir.join("index");
        let index = Arc::new(Index::open_or_create(&index_path, &embedding_config)?);

        let fts_path = index_dir.join("fts");
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
        let start_time = std::time::Instant::now();

        // Helper: emit progress if callback is set.
        let emit = |phase: &IngestPhase| {
            if let Some(ref cb) = options.progress {
                cb(phase);
            }
        };

        // Helper: check if cancellation has been requested.
        let is_cancelled = || {
            options.cancel.as_ref().map_or(false, |c| c.is_cancelled())
        };

        emit(&IngestPhase::Discovering);

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

        // Check cancellation after discovery.
        if is_cancelled() {
            self.index.save()?;
            self.fts_index.commit()?;
            return Ok(IngestResult {
                files_indexed: 0, files_skipped: 0, files_removed: 0,
                chunks_created: 0, api_calls: 0, files_failed: 0,
                errors: Vec::new(), duration_secs: start_time.elapsed().as_secs_f64(),
                cancelled: true,
            });
        }

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
            duration_secs: 0.0,
            cancelled: false,
        };

        // Parse all files and collect chunks + hashes.
        let mut all_batch_chunks: Vec<embedding::batch::Chunk> = Vec::new();
        let mut current_hashes: HashMap<PathBuf, String> = HashMap::new();
        let mut parsed_files: HashMap<PathBuf, (parser::MarkdownFile, Vec<chunker::Chunk>)> =
            HashMap::new();
        let mut discovered_paths: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        let total_files = discovered.len();
        for (file_idx, path) in discovered.iter().enumerate() {
            let path_str = path.to_string_lossy().to_string();
            discovered_paths.insert(path_str.clone());

            emit(&IngestPhase::Parsing {
                current: file_idx + 1,
                total: total_files,
                path: path_str.clone(),
            });

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
                        emit(&IngestPhase::Skipped {
                            current: file_idx + 1,
                            total: total_files,
                            path: path_str.clone(),
                        });
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

        // Check cancellation after parsing.
        if is_cancelled() {
            result.cancelled = true;
            result.duration_secs = start_time.elapsed().as_secs_f64();
            self.index.save()?;
            self.fts_index.commit()?;
            return Ok(result);
        }

        emit(&IngestPhase::Embedding { batch: 0, total_batches: 0 });

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

        // Check cancellation after embedding.
        if is_cancelled() {
            result.cancelled = true;
            result.duration_secs = start_time.elapsed().as_secs_f64();
            self.index.save()?;
            self.fts_index.commit()?;
            return Ok(result);
        }

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

            // Upsert into FTS index (strip markdown before indexing for clean BM25).
            let fts_chunks: Vec<fts::FtsChunkData> = chunks
                .iter()
                .map(|c| fts::FtsChunkData {
                    chunk_id: c.id.clone(),
                    source_path: c.source_path.to_string_lossy().to_string(),
                    content: fts::strip_markdown(&c.content),
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
                                content: fts::strip_markdown(&sc.content),
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
        emit(&IngestPhase::Cleaning);
        let mut removed_paths: Vec<String> = Vec::new();
        if options.file.is_none() {
            for path_str in &existing_paths {
                if !discovered_paths.contains(path_str) {
                    self.index.remove_file(path_str)?;
                    self.fts_index.remove_file(path_str)?;
                    result.files_removed += 1;
                    removed_paths.push(path_str.clone());
                    debug!(path = %path_str, "removed deleted file from index");
                }
            }
        }

        // Build / update link graph.
        if let Some(ref single_file) = options.file {
            // Single-file ingest: update links for just this file.
            let mut graph = self.index.get_link_graph().unwrap_or_else(|| links::LinkGraph {
                forward: HashMap::new(),
                last_updated: 0,
            });
            if let Some((md, _)) = parsed_files.get(single_file) {
                links::update_file_links(&mut graph, md);
            }
            self.index.update_link_graph(Some(graph));
        } else {
            // Full ingest: build link graph from all parsed files.
            // Also need to include skipped files that are still in the index.
            // For simplicity, build from parsed files only (changed files).
            // For a complete graph we'd need all files, but we build from what we parsed.
            // Re-parse skipped files for link extraction.
            let mut all_md_files: Vec<parser::MarkdownFile> = Vec::new();
            for (md, _) in parsed_files.values() {
                all_md_files.push(md.clone());
            }
            // Parse skipped files too (they weren't in parsed_files).
            for path in &discovered {
                if !parsed_files.contains_key(path) {
                    if let Ok(md) = parser::parse_markdown_file(&self.root, path) {
                        all_md_files.push(md);
                    }
                }
            }
            let graph = links::build_link_graph(&all_md_files);
            self.index.update_link_graph(Some(graph));

            // Remove links for deleted files.
            if !removed_paths.is_empty() {
                if let Some(mut graph) = self.index.get_link_graph() {
                    for path_str in &removed_paths {
                        links::remove_file_links(&mut graph, path_str);
                    }
                    self.index.update_link_graph(Some(graph));
                }
            }
        }

        // Save vector index first (atomic write-rename), then commit FTS.
        emit(&IngestPhase::Saving);
        self.index.save()?;
        self.fts_index.commit()?;

        // Run clustering if enabled.
        if self.config.clustering_enabled {
            emit(&IngestPhase::Clustering);
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

        result.duration_secs = start_time.elapsed().as_secs_f64();

        emit(&IngestPhase::Done);

        info!(
            files_indexed = result.files_indexed,
            files_skipped = result.files_skipped,
            files_removed = result.files_removed,
            chunks_created = result.chunks_created,
            api_calls = result.api_calls,
            duration_secs = result.duration_secs,
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
            self.config.bm25_norm_k,
        )
        .await
    }

    /// Preview what an ingestion would do, without making any API calls or modifying the index.
    ///
    /// This is intentionally synchronous because it performs no network requests.
    /// It discovers, parses, and chunks files, then compares hashes with the existing index.
    pub fn preview(&self, reindex: bool, file: Option<PathBuf>) -> Result<IngestPreview> {
        let disco = discovery::FileDiscovery::new(&self.root, &self.config);

        // Discover files to process.
        let discovered = if let Some(ref single_file) = file {
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

        // Get existing hashes from index for skip detection.
        let existing_hashes = self.index.get_file_hashes();

        let mut files = Vec::new();
        let mut total_chunks: usize = 0;
        let mut estimated_tokens: usize = 0;
        let mut files_to_process: usize = 0;
        let mut files_unchanged: usize = 0;

        for path in &discovered {
            let path_str = path.to_string_lossy().to_string();

            // Parse the file.
            let md = match parser::parse_markdown_file(&self.root, path) {
                Ok(md) => md,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to parse during preview");
                    continue;
                }
            };

            // Determine file status.
            let status = if reindex {
                PreviewFileStatus::Changed
            } else if let Some(existing) = existing_hashes.get(&path_str) {
                if *existing == md.content_hash {
                    PreviewFileStatus::Unchanged
                } else {
                    PreviewFileStatus::Changed
                }
            } else {
                PreviewFileStatus::New
            };

            // Chunk the document.
            let chunks = match chunker::chunk_document(
                &md,
                self.config.chunk_max_tokens,
                self.config.chunk_overlap_tokens,
            ) {
                Ok(c) => c,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to chunk during preview");
                    continue;
                }
            };

            let chunk_count = chunks.len();
            let file_tokens: usize = chunks.iter().map(|c| chunker::count_tokens(&c.content)).sum();

            if status != PreviewFileStatus::Unchanged {
                files_to_process += 1;
                total_chunks += chunk_count;
                estimated_tokens += file_tokens;
            } else {
                files_unchanged += 1;
            }

            files.push(PreviewFileInfo {
                path: path_str,
                status,
                chunks: chunk_count,
                estimated_tokens: file_tokens,
            });
        }

        // Estimate API calls: chunks are sent in batches.
        let batch_size = self.config.embedding_batch_size.max(1);
        let estimated_api_calls = if total_chunks == 0 {
            0
        } else {
            (total_chunks + batch_size - 1) / batch_size
        };

        Ok(IngestPreview {
            total_files: discovered.len(),
            files_to_process,
            files_unchanged,
            total_chunks,
            estimated_tokens,
            estimated_api_calls,
            files,
        })
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

    /// Initialize a new markdown-vdb project by creating `.markdownvdb/.config`
    /// with default/example values.
    ///
    /// Returns `Error::ConfigAlreadyExists` if the config already exists.
    pub fn init(root: &Path) -> Result<()> {
        let dir_path = root.join(".markdownvdb");
        let config_path = dir_path.join(".config");

        // Check for both new and legacy config locations.
        if config_path.exists() {
            return Err(Error::ConfigAlreadyExists {
                path: config_path,
            });
        }
        let legacy_path = root.join(".markdownvdb");
        if legacy_path.is_file() {
            return Err(Error::ConfigAlreadyExists {
                path: legacy_path,
            });
        }

        // Create the .markdownvdb directory if it doesn't exist.
        if !dir_path.exists() {
            std::fs::create_dir_all(&dir_path)?;
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

# Chunking
MDVDB_CHUNK_MAX_TOKENS=512
MDVDB_CHUNK_OVERLAP_TOKENS=50

# Search defaults
MDVDB_SEARCH_DEFAULT_LIMIT=10
MDVDB_SEARCH_MIN_SCORE=0.0
MDVDB_SEARCH_MODE=hybrid
MDVDB_SEARCH_RRF_K=60.0

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

    /// Query links originating from a specific file.
    pub fn links(&self, path: &str) -> Result<links::LinkQueryResult> {
        let graph = self.index.get_link_graph().ok_or_else(|| {
            Error::Config("no link graph available; run ingest first".to_string())
        })?;
        let indexed_files: std::collections::HashSet<String> =
            self.index.get_file_hashes().keys().cloned().collect();
        let backlink_map = links::compute_backlinks(&graph);
        Ok(links::query_links(path, &graph, &backlink_map, &indexed_files))
    }

    /// Query backlinks pointing to a specific file.
    pub fn backlinks(&self, path: &str) -> Result<Vec<links::ResolvedLink>> {
        let graph = self.index.get_link_graph().ok_or_else(|| {
            Error::Config("no link graph available; run ingest first".to_string())
        })?;
        let indexed_files: std::collections::HashSet<String> =
            self.index.get_file_hashes().keys().cloned().collect();
        let backlink_map = links::compute_backlinks(&graph);
        let entries = backlink_map.get(path).cloned().unwrap_or_default();
        Ok(entries
            .into_iter()
            .map(|entry| {
                let state = if indexed_files.contains(&entry.source) {
                    links::LinkState::Valid
                } else {
                    links::LinkState::Broken
                };
                links::ResolvedLink { entry, state }
            })
            .collect())
    }

    /// Find orphan files (no incoming or outgoing links).
    pub fn orphans(&self) -> Result<Vec<links::OrphanFile>> {
        let graph = self.index.get_link_graph().ok_or_else(|| {
            Error::Config("no link graph available; run ingest first".to_string())
        })?;
        let indexed_files: std::collections::HashSet<String> =
            self.index.get_file_hashes().keys().cloned().collect();
        Ok(links::find_orphans(&graph, &indexed_files))
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

    /// Initialize a user-level config file at the given path.
    ///
    /// Creates parent directories if needed. Returns `Error::ConfigAlreadyExists`
    /// if the file already exists.
    pub fn init_global(config_path: &Path) -> Result<()> {
        if config_path.exists() {
            return Err(Error::ConfigAlreadyExists {
                path: config_path.to_path_buf(),
            });
        }

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let template = "\
# mdvdb user-level configuration
# Values here apply to all projects unless overridden by project .markdownvdb

# API credentials
# OPENAI_API_KEY=sk-...

# Default embedding provider
# MDVDB_EMBEDDING_PROVIDER=openai
# MDVDB_EMBEDDING_MODEL=text-embedding-3-small
# MDVDB_EMBEDDING_DIMENSIONS=1536

# Ollama host (if using Ollama)
# OLLAMA_HOST=http://localhost:11434
";

        std::fs::write(config_path, template)?;
        info!(path = %config_path.display(), "created user-level config file");
        Ok(())
    }

    /// Run diagnostic checks on the project configuration and index.
    pub async fn doctor(&self) -> Result<DoctorResult> {
        let mut checks = Vec::new();

        // 1. Config loaded (always passes since we're already constructed).
        checks.push(DoctorCheck {
            name: "Config loaded".to_string(),
            status: CheckStatus::Pass,
            detail: format!(
                "{:?} / {} / {}",
                self.config.embedding_provider,
                self.config.embedding_model,
                self.config.embedding_dimensions
            ),
        });

        // 2. User config existence.
        match Config::user_config_path() {
            Some(path) if path.is_file() => {
                checks.push(DoctorCheck {
                    name: "User config".to_string(),
                    status: CheckStatus::Pass,
                    detail: path.display().to_string(),
                });
            }
            Some(path) => {
                checks.push(DoctorCheck {
                    name: "User config".to_string(),
                    status: CheckStatus::Warn,
                    detail: format!("{} (not found)", path.display()),
                });
            }
            None => {
                checks.push(DoctorCheck {
                    name: "User config".to_string(),
                    status: CheckStatus::Warn,
                    detail: "home directory not resolved".to_string(),
                });
            }
        }

        // 3. Project config.
        let project_config = self.root.join(".markdownvdb");
        if project_config.is_dir() {
            checks.push(DoctorCheck {
                name: "Project config".to_string(),
                status: CheckStatus::Pass,
                detail: ".markdownvdb/".to_string(),
            });
        } else {
            checks.push(DoctorCheck {
                name: "Project config".to_string(),
                status: CheckStatus::Fail,
                detail: ".markdownvdb not found".to_string(),
            });
        }

        // 4. API key.
        match self.config.embedding_provider {
            config::EmbeddingProviderType::OpenAI => {
                if self.config.openai_api_key.is_some() {
                    checks.push(DoctorCheck {
                        name: "API key".to_string(),
                        status: CheckStatus::Pass,
                        detail: "OPENAI_API_KEY is set".to_string(),
                    });
                } else {
                    checks.push(DoctorCheck {
                        name: "API key".to_string(),
                        status: CheckStatus::Fail,
                        detail: "OPENAI_API_KEY not set".to_string(),
                    });
                }
            }
            config::EmbeddingProviderType::Mock => {
                checks.push(DoctorCheck {
                    name: "API key".to_string(),
                    status: CheckStatus::Pass,
                    detail: "mock provider (no key needed)".to_string(),
                });
            }
            _ => {
                checks.push(DoctorCheck {
                    name: "API key".to_string(),
                    status: CheckStatus::Pass,
                    detail: format!("{:?} provider configured", self.config.embedding_provider),
                });
            }
        }

        // 5. Provider connectivity.
        let start = std::time::Instant::now();
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.provider.embed_batch(&["test".to_string()]),
        )
        .await
        {
            Ok(Ok(_vectors)) => {
                let ms = start.elapsed().as_millis();
                checks.push(DoctorCheck {
                    name: "Provider reachable".to_string(),
                    status: CheckStatus::Pass,
                    detail: format!("OK ({ms}ms)"),
                });
            }
            Ok(Err(e)) => {
                checks.push(DoctorCheck {
                    name: "Provider reachable".to_string(),
                    status: CheckStatus::Fail,
                    detail: e.to_string(),
                });
            }
            Err(_) => {
                checks.push(DoctorCheck {
                    name: "Provider reachable".to_string(),
                    status: CheckStatus::Fail,
                    detail: "timeout (5s)".to_string(),
                });
            }
        }

        // 6. Index integrity.
        let status = self.index.status();
        if status.vector_count == status.chunk_count {
            checks.push(DoctorCheck {
                name: "Index".to_string(),
                status: CheckStatus::Pass,
                detail: format!(
                    "{} docs, {} chunks, {} vectors",
                    status.document_count, status.chunk_count, status.vector_count
                ),
            });
        } else {
            checks.push(DoctorCheck {
                name: "Index".to_string(),
                status: CheckStatus::Warn,
                detail: format!(
                    "{} docs, {} chunks, {} vectors (mismatch)",
                    status.document_count, status.chunk_count, status.vector_count
                ),
            });
        }

        // 7. Source directories.
        let disco = discovery::FileDiscovery::new(&self.root, &self.config);
        match disco.discover() {
            Ok(files) => {
                let dir_list: Vec<String> = self
                    .config
                    .source_dirs
                    .iter()
                    .map(|d| format!("{}/", d.display()))
                    .collect();
                checks.push(DoctorCheck {
                    name: "Source directories".to_string(),
                    status: CheckStatus::Pass,
                    detail: format!("{} ({} .md files)", dir_list.join(" "), files.len()),
                });
            }
            Err(e) => {
                checks.push(DoctorCheck {
                    name: "Source directories".to_string(),
                    status: CheckStatus::Fail,
                    detail: e.to_string(),
                });
            }
        }

        let passed = checks.iter().filter(|c| c.status == CheckStatus::Pass).count();
        let total = checks.len();

        Ok(DoctorResult {
            checks,
            passed,
            total,
        })
    }
}

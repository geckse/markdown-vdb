// Full + incremental ingestion pipeline

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use tracing::{debug, info, warn};

use crate::chunker::{self, Chunk};
use crate::config::Config;
use crate::discovery::FileDiscovery;
use crate::embedding::batch::{self, Chunk as BatchChunk};
use crate::embedding::provider::EmbeddingProvider;
use crate::error::Result;
use crate::index::Index;
use crate::parser::{self, MarkdownFile};

/// Result of ingesting a single file.
#[derive(Debug, Serialize)]
pub struct IngestResult {
    /// Relative path of the ingested file.
    pub path: PathBuf,
    /// Number of chunks produced from the file.
    pub chunks_total: usize,
    /// Number of chunks that were embedded (new or changed).
    pub chunks_embedded: usize,
    /// Number of chunks skipped (unchanged content hash).
    pub chunks_skipped: usize,
    /// Number of API calls made to the embedding provider.
    pub api_calls: usize,
    /// Whether the file was skipped entirely (unchanged hash).
    pub skipped: bool,
}

/// Convert a chunker::Chunk to a batch::Chunk for embedding.
fn to_batch_chunk(chunk: &Chunk) -> BatchChunk {
    BatchChunk {
        id: chunk.id.clone(),
        source_path: chunk.source_path.clone(),
        content: chunk.content.clone(),
    }
}

/// Ingest a single markdown file through the full pipeline:
/// parse → hash check → chunk → embed → upsert → save.
///
/// If the file's content hash matches the existing hash in the index,
/// the file is skipped entirely and no embedding calls are made.
pub async fn ingest_file(
    project_root: &Path,
    relative_path: &Path,
    index: &Index,
    provider: &dyn EmbeddingProvider,
    max_tokens: usize,
    overlap_tokens: usize,
    batch_size: usize,
) -> Result<IngestResult> {
    let rel_str = relative_path.to_string_lossy().to_string();
    debug!(path = %rel_str, "ingesting file");

    // 1. Parse the markdown file.
    let file: MarkdownFile = parser::parse_markdown_file(project_root, relative_path)?;

    // 2. Check content hash against index — skip if unchanged.
    let existing_hashes = index.get_file_hashes();
    let existing_hash = existing_hashes.get(&rel_str);
    if existing_hash.map(|h| h.as_str()) == Some(&file.content_hash) {
        debug!(path = %rel_str, "file unchanged, skipping");
        let existing_file = index.get_file(&rel_str);
        let chunk_count = existing_file.map(|f| f.chunk_ids.len()).unwrap_or(0);
        return Ok(IngestResult {
            path: relative_path.to_path_buf(),
            chunks_total: chunk_count,
            chunks_embedded: 0,
            chunks_skipped: chunk_count,
            api_calls: 0,
            skipped: true,
        });
    }

    // 3. Chunk the document.
    let chunks = chunker::chunk_document(&file, max_tokens, overlap_tokens)?;
    let chunks_total = chunks.len();

    if chunks.is_empty() {
        debug!(path = %rel_str, "no chunks produced, upserting empty file");
        index.upsert(&file, &[], &[])?;
        index.save()?;
        return Ok(IngestResult {
            path: relative_path.to_path_buf(),
            chunks_total: 0,
            chunks_embedded: 0,
            chunks_skipped: 0,
            api_calls: 0,
            skipped: false,
        });
    }

    // 4. Convert to batch chunks and embed.
    let batch_chunks: Vec<BatchChunk> = chunks.iter().map(to_batch_chunk).collect();

    let mut current_hashes = HashMap::new();
    current_hashes.insert(
        relative_path.to_path_buf(),
        file.content_hash.clone(),
    );

    // Pass empty existing hashes since we already checked above and know the file changed.
    let empty_existing: HashMap<PathBuf, String> = HashMap::new();

    let embed_result =
        batch::embed_chunks(provider, &batch_chunks, &empty_existing, &current_hashes, batch_size)
            .await?;

    // 5. Reorder embeddings to match chunk order.
    let mut ordered_embeddings: Vec<Vec<f32>> = Vec::with_capacity(chunks_total);
    for chunk in &chunks {
        let embedding = embed_result
            .embeddings
            .get(&chunk.id)
            .cloned()
            .unwrap_or_default();
        ordered_embeddings.push(embedding);
    }

    let chunks_embedded = embed_result.embeddings.len();
    let chunks_skipped = embed_result.skipped.len();

    // 6. Upsert into the index.
    index.upsert(&file, &chunks, &ordered_embeddings)?;

    // 7. Save the index to disk.
    index.save()?;

    info!(
        path = %rel_str,
        chunks_total,
        chunks_embedded,
        chunks_skipped,
        api_calls = embed_result.api_calls,
        "file ingested"
    );

    Ok(IngestResult {
        path: relative_path.to_path_buf(),
        chunks_total,
        chunks_embedded,
        chunks_skipped,
        api_calls: embed_result.api_calls,
        skipped: false,
    })
}

/// Aggregate result of a full ingestion run across all discovered files.
#[derive(Debug, Serialize)]
pub struct FullIngestResult {
    /// Total number of files discovered.
    pub files_discovered: usize,
    /// Number of files that were ingested (new or changed).
    pub files_ingested: usize,
    /// Number of files skipped (unchanged content hash).
    pub files_skipped: usize,
    /// Number of stale files removed from the index.
    pub files_removed: usize,
    /// Total chunks across all files.
    pub chunks_total: usize,
    /// Total chunks embedded (new or changed).
    pub chunks_embedded: usize,
    /// Total chunks skipped (unchanged).
    pub chunks_skipped: usize,
    /// Total API calls made to the embedding provider.
    pub api_calls: usize,
    /// Per-file results.
    pub results: Vec<IngestResult>,
}

/// Perform a full ingestion: discover all markdown files, parse and chunk them,
/// embed changed files, upsert into the index, and remove stale entries.
pub async fn ingest_full(
    project_root: &Path,
    config: &Config,
    index: &Index,
    provider: &dyn EmbeddingProvider,
    max_tokens: usize,
    overlap_tokens: usize,
    batch_size: usize,
) -> Result<FullIngestResult> {
    // 1. Discover all markdown files.
    let discovery = FileDiscovery::new(project_root, config);
    let discovered_paths = discovery.discover()?;
    let files_discovered = discovered_paths.len();
    info!(files = files_discovered, "discovered markdown files");

    // 2. Track which files are currently on disk for stale detection.
    let discovered_set: HashSet<String> = discovered_paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    // 3. Ingest each discovered file.
    let mut results = Vec::with_capacity(files_discovered);
    let mut files_ingested: usize = 0;
    let mut files_skipped: usize = 0;
    let mut chunks_total: usize = 0;
    let mut chunks_embedded: usize = 0;
    let mut chunks_skipped: usize = 0;
    let mut api_calls: usize = 0;

    for relative_path in &discovered_paths {
        match ingest_file(
            project_root,
            relative_path,
            index,
            provider,
            max_tokens,
            overlap_tokens,
            batch_size,
        )
        .await
        {
            Ok(result) => {
                if result.skipped {
                    files_skipped += 1;
                } else {
                    files_ingested += 1;
                }
                chunks_total += result.chunks_total;
                chunks_embedded += result.chunks_embedded;
                chunks_skipped += result.chunks_skipped;
                api_calls += result.api_calls;
                results.push(result);
            }
            Err(e) => {
                warn!(
                    path = %relative_path.display(),
                    error = %e,
                    "failed to ingest file, skipping"
                );
            }
        }
    }

    // 4. Remove stale entries (files in the index that no longer exist on disk).
    let indexed_hashes = index.get_file_hashes();
    let mut files_removed: usize = 0;
    for indexed_path in indexed_hashes.keys() {
        if !discovered_set.contains(indexed_path) {
            debug!(path = %indexed_path, "removing stale file from index");
            index.remove_file(indexed_path)?;
            files_removed += 1;
        }
    }

    // 5. Save the index once after all mutations.
    if files_ingested > 0 || files_removed > 0 {
        index.save()?;
    }

    info!(
        files_discovered,
        files_ingested,
        files_skipped,
        files_removed,
        chunks_total,
        chunks_embedded,
        chunks_skipped,
        api_calls,
        "full ingestion complete"
    );

    Ok(FullIngestResult {
        files_discovered,
        files_ingested,
        files_skipped,
        files_removed,
        chunks_total,
        chunks_embedded,
        chunks_skipped,
        api_calls,
        results,
    })
}

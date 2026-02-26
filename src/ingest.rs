// Full + incremental ingestion pipeline

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tracing::{debug, info};

use crate::chunker::{self, Chunk};
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

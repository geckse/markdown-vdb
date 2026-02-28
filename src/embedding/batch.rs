use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;

use super::provider::EmbeddingProvider;

/// A markdown chunk to be embedded.
///
/// This is a temporary definition used until the chunking engine (Phase 3)
/// provides the canonical `Chunk` type.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Unique ID: `"{relative_path}#{chunk_index}"`.
    pub id: String,
    /// Path to source file (relative to project root).
    pub source_path: PathBuf,
    /// The text content of this chunk (what gets embedded).
    pub content: String,
}

/// Result of a batch embedding operation.
#[derive(Debug, Serialize)]
pub struct EmbeddingResult {
    /// Map from chunk ID to its embedding vector.
    pub embeddings: HashMap<String, Vec<f32>>,
    /// Chunk IDs that were skipped (unchanged content).
    pub skipped: Vec<String>,
    /// Number of API calls made to the embedding provider.
    pub api_calls: usize,
}

/// Embed chunks using the given provider, skipping files whose content hash is unchanged.
///
/// Algorithm:
/// 1. Group chunks by `source_path`.
/// 2. For each file, compare `current_hashes[path]` with `existing_hashes[path]` —
///    if equal, add all chunk IDs to the skipped list.
/// 3. Collect remaining chunks into batches of `batch_size`.
/// 4. Process batches sequentially (provider is a borrowed trait object).
/// 5. Assemble and return the `EmbeddingResult`.
pub async fn embed_chunks(
    provider: &dyn EmbeddingProvider,
    chunks: &[Chunk],
    existing_hashes: &HashMap<PathBuf, String>,
    current_hashes: &HashMap<PathBuf, String>,
    batch_size: usize,
    on_batch: Option<&(dyn Fn(usize, usize) + Send + Sync)>,
) -> crate::Result<EmbeddingResult> {
    let mut skipped = Vec::new();
    let mut to_embed: Vec<&Chunk> = Vec::new();

    // Group by source_path and decide skip vs embed
    let mut by_path: HashMap<&PathBuf, Vec<&Chunk>> = HashMap::new();
    for chunk in chunks {
        by_path.entry(&chunk.source_path).or_default().push(chunk);
    }

    for (path, file_chunks) in &by_path {
        let unchanged = match (current_hashes.get(*path), existing_hashes.get(*path)) {
            (Some(current), Some(existing)) => current == existing,
            _ => false,
        };

        if unchanged {
            tracing::debug!(path = %path.display(), count = file_chunks.len(), "skipping unchanged file");
            for chunk in file_chunks {
                skipped.push(chunk.id.clone());
            }
        } else {
            tracing::debug!(path = %path.display(), count = file_chunks.len(), "file changed, will embed");
            to_embed.extend(file_chunks);
        }
    }

    if to_embed.is_empty() {
        tracing::info!(skipped = skipped.len(), "all chunks skipped (no changes)");
        return Ok(EmbeddingResult {
            embeddings: HashMap::new(),
            skipped,
            api_calls: 0,
        });
    }

    // Split into batches
    let batches: Vec<Vec<&Chunk>> = to_embed.chunks(batch_size).map(|b| b.to_vec()).collect();
    let total_batches = batches.len();
    tracing::info!(
        chunks = to_embed.len(),
        batches = total_batches,
        batch_size,
        "embedding chunks"
    );

    // Process batches concurrently (up to 4 at a time).
    use futures::stream::{self, StreamExt};

    const MAX_CONCURRENT: usize = 4;

    type BatchResult = crate::Result<(usize, Vec<(String, Vec<f32>)>)>;
    let mut stream = stream::iter(batches.into_iter().enumerate().map(|(batch_idx, batch)| {
            let chunk_ids: Vec<String> = batch.iter().map(|c| c.id.clone()).collect();
            let texts: Vec<String> = batch.iter().map(|c| c.content.clone()).collect();
            async move {
                let vectors = provider.embed_batch(&texts).await?;
                tracing::info!(
                    batch = batch_idx + 1,
                    total = total_batches,
                    "batch complete"
                );
                let pairs: Vec<(String, Vec<f32>)> =
                    chunk_ids.into_iter().zip(vectors).collect();
                let result: BatchResult = Ok((batch_idx, pairs));
                result
            }
        }))
        .buffer_unordered(MAX_CONCURRENT);

    let mut embeddings: HashMap<String, Vec<f32>> = HashMap::new();
    let mut api_calls: usize = 0;
    let mut completed_count: usize = 0;

    while let Some(result) = stream.next().await {
        let (_batch_idx, pairs) = result?;
        api_calls += 1;
        completed_count += 1;
        for (id, vector) in pairs {
            embeddings.insert(id, vector);
        }
        if let Some(cb) = &on_batch {
            cb(completed_count, total_batches);
        }
    }

    tracing::info!(
        embedded = embeddings.len(),
        skipped = skipped.len(),
        api_calls,
        "embedding complete"
    );

    Ok(EmbeddingResult {
        embeddings,
        skipped,
        api_calls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::mock::MockProvider;

    fn make_chunk(id: &str, path: &str, content: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            source_path: PathBuf::from(path),
            content: content.to_string(),
        }
    }

    #[tokio::test]
    async fn test_batch_all_chunks_embedded() {
        let provider = MockProvider::new(64);
        let chunks = vec![
            make_chunk("a.md#0", "a.md", "hello"),
            make_chunk("b.md#0", "b.md", "world"),
        ];
        let existing: HashMap<PathBuf, String> = HashMap::new();
        let mut current = HashMap::new();
        current.insert(PathBuf::from("a.md"), "hash_a".into());
        current.insert(PathBuf::from("b.md"), "hash_b".into());

        let result = embed_chunks(&provider, &chunks, &existing, &current, 10, None)
            .await
            .unwrap();

        assert_eq!(result.embeddings.len(), 2);
        assert!(result.skipped.is_empty());
        assert!(result.api_calls > 0);
        assert!(result.embeddings.contains_key("a.md#0"));
        assert!(result.embeddings.contains_key("b.md#0"));
    }

    #[tokio::test]
    async fn test_batch_unchanged_skipped() {
        let provider = MockProvider::new(64);
        let chunks = vec![
            make_chunk("a.md#0", "a.md", "hello"),
            make_chunk("a.md#1", "a.md", "world"),
        ];
        let mut existing = HashMap::new();
        existing.insert(PathBuf::from("a.md"), "same_hash".into());
        let mut current = HashMap::new();
        current.insert(PathBuf::from("a.md"), "same_hash".into());

        let result = embed_chunks(&provider, &chunks, &existing, &current, 10, None)
            .await
            .unwrap();

        assert!(result.embeddings.is_empty());
        assert_eq!(result.skipped.len(), 2);
        assert_eq!(result.api_calls, 0);
        assert_eq!(provider.call_count(), 0);
    }

    #[tokio::test]
    async fn test_batch_mixed_scenario() {
        let provider = MockProvider::new(64);
        let chunks = vec![
            make_chunk("a.md#0", "a.md", "unchanged"),
            make_chunk("b.md#0", "b.md", "changed content"),
        ];
        let mut existing = HashMap::new();
        existing.insert(PathBuf::from("a.md"), "hash_a".into());
        existing.insert(PathBuf::from("b.md"), "old_hash_b".into());
        let mut current = HashMap::new();
        current.insert(PathBuf::from("a.md"), "hash_a".into());
        current.insert(PathBuf::from("b.md"), "new_hash_b".into());

        let result = embed_chunks(&provider, &chunks, &existing, &current, 10, None)
            .await
            .unwrap();

        assert_eq!(result.embeddings.len(), 1);
        assert_eq!(result.skipped.len(), 1);
        assert!(result.embeddings.contains_key("b.md#0"));
        assert!(result.skipped.contains(&"a.md#0".to_string()));
    }

    #[tokio::test]
    async fn test_batch_size_batching() {
        let provider = MockProvider::new(64);
        let chunks = vec![
            make_chunk("a.md#0", "a.md", "one"),
            make_chunk("a.md#1", "a.md", "two"),
            make_chunk("a.md#2", "a.md", "three"),
            make_chunk("a.md#3", "a.md", "four"),
            make_chunk("a.md#4", "a.md", "five"),
        ];
        let existing: HashMap<PathBuf, String> = HashMap::new();
        let mut current = HashMap::new();
        current.insert(PathBuf::from("a.md"), "hash_a".into());

        let result = embed_chunks(&provider, &chunks, &existing, &current, 2, None)
            .await
            .unwrap();

        assert_eq!(result.embeddings.len(), 5);
        assert_eq!(result.api_calls, 3); // ceil(5/2) = 3
        assert_eq!(provider.call_count(), 3);
    }

    #[tokio::test]
    async fn test_batch_empty_chunks() {
        let provider = MockProvider::new(64);
        let chunks: Vec<Chunk> = vec![];
        let existing: HashMap<PathBuf, String> = HashMap::new();
        let current: HashMap<PathBuf, String> = HashMap::new();

        let result = embed_chunks(&provider, &chunks, &existing, &current, 10, None)
            .await
            .unwrap();

        assert!(result.embeddings.is_empty());
        assert!(result.skipped.is_empty());
        assert_eq!(result.api_calls, 0);
    }
}

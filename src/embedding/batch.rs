use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use super::provider::{EmbeddingProvider, EmbeddingPurpose};

static WORKING_BATCH_SIZES: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();

fn working_batch_size(provider: &dyn EmbeddingProvider, configured: usize) -> usize {
    let cache = WORKING_BATCH_SIZES.get_or_init(|| Mutex::new(HashMap::new()));
    cache
        .lock()
        .ok()
        .and_then(|sizes| sizes.get(&provider.batch_cache_key()).copied())
        .unwrap_or(configured)
        .min(configured)
        .max(1)
}

fn remember_batch_size(provider: &dyn EmbeddingProvider, size: usize) {
    let cache = WORKING_BATCH_SIZES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut sizes) = cache.lock() {
        sizes
            .entry(provider.batch_cache_key())
            .and_modify(|current| *current = (*current).min(size))
            .or_insert(size);
    }
}

pub(crate) async fn embed_inputs_adaptively(
    provider: &dyn EmbeddingProvider,
    texts: Vec<String>,
) -> crate::Result<(Vec<Vec<f32>>, usize, usize)> {
    if texts.is_empty() {
        return Ok((Vec::new(), 0, 0));
    }
    let cached_batch_size = working_batch_size(provider, texts.len());
    let mut queue = texts
        .chunks(cached_batch_size)
        .map(<[String]>::to_vec)
        .collect::<VecDeque<_>>();
    let mut embeddings = Vec::new();
    let mut api_calls = 0;
    let mut estimated_input_tokens = 0;

    while let Some(texts) = queue.pop_front() {
        api_calls += 1;
        match provider
            .embed_batch_for(&texts, EmbeddingPurpose::Document)
            .await
        {
            Ok(vectors) => {
                estimated_input_tokens += texts
                    .iter()
                    .map(|text| crate::chunker::count_tokens(text))
                    .sum::<usize>();
                embeddings.extend(vectors);
            }
            Err(error) if texts.len() > 1 && provider.is_batch_size_error(&error) => {
                let midpoint = texts.len() / 2;
                remember_batch_size(provider, texts.len().div_ceil(2));
                let right_texts = texts[midpoint..].to_vec();
                let left_texts = texts[..midpoint].to_vec();
                // Push right first so the left half is processed first and
                // input ordering remains stable.
                queue.push_front(right_texts);
                queue.push_front(left_texts);
            }
            Err(error) => return Err(error),
        }
    }
    Ok((embeddings, api_calls, estimated_input_tokens))
}

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
#[derive(Debug, Default, Serialize)]
pub struct EmbeddingResult {
    /// Map from chunk ID to its embedding vector.
    pub embeddings: HashMap<String, Vec<f32>>,
    /// Chunk IDs that were skipped (unchanged content).
    pub skipped: Vec<String>,
    /// Number of API calls made to the embedding provider.
    pub api_calls: usize,
    /// Provider-independent local count of inputs successfully embedded.
    pub estimated_input_tokens: usize,
}

/// Monotonic progress emitted after each logical embedding batch completes.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct EmbeddingBatchProgress {
    pub completed_batches: usize,
    pub total_batches: usize,
    pub completed_chunks: usize,
    pub total_chunks: usize,
    pub estimated_input_tokens: usize,
    pub total_estimated_input_tokens: usize,
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
    on_batch: Option<&(dyn Fn(&EmbeddingBatchProgress) + Send + Sync)>,
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
            estimated_input_tokens: 0,
        });
    }

    // Split into batches, reusing the smallest provider/model size that worked
    // after a size-limit response earlier in this process.
    let effective_batch_size = working_batch_size(provider, batch_size);
    let batches: Vec<Vec<&Chunk>> = to_embed
        .chunks(effective_batch_size)
        .map(|b| b.to_vec())
        .collect();
    let total_batches = batches.len();
    let total_chunks = to_embed.len();
    let total_estimated_input_tokens = to_embed
        .iter()
        .map(|chunk| crate::chunker::count_tokens(&chunk.content))
        .sum();
    if let Some(cb) = &on_batch {
        cb(&EmbeddingBatchProgress {
            total_batches,
            total_chunks,
            total_estimated_input_tokens,
            ..EmbeddingBatchProgress::default()
        });
    }
    tracing::info!(
        chunks = to_embed.len(),
        batches = total_batches,
        batch_size = effective_batch_size,
        "embedding chunks"
    );

    // Process batches concurrently (up to 4 at a time).
    use futures::stream::{self, StreamExt};

    const MAX_CONCURRENT: usize = 4;

    type BatchResult = crate::Result<(usize, Vec<(String, Vec<f32>)>, usize, usize)>;
    let mut stream = stream::iter(batches.into_iter().enumerate().map(|(batch_idx, batch)| {
        let chunk_ids: Vec<String> = batch.iter().map(|c| c.id.clone()).collect();
        let texts: Vec<String> = batch.iter().map(|c| c.content.clone()).collect();
        async move {
            let (vectors, api_calls, estimated_input_tokens) =
                embed_inputs_adaptively(provider, texts).await?;
            let pairs = chunk_ids.into_iter().zip(vectors).collect();
            tracing::info!(
                batch = batch_idx + 1,
                total = total_batches,
                "batch complete"
            );
            let result: BatchResult = Ok((batch_idx, pairs, api_calls, estimated_input_tokens));
            result
        }
    }))
    .buffer_unordered(MAX_CONCURRENT);

    let mut embeddings: HashMap<String, Vec<f32>> = HashMap::new();
    let mut api_calls: usize = 0;
    let mut completed_count: usize = 0;
    let mut completed_chunks: usize = 0;
    let mut estimated_input_tokens: usize = 0;

    while let Some(result) = stream.next().await {
        let (_batch_idx, pairs, batch_api_calls, batch_estimated_input_tokens) = result?;
        api_calls += batch_api_calls;
        completed_count += 1;
        completed_chunks += pairs.len();
        estimated_input_tokens += batch_estimated_input_tokens;
        for (id, vector) in pairs {
            embeddings.insert(id, vector);
        }
        if let Some(cb) = &on_batch {
            cb(&EmbeddingBatchProgress {
                completed_batches: completed_count,
                total_batches,
                completed_chunks,
                total_chunks,
                estimated_input_tokens,
                total_estimated_input_tokens,
                api_calls,
            });
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
        estimated_input_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::mock::MockProvider;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct LimitedBatchProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl EmbeddingProvider for LimitedBatchProvider {
        async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if texts.len() > 2 {
                return Err(crate::Error::EmbeddingProvider(
                    "413 payload too large".into(),
                ));
            }
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }

        fn dimensions(&self) -> usize {
            2
        }

        fn model(&self) -> &str {
            "adaptive-test-model"
        }

        fn name(&self) -> &str {
            "limited-test-provider"
        }
    }

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

    #[tokio::test]
    async fn adaptively_splits_and_caches_provider_batch_limit() {
        let provider = LimitedBatchProvider {
            calls: AtomicUsize::new(0),
        };
        let chunks = (0..5)
            .map(|index| make_chunk(&format!("a.md#{index}"), "a.md", "text"))
            .collect::<Vec<_>>();
        let existing = HashMap::new();
        let current = HashMap::from([(PathBuf::from("a.md"), "changed".to_string())]);

        let first = embed_chunks(&provider, &chunks, &existing, &current, 5, None)
            .await
            .unwrap();
        assert_eq!(first.embeddings.len(), 5);
        assert!(first.api_calls > 3, "failed oversized calls are counted");

        let calls_before = provider.calls.load(Ordering::SeqCst);
        let second = embed_chunks(&provider, &chunks, &existing, &current, 5, None)
            .await
            .unwrap();
        let second_calls = provider.calls.load(Ordering::SeqCst) - calls_before;
        assert_eq!(second.embeddings.len(), 5);
        assert_eq!(
            second_calls, 3,
            "cached size 2 avoids another oversized call"
        );
    }

    #[tokio::test]
    async fn progress_and_token_accounting_are_monotonic_and_ignore_failed_attempts() {
        struct RetryProvider;

        #[async_trait]
        impl EmbeddingProvider for RetryProvider {
            async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
                if texts.len() > 1 {
                    return Err(crate::Error::EmbeddingProvider(
                        "413 payload too large".into(),
                    ));
                }
                Ok(vec![vec![1.0, 0.0]])
            }

            fn dimensions(&self) -> usize {
                2
            }

            fn model(&self) -> &str {
                "token-retry-test-model"
            }

            fn name(&self) -> &str {
                "token-retry-test-provider"
            }
        }

        let chunks = vec![
            make_chunk("a.md#0", "a.md", "one two three"),
            make_chunk("a.md#1", "a.md", "four five"),
        ];
        let expected_tokens = chunks
            .iter()
            .map(|chunk| crate::chunker::count_tokens(&chunk.content))
            .sum::<usize>();
        let progress = Mutex::new(Vec::<EmbeddingBatchProgress>::new());
        let callback = |event: &EmbeddingBatchProgress| {
            progress.lock().unwrap().push(event.clone());
        };

        let result = embed_chunks(
            &RetryProvider,
            &chunks,
            &HashMap::new(),
            &HashMap::from([(PathBuf::from("a.md"), "changed".to_string())]),
            2,
            Some(&callback),
        )
        .await
        .unwrap();

        assert_eq!(
            result.api_calls, 3,
            "the failed oversized attempt is counted"
        );
        assert_eq!(
            result.estimated_input_tokens, expected_tokens,
            "successfully embedded inputs are counted exactly once"
        );

        let progress = progress.into_inner().unwrap();
        assert_eq!(progress.first().unwrap().completed_chunks, 0);
        assert_eq!(progress.last().unwrap().completed_chunks, 2);
        assert_eq!(
            progress.last().unwrap().estimated_input_tokens,
            expected_tokens
        );
        assert!(progress.windows(2).all(|events| {
            events[0].completed_batches <= events[1].completed_batches
                && events[0].completed_chunks <= events[1].completed_chunks
                && events[0].estimated_input_tokens <= events[1].estimated_input_tokens
                && events[0].api_calls <= events[1].api_calls
        }));
    }
}

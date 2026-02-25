use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::provider::EmbeddingProvider;

/// A mock embedding provider for deterministic testing.
///
/// Generates vectors by hashing input text with SHA-256 and using the
/// resulting bytes as f32 values. Tracks how many times `embed_batch`
/// has been called.
pub struct MockProvider {
    dimensions: usize,
    call_count: Arc<AtomicUsize>,
}

impl MockProvider {
    /// Create a new mock provider with the given vector dimensions.
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions,
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns the number of times `embed_batch` has been called.
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Generate a deterministic vector from input text using SHA-256.
    fn deterministic_vector(&self, text: &str) -> Vec<f32> {
        let mut result = Vec::with_capacity(self.dimensions);
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        let hash = hasher.finalize();

        // Use hash bytes to fill the vector, cycling through the hash if needed
        for i in 0..self.dimensions {
            let byte_idx = i % hash.len();
            // Normalize byte to [0, 1) range
            result.push(hash[byte_idx] as f32 / 255.0);
        }

        result
    }
}

#[async_trait]
impl EmbeddingProvider for MockProvider {
    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let vectors = texts.iter().map(|t| self.deterministic_vector(t)).collect();
        Ok(vectors)
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn name(&self) -> &str {
        "mock"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_deterministic() {
        let provider = MockProvider::new(128);
        let texts = vec!["hello world".to_string(), "foo bar".to_string()];

        let first = provider.embed_batch(&texts).await.unwrap();
        let second = provider.embed_batch(&texts).await.unwrap();

        assert_eq!(first, second, "same input must produce same vectors");
    }

    #[tokio::test]
    async fn test_mock_call_counting() {
        let provider = MockProvider::new(64);
        assert_eq!(provider.call_count(), 0);

        provider.embed_batch(&["a".into()]).await.unwrap();
        assert_eq!(provider.call_count(), 1);

        provider.embed_batch(&["b".into()]).await.unwrap();
        assert_eq!(provider.call_count(), 2);
    }

    #[tokio::test]
    async fn test_mock_dimensions() {
        for dims in [32, 128, 1536] {
            let provider = MockProvider::new(dims);
            let result = provider.embed_batch(&["test".into()]).await.unwrap();
            assert_eq!(result[0].len(), dims);
            assert_eq!(provider.dimensions(), dims);
        }
    }
}

use async_trait::async_trait;

use super::provider::EmbeddingProvider;

/// Ollama embedding provider.
pub struct OllamaProvider {
    host: String,
    model: String,
    dimensions: usize,
}

impl OllamaProvider {
    /// Create a new Ollama embedding provider.
    pub fn new(host: String, model: String, dimensions: usize) -> Self {
        Self {
            host,
            model,
            dimensions,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaProvider {
    async fn embed_batch(&self, _texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        todo!("Ollama embed_batch not yet implemented")
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn name(&self) -> &str {
        "ollama"
    }
}

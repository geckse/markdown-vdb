use async_trait::async_trait;

use super::provider::EmbeddingProvider;

/// OpenAI-compatible embedding provider.
pub struct OpenAIProvider {
    api_key: String,
    model: String,
    dimensions: usize,
    endpoint: Option<String>,
}

impl OpenAIProvider {
    /// Create a new OpenAI embedding provider.
    pub fn new(
        api_key: String,
        model: String,
        dimensions: usize,
        endpoint: Option<String>,
    ) -> Self {
        Self {
            api_key,
            model,
            dimensions,
            endpoint,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIProvider {
    async fn embed_batch(&self, _texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        todo!("OpenAI embed_batch not yet implemented")
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn name(&self) -> &str {
        "openai"
    }
}

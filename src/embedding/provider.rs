use async_trait::async_trait;

use crate::config::{Config, EmbeddingProviderType};
use crate::error::Error;

use super::mock::MockProvider;
use super::ollama::OllamaProvider;
use super::openai::OpenAIProvider;

/// Trait for embedding text into vector representations.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a batch of texts, returning one vector per input.
    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>>;

    /// The dimensionality of the embedding vectors produced.
    fn dimensions(&self) -> usize;

    /// Human-readable name for this provider.
    fn name(&self) -> &str;
}

/// Create an embedding provider based on the current configuration.
pub fn create_provider(config: &Config) -> crate::Result<Box<dyn EmbeddingProvider>> {
    match config.embedding_provider {
        EmbeddingProviderType::OpenAI => {
            let api_key = config.openai_api_key.as_ref().ok_or_else(|| {
                Error::EmbeddingProvider("OpenAI provider requires OPENAI_API_KEY to be set".into())
            })?;
            Ok(Box::new(OpenAIProvider::new(
                api_key.clone(),
                config.embedding_model.clone(),
                config.embedding_dimensions,
                config.embedding_endpoint.clone(),
            )))
        }
        EmbeddingProviderType::Ollama => Ok(Box::new(OllamaProvider::new(
            config.ollama_host.clone(),
            config.embedding_model.clone(),
            config.embedding_dimensions,
        ))),
        EmbeddingProviderType::Mock => {
            Ok(Box::new(MockProvider::new(config.embedding_dimensions)))
        }
        EmbeddingProviderType::Custom => {
            let endpoint = config.embedding_endpoint.as_ref().ok_or_else(|| {
                Error::EmbeddingProvider(
                    "Custom provider requires MDVDB_EMBEDDING_ENDPOINT to be set".into(),
                )
            })?;
            Ok(Box::new(OpenAIProvider::new(
                config.openai_api_key.clone().unwrap_or_default(),
                config.embedding_model.clone(),
                config.embedding_dimensions,
                Some(endpoint.clone()),
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn base_config() -> Config {
        Config {
            embedding_provider: EmbeddingProviderType::OpenAI,
            embedding_model: "text-embedding-3-small".into(),
            embedding_dimensions: 1536,
            embedding_batch_size: 100,
            openai_api_key: Some("sk-test-key".into()),
            ollama_host: "http://localhost:11434".into(),
            embedding_endpoint: None,
            source_dirs: vec![PathBuf::from(".")],
            index_file: PathBuf::from(".markdownvdb.index"),
            ignore_patterns: vec![],
            watch_enabled: true,
            watch_debounce_ms: 300,
            chunk_max_tokens: 512,
            chunk_overlap_tokens: 50,
            clustering_enabled: true,
            clustering_rebalance_threshold: 50,
            search_default_limit: 10,
            search_min_score: 0.0,
        }
    }

    #[test]
    fn test_create_provider_openai() {
        let config = base_config();
        let provider = create_provider(&config).unwrap();
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.dimensions(), 1536);
    }

    #[test]
    fn test_create_provider_ollama() {
        let mut config = base_config();
        config.embedding_provider = EmbeddingProviderType::Ollama;
        let provider = create_provider(&config).unwrap();
        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.dimensions(), 1536);
    }

    #[test]
    fn test_create_provider_missing_key() {
        let mut config = base_config();
        config.openai_api_key = None;
        let result = create_provider(&config);
        let err = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error for missing API key"),
        };
        assert!(err.contains("OPENAI_API_KEY"));
    }
}

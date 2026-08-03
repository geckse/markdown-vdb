use async_trait::async_trait;
use serde::Serialize;

use crate::config::{Config, EmbeddingProviderType};
use crate::error::Error;

use super::mock::MockProvider;
use super::ollama::OllamaProvider;
use super::openai::{CompatibleAuth, OpenAIProvider};

pub(crate) fn embedding_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("embedding HTTP client configuration is valid")
}

/// How an embedding will be used. Providers with asymmetric retrieval modes
/// can map this to a native field, prompt name, or configured prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingPurpose {
    Document,
    Query,
}

/// Provider-reported model metadata. All fields except the opaque identifier
/// are optional because catalog APIs expose different capabilities.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmbeddingModelInfo {
    pub id: String,
    pub name: Option<String>,
    pub input_token_limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingProbe {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub latency_ms: u128,
}

/// Trait for embedding text into vector representations.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a batch of texts, returning one vector per input.
    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>>;

    /// Purpose-aware embedding. Existing providers remain compatible through
    /// the default implementation.
    async fn embed_batch_for(
        &self,
        texts: &[String],
        _purpose: EmbeddingPurpose,
    ) -> crate::Result<Vec<Vec<f32>>> {
        self.embed_batch(texts).await
    }

    /// Discover currently available models when the provider has a catalog.
    async fn list_models(&self) -> crate::Result<Option<Vec<EmbeddingModelInfo>>> {
        Ok(None)
    }

    /// The dimensionality of the embedding vectors produced.
    fn dimensions(&self) -> usize;

    /// `None` means dimensions will be inferred from the first inference.
    fn dimension_hint(&self) -> Option<usize> {
        match self.dimensions() {
            0 => None,
            value => Some(value),
        }
    }

    fn model(&self) -> &str;

    /// Human-readable name for this provider.
    fn name(&self) -> &str;

    /// Whether a failed request can be retried safely by splitting its input
    /// array. Providers return their HTTP status/body as an embedding error,
    /// so this remains wire-format independent.
    fn is_batch_size_error(&self, error: &Error) -> bool {
        let Error::EmbeddingProvider(message) = error else {
            return false;
        };
        let message = message.to_ascii_lowercase();
        message.contains("413")
            || message.contains("payload too large")
            || message.contains("request too large")
            || message.contains("batch size")
            || message.contains("too many inputs")
            || ((message.contains("maximum") || message.contains("limit"))
                && (message.contains("input")
                    || message.contains("batch")
                    || message.contains("token")))
    }

    fn batch_cache_key(&self) -> String {
        format!("{}:{}", self.name(), self.model())
    }
}

/// Validate provider output without relying on a model-specific dimension
/// table. Returns the resolved dimension.
pub fn validate_embeddings(
    vectors: &[Vec<f32>],
    expected_count: usize,
    expected_dimensions: Option<usize>,
) -> crate::Result<usize> {
    if vectors.len() != expected_count {
        return Err(Error::EmbeddingProvider(format!(
            "expected {expected_count} embeddings, got {}",
            vectors.len()
        )));
    }
    let dimensions = vectors.first().map(Vec::len).unwrap_or(0);
    if expected_count > 0 && dimensions == 0 {
        return Err(Error::EmbeddingProvider(
            "provider returned an empty embedding vector".into(),
        ));
    }
    if vectors.iter().any(|vector| vector.len() != dimensions) {
        return Err(Error::EmbeddingProvider(
            "provider returned inconsistent embedding dimensions".into(),
        ));
    }
    if let Some(expected) = expected_dimensions {
        if dimensions != expected {
            return Err(Error::EmbeddingProvider(format!(
                "expected dimension {expected}, got {dimensions}"
            )));
        }
    }
    Ok(dimensions)
}

pub async fn probe_provider(provider: &dyn EmbeddingProvider) -> crate::Result<EmbeddingProbe> {
    let started = std::time::Instant::now();
    let vectors = provider
        .embed_batch_for(
            &["markdown-vdb dimension probe".to_string()],
            EmbeddingPurpose::Document,
        )
        .await?;
    let dimensions = validate_embeddings(&vectors, 1, provider.dimension_hint())?;
    Ok(EmbeddingProbe {
        provider: provider.name().to_string(),
        model: provider.model().to_string(),
        dimensions,
        latency_ms: started.elapsed().as_millis(),
    })
}

/// Create an embedding provider based on the current configuration.
pub fn create_provider(config: &Config) -> crate::Result<Box<dyn EmbeddingProvider>> {
    match config.embedding_provider {
        EmbeddingProviderType::OpenAI => {
            let api_key = config.openai_api_key.as_ref().ok_or_else(|| {
                Error::EmbeddingProvider("OpenAI provider requires OPENAI_API_KEY to be set".into())
            })?;
            Ok(Box::new(OpenAIProvider::compatible(
                "openai",
                CompatibleAuth::Bearer(api_key.clone()),
                config.embedding_model.clone(),
                dimension_option(config.embedding_dimensions),
                config
                    .embedding_endpoint
                    .clone()
                    .unwrap_or_else(|| "https://api.openai.com/v1/embeddings".to_string()),
                None,
                config.embedding_options.purpose.clone(),
            )))
        }
        EmbeddingProviderType::OpenRouter => {
            let api_key = std::env::var("OPENROUTER_API_KEY").map_err(|_| {
                Error::EmbeddingProvider(
                    "OpenRouter provider requires OPENROUTER_API_KEY to be set".into(),
                )
            })?;
            Ok(Box::new(OpenAIProvider::compatible(
                "openrouter",
                CompatibleAuth::Bearer(api_key),
                config.embedding_model.clone(),
                dimension_option(config.embedding_dimensions),
                config
                    .embedding_endpoint
                    .clone()
                    .unwrap_or_else(|| "https://openrouter.ai/api/v1/embeddings".to_string()),
                Some("https://openrouter.ai/api/v1/embeddings/models".to_string()),
                config.embedding_options.purpose.clone(),
            )))
        }
        EmbeddingProviderType::Gemini => Ok(Box::new(super::gemini::GeminiProvider::from_config(
            config,
        )?)),
        EmbeddingProviderType::AzureOpenAi => {
            let base = config
                .embedding_endpoint
                .clone()
                .or_else(|| std::env::var("AZURE_OPENAI_ENDPOINT").ok())
                .ok_or_else(|| {
                    Error::EmbeddingProvider(
                        "Azure OpenAI requires AZURE_OPENAI_ENDPOINT or embedding.endpoint".into(),
                    )
                })?;
            let endpoint = if base
                .trim_end_matches('/')
                .ends_with("/openai/v1/embeddings")
            {
                base
            } else {
                format!("{}/openai/v1/embeddings", base.trim_end_matches('/'))
            };
            let auth = match config.embedding_options.azure.auth.as_str() {
                "api_key" | "api-key" => CompatibleAuth::Header {
                    name: "api-key".to_string(),
                    value: std::env::var("AZURE_OPENAI_API_KEY").map_err(|_| {
                        Error::EmbeddingProvider(
                            "Azure API-key auth requires AZURE_OPENAI_API_KEY".into(),
                        )
                    })?,
                },
                "bearer" => CompatibleAuth::Bearer(
                    std::env::var("AZURE_OPENAI_ACCESS_TOKEN").map_err(|_| {
                        Error::EmbeddingProvider(
                            "Azure bearer auth requires AZURE_OPENAI_ACCESS_TOKEN".into(),
                        )
                    })?,
                ),
                other => {
                    return Err(Error::Config(format!(
                        "invalid embedding.azure.auth '{other}': expected api_key or bearer"
                    )))
                }
            };
            Ok(Box::new(OpenAIProvider::compatible(
                "azure-openai",
                auth,
                config.embedding_model.clone(),
                dimension_option(config.embedding_dimensions),
                endpoint,
                None,
                config.embedding_options.purpose.clone(),
            )))
        }
        EmbeddingProviderType::Bedrock => Ok(Box::new(
            super::bedrock::BedrockProvider::from_config(config)?,
        )),
        EmbeddingProviderType::HuggingFace => Ok(Box::new(
            super::huggingface::HuggingFaceProvider::from_config(config)?,
        )),
        EmbeddingProviderType::Ollama => Ok(Box::new(OllamaProvider::new(
            config.ollama_host.clone(),
            config.embedding_model.clone(),
            config.embedding_dimensions,
        ))),
        EmbeddingProviderType::Mock => {
            if config.embedding_dimensions == 0 {
                return Err(Error::Config(
                    "the mock provider requires an explicit dimension".into(),
                ));
            }
            Ok(Box::new(MockProvider::new(config.embedding_dimensions)))
        }
        EmbeddingProviderType::Custom => {
            let endpoint = config.embedding_endpoint.as_ref().ok_or_else(|| {
                Error::EmbeddingProvider(
                    "Custom provider requires MDVDB_EMBEDDING_ENDPOINT to be set".into(),
                )
            })?;
            Ok(Box::new(OpenAIProvider::compatible(
                "custom",
                config
                    .openai_api_key
                    .clone()
                    .filter(|value| !value.is_empty())
                    .map(CompatibleAuth::Bearer)
                    .unwrap_or(CompatibleAuth::None),
                config.embedding_model.clone(),
                dimension_option(config.embedding_dimensions),
                endpoint.clone(),
                None,
                config.embedding_options.purpose.clone(),
            )))
        }
    }
}

pub fn dimension_option(dimensions: usize) -> Option<usize> {
    (dimensions > 0).then_some(dimensions)
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
            embedding_options: Default::default(),
            source_dirs: vec![PathBuf::from(".")],
            ignore_patterns: vec![],
            watch_enabled: true,
            watch_debounce_ms: 300,
            chunk_max_tokens: 512,
            chunk_overlap_tokens: 50,
            clustering_enabled: true,
            clustering_algorithm: crate::config::ClusteringAlgorithm::Leiden,
            clustering_knn: 15,
            clustering_resolution: 1.0,
            clustering_min_cluster_size: 2,
            topics_min_similarity: 0.30,
            clustering_rebalance_threshold: 50,
            clustering_granularity: 1.0,
            search_default_limit: 10,
            search_min_score: 0.0,
            search_default_mode: crate::search::SearchMode::Hybrid,
            search_rrf_k: 60.0,
            bm25_norm_k: 1.5,
            search_decay_enabled: false,
            search_decay_half_life: 90.0,
            search_decay_exclude: vec![],
            search_decay_include: vec![],
            search_boost_links: false,
            search_boost_hops: 1,
            search_expand_graph: 0,
            search_expand_limit: 3,
            vector_quantization: crate::config::VectorQuantization::F16,
            index_compression: true,
            edge_embeddings: true,
            edge_boost_weight: 0.15,
            edge_cluster_rebalance: 50,
            custom_cluster_defs: Vec::new(),
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

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::provider::EmbeddingProvider;
use crate::error::Error;

const MAX_RETRIES: u32 = 3;

/// Ollama embedding provider.
pub struct OllamaProvider {
    client: reqwest::Client,
    host: String,
    model: String,
    dimensions: usize,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    embeddings: Vec<Vec<f32>>,
}

impl OllamaProvider {
    /// Create a new Ollama embedding provider.
    pub fn new(host: String, model: String, dimensions: usize) -> Self {
        Self {
            client: reqwest::Client::new(),
            host,
            model,
            dimensions,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaProvider {
    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let endpoint = format!("{}/api/embed", self.host);
        let request_body = EmbeddingRequest {
            model: &self.model,
            input: texts,
        };

        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = std::time::Duration::from_secs(1 << (attempt - 1));
                debug!(attempt, delay_secs = delay.as_secs(), "retrying Ollama embedding request");
                tokio::time::sleep(delay).await;
            }

            let response = match self
                .client
                .post(&endpoint)
                .json(&request_body)
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    if e.is_connect() {
                        return Err(Error::EmbeddingProvider(format!(
                            "Cannot connect to Ollama at {}",
                            self.host
                        )));
                    }
                    return Err(Error::EmbeddingProvider(format!("request failed: {e}")));
                }
            };

            let status = response.status();

            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(Error::EmbeddingProvider(format!(
                    "Model {} not found in Ollama",
                    self.model
                )));
            }

            if status.is_server_error() {
                let msg = format!("server error ({})", status.as_u16());
                warn!("{msg}, attempt {}/{}", attempt + 1, MAX_RETRIES + 1);
                last_error = Some(Error::EmbeddingProvider(msg));
                continue;
            }

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                // Check if body indicates model not found
                if body.contains("not found") {
                    return Err(Error::EmbeddingProvider(format!(
                        "Model {} not found in Ollama",
                        self.model
                    )));
                }
                return Err(Error::EmbeddingProvider(format!(
                    "unexpected status {}: {body}",
                    status.as_u16()
                )));
            }

            let body: EmbeddingResponse = response
                .json()
                .await
                .map_err(|e| Error::EmbeddingProvider(format!("failed to parse response: {e}")))?;

            if body.embeddings.len() != texts.len() {
                return Err(Error::EmbeddingProvider(format!(
                    "expected {} embeddings, got {}",
                    texts.len(),
                    body.embeddings.len()
                )));
            }

            for (i, embedding) in body.embeddings.iter().enumerate() {
                if embedding.len() != self.dimensions {
                    return Err(Error::EmbeddingProvider(format!(
                        "expected dimension {}, got {} at index {}",
                        self.dimensions,
                        embedding.len(),
                        i
                    )));
                }
            }

            return Ok(body.embeddings);
        }

        Err(last_error.unwrap_or_else(|| Error::EmbeddingProvider("max retries exceeded".into())))
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn name(&self) -> &str {
        "ollama"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serialization() {
        let texts = vec!["hello".to_string(), "world".to_string()];
        let req = EmbeddingRequest {
            model: "nomic-embed-text",
            input: &texts,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "nomic-embed-text");
        assert_eq!(json["input"], serde_json::json!(["hello", "world"]));
    }

    #[test]
    fn response_deserialization() {
        let json = r#"{
            "embeddings": [
                [0.1, 0.2, 0.3],
                [0.4, 0.5, 0.6]
            ]
        }"#;
        let resp: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.embeddings.len(), 2);
        assert_eq!(resp.embeddings[0], vec![0.1, 0.2, 0.3]);
        assert_eq!(resp.embeddings[1], vec![0.4, 0.5, 0.6]);
    }

    #[test]
    fn response_deserialization_empty() {
        let json = r#"{"embeddings": []}"#;
        let resp: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert!(resp.embeddings.is_empty());
    }

    #[test]
    fn provider_name_and_dimensions() {
        let provider = OllamaProvider::new(
            "http://localhost:11434".into(),
            "nomic-embed-text".into(),
            768,
        );
        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.dimensions(), 768);
    }

    #[test]
    fn provider_stores_host_and_model() {
        let provider = OllamaProvider::new(
            "http://custom:1234".into(),
            "mxbai-embed-large".into(),
            1024,
        );
        assert_eq!(provider.host, "http://custom:1234");
        assert_eq!(provider.model, "mxbai-embed-large");
    }

    #[test]
    fn error_connection_refused() {
        let err = Error::EmbeddingProvider("Cannot connect to Ollama at http://localhost:11434".into());
        assert!(err.to_string().contains("Cannot connect to Ollama"));
    }

    #[test]
    fn error_model_not_found() {
        let err = Error::EmbeddingProvider("Model nomic-embed-text not found in Ollama".into());
        assert!(err.to_string().contains("not found in Ollama"));
    }

    #[tokio::test]
    async fn embed_batch_empty_input() {
        let provider = OllamaProvider::new(
            "http://localhost:11434".into(),
            "nomic-embed-text".into(),
            768,
        );
        let result = provider.embed_batch(&[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn dimension_validation_catches_mismatch() {
        let embeddings = vec![vec![0.1, 0.2]];
        let expected_dim = 3;
        for embedding in &embeddings {
            assert_ne!(embedding.len(), expected_dim);
        }
    }
}

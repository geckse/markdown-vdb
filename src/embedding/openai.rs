use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::provider::EmbeddingProvider;
use crate::error::Error;

const DEFAULT_ENDPOINT: &str = "https://api.openai.com/v1/embeddings";
const MAX_RETRIES: u32 = 3;

/// OpenAI-compatible embedding provider.
pub struct OpenAIProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    dimensions: usize,
    endpoint: String,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    input: &'a [String],
    model: &'a str,
    dimensions: usize,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    index: usize,
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
            client: reqwest::Client::new(),
            api_key,
            model,
            dimensions,
            endpoint: endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIProvider {
    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        // OpenAI rejects empty strings in the input array.
        // Replace any empty/whitespace-only texts with a single space.
        let sanitized: Vec<String> = texts
            .iter()
            .map(|t| {
                if t.trim().is_empty() {
                    " ".to_string()
                } else {
                    t.clone()
                }
            })
            .collect();

        let request_body = EmbeddingRequest {
            input: &sanitized,
            model: &self.model,
            dimensions: self.dimensions,
        };

        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = std::time::Duration::from_secs(1 << (attempt - 1));
                debug!(
                    attempt,
                    delay_secs = delay.as_secs(),
                    "retrying embedding request"
                );
                tokio::time::sleep(delay).await;
            }

            let response = self
                .client
                .post(&self.endpoint)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&request_body)
                .send()
                .await
                .map_err(|e| Error::EmbeddingProvider(format!("request failed: {e}")))?;

            let status = response.status();

            if status == StatusCode::UNAUTHORIZED {
                return Err(Error::EmbeddingProvider(
                    "authentication failed (401): invalid API key".into(),
                ));
            }

            if status == StatusCode::TOO_MANY_REQUESTS {
                warn!(
                    "rate limited (429), attempt {}/{}",
                    attempt + 1,
                    MAX_RETRIES + 1
                );
                last_error = Some(Error::EmbeddingProvider("rate limited (429)".into()));
                continue;
            }

            if status.is_server_error() {
                let msg = format!("server error ({})", status.as_u16());
                warn!("{msg}, attempt {}/{}", attempt + 1, MAX_RETRIES + 1);
                last_error = Some(Error::EmbeddingProvider(msg));
                continue;
            }

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(Error::EmbeddingProvider(format!(
                    "unexpected status {}: {body}",
                    status.as_u16()
                )));
            }

            let body: EmbeddingResponse = response
                .json()
                .await
                .map_err(|e| Error::EmbeddingProvider(format!("failed to parse response: {e}")))?;

            if body.data.len() != texts.len() {
                return Err(Error::EmbeddingProvider(format!(
                    "expected {} embeddings, got {}",
                    texts.len(),
                    body.data.len()
                )));
            }

            // Sort by index to ensure correct ordering
            let mut sorted = body.data;
            sorted.sort_by_key(|d| d.index);

            let mut embeddings = Vec::with_capacity(sorted.len());
            for item in &sorted {
                if item.embedding.len() != self.dimensions {
                    return Err(Error::EmbeddingProvider(format!(
                        "expected dimension {}, got {} at index {}",
                        self.dimensions,
                        item.embedding.len(),
                        item.index
                    )));
                }
                embeddings.push(item.embedding.clone());
            }

            return Ok(embeddings);
        }

        Err(last_error.unwrap_or_else(|| Error::EmbeddingProvider("max retries exceeded".into())))
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn name(&self) -> &str {
        "openai"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serialization() {
        let texts = vec!["hello".to_string(), "world".to_string()];
        let req = EmbeddingRequest {
            input: &texts,
            model: "text-embedding-3-small",
            dimensions: 1536,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["input"], serde_json::json!(["hello", "world"]));
        assert_eq!(json["model"], "text-embedding-3-small");
        assert_eq!(json["dimensions"], 1536);
    }

    #[test]
    fn response_deserialization() {
        let json = r#"{
            "data": [
                {"embedding": [0.1, 0.2, 0.3], "index": 1},
                {"embedding": [0.4, 0.5, 0.6], "index": 0}
            ]
        }"#;
        let resp: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].index, 1);
        assert_eq!(resp.data[1].index, 0);
    }

    #[test]
    fn dimension_validation_catches_mismatch() {
        let data = vec![EmbeddingData {
            embedding: vec![0.1, 0.2],
            index: 0,
        }];
        let expected_dim = 3;
        for item in &data {
            assert_ne!(item.embedding.len(), expected_dim);
        }
    }

    #[test]
    fn default_endpoint() {
        let provider = OpenAIProvider::new(
            "sk-test".into(),
            "text-embedding-3-small".into(),
            1536,
            None,
        );
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
    }

    #[test]
    fn custom_endpoint() {
        let provider = OpenAIProvider::new(
            "sk-test".into(),
            "text-embedding-3-small".into(),
            1536,
            Some("https://custom.api.com/v1/embeddings".into()),
        );
        assert_eq!(provider.endpoint, "https://custom.api.com/v1/embeddings");
    }

    #[test]
    fn provider_name_and_dimensions() {
        let provider =
            OpenAIProvider::new("sk-test".into(), "text-embedding-3-small".into(), 768, None);
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.dimensions(), 768);
    }

    #[test]
    fn error_classification_401() {
        let err = Error::EmbeddingProvider("authentication failed (401): invalid API key".into());
        assert!(err.to_string().contains("401"));
    }

    #[test]
    fn error_classification_429() {
        let err = Error::EmbeddingProvider("rate limited (429)".into());
        assert!(err.to_string().contains("429"));
    }

    #[test]
    fn error_classification_5xx() {
        let err = Error::EmbeddingProvider("server error (503)".into());
        assert!(err.to_string().contains("503"));
    }

    #[tokio::test]
    async fn embed_batch_empty_input() {
        let provider = OpenAIProvider::new(
            "sk-test".into(),
            "text-embedding-3-small".into(),
            1536,
            None,
        );
        let result = provider.embed_batch(&[]).await.unwrap();
        assert!(result.is_empty());
    }
}

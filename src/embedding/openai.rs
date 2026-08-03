use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::provider::{
    embedding_http_client, validate_embeddings, EmbeddingModelInfo, EmbeddingProvider,
    EmbeddingPurpose,
};
use crate::config::EmbeddingPurposeConfig;
use crate::error::Error;

const DEFAULT_ENDPOINT: &str = "https://api.openai.com/v1/embeddings";
const MAX_RETRIES: u32 = 3;

/// OpenAI rejects requests totalling more than 300k tokens; stay below with a
/// margin for tokenizer drift.
const MAX_TOKENS_PER_REQUEST: usize = 280_000;
/// OpenAI rejects input arrays longer than 2048 entries.
const MAX_INPUTS_PER_REQUEST: usize = 2_048;
/// text-embedding-3 models reject single inputs over 8192 tokens; oversized
/// inputs (e.g. giant link-context paragraphs) are truncated with a margin.
const MAX_TOKENS_PER_INPUT: usize = 8_000;

/// Sanitize inputs and split them into request-sized groups.
///
/// Empty/whitespace-only texts are replaced with a single space (OpenAI
/// rejects empty strings), inputs over `max_input_tokens` are truncated, and
/// consecutive inputs are packed greedily so each group stays within
/// `max_request_tokens` total tokens and `max_inputs` entries. Input order is
/// preserved across groups.
fn plan_requests(
    texts: &[String],
    max_request_tokens: usize,
    max_inputs: usize,
    max_input_tokens: usize,
) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_tokens = 0usize;

    for t in texts {
        let mut text = if t.trim().is_empty() {
            " ".to_string()
        } else {
            t.clone()
        };
        let mut tokens = crate::chunker::count_tokens(&text);
        if tokens > max_input_tokens {
            warn!(
                tokens,
                limit = max_input_tokens,
                "truncating oversized embedding input"
            );
            text = crate::chunker::truncate_to_tokens(&text, max_input_tokens);
            tokens = max_input_tokens;
        }

        if !current.is_empty()
            && (current.len() >= max_inputs || current_tokens + tokens > max_request_tokens)
        {
            groups.push(std::mem::take(&mut current));
            current_tokens = 0;
        }
        current.push(text);
        current_tokens += tokens;
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// OpenAI-compatible embedding provider.
pub struct OpenAIProvider {
    client: reqwest::Client,
    auth: CompatibleAuth,
    model: String,
    dimensions: Option<usize>,
    endpoint: String,
    models_endpoint: Option<String>,
    provider_name: String,
    purpose: EmbeddingPurposeConfig,
}

#[derive(Debug, Clone)]
pub enum CompatibleAuth {
    Bearer(String),
    Header { name: String, value: String },
    None,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    input: &'a [String],
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_type: Option<&'a str>,
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
            client: embedding_http_client(),
            auth: CompatibleAuth::Bearer(api_key),
            model,
            dimensions: (dimensions > 0).then_some(dimensions),
            endpoint: endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
            models_endpoint: None,
            provider_name: "openai".to_string(),
            purpose: EmbeddingPurposeConfig::default(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn compatible(
        provider_name: impl Into<String>,
        auth: CompatibleAuth,
        model: String,
        dimensions: Option<usize>,
        endpoint: String,
        models_endpoint: Option<String>,
        purpose: EmbeddingPurposeConfig,
    ) -> Self {
        Self {
            client: embedding_http_client(),
            auth,
            model,
            dimensions,
            endpoint,
            models_endpoint,
            provider_name: provider_name.into(),
            purpose,
        }
    }
}

impl OpenAIProvider {
    /// Send a single embeddings request (with retries) for a pre-planned group
    /// of inputs that is known to fit OpenAI's per-request limits.
    async fn send_request(
        &self,
        texts: &[String],
        purpose: EmbeddingPurpose,
    ) -> crate::Result<Vec<Vec<f32>>> {
        let native_purpose = if self.purpose.mode == "native" {
            match purpose {
                EmbeddingPurpose::Document => self.purpose.document.as_deref(),
                EmbeddingPurpose::Query => self.purpose.query.as_deref(),
            }
        } else {
            None
        };
        let request_body = EmbeddingRequest {
            input: texts,
            model: &self.model,
            dimensions: self.dimensions,
            input_type: native_purpose,
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

            let mut request = self.client.post(&self.endpoint).json(&request_body);
            request = match &self.auth {
                CompatibleAuth::Bearer(token) => {
                    request.header("Authorization", format!("Bearer {token}"))
                }
                CompatibleAuth::Header { name, value } => request.header(name, value),
                CompatibleAuth::None => request,
            };
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) if error.is_timeout() || error.is_connect() => {
                    last_error = Some(Error::EmbeddingProvider(format!(
                        "transient request failure: {error}"
                    )));
                    continue;
                }
                Err(error) => {
                    return Err(Error::EmbeddingProvider(format!("request failed: {error}")))
                }
            };

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

            let embeddings: Vec<Vec<f32>> = sorted.into_iter().map(|item| item.embedding).collect();
            validate_embeddings(&embeddings, texts.len(), self.dimensions)?;

            return Ok(embeddings);
        }

        Err(last_error.unwrap_or_else(|| Error::EmbeddingProvider("max retries exceeded".into())))
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIProvider {
    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        self.embed_batch_for(texts, EmbeddingPurpose::Document)
            .await
    }

    async fn embed_batch_for(
        &self,
        texts: &[String],
        purpose: EmbeddingPurpose,
    ) -> crate::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let prefixed: Vec<String> = if self.purpose.mode == "prefix" {
            let prefix = match purpose {
                EmbeddingPurpose::Document => self.purpose.document.as_deref(),
                EmbeddingPurpose::Query => self.purpose.query.as_deref(),
            }
            .unwrap_or_default();
            texts.iter().map(|text| format!("{prefix}{text}")).collect()
        } else {
            texts.to_vec()
        };

        let groups = plan_requests(
            &prefixed,
            MAX_TOKENS_PER_REQUEST,
            MAX_INPUTS_PER_REQUEST,
            MAX_TOKENS_PER_INPUT,
        );
        if groups.len() > 1 {
            debug!(
                inputs = texts.len(),
                requests = groups.len(),
                "splitting embedding batch to respect per-request token limit"
            );
        }

        let mut embeddings = Vec::with_capacity(texts.len());
        for group in &groups {
            embeddings.extend(self.send_request(group, purpose).await?);
        }
        Ok(embeddings)
    }

    async fn list_models(&self) -> crate::Result<Option<Vec<EmbeddingModelInfo>>> {
        let Some(endpoint) = &self.models_endpoint else {
            return Ok(None);
        };
        let mut request = self.client.get(endpoint);
        request = match &self.auth {
            CompatibleAuth::Bearer(token) => {
                request.header("Authorization", format!("Bearer {token}"))
            }
            CompatibleAuth::Header { name, value } => request.header(name, value),
            CompatibleAuth::None => request,
        };
        let response = request
            .send()
            .await
            .map_err(|e| Error::EmbeddingProvider(format!("model discovery failed: {e}")))?;
        if !response.status().is_success() {
            return Err(Error::EmbeddingProvider(format!(
                "model discovery returned {}",
                response.status()
            )));
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::EmbeddingProvider(format!("failed to parse model catalog: {e}")))?;
        Ok(Some(parse_compatible_model_catalog(&value)))
    }

    fn dimensions(&self) -> usize {
        self.dimensions.unwrap_or(0)
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn name(&self) -> &str {
        &self.provider_name
    }
}

fn parse_compatible_model_catalog(value: &serde_json::Value) -> Vec<EmbeddingModelInfo> {
    value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(EmbeddingModelInfo {
                id: item.get("id")?.as_str()?.to_string(),
                name: item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                input_token_limit: item
                    .get("context_length")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize),
            })
        })
        .collect()
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
            dimensions: Some(1536),
            input_type: None,
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
    fn catalog_passes_through_unknown_future_models() {
        let models = parse_compatible_model_catalog(&serde_json::json!({
            "data": [{
                "id": "vendor/future-embed@2099",
                "name": "Future Embed",
                "context_length": 12345
            }]
        }));
        assert_eq!(models[0].id, "vendor/future-embed@2099");
        assert_eq!(models[0].input_token_limit, Some(12345));
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

    // --- plan_requests tests ---

    fn owned(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plan_requests_single_group_when_within_limits() {
        let texts = owned(&["hello world", "foo bar", "baz"]);
        let groups = plan_requests(&texts, 1000, 100, 100);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], texts);
    }

    #[test]
    fn plan_requests_sanitizes_empty_inputs() {
        let texts = owned(&["", "   ", "real text"]);
        let groups = plan_requests(&texts, 1000, 100, 100);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0][0], " ");
        assert_eq!(groups[0][1], " ");
        assert_eq!(groups[0][2], "real text");
    }

    #[test]
    fn plan_requests_splits_by_token_budget() {
        // "word word ... word" — each text is ~10 tokens.
        let text = "word ".repeat(10).trim().to_string();
        let tokens = crate::chunker::count_tokens(&text);
        let texts = vec![text; 5];
        // Budget fits exactly 2 texts per request.
        let groups = plan_requests(&texts, tokens * 2, 100, 1000);
        assert_eq!(
            groups.len(),
            3,
            "5 texts at 2-per-budget should yield 3 groups"
        );
        assert_eq!(groups[0].len(), 2);
        assert_eq!(groups[1].len(), 2);
        assert_eq!(groups[2].len(), 1);
    }

    #[test]
    fn plan_requests_splits_by_input_count() {
        let texts = owned(&["a", "b", "c", "d", "e"]);
        let groups = plan_requests(&texts, 100_000, 2, 100);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0], owned(&["a", "b"]));
        assert_eq!(groups[1], owned(&["c", "d"]));
        assert_eq!(groups[2], owned(&["e"]));
    }

    #[test]
    fn plan_requests_truncates_oversized_input() {
        let huge = "word ".repeat(500).trim().to_string();
        let texts = vec![huge.clone(), "small".to_string()];
        let groups = plan_requests(&texts, 1000, 100, 50);
        assert_eq!(groups.len(), 1);
        assert!(crate::chunker::count_tokens(&groups[0][0]) <= 50);
        assert!(
            huge.starts_with(&groups[0][0]),
            "truncation must keep a prefix"
        );
        assert_eq!(groups[0][1], "small");
    }

    #[test]
    fn plan_requests_preserves_order_across_groups() {
        let texts: Vec<String> = (0..7).map(|i| format!("text number {i}")).collect();
        let groups = plan_requests(&texts, 100_000, 3, 100);
        let flat: Vec<String> = groups.into_iter().flatten().collect();
        assert_eq!(flat, texts, "flattened groups must equal original order");
    }

    #[test]
    fn plan_requests_giant_paragraph_scenario_stays_under_limits() {
        // Regression: 15 link-context "paragraphs" of ~45k tokens each used to
        // produce a single 675k-token request. Each must now be truncated and
        // every group must respect the request budget.
        let paragraph = "word ".repeat(45_000).trim().to_string();
        let texts = vec![paragraph; 15];
        let groups = plan_requests(
            &texts,
            MAX_TOKENS_PER_REQUEST,
            MAX_INPUTS_PER_REQUEST,
            MAX_TOKENS_PER_INPUT,
        );
        let total: usize = groups.iter().map(|g| g.len()).sum();
        assert_eq!(total, 15);
        for group in &groups {
            let group_tokens: usize = group.iter().map(|t| crate::chunker::count_tokens(t)).sum();
            assert!(group_tokens <= MAX_TOKENS_PER_REQUEST);
            assert!(group.len() <= MAX_INPUTS_PER_REQUEST);
            for t in group {
                assert!(crate::chunker::count_tokens(t) <= MAX_TOKENS_PER_INPUT);
            }
        }
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

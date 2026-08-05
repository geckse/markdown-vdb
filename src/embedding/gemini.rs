use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::{json, Value};

use super::provider::{
    describe_request_error, dimension_option, embedding_http_client, validate_embeddings,
    EmbeddingModelInfo, EmbeddingProvider, EmbeddingPurpose,
};
use crate::config::{Config, EmbeddingPurposeConfig};
use crate::error::Error;

const API_ROOT: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    dimensions: Option<usize>,
    endpoint: String,
    purpose: EmbeddingPurposeConfig,
}

impl GeminiProvider {
    pub fn from_config(config: &Config) -> crate::Result<Self> {
        let api_key = std::env::var("GEMINI_API_KEY").map_err(|_| {
            Error::EmbeddingProvider("Gemini provider requires GEMINI_API_KEY to be set".into())
        })?;
        let model_path = config.embedding_model.trim_start_matches("models/");
        let endpoint = config.embedding_endpoint.clone().unwrap_or_else(|| {
            format!(
                "{API_ROOT}/models/{}:batchEmbedContents",
                encode_model_path(model_path)
            )
        });
        Ok(Self {
            client: embedding_http_client(),
            api_key,
            model: config.embedding_model.clone(),
            dimensions: dimension_option(config.embedding_dimensions),
            endpoint,
            purpose: config.embedding_options.purpose.clone(),
        })
    }

    fn purpose_value(&self, purpose: EmbeddingPurpose) -> Option<&str> {
        match purpose {
            EmbeddingPurpose::Document => self.purpose.document.as_deref(),
            EmbeddingPurpose::Query => self.purpose.query.as_deref(),
        }
    }

    async fn send(
        &self,
        texts: &[String],
        purpose: EmbeddingPurpose,
    ) -> crate::Result<Vec<Vec<f32>>> {
        let prefix = (self.purpose.mode == "prefix")
            .then(|| self.purpose_value(purpose))
            .flatten()
            .unwrap_or_default();
        let task_type = (self.purpose.mode == "native")
            .then(|| self.purpose_value(purpose))
            .flatten();
        let model_resource = if self.model.starts_with("models/") {
            self.model.clone()
        } else {
            format!("models/{}", self.model)
        };

        let requests: Vec<Value> = texts
            .iter()
            .map(|text| {
                build_embed_request(
                    &model_resource,
                    &format!("{prefix}{text}"),
                    task_type,
                    self.dimensions,
                )
            })
            .collect();

        let mut last_error = None;
        for attempt in 0..=3u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(250 * (1 << attempt))).await;
            }
            let response = self
                .client
                .post(&self.endpoint)
                .header("x-goog-api-key", &self.api_key)
                .json(&json!({"requests": requests}))
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) if error.is_timeout() || error.is_connect() => {
                    last_error = Some(format!("Gemini request failed: {error}"));
                    continue;
                }
                Err(error) => {
                    return Err(Error::EmbeddingProvider(format!(
                        "Gemini request failed: {error}"
                    )))
                }
            };
            if response.status() == StatusCode::TOO_MANY_REQUESTS
                || response.status().is_server_error()
            {
                last_error = Some(format!("Gemini returned {}", response.status()));
                continue;
            }
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(Error::EmbeddingProvider(format!(
                    "Gemini returned {}: {body}",
                    status.as_u16()
                )));
            }
            let value: Value = match response.json().await {
                Ok(value) => value,
                Err(error) if error.is_timeout() => {
                    last_error = Some(format!(
                        "transient response read failure: {}",
                        describe_request_error(&error)
                    ));
                    continue;
                }
                Err(error) => {
                    return Err(Error::EmbeddingProvider(format!(
                        "failed to parse Gemini response: {}",
                        describe_request_error(&error)
                    )))
                }
            };
            let vectors: Vec<Vec<f32>> = value["embeddings"]
                .as_array()
                .ok_or_else(|| {
                    Error::EmbeddingProvider("Gemini response has no embeddings array".into())
                })?
                .iter()
                .map(|embedding| parse_vector(&embedding["values"]))
                .collect::<crate::Result<_>>()?;
            validate_embeddings(&vectors, texts.len(), self.dimensions)?;
            return Ok(vectors);
        }
        Err(Error::EmbeddingProvider(
            last_error.unwrap_or_else(|| "Gemini retries exhausted".to_string()),
        ))
    }
}

#[async_trait]
impl EmbeddingProvider for GeminiProvider {
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
            return Ok(Vec::new());
        }
        self.send(texts, purpose).await
    }

    async fn list_models(&self) -> crate::Result<Option<Vec<EmbeddingModelInfo>>> {
        let mut models = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut request = self
                .client
                .get(format!("{API_ROOT}/models"))
                .header("x-goog-api-key", &self.api_key)
                .query(&[("pageSize", "1000")]);
            if let Some(token) = &page_token {
                request = request.query(&[("pageToken", token)]);
            }
            let response = request.send().await.map_err(|e| {
                Error::EmbeddingProvider(format!("Gemini model discovery failed: {e}"))
            })?;
            if !response.status().is_success() {
                return Err(Error::EmbeddingProvider(format!(
                    "Gemini model discovery returned {}",
                    response.status()
                )));
            }
            let value: Value = response.json().await.map_err(|e| {
                Error::EmbeddingProvider(format!(
                    "failed to parse Gemini model catalog: {}",
                    describe_request_error(&e)
                ))
            })?;
            models.extend(parse_model_page(&value));
            page_token = value["nextPageToken"].as_str().map(str::to_string);
            if page_token.is_none() {
                break;
            }
        }
        Ok(Some(models))
    }

    fn dimensions(&self) -> usize {
        self.dimensions.unwrap_or(0)
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn name(&self) -> &str {
        "gemini"
    }
}

fn parse_model_page(value: &Value) -> Vec<EmbeddingModelInfo> {
    value["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| {
            item["supportedGenerationMethods"]
                .as_array()
                .is_some_and(|methods| {
                    methods.iter().any(|method| {
                        matches!(method.as_str(), Some("embedContent" | "batchEmbedContents"))
                    })
                })
        })
        .filter_map(|item| {
            let id = item["name"].as_str()?;
            Some(EmbeddingModelInfo {
                id: id.trim_start_matches("models/").to_string(),
                name: item["displayName"].as_str().map(str::to_string),
                input_token_limit: item["inputTokenLimit"].as_u64().map(|value| value as usize),
            })
        })
        .collect()
}

fn parse_vector(value: &Value) -> crate::Result<Vec<f32>> {
    value
        .as_array()
        .ok_or_else(|| Error::EmbeddingProvider("embedding is not a float array".into()))?
        .iter()
        .map(|number| {
            number
                .as_f64()
                .map(|value| value as f32)
                .ok_or_else(|| Error::EmbeddingProvider("embedding contains a non-number".into()))
        })
        .collect()
}

fn build_embed_request(
    model_resource: &str,
    text: &str,
    task_type: Option<&str>,
    dimensions: Option<usize>,
) -> Value {
    let mut request = json!({
        "model": model_resource,
        "content": {"parts": [{"text": text}]}
    });
    let mut embed_config = serde_json::Map::new();
    if let Some(task_type) = task_type {
        embed_config.insert("taskType".into(), Value::String(task_type.to_string()));
    }
    if let Some(dimensions) = dimensions {
        embed_config.insert("outputDimensionality".into(), json!(dimensions));
    }
    if !embed_config.is_empty() {
        request["embedContentConfig"] = Value::Object(embed_config);
    }
    request
}

fn encode_model_path(model: &str) -> String {
    model
        .split('/')
        .map(percent_encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_encode_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ids_are_opaque_path_data() {
        assert_eq!(
            encode_model_path("vendor/new model:v1"),
            "vendor/new%20model%3Av1"
        );
    }

    #[test]
    fn parses_float_vector() {
        assert_eq!(parse_vector(&json!([0.1, 0.2])).unwrap().len(), 2);
    }

    #[test]
    fn uses_current_model_independent_embed_config_shape() {
        let request = build_embed_request(
            "models/future-embed",
            "query text",
            Some("RETRIEVAL_QUERY"),
            Some(768),
        );
        assert_eq!(
            request["embedContentConfig"],
            json!({"taskType": "RETRIEVAL_QUERY", "outputDimensionality": 768})
        );
        assert!(request.get("taskType").is_none());
        assert!(request.get("outputDimensionality").is_none());
    }

    #[test]
    fn catalog_includes_unknown_future_embedding_models() {
        let models = parse_model_page(&json!({
            "models": [
                {
                    "name": "models/future-embed-2099",
                    "supportedGenerationMethods": ["embedContent"],
                    "inputTokenLimit": 9999
                },
                {
                    "name": "models/chat-only",
                    "supportedGenerationMethods": ["generateContent"]
                }
            ]
        }));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "future-embed-2099");
    }
}

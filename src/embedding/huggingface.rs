use async_trait::async_trait;
use reqwest::{RequestBuilder, StatusCode};
use serde_json::{json, Value};

use super::provider::{
    describe_request_error, dimension_option, embedding_http_client, validate_embeddings,
    EmbeddingModelInfo, EmbeddingProvider, EmbeddingPurpose,
};
use crate::config::{Config, HuggingFaceEmbeddingConfig};
use crate::error::Error;

pub struct HuggingFaceProvider {
    client: reqwest::Client,
    token: Option<String>,
    model: String,
    dimensions: Option<usize>,
    endpoint: String,
    serverless: bool,
    options: HuggingFaceEmbeddingConfig,
}

impl HuggingFaceProvider {
    pub fn from_config(config: &Config) -> crate::Result<Self> {
        let options = config.embedding_options.huggingface.clone();
        let serverless = match options.mode.as_str() {
            "serverless" => true,
            "endpoint" => false,
            other => {
                return Err(Error::Config(format!(
                    "invalid embedding.huggingface.mode '{other}': expected serverless or endpoint"
                )))
            }
        };
        let token = std::env::var("HF_TOKEN").ok().filter(|v| !v.is_empty());
        if serverless && token.is_none() {
            return Err(Error::EmbeddingProvider(
                "Hugging Face serverless mode requires HF_TOKEN".into(),
            ));
        }
        let endpoint = if serverless {
            serverless_endpoint(&config.embedding_model)
        } else {
            options
                .endpoint
                .clone()
                .or_else(|| config.embedding_endpoint.clone())
                .ok_or_else(|| {
                    Error::EmbeddingProvider(
                        "Hugging Face endpoint mode requires embedding.huggingface.endpoint".into(),
                    )
                })?
        };
        Ok(Self {
            client: embedding_http_client(),
            token,
            model: config.embedding_model.clone(),
            dimensions: dimension_option(config.embedding_dimensions),
            endpoint,
            serverless,
            options,
        })
    }

    fn authenticate(&self, request: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(token) => request.header("Authorization", format!("Bearer {token}")),
            None => request,
        }
    }

    fn prompt_name(&self, purpose: EmbeddingPurpose) -> Option<&str> {
        match purpose {
            EmbeddingPurpose::Document => self.options.document_prompt_name.as_deref(),
            EmbeddingPurpose::Query => self.options.query_prompt_name.as_deref(),
        }
    }

    async fn send(
        &self,
        texts: &[String],
        purpose: EmbeddingPurpose,
    ) -> crate::Result<Vec<Vec<f32>>> {
        let mut body = json!({
            "inputs": texts,
            "normalize": self.options.normalize,
            "truncate": self.options.truncate,
            "truncation_direction": self.options.truncation_direction,
        });
        if let Some(prompt_name) = self.prompt_name(purpose) {
            body["prompt_name"] = Value::String(prompt_name.to_string());
        }

        let mut last_error = None;
        for attempt in 0..=3u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(250 * (1 << attempt))).await;
            }
            let response = self
                .authenticate(self.client.post(&self.endpoint).json(&body))
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) if error.is_timeout() || error.is_connect() => {
                    last_error = Some(format!("Hugging Face request failed: {error}"));
                    continue;
                }
                Err(error) => {
                    return Err(Error::EmbeddingProvider(format!(
                        "Hugging Face request failed: {error}"
                    )))
                }
            };
            if response.status() == StatusCode::TOO_MANY_REQUESTS
                || response.status().is_server_error()
            {
                last_error = Some(format!("Hugging Face returned {}", response.status()));
                continue;
            }
            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(Error::EmbeddingProvider(format!(
                    "Hugging Face returned {}: {text}",
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
                        "failed to parse Hugging Face response: {}",
                        describe_request_error(&error)
                    )))
                }
            };
            let vectors = parse_embeddings(&value, texts.len())?;
            validate_embeddings(&vectors, texts.len(), self.dimensions)?;
            return Ok(vectors);
        }
        Err(Error::EmbeddingProvider(last_error.unwrap_or_else(|| {
            "Hugging Face retries exhausted".to_string()
        })))
    }
}

#[async_trait]
impl EmbeddingProvider for HuggingFaceProvider {
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
        if !self.serverless {
            return Ok(None);
        }
        let response = self
            .authenticate(
                self.client
                    .get("https://huggingface.co/api/models")
                    .query(&[
                        ("inference_provider", "hf-inference"),
                        ("pipeline_tag", "feature-extraction"),
                        ("limit", "1000"),
                    ]),
            )
            .send()
            .await
            .map_err(|e| {
                Error::EmbeddingProvider(format!("Hugging Face model discovery failed: {e}"))
            })?;
        if !response.status().is_success() {
            return Err(Error::EmbeddingProvider(format!(
                "Hugging Face model discovery returned {}",
                response.status()
            )));
        }
        let values: Vec<Value> = response.json().await.map_err(|e| {
            Error::EmbeddingProvider(format!(
                "failed to parse Hugging Face model catalog: {}",
                describe_request_error(&e)
            ))
        })?;
        let models = values
            .into_iter()
            .filter_map(|value| {
                let id = value.get("id")?.as_str()?.to_string();
                Some(EmbeddingModelInfo {
                    name: value
                        .get("modelId")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    id,
                    input_token_limit: None,
                })
            })
            .collect();
        Ok(Some(models))
    }

    fn dimensions(&self) -> usize {
        self.dimensions.unwrap_or(0)
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn name(&self) -> &str {
        "huggingface"
    }
}

fn parse_embeddings(value: &Value, input_count: usize) -> crate::Result<Vec<Vec<f32>>> {
    let array = value
        .as_array()
        .ok_or_else(|| Error::EmbeddingProvider("Hugging Face response is not an array".into()))?;
    if input_count == 1 && array.iter().all(Value::is_number) {
        return Ok(vec![parse_vector(value)?]);
    }
    if array.iter().all(|item| {
        item.as_array()
            .is_some_and(|values| values.iter().all(Value::is_number))
    }) {
        if input_count == 1 && array.len() != 1 {
            return Err(Error::EmbeddingProvider(
                "Hugging Face returned token-level embeddings for one input; use a sentence-embedding model or a TEI endpoint with pooling"
                    .into(),
            ));
        }
        return array.iter().map(parse_vector).collect();
    }
    Err(Error::EmbeddingProvider(
        "Hugging Face returned token-level or non-dense embeddings; use a sentence-embedding model or a TEI endpoint with pooling"
            .into(),
    ))
}

fn parse_vector(value: &Value) -> crate::Result<Vec<f32>> {
    value
        .as_array()
        .ok_or_else(|| Error::EmbeddingProvider("embedding is not an array".into()))?
        .iter()
        .map(|number| {
            number
                .as_f64()
                .map(|value| value as f32)
                .ok_or_else(|| Error::EmbeddingProvider("embedding contains a non-number".into()))
        })
        .collect()
}

fn encode_model_path(model: &str) -> String {
    model
        .split('/')
        .map(|segment| {
            segment
                .bytes()
                .map(|byte| {
                    if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                        (byte as char).to_string()
                    } else {
                        format!("%{byte:02X}")
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn serverless_endpoint(model: &str) -> String {
    format!(
        "https://router.huggingface.co/hf-inference/models/{}/pipeline/feature-extraction",
        encode_model_path(model)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_single_and_batched_vectors() {
        assert_eq!(parse_embeddings(&json!([0.1, 0.2]), 1).unwrap().len(), 1);
        assert_eq!(
            parse_embeddings(&json!([[0.1, 0.2], [0.3, 0.4]]), 2)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn rejects_token_embeddings() {
        let error = parse_embeddings(&json!([[[0.1, 0.2]]]), 1).unwrap_err();
        assert!(error.to_string().contains("token-level"));

        let error = parse_embeddings(&json!([[0.1, 0.2], [0.3, 0.4]]), 1).unwrap_err();
        assert!(error.to_string().contains("pooling"));
    }

    #[test]
    fn serverless_route_keeps_opaque_model_ids_in_the_feature_extraction_pipeline() {
        assert_eq!(
            serverless_endpoint("vendor/future model:v9"),
            "https://router.huggingface.co/hf-inference/models/vendor/future%20model%3Av9/pipeline/feature-extraction"
        );
    }
}

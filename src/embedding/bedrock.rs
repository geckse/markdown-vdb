use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use reqwest::{header, Method, StatusCode, Url};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::provider::{
    describe_request_error, dimension_option, embedding_http_client, validate_embeddings,
    EmbeddingModelInfo, EmbeddingProvider, EmbeddingPurpose,
};
use crate::config::{BedrockEmbeddingConfig, BedrockInvocation, Config};
use crate::error::Error;

#[derive(Clone)]
struct AwsCredentials {
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
}

#[derive(Clone)]
enum BedrockAuth {
    Bearer(String),
    SigV4(AwsCredentials),
}

pub struct BedrockProvider {
    client: reqwest::Client,
    auth: BedrockAuth,
    region: String,
    model: String,
    dimensions: Option<usize>,
    runtime_base: String,
    control_base: String,
    options: BedrockEmbeddingConfig,
}

impl BedrockProvider {
    pub fn from_config(config: &Config) -> crate::Result<Self> {
        let options = config.embedding_options.bedrock.clone();
        if !matches!(options.format.as_str(), "titan" | "cohere" | "custom") {
            return Err(Error::Config(format!(
                "invalid embedding.bedrock.format '{}': expected titan, cohere, or custom",
                options.format
            )));
        }
        if options.format == "custom" && options.request_template.is_none() {
            return Err(Error::Config(
                "embedding.bedrock.request_template is required for custom format".into(),
            ));
        }
        let region = options
            .region
            .clone()
            .or_else(|| std::env::var("AWS_REGION").ok())
            .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Error::EmbeddingProvider(
                    "Bedrock requires embedding.bedrock.region, AWS_REGION, or AWS_DEFAULT_REGION"
                        .into(),
                )
            })?;
        let auth = load_auth(options.profile.as_deref())?;
        let (runtime_base, control_base) = match &options.endpoint {
            Some(endpoint) => {
                let base = endpoint.trim_end_matches('/').to_string();
                (base.clone(), base)
            }
            None => (
                format!("https://bedrock-runtime.{region}.amazonaws.com"),
                format!("https://bedrock.{region}.amazonaws.com"),
            ),
        };
        Ok(Self {
            client: embedding_http_client(),
            auth,
            region,
            model: config.embedding_model.clone(),
            dimensions: dimension_option(config.embedding_dimensions),
            runtime_base,
            control_base,
            options,
        })
    }

    fn purpose_value(&self, purpose: EmbeddingPurpose) -> String {
        match purpose {
            EmbeddingPurpose::Document => self
                .options
                .document_purpose
                .clone()
                .unwrap_or_else(|| "search_document".to_string()),
            EmbeddingPurpose::Query => self
                .options
                .query_purpose
                .clone()
                .unwrap_or_else(|| "search_query".to_string()),
        }
    }

    fn request_body(&self, texts: &[String], purpose: EmbeddingPurpose) -> crate::Result<Value> {
        match self.options.format.as_str() {
            "titan" => {
                let text = texts.first().ok_or_else(|| {
                    Error::EmbeddingProvider("Titan invocation requires one input".into())
                })?;
                let mut body = json!({"inputText": text, "normalize": true});
                if let Some(dimensions) = self.dimensions {
                    body["dimensions"] = json!(dimensions);
                }
                Ok(body)
            }
            "cohere" => {
                let mut body = json!({
                    "texts": texts,
                    "input_type": self.purpose_value(purpose),
                    "embedding_types": ["float"]
                });
                if let Some(dimensions) = self.dimensions {
                    body["output_dimension"] = json!(dimensions);
                }
                Ok(body)
            }
            "custom" => replace_placeholders(
                self.options.request_template.clone().ok_or_else(|| {
                    Error::Config("missing custom Bedrock request template".into())
                })?,
                texts,
                self.dimensions,
                &self.purpose_value(purpose),
            ),
            _ => unreachable!("format validated in constructor"),
        }
    }

    async fn invoke_value(&self, body: &Value) -> crate::Result<Value> {
        let body = serde_json::to_string(body)
            .map_err(|e| Error::EmbeddingProvider(format!("invalid Bedrock request: {e}")))?;
        let url = format!(
            "{}/model/{}/invoke",
            self.runtime_base,
            percent_encode(&self.model)
        );
        self.send_json(Method::POST, &url, "bedrock", &body).await
    }

    async fn send_json(
        &self,
        method: Method,
        url: &str,
        service: &str,
        body: &str,
    ) -> crate::Result<Value> {
        let url = Url::parse(url)
            .map_err(|e| Error::EmbeddingProvider(format!("invalid Bedrock endpoint: {e}")))?;
        let mut last_error = None;
        for attempt in 0..=3u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(250 * (1 << attempt))).await;
            }
            let mut request = self
                .client
                .request(method.clone(), url.clone())
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json");
            request = match &self.auth {
                BedrockAuth::Bearer(token) => {
                    request.header(header::AUTHORIZATION, format!("Bearer {token}"))
                }
                BedrockAuth::SigV4(credentials) => {
                    let signed = sigv4_headers(
                        &method,
                        &url,
                        service,
                        &self.region,
                        body.as_bytes(),
                        credentials,
                        SystemTime::now(),
                    )?;
                    for (name, value) in signed {
                        request = request.header(name, value);
                    }
                    request
                }
            };
            if method != Method::GET {
                request = request.body(body.to_string());
            }
            let response = request.send().await;
            let response = match response {
                Ok(response) => response,
                Err(error) if error.is_timeout() || error.is_connect() => {
                    last_error = Some(format!("Bedrock request failed: {error}"));
                    continue;
                }
                Err(error) => {
                    return Err(Error::EmbeddingProvider(format!(
                        "Bedrock request failed: {error}"
                    )))
                }
            };
            if response.status() == StatusCode::TOO_MANY_REQUESTS
                || response.status().is_server_error()
            {
                last_error = Some(format!("Bedrock returned {}", response.status()));
                continue;
            }
            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(Error::EmbeddingProvider(format!(
                    "Bedrock returned {}: {text}",
                    status.as_u16()
                )));
            }
            match response.json().await {
                Ok(value) => return Ok(value),
                Err(error) if error.is_timeout() => {
                    last_error = Some(format!(
                        "transient response read failure: {}",
                        describe_request_error(&error)
                    ));
                    continue;
                }
                Err(error) => {
                    return Err(Error::EmbeddingProvider(format!(
                        "failed to parse Bedrock response: {}",
                        describe_request_error(&error)
                    )))
                }
            }
        }
        Err(Error::EmbeddingProvider(
            last_error.unwrap_or_else(|| "Bedrock retries exhausted".to_string()),
        ))
    }

    fn parse_response(&self, value: &Value, expected: usize) -> crate::Result<Vec<Vec<f32>>> {
        let pointer = match self.options.format.as_str() {
            "titan" => "/embedding",
            "cohere" => "/embeddings",
            "custom" => self.options.embeddings_pointer.as_str(),
            _ => unreachable!(),
        };
        let selected = value.pointer(pointer).ok_or_else(|| {
            Error::EmbeddingProvider(format!(
                "Bedrock response does not contain configured embeddings pointer '{pointer}'"
            ))
        })?;
        let vectors = parse_vectors(selected, self.options.item_embedding_pointer.as_deref())?;
        validate_embeddings(&vectors, expected, self.dimensions)?;
        Ok(vectors)
    }
}

#[async_trait]
impl EmbeddingProvider for BedrockProvider {
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
        let single = self.options.format == "titan"
            || matches!(self.options.invocation, BedrockInvocation::Single);
        if single {
            let mut vectors = Vec::with_capacity(texts.len());
            for text in texts {
                let body = self.request_body(std::slice::from_ref(text), purpose)?;
                let response = self.invoke_value(&body).await?;
                vectors.extend(self.parse_response(&response, 1)?);
            }
            validate_embeddings(&vectors, texts.len(), self.dimensions)?;
            Ok(vectors)
        } else {
            let body = self.request_body(texts, purpose)?;
            let response = self.invoke_value(&body).await?;
            self.parse_response(&response, texts.len())
        }
    }

    async fn list_models(&self) -> crate::Result<Option<Vec<EmbeddingModelInfo>>> {
        let url = format!(
            "{}/foundation-models?byOutputModality=EMBEDDING",
            self.control_base
        );
        let value = self.send_json(Method::GET, &url, "bedrock", "").await?;
        Ok(Some(parse_model_catalog(&value)))
    }

    fn dimensions(&self) -> usize {
        self.dimensions.unwrap_or(0)
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn name(&self) -> &str {
        "bedrock"
    }
}

fn parse_model_catalog(value: &Value) -> Vec<EmbeddingModelInfo> {
    value["modelSummaries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(EmbeddingModelInfo {
                id: item.get("modelId")?.as_str()?.to_string(),
                name: item
                    .get("modelName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                input_token_limit: None,
            })
        })
        .collect()
}

fn replace_placeholders(
    value: Value,
    texts: &[String],
    dimensions: Option<usize>,
    purpose: &str,
) -> crate::Result<Value> {
    match value {
        Value::String(value) if value == "$input" => texts
            .first()
            .cloned()
            .map(Value::String)
            .ok_or_else(|| Error::Config("$input requires at least one input".into())),
        Value::String(value) if value == "$inputs" => Ok(json!(texts)),
        Value::String(value) if value == "$dimensions" => dimensions
            .map(|value| json!(value))
            .ok_or_else(|| Error::Config("$dimensions requires an explicit dimension".into())),
        Value::String(value) if value == "$purpose" => Ok(Value::String(purpose.to_string())),
        Value::Array(values) => values
            .into_iter()
            .map(|value| replace_placeholders(value, texts, dimensions, purpose))
            .collect(),
        Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| {
                replace_placeholders(value, texts, dimensions, purpose).map(|value| (key, value))
            })
            .collect(),
        value => Ok(value),
    }
}

fn parse_vectors(value: &Value, item_pointer: Option<&str>) -> crate::Result<Vec<Vec<f32>>> {
    if let Some(float) = value.get("float") {
        return parse_vectors(float, item_pointer);
    }
    let array = value.as_array().ok_or_else(|| {
        Error::EmbeddingProvider("configured Bedrock embedding value is not an array".into())
    })?;
    if array.iter().all(Value::is_number) {
        return Ok(vec![parse_vector(value)?]);
    }
    if let Some(pointer) = item_pointer {
        return array
            .iter()
            .map(|item| {
                let value = item.pointer(pointer).ok_or_else(|| {
                    Error::EmbeddingProvider(format!(
                        "Bedrock response item does not contain '{pointer}'"
                    ))
                })?;
                parse_vector(value)
            })
            .collect();
    }
    array.iter().map(parse_vector).collect()
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

fn load_auth(profile: Option<&str>) -> crate::Result<BedrockAuth> {
    if let Ok(token) = std::env::var("AWS_BEARER_TOKEN_BEDROCK") {
        if !token.is_empty() {
            return Ok(BedrockAuth::Bearer(token));
        }
    }
    let access = std::env::var("AWS_ACCESS_KEY_ID")
        .ok()
        .filter(|v| !v.is_empty());
    let secret = std::env::var("AWS_SECRET_ACCESS_KEY")
        .ok()
        .filter(|v| !v.is_empty());
    match (access, secret) {
        (Some(access_key), Some(secret_key)) => {
            return Ok(BedrockAuth::SigV4(AwsCredentials {
                access_key,
                secret_key,
                session_token: std::env::var("AWS_SESSION_TOKEN")
                    .ok()
                    .filter(|v| !v.is_empty()),
            }))
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err(Error::EmbeddingProvider(
                "incomplete AWS environment credentials: both AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY are required"
                    .into(),
            ))
        }
        (None, None) => {}
    }

    let profile = profile
        .map(str::to_string)
        .or_else(|| std::env::var("AWS_PROFILE").ok())
        .unwrap_or_else(|| "default".to_string());
    let path = std::env::var("AWS_SHARED_CREDENTIALS_FILE")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".aws/credentials")))
        .ok_or_else(|| Error::EmbeddingProvider("cannot locate AWS credentials file".into()))?;
    let content = std::fs::read_to_string(&path).map_err(|e| {
        Error::EmbeddingProvider(format!(
            "failed to read AWS credentials '{}': {e}",
            path.display()
        ))
    })?;
    let values = parse_ini_profile(&content, &profile).ok_or_else(|| {
        Error::EmbeddingProvider(format!(
            "AWS shared credentials profile '{profile}' was not found"
        ))
    })?;
    let access_key = values.get("aws_access_key_id").cloned();
    let secret_key = values.get("aws_secret_access_key").cloned();
    match (access_key, secret_key) {
        (Some(access_key), Some(secret_key)) => Ok(BedrockAuth::SigV4(AwsCredentials {
            access_key,
            secret_key,
            session_token: values.get("aws_session_token").cloned(),
        })),
        _ => Err(Error::EmbeddingProvider(format!(
            "AWS shared credentials profile '{profile}' is incomplete"
        ))),
    }
}

fn parse_ini_profile(content: &str, wanted: &str) -> Option<BTreeMap<String, String>> {
    let mut current = None;
    let mut result = BTreeMap::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current = Some(line[1..line.len() - 1].trim().to_string());
            continue;
        }
        if current.as_deref() != Some(wanted) {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            result.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    (!result.is_empty()).then_some(result)
}

fn sigv4_headers(
    method: &Method,
    url: &Url,
    service: &str,
    region: &str,
    payload: &[u8],
    credentials: &AwsCredentials,
    now: SystemTime,
) -> crate::Result<Vec<(String, String)>> {
    let timestamp = now
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::EmbeddingProvider(format!("system clock is before Unix epoch: {e}")))?
        .as_secs() as i64;
    let (date, amz_date) = aws_dates(timestamp);
    let payload_hash = sha256_hex(payload);
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let mut canonical = BTreeMap::new();
    canonical.insert("content-type".to_string(), "application/json".to_string());
    canonical.insert("host".to_string(), host.clone());
    canonical.insert("x-amz-content-sha256".to_string(), payload_hash.clone());
    canonical.insert("x-amz-date".to_string(), amz_date.clone());
    if let Some(token) = &credentials.session_token {
        canonical.insert("x-amz-security-token".to_string(), token.clone());
    }
    let signed_headers = canonical.keys().cloned().collect::<Vec<_>>().join(";");
    let canonical_headers = canonical
        .iter()
        .map(|(key, value)| format!("{key}:{}\n", value.trim()))
        .collect::<String>();
    let canonical_query = canonical_query(url);
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method.as_str(),
        url.path(),
        canonical_query,
        canonical_headers,
        signed_headers,
        payload_hash
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let date_key = hmac_sha256(
        format!("AWS4{}", credentials.secret_key).as_bytes(),
        date.as_bytes(),
    );
    let region_key = hmac_sha256(&date_key, region.as_bytes());
    let service_key = hmac_sha256(&region_key, service.as_bytes());
    let signing_key = hmac_sha256(&service_key, b"aws4_request");
    let signature = hex(&hmac_sha256(&signing_key, string_to_sign.as_bytes()));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        credentials.access_key, scope, signed_headers, signature
    );
    let mut headers = vec![
        ("host".to_string(), host),
        ("x-amz-content-sha256".to_string(), payload_hash),
        ("x-amz-date".to_string(), amz_date),
        ("authorization".to_string(), authorization),
    ];
    if let Some(token) = &credentials.session_token {
        headers.push(("x-amz-security-token".to_string(), token.clone()));
    }
    Ok(headers)
}

fn canonical_query(url: &Url) -> String {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| (percent_encode(&key), percent_encode(&value)))
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn sha256_hex(value: &[u8]) -> String {
    hex(&Sha256::digest(value))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for index in 0..64 {
        inner_pad[index] ^= block[index];
        outer_pad[index] ^= block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(data);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_hash);
    outer.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn aws_dates(timestamp: i64) -> (String, String) {
    let days = timestamp.div_euclid(86_400);
    let seconds = timestamp.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    (
        format!("{year:04}{month:02}{day:02}"),
        format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z"),
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += (month <= 2) as i64;
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_vector() {
        assert_eq!(
            hex(&hmac_sha256(
                b"key",
                b"The quick brown fox jumps over the lazy dog"
            )),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn aws_timestamp_format() {
        assert_eq!(aws_dates(0), ("19700101".into(), "19700101T000000Z".into()));
    }

    #[test]
    fn custom_template_inserts_typed_values() {
        let value = replace_placeholders(
            json!({"texts": "$inputs", "dimensions": "$dimensions"}),
            &["a".into(), "b".into()],
            Some(17),
            "query",
        )
        .unwrap();
        assert_eq!(value["texts"], json!(["a", "b"]));
        assert_eq!(value["dimensions"], 17);
    }

    #[test]
    fn parses_static_profile() {
        let profile = parse_ini_profile(
            "[default]\naws_access_key_id = a\naws_secret_access_key = b\n",
            "default",
        )
        .unwrap();
        assert_eq!(profile["aws_access_key_id"], "a");
    }

    #[test]
    fn catalog_passes_through_unknown_future_model_ids() {
        let models = parse_model_catalog(&json!({
            "modelSummaries": [{
                "modelId": "provider.future-embedding-model-v99",
                "modelName": "Future embedding model"
            }]
        }));
        assert_eq!(models[0].id, "provider.future-embedding-model-v99");
    }
}

---
title: "Embedding Providers"
description: "Configure the eight supported embedding backends, credentials, model discovery, and automatic dimensions"
category: "concepts"
---

# Embedding Providers

mdvdb converts Markdown chunks and search queries into vectors through one configured embedding
provider. Provider-native model identifiers are treated as opaque strings: using a new model,
deployment, endpoint label, or immutable revision does not require an mdvdb release when its
transport is already supported.

Start with automatic dimension discovery:

```yaml
# .markdownvdb/config.yaml
embedding:
  provider: openai
  model: text-embedding-3-small
  dimensions: auto
  batch_size: 100
```

Store credentials outside YAML:

```bash
printf '%s' "$OPENAI_API_KEY" \
  | mdvdb config secret set OPENAI_API_KEY --stdin

mdvdb embedding probe
mdvdb doctor
```

## Supported providers

| Provider | `embedding.provider` | Credential or connection | Model discovery |
|---|---|---|---|
| OpenAI | `openai` | `OPENAI_API_KEY`; optional `embedding.endpoint` | No |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` | Yes |
| Google Gemini | `gemini` | `GEMINI_API_KEY` | Yes |
| Azure OpenAI | `azure` | Endpoint plus an API key or bearer token | No |
| AWS Bedrock | `bedrock` | Bedrock bearer token, AWS credentials, or a shared profile | Yes |
| Hugging Face | `huggingface` | `HF_TOKEN` for serverless; optional for endpoint mode | Serverless only |
| Ollama | `ollama` | `OLLAMA_HOST`, default `http://localhost:11434` | No |
| OpenAI-compatible | `custom` | Exact `embedding.endpoint`; optional `OPENAI_API_KEY` | No |

Accepted aliases include `google`, `azure-openai`, `aws-bedrock`, and `hf`. The internal mock
provider is for tests and is not one of the eight user-facing backends.

## Settings and secret precedence

Ordinary settings resolve from highest to lowest priority:

1. Shell `MDVDB_*` overrides
2. Project `.markdownvdb/config.yaml`
3. User `~/.mdvdb/config.yaml`
4. Built-in defaults

Project YAML is deep-merged over user YAML. Mapping keys merge independently; a higher-priority
scalar or sequence replaces the lower-priority value.

Credentials and connection secrets use a separate chain:

1. Shell environment
2. Project-root `.env`
3. Project `.markdownvdb/.env`
4. User `~/.mdvdb/.env`
5. Legacy user `~/.mdvdb/config`

`mdvdb config secret set` writes the project `.markdownvdb/.env`;
`mdvdb config --global secret set` writes the user `.env`. Both read the value from stdin so it
does not appear in process arguments. Never put credentials in `config.yaml`.

Set `MDVDB_NO_USER_CONFIG=1` to ignore user-level YAML and user-level secret files.

## Automatic dimensions

`embedding.dimensions` accepts `auto` or a positive integer. Prefer `auto` unless a provider
requires a fixed output size:

```yaml
embedding:
  provider: ollama
  model: nomic-embed-text
  dimensions: auto
```

With an existing compatible index, mdvdb reuses the dimension recorded by that index. A new index
or explicit full reindex makes one minimal provider request before creating the replacement index.
Use `mdvdb embedding probe --json` to make the same live check explicitly:

```json
{
  "provider": "ollama",
  "model": "nomic-embed-text",
  "dimensions": 768,
  "latency_ms": 42
}
```

The vector itself is never printed. A probe requires provider connectivity and valid credentials,
and a hosted provider may charge for the request.

Changing the provider, model, dimensions, semantic endpoint, purpose handling, normalization, or
provider codec changes the embedding space. Run:

```bash
mdvdb ingest --reindex
```

Semantic search and incremental ingestion remain blocked on an incompatible index until the
reindex succeeds. Lexical and metadata operations remain available, and a failed replacement
leaves the previous on-disk generation intact.

## Model discovery

Discovery reads a provider's live catalog:

```bash
# Configured provider
mdvdb embedding models

# Temporary provider override
mdvdb embedding models --provider gemini --json
```

OpenRouter, Gemini, Bedrock, and Hugging Face serverless expose discovery. OpenAI, Azure OpenAI,
Ollama, Custom, and Hugging Face endpoint mode accept model IDs directly without a catalog.
Discovery is advisory, not an allowlist; an undiscovered provider-native ID is still accepted.

Prefer immutable model revisions when an embedding space must be reproducible. A remote alias can
change without any local configuration change.

## Provider configuration

### OpenAI

OpenAI is the default:

```yaml
embedding:
  provider: openai
  model: text-embedding-3-small
  dimensions: auto
```

```dotenv
OPENAI_API_KEY=...
```

Set `embedding.endpoint` only when you intentionally need an alternative OpenAI-compatible route
with OpenAI authentication behavior.

### OpenRouter

```yaml
embedding:
  provider: openrouter
  model: vendor/provider-model-id
  dimensions: auto
```

```dotenv
OPENROUTER_API_KEY=...
```

The default endpoint is `https://openrouter.ai/api/v1/embeddings`.

### Google Gemini

```yaml
embedding:
  provider: gemini
  model: provider-native-model-id
  dimensions: auto
  purpose:
    mode: native
    query: RETRIEVAL_QUERY
    document: RETRIEVAL_DOCUMENT
```

```dotenv
GEMINI_API_KEY=...
```

Gemini uses its native batch embedding API. Purpose handling is explicit configuration:

- `none` sends text unchanged.
- `native` sends the configured query/document task values.
- `prefix` prepends the configured query/document strings.

### Azure OpenAI

```yaml
embedding:
  provider: azure
  model: my-embedding-deployment
  dimensions: auto
  endpoint: https://my-resource.openai.azure.com
  azure:
    auth: api_key
```

```dotenv
AZURE_OPENAI_API_KEY=...
```

The endpoint can instead come from `AZURE_OPENAI_ENDPOINT`. For an externally acquired Microsoft
Entra token, set `embedding.azure.auth: bearer` and provide
`AZURE_OPENAI_ACCESS_TOKEN`. mdvdb invokes `{endpoint}/openai/v1/embeddings`.

### AWS Bedrock

```yaml
embedding:
  provider: bedrock
  model: provider.model-id-or-inference-profile
  dimensions: auto
  bedrock:
    region: eu-central-1
    format: titan       # titan | cohere | custom
```

Authentication resolves in this order:

1. `AWS_BEARER_TOKEN_BEDROCK`
2. `AWS_ACCESS_KEY_ID` plus `AWS_SECRET_ACCESS_KEY`, with optional `AWS_SESSION_TOKEN`
3. `embedding.bedrock.profile`, `AWS_PROFILE`, or the `default` shared-credentials profile

`format` selects the request/response codec independently of the model ID. Bedrock also supports
single or batch invocation, a custom JSON request template, typed `$input`, `$inputs`,
`$dimensions`, and `$purpose` placeholders, and RFC 6901 response pointers. See the
[provider transport guide](https://github.com/geckse/markdown-vdb/blob/main/docs/embedding-providers.md#aws-bedrock)
for the custom-codec schema.

### Hugging Face

Serverless mode derives its route from the Hub model ID and requires `HF_TOKEN`:

```yaml
embedding:
  provider: huggingface
  model: sentence-transformers/provider-model
  dimensions: auto
  huggingface:
    mode: serverless
    normalize: true
    truncate: true
    truncation_direction: right
    query_prompt_name: query
    document_prompt_name: passage
```

For a managed Inference Endpoint, self-hosted TEI instance, or compatible private service, use its
exact URL:

```yaml
embedding:
  provider: huggingface
  model: stable-endpoint-label
  dimensions: auto
  huggingface:
    mode: endpoint
    endpoint: https://example.endpoints.huggingface.cloud/embed
```

`HF_TOKEN` is optional in endpoint mode. The response must contain one pooled dense float vector
per input; token-level tensors and sparse output are rejected.

### Ollama

```yaml
embedding:
  provider: ollama
  model: nomic-embed-text
  dimensions: auto
```

Ollama uses `http://localhost:11434` by default. Override a remote service in the shell or secret
file:

```dotenv
OLLAMA_HOST=http://192.168.1.100:11434
```

Make sure the model is present before probing or ingesting:

```bash
ollama pull nomic-embed-text
```

### OpenAI-compatible endpoint

Use `custom` for a service that accepts and returns the OpenAI embeddings wire format:

```yaml
embedding:
  provider: custom
  model: provider-native-model-id
  dimensions: auto
  endpoint: https://embeddings.example.test/v1/embeddings
```

If the endpoint requires a bearer token, store it as `OPENAI_API_KEY`; omit it for an unauthenticated
private endpoint. mdvdb uses the URL exactly as configured.

## Related pages

- [`mdvdb embedding`](../commands/embedding.md) — discover models and probe dimensions
- [`mdvdb config`](../commands/config.md) — inspect settings and manage secrets
- [`mdvdb doctor`](../commands/doctor.md) — validate provider connectivity and index compatibility
- [Configuration](../configuration.md) — complete YAML and precedence reference
- [Ingestion](../commands/ingest.md) — build or replace the index

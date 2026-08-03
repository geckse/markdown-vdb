# Embedding providers

mdvdb treats model identifiers as opaque provider-native strings. A new model,
deployment, inference profile, endpoint label, or immutable revision does not
require an mdvdb update when it uses one of the supported wire formats.

Start with automatic dimension inference:

```yaml
# .markdownvdb/config.yaml
embedding:
  provider: openrouter
  model: vendor/model-id
  dimensions: auto
  batch_size: 100
```

Put credentials in `.markdownvdb/.env` (or `~/.mdvdb/.env` for a shared user
credential). Tesseract and `mdvdb config secret set NAME --stdin` write these
files atomically with owner-only permissions. Do not put secrets in YAML.

## Connections

| Provider | `embedding.provider` | Connection |
|---|---|---|
| OpenAI | `openai` | `OPENAI_API_KEY`; optional `embedding.endpoint` |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY`; posts to `https://openrouter.ai/api/v1/embeddings` |
| Google Gemini | `gemini` | `GEMINI_API_KEY`; uses native `batchEmbedContents` |
| Azure OpenAI | `azure` | `AZURE_OPENAI_ENDPOINT` or `embedding.endpoint`, plus API key or bearer token |
| AWS Bedrock | `bedrock` | Bedrock bearer token, environment/session credentials, or a shared-credentials profile |
| Hugging Face | `huggingface` | `HF_TOKEN` for serverless; token is optional for a private-network Endpoint/TEI URL |
| Ollama | `ollama` | `OLLAMA_HOST` (default `http://localhost:11434`) |
| OpenAI-compatible | `custom` | Exact `embedding.endpoint`; optional `OPENAI_API_KEY` bearer token |

The OpenAI, OpenRouter, Azure, and custom transports share the
OpenAI-compatible embedding response codec, but keep separate URL and
authentication policies.

### OpenRouter

```dotenv
OPENROUTER_API_KEY=...
```

```yaml
embedding:
  provider: openrouter
  model: any/provider-model-id
  dimensions: auto
```

`mdvdb embedding models --provider openrouter --json` reads OpenRouter's live
embedding catalog. Catalog membership is a suggestion only; an undiscovered ID
is still accepted.

### Gemini

```dotenv
GEMINI_API_KEY=...
```

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

Purpose handling is configuration, never model-name logic. `mode: none` sends
the text unchanged. `mode: native` sends the configured task values. `mode:
prefix` prepends the configured query/document strings to the text.

### Azure OpenAI

```dotenv
AZURE_OPENAI_ENDPOINT=https://my-resource.openai.azure.com
AZURE_OPENAI_API_KEY=...
```

```yaml
embedding:
  provider: azure
  model: my-embedding-deployment
  dimensions: auto
  azure:
    auth: api_key
```

For externally acquired Microsoft Entra tokens, set `auth: bearer` and store the
token as `AZURE_OPENAI_ACCESS_TOKEN`. The invocation URL is
`{endpoint}/openai/v1/embeddings`. Azure deployment names remain free text
because this data-plane API has no suitable model/deployment catalog.

### Hugging Face

Serverless `hf-inference` derives its route from the Hub model ID:

```dotenv
HF_TOKEN=hf_...
```

The derived route is
`https://router.huggingface.co/hf-inference/models/{model}/pipeline/feature-extraction`.

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

For a managed Inference Endpoint, self-hosted TEI instance, or compatible
private service, post to its exact URL:

```yaml
embedding:
  provider: huggingface
  model: stable-endpoint-label
  dimensions: auto
  huggingface:
    mode: endpoint
    endpoint: https://example.endpoints.huggingface.cloud/embed
```

The response must contain one pooled dense float vector per input. Token-level
tensors and sparse output are rejected with a pooling/TEI diagnostic.

### AWS Bedrock

Choose the body codec independently from the model ID:

```yaml
embedding:
  provider: bedrock
  model: provider.model-id-or-inference-profile
  dimensions: auto
  bedrock:
    region: eu-central-1
    format: titan # titan | cohere | custom
```

Authentication is resolved in this order:

1. `AWS_BEARER_TOKEN_BEDROCK`
2. Complete `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY`, with optional
   `AWS_SESSION_TOKEN`
3. `embedding.bedrock.profile`, `AWS_PROFILE`, or the `default` profile in the
   static shared credentials file

Access-key requests and catalog calls are signed with SigV4. `titan` uses a
single-input body; `cohere` uses a batch body. Neither codec examines the model
ID.

For a future JSON schema, use typed placeholders and RFC 6901 response
pointers:

```yaml
embedding:
  provider: bedrock
  model: arbitrary.future-model
  dimensions: 1024
  bedrock:
    region: us-east-1
    format: custom
    invocation: batch
    request_template:
      records: $inputs
      output_size: $dimensions
      input_kind: $purpose
    query_purpose: query
    document_purpose: document
    embeddings_pointer: /result/items
    item_embedding_pointer: /vector
```

`$input`, `$inputs`, `$dimensions`, and `$purpose` are replaced as typed JSON
values. They are not string-interpolated. Omit `$dimensions` when using `auto`
and the remote schema does not require an output size.

## Discovery, probing, and index safety

```bash
mdvdb embedding models --json
mdvdb embedding models --provider huggingface --json
mdvdb embedding probe --json
```

The probe performs one minimal inference and reports provider, model, resolved
dimensions, and latency without printing the vector. Model discovery is live
for OpenRouter, Gemini, Bedrock, and Hugging Face serverless. It is optional and
never acts as an allowlist.

With `dimensions: auto`, an existing compatible index supplies its stored
dimension without a network request. A new index and an explicit full reindex
probe before initializing a replacement generation. If probing or rebuilding
fails, the previous on-disk generation remains intact. Changes to provider,
model, codec, purpose settings, normalization, dimensions, or semantic endpoint
block semantic search and incremental ingestion until `mdvdb ingest --reindex`
succeeds; lexical and metadata operations remain available.

Use immutable provider model revisions when an embedding space must remain
reproducible. Remote aliases can change without notice.

## Provider references

- [OpenRouter embeddings and model discovery](https://openrouter.ai/docs/api/reference/embeddings)
- [Gemini embedding API](https://ai.google.dev/api/embeddings) and [paginated model catalog](https://ai.google.dev/api/models)
- [Azure OpenAI v1 embeddings](https://learn.microsoft.com/en-us/rest/api/aifoundry/azureopenai/embeddings)
- [Bedrock Invoke API](https://docs.aws.amazon.com/bedrock/latest/userguide/inference-api.html), [embedding-model schemas](https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters.html), [catalog filtering](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_ListFoundationModels.html), [API-key authentication](https://docs.aws.amazon.com/bedrock/latest/userguide/api-keys-use.html), and [SigV4 signing](https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html)
- [Hugging Face Feature Extraction](https://huggingface.co/docs/inference-providers/tasks/feature-extraction), [live Hub provider filtering](https://huggingface.co/docs/inference-providers/hub-api), and [TEI endpoints](https://huggingface.co/docs/inference-endpoints/engines/tei)

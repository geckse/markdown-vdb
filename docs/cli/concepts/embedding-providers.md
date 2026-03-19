---
title: "Embedding Providers"
description: "Setup and configuration for OpenAI, Ollama, and Custom embedding providers"
category: "concepts"
---

# Embedding Providers

mdvdb uses embedding providers to convert text into vector representations for semantic search. Three provider types are supported: **OpenAI** (default, cloud-hosted), **Ollama** (local, self-hosted), and **Custom** (any OpenAI-compatible API endpoint).

## Provider Overview

| Provider | Type | Default Model | Default Dimensions | API Key Required | Network Required |
|----------|------|---------------|-------------------|-----------------|-----------------|
| **openai** | Cloud | `text-embedding-3-small` | `1536` | Yes (`OPENAI_API_KEY`) | Yes |
| **ollama** | Local | `text-embedding-3-small`* | `1536`* | No | No (localhost) |
| **custom** | Any | `text-embedding-3-small`* | `1536`* | Optional | Yes |

\* *Ollama and Custom providers inherit the default model/dimensions values but you should override them to match your actual model. See setup sections below.*

## How Embedding Works

During **ingestion**, mdvdb:
1. Chunks each markdown file into sections (by headings, with a token size guard).
2. Computes a SHA-256 content hash for each source file.
3. Compares hashes against the index -- unchanged files are skipped entirely.
4. Sends changed chunks to the embedding provider in batches (up to `MDVDB_EMBEDDING_BATCH_SIZE` texts per request, with up to 4 concurrent batch requests).
5. Stores the resulting vectors in the HNSW index.

During **search** (semantic, hybrid, or edge modes), the query text is embedded using the same provider, and the resulting vector is compared against stored vectors using cosine similarity.

## OpenAI (Default)

The default provider uses [OpenAI's embedding API](https://platform.openai.com/docs/guides/embeddings). The recommended model is `text-embedding-3-small`, which offers a good balance of quality and cost.

### Setup

1. **Get an API key** from [platform.openai.com](https://platform.openai.com/api-keys).

2. **Set the key** in your project config or environment:

   ```bash
   # In .markdownvdb/.config
   OPENAI_API_KEY=sk-proj-your-key-here
   ```

   Or as a shell environment variable:

   ```bash
   export OPENAI_API_KEY=sk-proj-your-key-here
   ```

   Or in a `.env` file (useful if other tools also read this key):

   ```bash
   # .env
   OPENAI_API_KEY=sk-proj-your-key-here
   ```

3. **Verify** the provider is reachable:

   ```bash
   mdvdb doctor
   ```

   Look for the "Provider reachable" check to show "Pass".

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `MDVDB_EMBEDDING_PROVIDER` | `openai` | Set to `openai` (or omit -- it is the default) |
| `MDVDB_EMBEDDING_MODEL` | `text-embedding-3-small` | OpenAI model name |
| `MDVDB_EMBEDDING_DIMENSIONS` | `1536` | Vector dimensions (must match model) |
| `OPENAI_API_KEY` | *(required)* | Your OpenAI API key |
| `MDVDB_EMBEDDING_BATCH_SIZE` | `100` | Texts per API request |

### Available Models

| Model | Dimensions | Notes |
|-------|-----------|-------|
| `text-embedding-3-small` | 1536 | Default. Good quality, low cost. |
| `text-embedding-3-large` | 3072 | Higher quality, higher cost. |
| `text-embedding-ada-002` | 1536 | Legacy model. |

When changing models, you **must** update `MDVDB_EMBEDDING_DIMENSIONS` to match and re-ingest all files (`mdvdb ingest --reindex`). Dimension mismatch will cause errors.

### Custom Endpoint

OpenAI-compatible providers (e.g., Azure OpenAI, LiteLLM proxy) can be used by setting a custom endpoint while keeping the `openai` provider type:

```bash
MDVDB_EMBEDDING_PROVIDER=openai
MDVDB_EMBEDDING_ENDPOINT=https://your-deployment.openai.azure.com/openai/deployments/your-model/embeddings?api-version=2024-02-01
OPENAI_API_KEY=your-azure-key
```

### Retry Behavior

The OpenAI provider retries automatically on transient failures:
- **Rate limiting (429)**: retries up to 3 times with exponential backoff (1s, 2s, 4s).
- **Server errors (5xx)**: retries up to 3 times with exponential backoff.
- **Authentication errors (401)**: fails immediately (no retry).
- **Other client errors (4xx)**: fails immediately (no retry).

## Ollama (Local)

[Ollama](https://ollama.ai) runs embedding models locally on your machine. No API key is needed, and no data leaves your network.

### Setup

1. **Install Ollama** from [ollama.ai](https://ollama.ai).

2. **Pull an embedding model**:

   ```bash
   ollama pull nomic-embed-text
   ```

3. **Configure mdvdb** to use Ollama:

   ```bash
   # In .markdownvdb/.config
   MDVDB_EMBEDDING_PROVIDER=ollama
   MDVDB_EMBEDDING_MODEL=nomic-embed-text
   MDVDB_EMBEDDING_DIMENSIONS=768
   ```

4. **Verify** Ollama is running and accessible:

   ```bash
   mdvdb doctor
   ```

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `MDVDB_EMBEDDING_PROVIDER` | | Set to `ollama` |
| `MDVDB_EMBEDDING_MODEL` | `text-embedding-3-small` | Model name (override to your Ollama model) |
| `MDVDB_EMBEDDING_DIMENSIONS` | `1536` | Vector dimensions (override to match your model) |
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama server URL |
| `MDVDB_EMBEDDING_BATCH_SIZE` | `100` | Texts per API request |

### Popular Ollama Embedding Models

| Model | Dimensions | Notes |
|-------|-----------|-------|
| `nomic-embed-text` | 768 | Recommended. Good quality, fast. |
| `mxbai-embed-large` | 1024 | Higher quality, larger model. |
| `all-minilm` | 384 | Smallest, fastest. Good for testing. |
| `snowflake-arctic-embed` | 1024 | Strong retrieval performance. |

### Remote Ollama

To use an Ollama instance running on a different machine:

```bash
OLLAMA_HOST=http://192.168.1.100:11434
```

### Error Handling

- **Connection refused**: Ollama server is not running. Start it with `ollama serve`.
- **Model not found (404)**: The specified model is not pulled. Run `ollama pull <model>`.
- **Server errors (5xx)**: retries up to 3 times with exponential backoff.

## Custom Provider

The Custom provider works with any API endpoint that implements the [OpenAI embeddings API format](https://platform.openai.com/docs/api-reference/embeddings). This includes self-hosted inference servers, API proxies, and alternative embedding services.

### Setup

1. **Set the endpoint** to your embedding API:

   ```bash
   # In .markdownvdb/.config
   MDVDB_EMBEDDING_PROVIDER=custom
   MDVDB_EMBEDDING_ENDPOINT=http://localhost:8080/v1/embeddings
   MDVDB_EMBEDDING_MODEL=your-model-name
   MDVDB_EMBEDDING_DIMENSIONS=768
   ```

2. **Set an API key** if your endpoint requires authentication:

   ```bash
   OPENAI_API_KEY=your-api-key
   ```

   If no API key is needed, you can omit `OPENAI_API_KEY` -- the Custom provider will send an empty authorization header.

3. **Verify** the endpoint is accessible:

   ```bash
   mdvdb doctor
   ```

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `MDVDB_EMBEDDING_PROVIDER` | | Set to `custom` |
| `MDVDB_EMBEDDING_ENDPOINT` | *(required)* | Your embedding API endpoint URL |
| `MDVDB_EMBEDDING_MODEL` | `text-embedding-3-small` | Model name sent in requests |
| `MDVDB_EMBEDDING_DIMENSIONS` | `1536` | Vector dimensions (must match your model) |
| `OPENAI_API_KEY` | *(optional)* | API key for authentication (if needed) |
| `MDVDB_EMBEDDING_BATCH_SIZE` | `100` | Texts per API request |

### API Format

The Custom provider sends requests in the OpenAI embeddings format:

```json
{
  "input": ["text to embed", "another text"],
  "model": "your-model-name",
  "dimensions": 768
}
```

And expects responses in the same format:

```json
{
  "data": [
    { "embedding": [0.1, 0.2, ...], "index": 0 },
    { "embedding": [0.3, 0.4, ...], "index": 1 }
  ]
}
```

### Compatible Services

Examples of services that work with the Custom provider:

| Service | Endpoint Example |
|---------|-----------------|
| [LiteLLM](https://litellm.ai) | `http://localhost:4000/v1/embeddings` |
| [vLLM](https://vllm.ai) | `http://localhost:8000/v1/embeddings` |
| [TEI](https://github.com/huggingface/text-embeddings-inference) | `http://localhost:8080/v1/embeddings` |
| [LocalAI](https://localai.io) | `http://localhost:8080/v1/embeddings` |
| Azure OpenAI | `https://<resource>.openai.azure.com/openai/deployments/<model>/embeddings?api-version=2024-02-01` |

## Shared Configuration

These variables apply to all providers:

| Variable | Default | Description |
|----------|---------|-------------|
| `MDVDB_EMBEDDING_BATCH_SIZE` | `100` | Maximum number of texts sent in a single API request. Larger batches reduce API calls but increase memory usage and latency per request. |
| `MDVDB_EMBEDDING_DIMENSIONS` | `1536` | Number of dimensions in each embedding vector. Must match the model's output dimensions. |

### Batch Processing

mdvdb processes embeddings in batches for efficiency:

1. Texts are grouped into batches of `MDVDB_EMBEDDING_BATCH_SIZE`.
2. Up to **4 batches** are processed concurrently (concurrent API requests).
3. Each batch is sent as a single API call to the provider.

### Content-Hash Skipping

During incremental ingestion, mdvdb computes SHA-256 hashes of each source file. If a file's hash matches what is already in the index, all of its chunks are skipped -- no embedding API call is made. This dramatically reduces API costs on subsequent ingests.

To force re-embedding of all files (e.g., after changing models), use:

```bash
mdvdb ingest --reindex
```

## Switching Providers

When switching between providers or changing the embedding model:

1. **Update the configuration** with the new provider, model, and dimensions.
2. **Re-ingest all files** with `mdvdb ingest --reindex` to rebuild all embeddings.
3. **Verify** with `mdvdb doctor` that the new provider is reachable and working.

Mixing embeddings from different providers or models in the same index will produce poor search results because the vector spaces are incompatible.

## Troubleshooting

| Problem | Cause | Solution |
|---------|-------|----------|
| "OpenAI provider requires OPENAI_API_KEY to be set" | Missing API key | Set `OPENAI_API_KEY` in config or environment |
| "authentication failed (401): invalid API key" | Wrong API key | Verify your API key at platform.openai.com |
| "Cannot connect to Ollama at ..." | Ollama not running | Start Ollama with `ollama serve` |
| "Model X not found in Ollama" | Model not pulled | Run `ollama pull <model>` |
| "Custom provider requires MDVDB_EMBEDDING_ENDPOINT to be set" | Missing endpoint | Set `MDVDB_EMBEDDING_ENDPOINT` |
| "expected dimension X, got Y" | Dimension mismatch | Set `MDVDB_EMBEDDING_DIMENSIONS` to match model output |
| "rate limited (429)" | Too many API requests | Reduce `MDVDB_EMBEDDING_BATCH_SIZE` or wait |

## See Also

- [mdvdb search](../commands/search.md) -- Search command reference
- [mdvdb ingest](../commands/ingest.md) -- Ingest command reference
- [mdvdb doctor](../commands/doctor.md) -- Diagnostic checks including provider connectivity
- [Search Modes](./search-modes.md) -- How different search modes use embeddings
- [Configuration](../configuration.md) -- All environment variables
- [Chunking](./chunking.md) -- How text is prepared before embedding

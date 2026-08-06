---
title: "mdvdb embedding"
description: "Discover provider models and probe the configured embedding dimensions"
category: "commands"
---

# mdvdb embedding

Inspect a provider's live model catalog or make one minimal inference to verify the configured provider, model, credentials, and vector dimensions.

## Usage

```bash
mdvdb embedding <COMMAND> [OPTIONS]
```

| Command | Description |
|---------|-------------|
| `models [--provider <PROVIDER>]` | List models reported by a provider's live catalog |
| `probe` | Embed one short probe input and report the resolved dimensions and latency |

The commands also accept all [global options](./index.md#global-options), including `--json` and `--root`.

## List models

```bash
# Use the configured provider
mdvdb embedding models

# Temporarily select a provider for discovery
mdvdb embedding models --provider gemini

# Consume the catalog as JSON
mdvdb embedding models --provider huggingface --json
```

Canonical provider names are `openai`, `openrouter`, `gemini`, `azure`, `bedrock`, `huggingface`, `ollama`, and `custom`. Accepted aliases include `google`, `azure-openai`, `aws-bedrock`, and `hf`.

Catalog support varies by provider. OpenAI, Azure OpenAI, Ollama, and Custom currently return `discovery_available: false`; enter their model IDs directly. Other providers may still return no catalog if the remote service does not expose one.

```json
{
  "provider": "gemini",
  "discovery_available": true,
  "models": [
    {
      "id": "models/gemini-embedding-001",
      "name": "Gemini Embedding 001",
      "input_token_limit": 2048
    }
  ]
}
```

Each model has an opaque `id`; `name` and `input_token_limit` may be `null` because provider catalogs expose different metadata.

## Probe the configured model

```bash
mdvdb embedding probe
mdvdb embedding probe --json
```

Human-readable output has the form:

```text
openrouter · openai/text-embedding-3-small · 1536 dimensions · 184 ms
```

JSON output:

```json
{
  "provider": "openrouter",
  "model": "openai/text-embedding-3-small",
  "dimensions": 1536,
  "latency_ms": 184
}
```

`probe` uses the configured provider; it has no provider override. It performs a real one-input embedding request, so it requires valid credentials/connectivity and may count toward provider usage. The returned vector itself is never printed.

This command is particularly useful with `embedding.dimensions: auto`: it shows the dimension mdvdb will validate and persist when building an index.

## Related commands

- [`mdvdb config`](./config.md) -- Inspect or change provider configuration and secrets
- [`mdvdb doctor`](./doctor.md) -- Run provider and index diagnostics
- [`mdvdb ingest`](./ingest.md) -- Build or reindex using the configured model
- [Embedding Providers](../concepts/embedding-providers.md) -- Provider-specific setup

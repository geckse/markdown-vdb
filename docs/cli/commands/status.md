---
title: "mdvdb status"
description: "Show index status including document counts, vector counts, and embedding configuration"
category: "commands"
---

# mdvdb status

Show the current status of the index, including document and chunk counts, vector count, file size, last update time, and the embedding configuration used to build the index.

## Usage

```bash
mdvdb status [OPTIONS]
```

## Options

This command has no command-specific options. Only [global options](#global-options) apply.

## Global Options

These options apply to all commands. See [Commands Index](./index.md) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (-v info, -vv debug, -vvv trace) |
| `--root` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |

## Human-Readable Output

When run without `--json`, status displays a formatted summary:

```
  ● Index Status

  Documents:  57
  Chunks:     342
  Vectors:    342
  File size:  12.4 MB
  Updated:    3 minutes ago

  Embedding:  OpenAI
  Model:      text-embedding-3-small
  Dimensions: 1536
```

### Output Fields

| Field | Description |
|-------|-------------|
| **Documents** | Number of unique markdown files in the index |
| **Chunks** | Total number of text chunks across all files |
| **Vectors** | Total number of vectors in the HNSW index (should match chunk count) |
| **File size** | Size of the index file on disk (human-readable) |
| **Updated** | Time since the index was last saved (relative format) |
| **Embedding** | Name of the embedding provider (OpenAI, Ollama, or Custom) |
| **Model** | Embedding model identifier |
| **Dimensions** | Vector dimensionality |

## Examples

```bash
# Show index status
mdvdb status

# Show status as JSON
mdvdb status --json

# Show status for a specific project
mdvdb status --root /path/to/project
```

## JSON Output

### IndexStatus (`--json`)

```json
{
  "document_count": 57,
  "chunk_count": 342,
  "vector_count": 342,
  "last_updated": 1710856200,
  "file_size": 13021184,
  "embedding_config": {
    "provider": "OpenAI",
    "model": "text-embedding-3-small",
    "dimensions": 1536
  }
}
```

### IndexStatus Fields

| Field | Type | Description |
|-------|------|-------------|
| `document_count` | `number` | Number of unique markdown files in the index |
| `chunk_count` | `number` | Total number of chunks across all files |
| `vector_count` | `number` | Total number of vectors in the HNSW index |
| `last_updated` | `number` | Unix timestamp (seconds since epoch) of last index save |
| `file_size` | `number` | Size of the index file on disk in bytes |
| `embedding_config` | `EmbeddingConfig` | Embedding configuration snapshot |

### EmbeddingConfig Fields

| Field | Type | Description |
|-------|------|-------------|
| `provider` | `string` | Provider name (e.g., `"OpenAI"`, `"Ollama"`, `"Custom"`) |
| `model` | `string` | Model identifier (e.g., `"text-embedding-3-small"`) |
| `dimensions` | `number` | Vector dimensionality (e.g., `1536`) |

## Notes

- The `status` command opens the index in **read-only** mode. It never modifies the index.
- If no index exists (`.markdownvdb/index` not found), the command reports zero counts.
- The `vector_count` should always match `chunk_count`. A mismatch may indicate a corrupted index -- run [`mdvdb doctor`](./doctor.md) to diagnose.
- The `last_updated` field is a Unix timestamp in JSON mode, but displayed as a relative time string (e.g., "3 minutes ago") in human-readable mode.
- The `file_size` field is raw bytes in JSON mode, but displayed in human-readable format (e.g., "12.4 MB") in human-readable mode.

## Related Commands

- [`mdvdb ingest`](./ingest.md) -- Index files to populate the index
- [`mdvdb schema`](./schema.md) -- View the inferred metadata schema
- [`mdvdb tree`](./tree.md) -- View file tree with sync status indicators
- [`mdvdb doctor`](./doctor.md) -- Run diagnostic checks on the index
- [`mdvdb config`](./config.md) -- View resolved configuration

## See Also

- [Index Storage](../concepts/index-storage.md) -- Index file format and `.markdownvdb/` directory
- [Embedding Providers](../concepts/embedding-providers.md) -- Configure OpenAI, Ollama, or custom providers
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference
- [Configuration](../configuration.md) -- All environment variables and config options

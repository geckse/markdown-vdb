---
title: "mdvdb config"
description: "Show the fully resolved configuration with all values and their sources"
category: "commands"
---

# mdvdb config

Show the fully resolved configuration. Displays all configuration values after merging all sources (shell environment, project config, `.env`, user config, and built-in defaults). Useful for verifying which settings are active and debugging configuration issues.

## Usage

```bash
mdvdb config [OPTIONS]
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

When run without `--json`, the config command displays a formatted summary of all resolved values:

```
  ● Configuration

  Provider:     OpenAI
  Model:        text-embedding-3-small
  Dimensions:   1536
  Batch size:   100
  API key:      set
  Ollama host:  http://localhost:11434
  Source dirs:  .
  Chunking:     max_tokens=512, overlap=50
  Search:       mode=Hybrid, limit=10, min_score=0, rrf_k=60
  Watching:     enabled=true, debounce=300ms
  Clustering:   enabled=true, threshold=50

  User config: /home/user/.mdvdb/config (exists)
```

### Output Sections

| Section | Description |
|---------|-------------|
| **Provider** | Active embedding provider (`OpenAI`, `Ollama`, or `Custom`) |
| **Model** | Embedding model identifier |
| **Dimensions** | Vector dimensionality |
| **Batch size** | Number of texts sent per API batch |
| **API key** | Whether `OPENAI_API_KEY` is set (`set` in green, `not set` in yellow) |
| **Ollama host** | URL of the Ollama server |
| **Source dirs** | Comma-separated list of directories being scanned for markdown files |
| **Chunking** | Maximum tokens per chunk and overlap tokens between sub-split chunks |
| **Search** | Default search mode, result limit, minimum score threshold, and RRF fusion constant |
| **Watching** | Whether file watching is enabled and the debounce interval |
| **Clustering** | Whether clustering is enabled and the rebalance threshold |
| **User config** | Path to the user-level config file and whether it exists |

## Examples

```bash
# Show resolved configuration
mdvdb config

# Show configuration as JSON (for scripting)
mdvdb config --json

# Show configuration for a specific project
mdvdb config --root /path/to/project

# Check config after setting an environment variable
MDVDB_SEARCH_MODE=semantic mdvdb config

# Pipe JSON config to jq for specific values
mdvdb config --json | jq '.embedding_model'
```

## JSON Output

### Config (`--json`)

When `--json` is used, the full `Config` struct is serialized. This includes every configuration field with its resolved value.

```json
{
  "embedding_provider": "OpenAI",
  "embedding_model": "text-embedding-3-small",
  "embedding_dimensions": 1536,
  "embedding_batch_size": 100,
  "openai_api_key": null,
  "ollama_host": "http://localhost:11434",
  "embedding_endpoint": null,
  "source_dirs": ["."],
  "ignore_patterns": [],
  "watch_enabled": true,
  "watch_debounce_ms": 300,
  "chunk_max_tokens": 512,
  "chunk_overlap_tokens": 50,
  "clustering_enabled": true,
  "clustering_rebalance_threshold": 50,
  "search_default_limit": 10,
  "search_min_score": 0.0,
  "search_default_mode": "Hybrid",
  "search_rrf_k": 60.0,
  "bm25_norm_k": 1.5,
  "search_decay_enabled": false,
  "search_decay_half_life": 90.0,
  "search_decay_exclude": [],
  "search_decay_include": [],
  "search_boost_links": false,
  "search_boost_hops": 1,
  "search_expand_graph": 0,
  "search_expand_limit": 3,
  "vector_quantization": "F16",
  "index_compression": true,
  "edge_embeddings": true,
  "edge_boost_weight": 0.15,
  "edge_cluster_rebalance": 50
}
```

### Config Fields

| Field | Type | Description |
|-------|------|-------------|
| `embedding_provider` | `string` | Active provider: `"OpenAI"`, `"Ollama"`, or `"Custom"` |
| `embedding_model` | `string` | Model identifier (e.g., `"text-embedding-3-small"`) |
| `embedding_dimensions` | `number` | Vector dimensionality |
| `embedding_batch_size` | `number` | Texts per API batch |
| `openai_api_key` | `string?` | API key value (or `null` if not set) |
| `ollama_host` | `string` | Ollama server URL |
| `embedding_endpoint` | `string?` | Custom embedding endpoint URL (or `null`) |
| `source_dirs` | `string[]` | Directories to scan for markdown files |
| `ignore_patterns` | `string[]` | Additional ignore patterns |
| `watch_enabled` | `boolean` | Whether file watching is enabled |
| `watch_debounce_ms` | `number` | Debounce interval in milliseconds |
| `chunk_max_tokens` | `number` | Maximum tokens per chunk |
| `chunk_overlap_tokens` | `number` | Overlap tokens between sub-split chunks |
| `clustering_enabled` | `boolean` | Whether clustering is enabled |
| `clustering_rebalance_threshold` | `number` | Rebalance trigger threshold |
| `search_default_limit` | `number` | Default number of search results |
| `search_min_score` | `number` | Minimum similarity score threshold |
| `search_default_mode` | `string` | Default search mode: `"Hybrid"`, `"Semantic"`, or `"Lexical"` |
| `search_rrf_k` | `number` | RRF fusion constant |
| `bm25_norm_k` | `number` | BM25 saturation normalization constant |
| `search_decay_enabled` | `boolean` | Whether time decay is active by default |
| `search_decay_half_life` | `number` | Half-life in days for time decay |
| `search_decay_exclude` | `string[]` | Path prefixes excluded from decay |
| `search_decay_include` | `string[]` | Path prefixes where decay applies |
| `search_boost_links` | `boolean` | Whether link boosting is enabled by default |
| `search_boost_hops` | `number` | Number of link-graph hops for boosting |
| `search_expand_graph` | `number` | Graph expansion depth (0 = disabled) |
| `search_expand_limit` | `number` | Maximum number of graph-expanded results |
| `vector_quantization` | `string` | Vector quantization type: `"F16"` or `"F32"` |
| `index_compression` | `boolean` | Whether zstd metadata compression is enabled |
| `edge_embeddings` | `boolean` | Whether edge embeddings are computed |
| `edge_boost_weight` | `number` | Weight for edge-based boost in search scoring |
| `edge_cluster_rebalance` | `number` | Threshold for rebalancing edge clusters |

## Notes

- The `config` command reads configuration but **never modifies** any files.
- The `openai_api_key` field is included in JSON output. Be cautious when sharing or logging JSON config output.
- The JSON output reflects the **fully resolved** configuration after all sources have been merged. There is no way to see which source provided each value via the CLI; the config resolution order is documented in [Configuration](../configuration.md).
- In human-readable mode, the API key is displayed as `set` or `not set` rather than showing the actual value.

## Config Resolution Order

The config command shows the result of merging all configuration sources in this priority order:

1. **Shell environment variables** (highest priority)
2. **`.markdownvdb/.config`** (project config)
3. **`.markdownvdb`** (legacy flat file, if no `.config` exists)
4. **`.env`** (shared secrets fallback)
5. **`~/.mdvdb/config`** (user-level defaults)
6. **Built-in defaults** (lowest priority)

See [Configuration](../configuration.md) for detailed documentation of the resolution order, all environment variables, and config file formats.

## Related Commands

- [`mdvdb init`](./init.md) -- Create a new configuration file
- [`mdvdb doctor`](./doctor.md) -- Validate configuration and test provider connectivity
- [`mdvdb status`](./status.md) -- View index status and embedding configuration snapshot
- [`mdvdb ingest`](./ingest.md) -- Index files using the resolved configuration

## See Also

- [Configuration](../configuration.md) -- Complete configuration reference with all 35+ environment variables
- [Embedding Providers](../concepts/embedding-providers.md) -- Provider setup for OpenAI, Ollama, and custom
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference

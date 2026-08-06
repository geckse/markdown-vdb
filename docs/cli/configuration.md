---
title: "Configuration"
description: "Configure mdvdb with YAML, environment overrides, and separate secret files"
category: "guides"
---

# Configuration

mdvdb keeps ordinary settings in YAML and credentials in `.env` files. A project initialized with
`mdvdb init` has this layout:

```text
my-notes/
├── .markdownvdb/
│   ├── config.yaml   # project settings
│   ├── .env          # optional project credentials; never YAML
│   ├── index         # generated vector/metadata index
│   ├── fts/          # generated BM25 segments
│   └── cache/        # optional disposable derived state
└── ... Markdown files ...
```

User defaults live at `~/.mdvdb/config.yaml`; user-level credentials live at
`~/.mdvdb/.env`. Set `MDVDB_CONFIG_HOME` to replace the `~/.mdvdb` directory.

## Resolution and merging

Ordinary settings resolve in this order, from highest to lowest priority:

1. Shell `MDVDB_*` environment overrides
2. Project `.markdownvdb/config.yaml`
3. User `~/.mdvdb/config.yaml`
4. Built-in defaults

User and project YAML are deep-merged: mappings merge key by key, while a higher-priority scalar
or sequence replaces the lower-priority value. Set `MDVDB_NO_USER_CONFIG=1` to skip user settings.

Credentials and connection secrets have their own precedence:

1. Shell environment
2. `<project>/.env`
3. `<project>/.markdownvdb/.env`
4. `~/.mdvdb/.env`
5. Legacy `~/.mdvdb/config`

Only values already present in the shell may act as `MDVDB_*` overrides. `MDVDB_*` entries loaded
from `.env` are discarded; put ordinary settings in YAML.

Legacy project dotenv configurations are migrated to `config.yaml` when loaded. Non-`MDVDB_*`
values are preserved in a sibling `.env`, and the old config is retained as a backup. New
documentation and automation should use YAML directly.

## Canonical YAML shape

Every section is optional. Omitted values use the next lower-priority source or the built-in
default.

```yaml
# .markdownvdb/config.yaml
embedding:
  provider: openai
  model: text-embedding-3-small
  dimensions: auto
  batch_size: 100
  # endpoint: https://example.test/v1/embeddings

search:
  limit: 10
  min_score: 0.0
  mode: hybrid          # hybrid | semantic | lexical | edge
  rrf_k: 60.0
  bm25_norm_k: 1.5
  boost_links: false
  boost_hops: 1
  expand_graph: 0
  expand_limit: 3
  decay:
    enabled: false
    half_life: 90
    include: []
    exclude: []

chunking:
  max_tokens: 512
  overlap_tokens: 50

clustering:
  enabled: true
  algorithm: leiden    # leiden | kmeans
  knn: 15
  resolution: 1.0
  min_cluster_size: 2
  rebalance_threshold: 50
  granularity: 1.0     # K-means fallback only
  topics:
    min_similarity: 0.30
  custom: []

watch:
  enabled: true
  debounce_ms: 300

index:
  quantization: f16    # f16 | f32
  compression: true
  edge_embeddings: true
  edge_boost_weight: 0.15
  edge_cluster_rebalance: 50

sources:
  dirs: [.]
  ignore: []
```

`.gitignore`, `.mdvdbignore`, built-in directory exclusions, and `sources.ignore` all participate
in discovery. Paths are relative to the collection root.

## Embedding providers

Provider and model are independent: model identifiers are opaque provider-native strings. Use
`dimensions: auto` unless you need to pin a known positive dimension.

| Provider | `embedding.provider` | Credential / connection |
|---|---|---|
| OpenAI | `openai` | `OPENAI_API_KEY`; optional `embedding.endpoint` |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` |
| Google Gemini | `gemini` | `GEMINI_API_KEY` |
| Azure OpenAI | `azure` | `AZURE_OPENAI_ENDPOINT` plus API key or bearer token |
| AWS Bedrock | `bedrock` | Bedrock bearer token, AWS credentials, or a profile |
| Hugging Face | `huggingface` | `HF_TOKEN` for serverless; optional for private endpoints |
| Ollama | `ollama` | `OLLAMA_HOST`, default `http://localhost:11434` |
| OpenAI-compatible | `custom` | Exact `embedding.endpoint`; optional bearer token |

### OpenAI

```yaml
embedding:
  provider: openai
  model: text-embedding-3-small
  dimensions: auto
```

```dotenv
OPENAI_API_KEY=...
```

### Ollama

```yaml
embedding:
  provider: ollama
  model: nomic-embed-text
  dimensions: auto
```

```dotenv
# Optional; this is the default
OLLAMA_HOST=http://localhost:11434
```

### OpenAI-compatible endpoint

```yaml
embedding:
  provider: custom
  model: provider-native-model-id
  dimensions: auto
  endpoint: https://embeddings.example.test/v1/embeddings
```

Azure authentication mode, Gemini purpose values, Hugging Face endpoint behavior, and Bedrock
request codecs use nested provider options. See the
[provider transport guide](https://github.com/geckse/markdown-vdb/blob/main/docs/embedding-providers.md)
for those schemas.

Inspect live models when the provider exposes a catalog, or make a minimal inference to resolve
dimensions:

```bash
mdvdb embedding models --json
mdvdb embedding models --provider openrouter --json
mdvdb embedding probe --json
```

Catalog discovery is advisory, not an allowlist. Changing provider, model, dimensions, endpoint,
purpose, or codec changes the embedding space; run `mdvdb ingest --reindex` before semantic search
or incremental ingestion continues. The previous on-disk generation remains intact if probing or
replacement fails.

## Secrets

Never put credentials in `config.yaml`. Either export them in the shell, edit an appropriate
`.env`, or use the stdin-only secret command:

```bash
# Project-local .markdownvdb/.env
printf '%s' "$OPENAI_API_KEY" \
  | mdvdb config secret set OPENAI_API_KEY --stdin

# Shared ~/.mdvdb/.env
printf '%s' "$OPENAI_API_KEY" \
  | mdvdb config --global secret set OPENAI_API_KEY --stdin

# Remove an entry
mdvdb config secret unset OPENAI_API_KEY
```

Supported names are:

```text
OPENAI_API_KEY
OPENROUTER_API_KEY
GEMINI_API_KEY
AZURE_OPENAI_API_KEY
AZURE_OPENAI_ACCESS_TOKEN
HF_TOKEN
AWS_BEARER_TOKEN_BEDROCK
AWS_ACCESS_KEY_ID
AWS_SECRET_ACCESS_KEY
AWS_SESSION_TOKEN
OLLAMA_HOST
```

## Search, graph, and decay examples

```yaml
search:
  mode: hybrid
  limit: 20
  min_score: 0.2
  boost_links: true
  boost_hops: 2
  expand_graph: 1
  expand_limit: 5
  decay:
    enabled: true
    half_life: 30
    include: [memory/daily]
    exclude: [memory/pinned]
```

Decay applies `0.5^(age_days / half_life)` to eligible result scores. Exclusions take precedence
over inclusions. Per-query flags such as `--decay`, `--decay-half-life`, `--boost-links`, and
`--expand` override the defaults for one search.

## Leiden communities and Topics

Leiden community detection is the default automatic clustering algorithm. K-means remains an
optional fallback.

Topics are independent, user-defined semantic groupings. A document can belong to multiple Topics:

```yaml
clustering:
  algorithm: leiden
  knn: 15
  resolution: 1.0
  min_cluster_size: 2
  topics:
    min_similarity: 0.30
  custom:
    - name: Reliability
      description: Incidents, recovery, and resilience work
      seeds: [incident, rollback, failover]
      threshold: 0.35
```

Prefer `mdvdb clusters add|update|remove|list` for Topic mutations so writes are locked and atomic.
Run ingest after changing a Topic so its centroid and assignments can be computed.

## Project-local Shards

Shards are named recursive folder lenses over one shared collection index. They are deliberately
read only from the raw project config and never inherited from user YAML:

```yaml
shards:
  research:
    name: Research
    path: notes/research
    topics:
      - name: Methods
        description: Research methods and evaluation
        seeds: [methodology, experiment]
        threshold: 0.35
```

Use [`mdvdb shards`](./commands/shards.md) for CRUD. Shard-local Topics do not inherit from the
Collection or from ancestor, sibling, or child Shards. Derived Shard analysis is disposable cache;
document embeddings and link topology remain in the shared index.

## Schema overlays are separate

`.markdownvdb.schema.yml` is not runtime configuration. It annotates inferred frontmatter fields
and declares Relation, Formula, Lookup, and Rollup columns. Computed values are materialized into
Markdown frontmatter and become available to `search` filters and `collection` filtering/sorting.
See the [Quick Start](./quickstart.md#6-add-relations-and-computed-fields) for an example.

## Manage and inspect settings

Edit YAML directly or use dotted keys for scalar updates:

```bash
mdvdb config set search.limit 20
mdvdb config set clustering.algorithm leiden
mdvdb config unset search.min_score

# Mutate user defaults instead
mdvdb config --global set search.limit 20

# Show the fully resolved runtime configuration
mdvdb config
mdvdb config --json
```

Environment overrides remain useful for CI and one-off commands:

```bash
MDVDB_SEARCH_MODE=lexical mdvdb search "exact identifier"
MDVDB_NO_USER_CONFIG=1 mdvdb doctor
```

Common mappings include:

| Environment variable | YAML key |
|---|---|
| `MDVDB_EMBEDDING_PROVIDER` | `embedding.provider` |
| `MDVDB_EMBEDDING_MODEL` | `embedding.model` |
| `MDVDB_EMBEDDING_DIMENSIONS` | `embedding.dimensions` (numeric override) |
| `MDVDB_SOURCE_DIRS` | `sources.dirs` (comma-separated) |
| `MDVDB_IGNORE_PATTERNS` | `sources.ignore` (comma-separated) |
| `MDVDB_SEARCH_MODE` | `search.mode` |
| `MDVDB_SEARCH_DEFAULT_LIMIT` | `search.limit` |
| `MDVDB_SEARCH_DECAY` | `search.decay.enabled` |
| `MDVDB_SEARCH_DECAY_HALF_LIFE` | `search.decay.half_life` |
| `MDVDB_CLUSTERING_ALGORITHM` | `clustering.algorithm` |
| `MDVDB_WATCH` | `watch.enabled` |

## Related pages

- [`mdvdb init`](./commands/init.md) — create project or user YAML
- [`mdvdb config`](./commands/config.md) — inspect configuration command behavior
- [`mdvdb doctor`](./commands/doctor.md) — validate config, provider, and index
- [Quick Start](./quickstart.md) — complete first-run workflow
- [Search Modes](./concepts/search-modes.md) — retrieval behavior
- [Time Decay](./concepts/time-decay.md) — decay semantics and path controls
- [Ignore Files](./concepts/ignore-files.md) — discovery exclusions

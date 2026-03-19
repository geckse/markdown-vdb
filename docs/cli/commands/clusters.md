---
title: "mdvdb clusters"
description: "Show document clusters with K-means grouping and TF-IDF keyword labels"
category: "commands"
---

# mdvdb clusters

Show document clusters computed during ingestion. Documents are grouped using K-means clustering on their embedding vectors, with each cluster labeled using cross-cluster TF-IDF keyword extraction. This provides a high-level overview of the thematic structure of your content.

## Usage

```bash
mdvdb clusters [OPTIONS]
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

## How Clustering Works

mdvdb uses K-means clustering to group documents by semantic similarity:

1. **Document vectors** -- Each file's embedding is computed by averaging the vectors of all its chunks. This produces one vector per file.
2. **K selection** -- The number of clusters (K) is automatically determined based on the number of documents, scaled by the `MDVDB_CLUSTER_GRANULARITY` setting.
3. **K-means** -- The [linfa](https://github.com/rust-ml/linfa) K-means implementation groups document vectors into K clusters (up to 100 iterations, tolerance 1e-4).
4. **Keyword extraction** -- Cross-cluster TF-IDF analysis extracts the top 5 keywords that are most distinctive to each cluster compared to the corpus as a whole. Common English stop words are filtered out.
5. **Label generation** -- Each cluster's label is generated from its top keywords (e.g., "api, authentication, endpoints").

### Rebalancing

Clusters are not recomputed on every ingestion. A full rebalance is triggered when the number of documents added since the last rebalance exceeds `MDVDB_CLUSTERING_REBALANCE_THRESHOLD` (default: 50). Between rebalances, new documents are assigned to the nearest existing centroid.

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `MDVDB_CLUSTERING_ENABLED` | `true` | Enable or disable clustering |
| `MDVDB_CLUSTERING_REBALANCE_THRESHOLD` | `50` | Number of new documents before triggering a full rebalance |
| `MDVDB_CLUSTER_GRANULARITY` | `1.0` | Cluster granularity multiplier (range: 0.25 to 4.0). Lower values produce fewer, larger clusters. Higher values produce more, smaller clusters. |

See [Configuration](../configuration.md) for the full configuration reference.

## Human-Readable Output

When run without `--json`, clusters displays a formatted summary with distribution bars:

```
  ● Document Clusters (4 clusters, 57 documents)

  Cluster 0: ████████████████████ 22 docs api, authentication, endpoints
    Keywords: api, authentication, endpoints, oauth, tokens

  Cluster 1: █████████████░░░░░░░ 15 docs deployment, docker, kubernetes
    Keywords: deployment, docker, kubernetes, helm, ci

  Cluster 2: ██████████░░░░░░░░░░ 12 docs getting-started, tutorial, quickstart
    Keywords: getting-started, tutorial, quickstart, installation, setup

  Cluster 3: ████████░░░░░░░░░░░░ 8 docs architecture, design, patterns
    Keywords: architecture, design, patterns, modules, interfaces
```

### Output Elements

| Element | Description |
|---------|-------------|
| **Header** | Total cluster count and total document count |
| **Cluster ID** | Numeric cluster identifier (0-based) |
| **Distribution bar** | 20-character bar proportional to the largest cluster |
| **Document count** | Number of files in this cluster |
| **Label** | Auto-generated label from top keywords (or "(unlabeled)" if empty) |
| **Keywords** | Top 5 TF-IDF keywords distinguishing this cluster from others |

### Empty State

If no clusters are available (no files have been ingested, or clustering is disabled):

```
  ✗ No clusters available. Run mdvdb ingest first.
```

## Examples

```bash
# Show document clusters
mdvdb clusters

# Show clusters as JSON
mdvdb clusters --json

# Show clusters for a specific project
mdvdb clusters --root /path/to/project

# Show clusters with debug logging
mdvdb clusters -vv
```

## JSON Output

### ClusterSummary Array (`--json`)

The JSON output is an array of `ClusterSummary` objects:

```json
[
  {
    "id": 0,
    "document_count": 22,
    "label": "api, authentication, endpoints",
    "keywords": ["api", "authentication", "endpoints", "oauth", "tokens"]
  },
  {
    "id": 1,
    "document_count": 15,
    "label": "deployment, docker, kubernetes",
    "keywords": ["deployment", "docker", "kubernetes", "helm", "ci"]
  },
  {
    "id": 2,
    "document_count": 12,
    "label": "getting-started, tutorial, quickstart",
    "keywords": ["getting-started", "tutorial", "quickstart", "installation", "setup"]
  }
]
```

### ClusterSummary Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | Numeric cluster identifier (0-based) |
| `document_count` | `number` | Number of files belonging to this cluster |
| `label` | `string \| null` | Auto-generated label from keywords, or `null` if empty |
| `keywords` | `string[]` | Top TF-IDF keywords distinguishing this cluster |

### Empty State

When no clusters are available, the JSON output is an empty array:

```json
[]
```

## Notes

- The `clusters` command opens the index in **read-only** mode. It never modifies the index.
- Clustering operates at the **document level** (one vector per file), not the chunk level. Each file's vector is the average of its chunk vectors.
- Clusters are computed during [`mdvdb ingest`](./ingest.md), not during the `clusters` command itself.
- Zero-norm vectors (files whose chunks all have zero embeddings) are excluded from clustering.
- If only one document exists, it is placed in a single cluster.
- The cluster ID is not guaranteed to be stable across rebalances -- the same document may be assigned a different cluster ID after a rebalance.
- The `label` field is `null` (not an empty string) when the cluster has no meaningful label.

## Related Commands

- [`mdvdb ingest`](./ingest.md) -- Index files and compute clusters
- [`mdvdb schema`](./schema.md) -- View metadata schema (another data inspection command)
- [`mdvdb tree`](./tree.md) -- View file tree with sync status
- [`mdvdb status`](./status.md) -- Check index document count
- [`mdvdb graph`](./graph.md) -- View graph data including cluster membership

## See Also

- [Clustering](../concepts/clustering.md) -- Deep dive into K-means clustering, TF-IDF labels, and granularity tuning
- [Index Storage](../concepts/index-storage.md) -- How cluster state is persisted in the index
- [Configuration](../configuration.md) -- Clustering-related environment variables
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference

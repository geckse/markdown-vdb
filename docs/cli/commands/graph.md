---
title: "mdvdb graph"
description: "Show visualization-ready graph data with nodes, edges, and clusters for the indexed markdown files"
category: "commands"
---

# mdvdb graph

Show graph data (nodes, edges, clusters) for visualization. Outputs the complete graph structure of the indexed markdown files in a format suitable for graph visualization tools (e.g., D3.js, Cytoscape, Gephi). The graph can be generated at document level (one node per file) or chunk level (one node per chunk within each file).

## Usage

```bash
mdvdb graph [OPTIONS]
```

## Arguments

This command takes no arguments.

## Options

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--level` | | `document` | Graph granularity level: `document` or `chunk` |
| `--path` | | | Restrict graph to files under this path prefix |

### `--level <LEVEL>`

Controls the granularity of the graph nodes.

| Level | Description |
|-------|-------------|
| `document` (default) | One node per indexed file. Edges are markdown links between files. Clusters are document-level k-means clusters. |
| `chunk` | One node per chunk within each file. Edges are the top-k most similar chunk pairs across different files (based on cosine similarity of embeddings). Intra-file edges are excluded. |

At **document level**, edges come from the markdown link graph (explicit links between files). At **chunk level**, edges come from embedding similarity (the top 5 most similar cross-file chunks for each chunk).

### `--path <PREFIX>`

Restricts the graph to files whose paths start with the given prefix. Only nodes matching the prefix are included, and only edges between matching nodes are returned.

```bash
# Graph only for files under docs/
mdvdb graph --path docs/

# Graph only for API documentation
mdvdb graph --path docs/api/
```

## Global Options

These options apply to all commands. See [Commands Index](./index.md) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (-v info, -vv debug, -vvv trace) |
| `--root` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |

## Human-Readable Output

When run without `--json`, graph displays a summary of the graph data:

### Document Level

```
  ● Graph Data

  Level:     Document
  Nodes:     42
  Edges:     87
  Clusters:  5

    • Cluster 0 — api-reference, endpoints, authentication (12 docs)
    • Cluster 1 — setup, installation, configuration (8 docs)
    • Cluster 2 — architecture, design, patterns (10 docs)
    • Cluster 3 — guides, tutorials, howto (7 docs)
    • Cluster 4 — changelog, releases, versions (5 docs)
```

### Chunk Level

```
  ● Graph Data

  Level:     Chunk
  Chunks:    156
  Edges:     780
  Weights:   0.7234 — 0.9812

  Sample labels:
    • Introduction > Getting Started
    • API > Authentication > OAuth2
    • Architecture > Data Flow
    • Setup > Prerequisites
    • Deployment > Docker
```

### Output Elements

| Element | Description |
|---------|-------------|
| **Level** | The graph granularity (`Document` or `Chunk`) |
| **Nodes/Chunks** | Total number of graph nodes (files at document level, chunks at chunk level) |
| **Edges** | Total number of connections between nodes |
| **Clusters** | Number of document clusters with their labels and member counts (document level only) |
| **Weights** | Min and max edge weight range (chunk level only, based on cosine similarity) |
| **Sample labels** | Example chunk heading hierarchies (chunk level only) |

## Examples

```bash
# Show document-level graph summary
mdvdb graph

# Show chunk-level graph summary
mdvdb graph --level chunk

# Show document-level graph as JSON
mdvdb graph --json

# Show chunk-level graph as JSON
mdvdb graph --level chunk --json

# Restrict graph to a path prefix
mdvdb graph --path docs/api/

# Chunk-level graph for a specific directory as JSON
mdvdb graph --level chunk --path src/ --json

# Graph with debug logging
mdvdb graph -vv
```

## JSON Output

### GraphData (`--json`)

The JSON output is a `GraphData` object containing nodes, edges, and clusters arrays.

#### Document Level

```json
{
  "nodes": [
    {
      "id": "docs/architecture.md",
      "path": "docs/architecture.md",
      "label": null,
      "chunk_index": null,
      "cluster_id": 2
    },
    {
      "id": "docs/api/endpoints.md",
      "path": "docs/api/endpoints.md",
      "label": null,
      "chunk_index": null,
      "cluster_id": 0
    }
  ],
  "edges": [
    {
      "source": "docs/architecture.md",
      "target": "docs/api/endpoints.md",
      "weight": null,
      "relationship_type": "api-reference",
      "strength": 0.82,
      "context_text": "See the API endpoints documentation for details.",
      "edge_cluster_id": 1
    }
  ],
  "clusters": [
    {
      "id": 0,
      "label": "api-reference, endpoints, authentication",
      "keywords": ["api", "endpoints", "authentication", "rest", "http"],
      "member_count": 12
    },
    {
      "id": 2,
      "label": "architecture, design, patterns",
      "keywords": ["architecture", "design", "patterns", "system", "components"],
      "member_count": 10
    }
  ],
  "level": "document",
  "edge_clusters": [
    {
      "id": 0,
      "label": "api-documentation",
      "keywords": ["api", "documentation", "reference"],
      "member_count": 8
    },
    {
      "id": 1,
      "label": "setup-prerequisites",
      "keywords": ["setup", "install", "configure"],
      "member_count": 5
    }
  ]
}
```

#### Chunk Level

```json
{
  "nodes": [
    {
      "id": "docs/architecture.md#0",
      "path": "docs/architecture.md",
      "label": "Introduction",
      "chunk_index": 0,
      "cluster_id": 2,
      "size": 1234.0
    },
    {
      "id": "docs/architecture.md#1",
      "path": "docs/architecture.md",
      "label": "Architecture > Data Flow",
      "chunk_index": 1,
      "cluster_id": 2,
      "size": 890.0
    }
  ],
  "edges": [
    {
      "source": "docs/architecture.md#1",
      "target": "docs/api/endpoints.md#0",
      "weight": 0.8723
    }
  ],
  "clusters": [
    {
      "id": 0,
      "label": "api-reference, endpoints",
      "keywords": ["api", "endpoints", "authentication"],
      "member_count": 12
    }
  ],
  "level": "chunk"
}
```

### GraphData Fields

| Field | Type | Description |
|-------|------|-------------|
| `nodes` | `GraphNode[]` | All graph nodes (files at document level, chunks at chunk level) |
| `edges` | `GraphEdge[]` | Connections between nodes (links at document level, similarity at chunk level) |
| `clusters` | `GraphCluster[]` | Document cluster groupings with labels |
| `level` | `string` | Graph level: `"document"` or `"chunk"` |
| `edge_clusters` | `GraphCluster[]` | Edge cluster groupings (semantic relationship types). Only present at document level when edge clustering has been performed. Omitted when empty. |

### GraphNode Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | `string` | Unique node identifier. At document level: the file path. At chunk level: `"path#index"`. |
| `path` | `string` | Relative file path (same as `id` at document level) |
| `label` | `string?` | Display label. At document level: `null`. At chunk level: heading hierarchy joined by ` > ` (e.g., `"API > Authentication > OAuth2"`). |
| `chunk_index` | `number?` | Chunk index within the file. `null` at document level. |
| `cluster_id` | `number?` | Document cluster assignment. `null` if the file is not assigned to a cluster. |
| `size` | `number?` | Content length in characters (chunk level only). Omitted at document level. |

### GraphEdge Fields

| Field | Type | Description |
|-------|------|-------------|
| `source` | `string` | Source node `id` |
| `target` | `string` | Target node `id` |
| `weight` | `number?` | Edge weight. At document level: `null` (link-based). At chunk level: cosine similarity between chunk embeddings (0.0 to 1.0). |
| `relationship_type` | `string?` | Semantic relationship type label (document level only, from edge clustering). Omitted when not available. |
| `strength` | `number?` | Semantic edge strength -- cosine similarity between edge embedding and target document embedding (document level only). Omitted when not available. |
| `context_text` | `string?` | Paragraph context surrounding the link (document level only). Omitted when not available. |
| `edge_cluster_id` | `number?` | Edge cluster assignment (document level only). Omitted when not available. |

### GraphCluster Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | Cluster identifier (0-based) |
| `label` | `string` | Auto-generated label from top TF-IDF keywords |
| `keywords` | `string[]` | Cross-cluster TF-IDF keywords |
| `member_count` | `number` | Number of members in this cluster |

## Document vs Chunk Level

| Aspect | Document Level | Chunk Level |
|--------|----------------|-------------|
| **Nodes** | One per indexed file | One per chunk (heading section) |
| **Node ID** | File path (e.g., `docs/api.md`) | Chunk ID (e.g., `docs/api.md#0`) |
| **Edges** | Explicit markdown links | Top-k cosine similarity (cross-file only) |
| **Edge weight** | Not applicable (null) | Cosine similarity (0.0 to 1.0) |
| **Clusters** | Document-level k-means clusters | Same clusters (inherited from parent file) |
| **Edge metadata** | Relationship type, strength, context | None (similarity-only) |
| **Use case** | Knowledge graph visualization | Semantic similarity exploration |

## Notes

- The `graph` command opens the index in **read-only** mode. It never modifies the index.
- Run [`mdvdb ingest`](./ingest.md) to populate nodes (indexed files/chunks), edges (links and similarities), and clusters.
- At document level, edges come from the link graph. At chunk level, edges are computed from embedding similarity (top 5 most similar chunks per chunk, excluding intra-file pairs).
- The `edge_clusters` field is only populated at document level and only when edge clustering has been performed during ingestion (`MDVDB_EDGE_EMBEDDINGS=true`).
- For chunk-level graphs, the `size` field on nodes represents the character count of the chunk content, useful for sizing nodes in visualization.
- Cluster IDs at chunk level are inherited from the parent document's cluster assignment.
- The `--path` filter applies to both nodes and edges -- only edges between matching nodes are included.
- This command is designed to produce JSON output for piping into visualization tools. The human-readable output is a summary; use `--json` for the full graph data.

## Related Commands

- [`mdvdb edges`](./edges.md) -- Show semantic edges with relationship types and context
- [`mdvdb links`](./links.md) -- Show outgoing and incoming links for a specific file
- [`mdvdb orphans`](./orphans.md) -- Find files with no links at all
- [`mdvdb clusters`](./clusters.md) -- Show document clusters with keyword labels
- [`mdvdb search`](./search.md) -- Search with `--boost-links` or `--expand` for graph-aware results

## See Also

- [Link Graph](../concepts/link-graph.md) -- How mdvdb extracts links, builds semantic edges, and computes graph data
- [Clustering](../concepts/clustering.md) -- How document and edge clustering works
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference for all commands
- [Configuration](../configuration.md) -- Graph and edge config vars (`MDVDB_EDGE_EMBEDDINGS`, `MDVDB_EDGE_BOOST_WEIGHT`, `MDVDB_CLUSTERING_ENABLED`)

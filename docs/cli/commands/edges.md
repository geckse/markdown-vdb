---
title: "mdvdb edges"
description: "Show semantic edges between linked markdown files with relationship types and similarity scores"
category: "commands"
---

# mdvdb edges

Show semantic edges between linked files. Semantic edges are enriched link connections that include the paragraph context surrounding each link, an embedding-based similarity score (strength), and an auto-discovered relationship type label derived from edge clustering. This command displays the full catalog of semantic edges in the index, with optional filtering by file or relationship type.

## Usage

```bash
mdvdb edges [FILE] [OPTIONS]
```

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `FILE` | No | Filter edges to those involving this file (as source or target). Relative path, e.g., `docs/api.md`. |

When `FILE` is provided, only edges where the specified file is the source or the target are returned. When omitted, all semantic edges in the index are shown.

## Options

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--relationship` | | | Filter edges by relationship type (case-insensitive substring match on the cluster label) |

### `--relationship <TYPE>`

Filters edges to only those whose auto-discovered relationship type label contains the given substring (case-insensitive). Relationship types are generated from edge clustering during ingestion -- they are TF-IDF keyword labels describing groups of semantically similar edges.

For example, `--relationship "api"` would match edges with relationship types like "api-reference", "API Documentation", or "rest-api-endpoints".

If no edges match the filter, an empty result is returned.

## Global Options

These options apply to all commands. See [Commands Index](./index.md) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (-v info, -vv debug, -vvv trace) |
| `--root` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |

## Human-Readable Output

When run without `--json`, edges displays a formatted list of semantic connections:

```
  ● Semantic Edges (12)

    docs/architecture.md → docs/api/endpoints.md [api-reference] ████████░░ 0.82
      "See the API endpoints documentation for details on..."

    docs/setup.md → docs/prerequisites.md [setup-guides] ██████░░░░ 0.65
      "Before proceeding, make sure you have the prerequis..."

    docs/index.md → docs/architecture.md [overview] █████████░ 0.91
      "For a deep dive into the system architecture, see..."
```

If no semantic edges exist:

```
  ✗ No semantic edges found. Run mdvdb ingest first.
```

### Output Elements

| Element | Description |
|---------|-------------|
| **Edge count** | Total number of semantic edges (shown in header) |
| **Source → Target** | The source and target files connected by this edge |
| **Relationship type** | The auto-discovered cluster label in brackets (e.g., `[api-reference]`) |
| **Strength bar** | A visual bar representing the cosine similarity (0.0 to 1.0) |
| **Strength score** | The numeric cosine similarity between the edge embedding and target document embedding |
| **Context excerpt** | Truncated paragraph text surrounding the link (up to 80 characters) |

## Examples

```bash
# Show all semantic edges
mdvdb edges

# Show edges involving a specific file
mdvdb edges docs/architecture.md

# Filter edges by relationship type
mdvdb edges --relationship "api"

# Combine file filter and relationship filter
mdvdb edges docs/architecture.md --relationship "reference"

# Show edges as JSON
mdvdb edges --json

# Show edges for a file as JSON
mdvdb edges docs/api.md --json

# Show edges with debug logging
mdvdb edges -vv
```

## JSON Output

### EdgesOutput (`--json`)

```json
{
  "edges": [
    {
      "edge_id": "edge:docs/architecture.md->docs/api/endpoints.md@12",
      "source": "docs/architecture.md",
      "target": "docs/api/endpoints.md",
      "context_text": "See the API endpoints documentation for details on authentication and rate limiting.",
      "line_number": 12,
      "strength": 0.82,
      "relationship_type": "api-reference",
      "cluster_id": 2
    },
    {
      "edge_id": "edge:docs/setup.md->docs/prerequisites.md@24",
      "source": "docs/setup.md",
      "target": "docs/prerequisites.md",
      "context_text": "Before proceeding, make sure you have the prerequisites installed.",
      "line_number": 24,
      "strength": 0.65,
      "relationship_type": "setup-guides",
      "cluster_id": 0
    }
  ],
  "total_edges": 2,
  "file": "docs/architecture.md",
  "relationship_filter": "api"
}
```

### EdgesOutput Fields

| Field | Type | Description |
|-------|------|-------------|
| `edges` | `SemanticEdge[]` | Array of semantic edge objects |
| `total_edges` | `number` | Total number of edges returned (after filtering) |
| `file` | `string?` | The file filter applied, if any (omitted if no file filter) |
| `relationship_filter` | `string?` | The relationship filter applied, if any (omitted if no filter) |

### SemanticEdge Fields

| Field | Type | Description |
|-------|------|-------------|
| `edge_id` | `string` | Unique edge identifier in format `"edge:source.md->target.md@LINE"` |
| `source` | `string` | Source file path (relative to project root) |
| `target` | `string` | Target file path (resolved relative to project root) |
| `context_text` | `string` | Paragraph context surrounding the link in the source file |
| `line_number` | `number` | Line number of the link in the source file (1-based) |
| `strength` | `number?` | Cosine similarity between the edge embedding and the target document embedding (0.0 to 1.0). `null` if not yet computed. |
| `relationship_type` | `string?` | Auto-discovered relationship type label from edge clustering. `null` if edges have not been clustered. |
| `cluster_id` | `number?` | Edge cluster ID this edge belongs to. `null` if not clustered. |

### Edge ID Format

The `edge_id` uniquely identifies each semantic edge using the format:

```
edge:{source_path}->{target_path}@{line_number}
```

The line number disambiguates multiple links from the same source to the same target file.

## How Semantic Edges Work

Semantic edges are created during ingestion when `MDVDB_EDGE_EMBEDDINGS=true` (the default). The process:

1. **Link extraction** -- During parsing, markdown links and wikilinks are extracted with their surrounding paragraph context.
2. **Edge embedding** -- The paragraph context text is embedded using the configured embedding provider.
3. **Strength calculation** -- Cosine similarity is computed between the edge embedding and the target document's embedding.
4. **Edge clustering** -- Edges are grouped by semantic similarity using k-means clustering, and each cluster gets a TF-IDF keyword label as its relationship type.

## Notes

- The `edges` command opens the index in **read-only** mode. It never modifies the index.
- Semantic edges require `MDVDB_EDGE_EMBEDDINGS=true` (the default). If disabled, no edges will be found.
- Run [`mdvdb ingest`](./ingest.md) to compute edge embeddings and cluster them.
- The `--relationship` filter performs a case-insensitive substring match, so `--relationship "api"` matches "api-reference", "REST API", "api-docs", etc.
- The `file` argument filters edges where the specified file is **either** the source or the target.
- Edge strength ranges from 0.0 to 1.0 -- higher values indicate the link context is more semantically related to the target document.
- Relationship types are auto-discovered through clustering -- they are not manually assigned. The labels come from TF-IDF keyword extraction over the edge context paragraphs in each cluster.

## Related Commands

- [`mdvdb links`](./links.md) -- Show outgoing and incoming links for a specific file
- [`mdvdb backlinks`](./backlinks.md) -- Show files linking TO a specific file
- [`mdvdb orphans`](./orphans.md) -- Find files with no links at all
- [`mdvdb graph`](./graph.md) -- Visualization-ready graph data including edge metadata
- [`mdvdb search`](./search.md) -- Search with `--edge-search` mode to search edge embeddings
- [`mdvdb clusters`](./clusters.md) -- Show document clusters (separate from edge clusters)

## See Also

- [Link Graph](../concepts/link-graph.md) -- How mdvdb extracts links, builds semantic edges, and clusters them by relationship type
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference for all commands
- [Configuration](../configuration.md) -- Edge-related config vars (`MDVDB_EDGE_EMBEDDINGS`, `MDVDB_EDGE_BOOST_WEIGHT`, `MDVDB_EDGE_CLUSTER_REBALANCE`)

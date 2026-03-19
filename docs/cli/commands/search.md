---
title: "mdvdb search"
description: "Semantic, lexical, hybrid, and edge search across indexed markdown files"
category: "commands"
---

# mdvdb search

Search across indexed markdown files using semantic (vector), lexical (BM25), hybrid (RRF fusion), or edge-based search. Returns ranked results with chunk content, file metadata, and optional graph context.

## Usage

```bash
mdvdb search [OPTIONS] <QUERY>
```

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `<QUERY>` | Yes | Natural-language search query string |

## Options

| Flag | Short | Value | Default | Description |
|------|-------|-------|---------|-------------|
| `--limit` | `-l` | `<N>` | `10` | Maximum number of results to return |
| `--min-score` | | `<F>` | `0.0` | Minimum similarity score threshold (0.0 to 1.0) |
| `--filter` | `-f` | `<KEY=VALUE>` | | Metadata filter expression (repeatable) |
| `--boost-links` | | | `false` | Enable link boosting (favor results linked to/from top matches) |
| `--no-boost-links` | | | | Disable link boosting (even if enabled in config) |
| `--mode` | | `<MODE>` | `hybrid` | Search mode: `hybrid`, `semantic`, `lexical`, or `edge` |
| `--semantic` | | | | Shorthand for `--mode=semantic` |
| `--lexical` | | | | Shorthand for `--mode=lexical` |
| `--edge-search` | | | | Shorthand for `--mode=edge` (search edge embeddings) |
| `--path` | | `<PREFIX>` | | Restrict search to files under this path prefix |
| `--decay` | | | `false` | Enable time decay (favor recently modified files) |
| `--no-decay` | | | | Disable time decay (even if enabled in config) |
| `--decay-half-life` | | `<DAYS>` | `90.0` | Half-life in days for time decay (how many days until score is halved) |
| `--decay-exclude` | | `<PATTERNS>` | | Comma-separated path prefixes excluded from time decay |
| `--decay-include` | | `<PATTERNS>` | | Comma-separated path prefixes where time decay applies (whitelist) |
| `--hops` | | `<N>` | `1` | Number of link hops for graph-aware boosting (1-3) |
| `--expand` | | `<N>` | `0` | Graph expansion depth for context (0-3, 0 disables) |

### Option Details

#### `--limit`, `-l`

Sets the maximum number of results returned. The default is controlled by `MDVDB_SEARCH_DEFAULT_LIMIT` (default: `10`). See [Configuration](../configuration.md).

#### `--min-score`

Filters out results below this similarity threshold. Score interpretation varies by mode:

- **Semantic**: cosine similarity (absolute, 0.0-1.0)
- **Lexical**: BM25 score normalized via saturation `score / (score + k)`
- **Hybrid**: RRF score normalized by theoretical maximum

The default is controlled by `MDVDB_SEARCH_MIN_SCORE` (default: `0.0`). See [Configuration](../configuration.md).

#### `--filter`, `-f`

Metadata filter expressions in `KEY=VALUE` format. Filters match against frontmatter fields using AND logic (all filters must match). The flag can be repeated to apply multiple filters.

**Auto-type detection**: The value is automatically parsed as:

| Input | Detected Type | Example |
|-------|---------------|---------|
| A valid number | Number | `--filter year=2024` |
| `true` or `false` | Boolean | `--filter draft=false` |
| Anything else | String | `--filter status=published` |

If the frontmatter field is an array, the filter checks whether the array *contains* the value. For example, `--filter tags=rust` matches a document with `tags: [rust, cli, tools]`.

```bash
# Single filter
mdvdb search "query" --filter status=published

# Multiple filters (AND logic)
mdvdb search "query" -f status=published -f draft=false -f year=2024
```

#### `--path`

Restricts results to files whose path starts with the given prefix. Useful for scoping searches to a subdirectory.

```bash
# Only search within the docs/ directory
mdvdb search "authentication" --path docs/

# Only search within a specific project area
mdvdb search "error handling" --path src/api/
```

#### `--hops`

Sets the link-hop depth for graph-aware boosting. When enabled, results that are linked to or from other top results receive a score boost.

- **Range**: 1 to 3
- **Requires**: `--boost-links` must also be set
- **Default**: `1` (controlled by `MDVDB_SEARCH_BOOST_HOPS`)

```bash
# Boost results linked within 2 hops of top matches
mdvdb search "API design" --boost-links --hops 2
```

#### `--expand`

Controls graph context expansion depth. When set to a value greater than 0, the response includes additional chunks from files linked to the search results, providing supplementary context.

- **Range**: 0 to 3 (0 disables expansion)
- **Default**: `0` (controlled by `MDVDB_SEARCH_EXPAND_GRAPH`)

```bash
# Include chunks from directly linked files
mdvdb search "authentication" --expand 1

# Include chunks up to 3 hops away
mdvdb search "authentication" --expand 3
```

## Conflicting Options

Certain options are mutually exclusive and cannot be used together:

| Option A | Conflicts With | Reason |
|----------|---------------|--------|
| `--semantic` | `--lexical`, `--mode`, `--edge-search` | Only one search mode can be active |
| `--lexical` | `--semantic`, `--mode`, `--edge-search` | Only one search mode can be active |
| `--edge-search` | `--semantic`, `--lexical`, `--mode` | Only one search mode can be active |
| `--mode` | `--semantic`, `--lexical`, `--edge-search` | `--mode` sets the mode explicitly |
| `--boost-links` | `--no-boost-links` | Cannot enable and disable simultaneously |
| `--decay` | `--no-decay` | Cannot enable and disable simultaneously |

### Dependency Requirements

| Option | Requires | Reason |
|--------|----------|--------|
| `--hops` | `--boost-links` | Hop depth is only meaningful when link boosting is active |

## Search Modes

The search mode determines which retrieval signals are used. The default mode is `hybrid` (controlled by `MDVDB_SEARCH_MODE`).

| Mode | Flag | Description |
|------|------|-------------|
| `hybrid` | `--mode=hybrid` | Combines semantic (HNSW) and lexical (BM25) search using Reciprocal Rank Fusion (RRF). Best for general-purpose queries. |
| `semantic` | `--semantic` or `--mode=semantic` | Embedding-based vector search only (cosine similarity via HNSW). Best for meaning-based queries. Requires an embedding API call. |
| `lexical` | `--lexical` or `--mode=lexical` | BM25 keyword search only (via Tantivy). Best for exact term matching. No embedding API call needed. |
| `edge` | `--edge-search` or `--mode=edge` | Searches semantic edge embeddings between linked documents. Returns edge results instead of chunk results. |

For detailed explanations and diagrams, see [Search Modes](../concepts/search-modes.md).

## Time Decay

When enabled, time decay applies an exponential penalty to older files, favoring recently modified content. The formula is:

```
adjusted_score = score * 2^(-age_in_days / half_life_days)
```

- **`--decay`** / **`--no-decay`**: Override the config default (`MDVDB_SEARCH_DECAY`, default: `false`)
- **`--decay-half-life`**: Override the half-life (`MDVDB_SEARCH_DECAY_HALF_LIFE`, default: `90.0` days)
- **`--decay-exclude`**: Comma-separated path prefixes excluded from decay (e.g., `docs/reference,docs/api`)
- **`--decay-include`**: Comma-separated path prefixes where decay applies (whitelist mode)

Exclude takes precedence over include. For detailed explanations, see [Time Decay](../concepts/time-decay.md).

## Global Options

These options apply to all commands. See [Commands Index](./index.md) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (-v info, -vv debug, -vvv trace) |
| `--root` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |

## Examples

### Basic search

```bash
# Simple semantic search (uses default hybrid mode)
mdvdb search "how to authenticate users"

# Limit to 5 results with minimum score
mdvdb search "error handling" --limit 5 --min-score 0.3
```

### Search with filters

```bash
# Filter by frontmatter field
mdvdb search "deployment" --filter status=published

# Multiple filters (AND logic)
mdvdb search "API endpoints" -f category=backend -f draft=false

# Filter with numeric value
mdvdb search "release notes" --filter year=2024
```

### Search modes

```bash
# Semantic only (meaning-based, no keyword matching)
mdvdb search "things that are similar to authentication" --semantic

# Lexical only (exact keyword matching, no API call)
mdvdb search "AuthenticationError" --lexical

# Explicit mode flag
mdvdb search "user management" --mode=hybrid

# Edge search (find relationships between documents)
mdvdb search "references to auth module" --edge-search
```

### Path-scoped search

```bash
# Only search within docs/
mdvdb search "configuration" --path docs/

# Search within a specific directory
mdvdb search "error types" --path src/
```

### Link boosting and graph expansion

```bash
# Boost results that are linked to/from top matches
mdvdb search "architecture" --boost-links

# Multi-hop link boosting (2 hops)
mdvdb search "architecture" --boost-links --hops 2

# Include context from linked files
mdvdb search "authentication" --expand 1

# Combine boosting and expansion
mdvdb search "API design" --boost-links --hops 2 --expand 2
```

### Time decay

```bash
# Favor recently modified files
mdvdb search "release notes" --decay

# Custom half-life (30 days)
mdvdb search "release notes" --decay --decay-half-life 30

# Exclude reference docs from decay
mdvdb search "best practices" --decay --decay-exclude docs/reference,docs/api

# Apply decay only to blog posts
mdvdb search "updates" --decay --decay-include blog/
```

### JSON output

```bash
# JSON output for programmatic consumption
mdvdb search "authentication" --json

# JSON with timing information (requires -v)
mdvdb search "authentication" --json -v

# Pipe to jq for processing
mdvdb search "error handling" --json | jq '.results[].file.path'
```

## JSON Output

When `--json` is used, the output is a `SearchOutput` object:

```json
{
  "results": [
    {
      "score": 0.87,
      "chunk": {
        "chunk_id": "docs/auth.md#2",
        "heading_hierarchy": ["Authentication", "OAuth2 Flow"],
        "content": "The OAuth2 flow begins with...",
        "start_line": 15,
        "end_line": 42
      },
      "file": {
        "path": "docs/auth.md",
        "frontmatter": {
          "title": "Authentication Guide",
          "tags": ["auth", "security"]
        },
        "file_size": 4096,
        "path_components": ["docs", "auth.md"],
        "modified_at": 1710000000
      }
    }
  ],
  "query": "authentication",
  "total_results": 1,
  "mode": "hybrid"
}
```

### SearchOutput Fields

| Field | Type | Description |
|-------|------|-------------|
| `results` | `SearchResult[]` | Ranked search results ordered by relevance score |
| `query` | `string` | The original query string |
| `total_results` | `number` | Number of results returned |
| `mode` | `string` | The search mode used (`hybrid`, `semantic`, `lexical`, or `edge`) |
| `timings` | `SearchTimings?` | Timing breakdown (only included when `-v` is used) |
| `graph_context` | `GraphContextItem[]` | Chunks from linked files (only included when non-empty, requires `--expand`) |
| `edge_results` | `EdgeSearchResult[]` | Edge search results (only included when non-empty, `--edge-search` mode) |

### SearchResult Fields

| Field | Type | Description |
|-------|------|-------------|
| `score` | `number` | Relevance score (0.0-1.0) |
| `chunk.chunk_id` | `string` | Chunk identifier (e.g., `"path.md#0"`) |
| `chunk.heading_hierarchy` | `string[]` | Heading hierarchy leading to this chunk |
| `chunk.content` | `string` | The text content of the matched chunk |
| `chunk.start_line` | `number` | 1-based start line in the source file |
| `chunk.end_line` | `number` | 1-based end line in the source file (inclusive) |
| `file.path` | `string` | Relative path to the source markdown file |
| `file.frontmatter` | `object?` | Parsed YAML frontmatter (null if absent) |
| `file.file_size` | `number` | File size in bytes |
| `file.path_components` | `string[]` | Path split into components |
| `file.modified_at` | `number?` | Filesystem modification time as Unix timestamp |

### SearchTimings Fields

Included when `-v` (verbose) flag is used:

| Field | Type | Description |
|-------|------|-------------|
| `embed_secs` | `number` | Time spent embedding the query |
| `vector_search_secs` | `number` | Time spent in HNSW vector search (0 if lexical-only) |
| `lexical_search_secs` | `number` | Time spent in BM25 lexical search (0 if semantic-only) |
| `fusion_secs` | `number` | Time spent in RRF fusion (0 if not hybrid) |
| `assemble_secs` | `number` | Time spent assembling results (filtering, decay, link boosting) |
| `total_secs` | `number` | Total wall-clock time |

### GraphContextItem Fields

Included when `--expand` is set to a value greater than 0:

| Field | Type | Description |
|-------|------|-------------|
| `chunk` | `SearchResultChunk` | The matched chunk from the linked file |
| `file` | `SearchResultFile` | File-level metadata for the linked file |
| `linked_from` | `string` | Path of the result file this item is linked from |
| `hop_distance` | `number` | Number of link hops from the result file (1 = direct link) |
| `edge_context` | `string?` | Contextual information about the connecting edge |
| `edge_relationship` | `string?` | Relationship type of the connecting edge |

### EdgeSearchResult Fields

Included when using `--edge-search` or `--mode=edge`:

| Field | Type | Description |
|-------|------|-------------|
| `score` | `number` | Relevance score for this edge match |
| `edge_id` | `string` | Unique identifier for this edge |
| `source_path` | `string` | Source document path |
| `target_path` | `string` | Target document path |
| `link_text` | `string` | Link text connecting source to target |
| `context` | `string` | Contextual text surrounding the link |
| `relationship_type` | `string?` | Relationship type label (e.g., "references", "extends") |
| `cluster_id` | `number?` | Cluster ID this edge belongs to |
| `strength` | `number?` | Edge strength/weight |

## Configuration

The following environment variables affect search behavior. Set them in `.markdownvdb/.config`, `.env`, or `~/.mdvdb/config`. See [Configuration](../configuration.md) for full details.

| Variable | Default | Description |
|----------|---------|-------------|
| `MDVDB_SEARCH_DEFAULT_LIMIT` | `10` | Default result limit |
| `MDVDB_SEARCH_MIN_SCORE` | `0.0` | Default minimum score |
| `MDVDB_SEARCH_MODE` | `hybrid` | Default search mode |
| `MDVDB_SEARCH_RRF_K` | `60.0` | RRF fusion constant |
| `MDVDB_BM25_NORM_K` | `1.5` | BM25 score normalization constant |
| `MDVDB_SEARCH_DECAY` | `false` | Enable time decay by default |
| `MDVDB_SEARCH_DECAY_HALF_LIFE` | `90.0` | Default decay half-life in days |
| `MDVDB_SEARCH_DECAY_EXCLUDE` | | Paths excluded from decay |
| `MDVDB_SEARCH_DECAY_INCLUDE` | | Paths where decay applies |
| `MDVDB_SEARCH_BOOST_LINKS` | `false` | Enable link boosting by default |
| `MDVDB_SEARCH_BOOST_HOPS` | `1` | Default link boost hops |
| `MDVDB_SEARCH_EXPAND_GRAPH` | `0` | Default graph expansion depth |
| `MDVDB_SEARCH_EXPAND_LIMIT` | `3` | Maximum expanded context results |
| `MDVDB_EDGE_BOOST_WEIGHT` | `0.15` | Edge boost weight |

## Related Commands

- [`mdvdb ingest`](./ingest.md) -- Index files before searching
- [`mdvdb status`](./status.md) -- Check index health and counts
- [`mdvdb links`](./links.md) -- Explore the link graph for a file
- [`mdvdb edges`](./edges.md) -- View semantic edges between documents
- [`mdvdb config`](./config.md) -- View resolved search configuration

## See Also

- [Search Modes](../concepts/search-modes.md) -- Detailed explanation of hybrid, semantic, lexical, and edge search
- [Time Decay](../concepts/time-decay.md) -- How time-based scoring works
- [Link Graph](../concepts/link-graph.md) -- Link boosting and graph expansion explained
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference
- [Configuration](../configuration.md) -- All environment variables and config options

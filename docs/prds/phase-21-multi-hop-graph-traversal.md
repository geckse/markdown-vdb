# PRD: Phase 21 — Multi-Hop Graph Traversal Search System

## Overview

Extend the existing link graph (Phase 15) with multi-hop BFS traversal, graph-aware score boosting, and graph context expansion for search results. When a user searches, files linked to top results receive a configurable score boost based on hop distance. Optionally, chunks from linked files are surfaced as supplementary "graph context" alongside primary results. A new neighborhood API provides recursive link trees at configurable depth. All features are backward-compatible — boosting defaults to 1 hop, expansion is disabled by default.

## Problem Statement

Phase 15 introduced link extraction, backlinks, and single-hop link-aware search boosting. However, knowledge graphs are rarely one hop deep — a design doc links to an API spec, which links to a data model, forming a chain of related content. The existing `--boost-links` flag only considers direct (1-hop) neighbors, missing transitive relationships that carry strong topical relevance.

Additionally, when an agent retrieves search results, it often needs context from related documents to fully understand the answer. Currently, the agent must issue follow-up queries for each linked file. Graph context expansion automates this by including the most relevant chunks from linked files directly in the search response.

## Goals

- Multi-hop BFS traversal through both forward links and backlinks (depth 1–3)
- Configurable hop-depth score boost with exponential decay per hop
- Graph context expansion: surface best-matching chunks from linked files as a separate `graph_context` section
- Per-config defaults via `MDVDB_SEARCH_BOOST_HOPS`, `MDVDB_SEARCH_EXPAND_GRAPH`, `MDVDB_SEARCH_EXPAND_LIMIT`
- Per-query overrides via `--hops` and `--expand` CLI flags and `SearchQuery` builder methods
- Recursive neighborhood tree API (`links_neighborhood`) for link exploration at depth 1–3
- Human-readable tree formatting for neighborhood output
- Works with all three search modes (semantic, lexical, hybrid)
- Backward-compatible — no index format changes, no forced re-ingest
- `graph_context` field omitted from JSON when empty (zero noise for existing users)

## Non-Goals

- Changing the link extraction or backlink computation from Phase 15
- Index format changes or version bumps
- Graph visualization (handled separately in the Electron app)
- PageRank or other global graph centrality metrics
- Cross-index linking (only links within the same project)
- Weighting forward links differently from backlinks in BFS

## Technical Design

### Score Boost Formula

Exponential decay per hop — closer links get stronger boosts:

```
multiplier = 1.0 + 0.15 / 2^(distance - 1)
```

| Hop Distance | Multiplier | Boost |
|---|---|---|
| 1 | 1.150 | +15.0% |
| 2 | 1.075 | +7.5% |
| 3 | 1.0375 | +3.75% |

Boosting seeds: top 3 result files are used as BFS seeds. Only files outside the seed set receive boosts. Results are re-sorted by score after boosting.

### BFS Multi-Hop Traversal

**`bfs_neighbors()`** in `src/links.rs`:

```rust
pub fn bfs_neighbors(
    graph: &LinkGraph,
    backlinks: &HashMap<String, Vec<LinkEntry>>,
    seeds: &[String],
    max_depth: usize,
) -> HashMap<String, usize>  // path → min hop distance
```

- Standard BFS from seed files through both forward links and backlinks
- Returns map of discovered file paths to their minimum hop distance
- Hard depth cap at 3 (clamped internally)
- Seed files excluded from output
- Visited set prevents cycles; each file recorded at first (minimum) distance
- Returns empty map if `max_depth == 0` or `seeds.is_empty()`
- Time complexity: O(V + E)

### Graph Context Expansion

**`expand_graph_context()`** in `src/search.rs`:

When `expand_graph > 0`, the search pipeline appends a `graph_context` section containing the most relevant chunks from linked files that are NOT already in the primary results.

Algorithm:
1. Collect file paths from primary results (exclusion set)
2. Use top 3 result files as BFS seeds
3. BFS at configured depth to discover neighboring files
4. Filter out files already in results
5. HNSW similarity search across expansion targets using the query embedding
6. Per target file, select the chunk with the highest similarity score (fallback: first chunk)
7. Group by hop distance, sort by score within each hop
8. Truncate to `expand_limit` items **per hop level**
9. Final sort: hop distance ascending, then path alphabetically for determinism

**`GraphContextItem`** struct:

```rust
pub struct GraphContextItem {
    pub chunk: SearchResultChunk,   // Best-matching chunk from linked file
    pub file: SearchResultFile,     // File metadata
    pub linked_from: String,        // Which seed result this is linked from
    pub hop_distance: usize,        // BFS hop distance (1 = direct link)
}
```

**Linked-from resolution**: For each expanded item, find which seed file has a direct connection (forward or backward). Falls back to first seed for multi-hop cases with no direct link.

**Query embedding reuse**: In lexical-only mode with expansion enabled, the query embedding is computed specifically for HNSW lookup during expansion (not wasted otherwise).

### SearchResponse Wrapper

```rust
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub graph_context: Vec<GraphContextItem>,
    pub timings: SearchTimings,
}
```

The `search()` method returns `SearchResponse` instead of `Vec<SearchResult>`. The `graph_context` field is skipped in JSON serialization when empty, maintaining backward compatibility.

### Neighborhood Tree API

**`neighborhood()`** in `src/links.rs`:

```rust
pub fn neighborhood(
    graph: &LinkGraph,
    known_files: &HashSet<String>,
    file: &str,
    depth: usize,
) -> NeighborhoodResult
```

Builds two recursive trees — outgoing (forward links) and incoming (backlinks) — at configurable depth (clamped to 1–3). Uses per-branch cycle detection to allow a file to appear in multiple branches but not recurse infinitely.

**Data structures:**

```rust
pub struct NeighborhoodNode {
    pub path: String,
    pub state: LinkState,               // Valid or Broken
    pub children: Vec<NeighborhoodNode>,
}

pub struct NeighborhoodResult {
    pub file: String,
    pub outgoing: Vec<NeighborhoodNode>,
    pub incoming: Vec<NeighborhoodNode>,
    pub outgoing_count: usize,
    pub incoming_count: usize,
    pub outgoing_depth_count: usize,     // Actual depth explored
    pub incoming_depth_count: usize,
}
```

### Configuration

| Field | Env Var | Default | Range | Description |
|---|---|---|---|---|
| `search_boost_hops` | `MDVDB_SEARCH_BOOST_HOPS` | 1 | 1–3 | BFS depth for link score boosting |
| `search_expand_graph` | `MDVDB_SEARCH_EXPAND_GRAPH` | 0 | 0–3 | Graph context expansion depth (0 = disabled) |
| `search_expand_limit` | `MDVDB_SEARCH_EXPAND_LIMIT` | 3 | 1–10 | Max items per hop level in graph context |

Validation rejects out-of-range values with descriptive error messages.

### CLI Flags

**Search command:**

| Flag | Type | Range | Requires | Description |
|---|---|---|---|---|
| `--hops N` | `u8` | 1–3 | `--boost-links` | Override boost hop depth |
| `--expand N` | `u8` | 0–3 | — | Override graph expansion depth |

**Links command:**

| Flag | Type | Range | Default | Description |
|---|---|---|---|---|
| `--depth N` | `u8` | 1–3 | 1 | Neighborhood traversal depth |

### SearchQuery Builder Methods

```rust
pub fn with_boost_hops(mut self, hops: usize) -> Self
pub fn with_expand_graph(mut self, depth: usize) -> Self
```

Per-query overrides; `None` falls back to config defaults.

### Library API

```rust
// Existing search() now returns SearchResponse
vdb.search(query) -> Result<SearchResponse>

// New: multi-hop neighborhood tree
vdb.links_neighborhood(path, depth) -> Result<NeighborhoodResult>
```

### JSON Output

Search JSON includes `graph_context` when non-empty:

```json
{
  "results": [...],
  "query": "example",
  "total_results": 5,
  "mode": "hybrid",
  "timings": {...},
  "graph_context": [
    {
      "chunk": { "content": "...", "heading": "...", ... },
      "file": { "path": "docs/api.md", ... },
      "linked_from": "docs/main.md",
      "hop_distance": 1
    }
  ]
}
```

### Human-Readable Formatting

**Graph context** (`print_graph_context` in `src/format.rs`):
- Grouped by hop level with "→" prefix
- Shows linked-from path (dimmed), section heading, content preview (150 chars)

**Neighborhood tree** (`print_link_neighborhood` in `src/format.rs`):
- Tree connectors: `├──`, `└──`, `│` for proper visual nesting
- `[broken]` badge in red for invalid link targets
- Summary with total outgoing/incoming counts

### Search Pipeline Integration

The graph features integrate into the existing search pipeline as two final steps:

1. **Link Boost** (after truncation): BFS from top 3 results at `boost_hops` depth, apply multiplier, re-sort
2. **Graph Expansion** (after boost): BFS from top 3 results at `expand_graph` depth, HNSW lookup for best chunks, assemble `GraphContextItem` list

Pipeline by mode:
- **Semantic**: embed → HNSW → filter → assemble → truncate → **boost** → **expand**
- **Lexical**: BM25 → normalize → filter → assemble → truncate → **boost** → **expand**
- **Hybrid**: semantic + lexical → RRF → normalize → filter → assemble → truncate → **boost** → **expand**

### Re-exports

All new public types exported from `lib.rs`:

```rust
pub use search::{GraphContextItem, SearchResponse};
pub use links::{NeighborhoodNode, NeighborhoodResult};
```

## Testing Requirements

- **BFS traversal**: 1-hop, 2-hop, 3-hop depth; cycle handling; empty seeds; bidirectional traversal
- **Neighborhood tree**: depth 1/2/3; cycle detection per branch; broken link states; node/depth counting
- **Graph expansion**: disabled when expand=0; items not duplicating results; hop grouping and per-hop limits; linked_from resolution
- **Score boosting**: multiplier correctness at each hop; re-sorting after boost; no boost when disabled
- **Config**: parsing valid values; rejecting out-of-range values (hops=0, hops=4, expand=4, limit=0, limit=11)
- **CLI**: `--hops` requires `--boost-links`; `--expand` standalone; `--depth` on links command; JSON output structure with graph_context
- **API**: `SearchQuery` builder methods; `SearchResponse` structure; `links_neighborhood` returns correct tree
- **Backward compat**: default config produces identical results to pre-phase-21 behavior

## Acceptance Criteria

- [ ] `bfs_neighbors()` correctly traverses forward links and backlinks to configurable depth (1–3)
- [ ] `neighborhood()` returns recursive tree with per-branch cycle detection
- [ ] Score boost applies exponentially decaying multiplier per hop distance
- [ ] Graph expansion surfaces best-matching chunks from linked files, limited per hop
- [ ] `SearchResponse` wraps results + graph_context + timings
- [ ] `graph_context` omitted from JSON when empty
- [ ] Config validates all three new env vars with correct ranges
- [ ] CLI `--hops`, `--expand`, `--depth` flags work with range validation
- [ ] `SearchQuery::with_boost_hops()` and `with_expand_graph()` override config defaults
- [ ] `links_neighborhood()` API method available on `MarkdownVdb`
- [ ] Human-readable formatting for graph context and neighborhood trees
- [ ] All tests pass (`cargo test`)
- [ ] Clippy clean (`cargo clippy --all-targets`)

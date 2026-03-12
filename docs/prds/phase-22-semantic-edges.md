# PRD: Phase 22 — Semantic Edges (Auto-Inferred Graph RAG)

## Overview

Transform the flat link graph into a semantically-rich knowledge graph by extracting paragraph-level context around every link, embedding it alongside chunk vectors, and globally clustering all edge embeddings to auto-discover relationship types. Enable edge-weighted search boosting and a novel edge-first retrieval mode. New `mdvdb edges` CLI command and `--edge-search` flag on search. App visualization is covered separately in `docs/prds/app/phase-22-semantic-edge-visualization.md`.

## Problem Statement

The current link graph (Phase 15) stores `LinkEntry` objects with `{source, target, text, line_number, is_wikilink}`. Every edge is treated identically — a casual "see also" mention gets the same BFS traversal weight as a critical dependency link. The link boost in search applies a flat multiplier `1.0 + 0.15 / 2^(distance-1)` regardless of *why* the link exists or whether the link's context is relevant to the query.

This creates three problems:

1. **Blind graph traversal** — BFS follows all edges equally. When searching for "authentication flow", a link from `auth.md` to `changelog.md` (mentioning auth was updated) gets the same boost as a link to `token-validator.md` (a hard dependency). The irrelevant link pollutes graph expansion results.

2. **No edge semantics** — Users cannot ask "what depends on this file?" vs "what references this file?" because all edges are untyped. Traditional Graph RAG systems require labeled edges for meaningful traversal, but manually labeling every link is impractical.

3. **Invisible link intent** — The *reason* an author linked two documents lives in the surrounding paragraph, but this context is discarded during link extraction. When graph context is expanded in search results, the user sees the linked chunk but not *why* it was linked.

The data to solve all three problems already flows through the parser — the paragraph around each link explains the relationship. We just need to capture it, embed it, and use it.

## Goals

- Extract paragraph-level context around every internal link during parsing
- Embed link contexts alongside chunk embeddings in the same batch (no extra API round trips)
- Store edge embeddings in the shared HNSW index, distinguishable by `"edge:"` ID prefix
- Globally cluster all edge embeddings via K-means to auto-discover relationship types (no predefined taxonomy)
- Replace flat link boost with edge-weighted boost: `cosine(query_embedding, edge_embedding)` modulates the multiplier
- Add edge-first retrieval mode (`SearchMode::Edge`): search edges by semantic similarity, return connected documents
- New `mdvdb edges` CLI command to explore semantic edges
- New `--edge-search` flag on `mdvdb search` for edge-first retrieval
- Extend `mdvdb graph` JSON output with edge metadata (relationship type, strength, context excerpt)
- Extend `mdvdb graph` JSON output with edge metadata for app consumption (see app PRD)
- Backward compatible: old indices without semantic edges work unchanged
- All features have automated tests

## Non-Goals

- User-defined edge labels or typed link syntax (`[[target::depends-on]]`) — this is auto-inference only
- Frontmatter-declared relationships — separate future feature
- LLM-based edge classification — uses embedding clustering, not API calls for classification
- Temporal edge chains or modification-time-based edge directionality
- Edge-weighted PageRank or betweenness centrality
- Modifying markdown files — read-only as always
- App visualization details — covered in separate app PRD

---

## Technical Design

### Data Model Changes

**New types in `src/links.rs`:**

```rust
/// A semantic edge with extracted context from the link's surrounding paragraph.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, serde::Serialize)]
#[rkyv(derive(Debug))]
pub struct SemanticEdge {
    /// Deterministic ID: "edge:source.md->target.md@42" (line number for uniqueness).
    pub id: String,
    /// Source file path (relative).
    pub source: String,
    /// Target file path (relative).
    pub target: String,
    /// The paragraph surrounding the link (the "edge context").
    pub context: String,
    /// Display text of the link.
    pub link_text: String,
    /// Line number in source file (1-based).
    pub line_number: usize,
    /// Whether this was a [[wikilink]].
    pub is_wikilink: bool,
    /// Assigned edge cluster ID (auto-discovered relationship type), if clustered.
    pub cluster_id: Option<usize>,
    /// Relationship strength: cosine similarity between edge context embedding
    /// and target document's averaged embedding.
    pub strength: Option<f64>,
}

/// Global clustering state for semantic edges.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, serde::Serialize)]
#[rkyv(derive(Debug))]
pub struct EdgeClusterState {
    pub clusters: Vec<EdgeClusterInfo>,
    pub edges_since_rebalance: usize,
    pub edges_at_last_rebalance: usize,
}

/// A single edge cluster representing an auto-discovered relationship type.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, serde::Serialize)]
#[rkyv(derive(Debug))]
pub struct EdgeClusterInfo {
    pub id: usize,
    /// Auto-generated label from top TF-IDF keywords (e.g., "imports / requires / dependency").
    pub label: String,
    /// Cluster centroid vector.
    pub centroid: Vec<f32>,
    /// Edge IDs belonging to this cluster.
    pub member_edge_ids: Vec<String>,
    /// Top keywords extracted via cross-cluster TF-IDF on edge context text.
    pub keywords: Vec<String>,
}
```

Edge ID format: `"edge:notes/design.md->specs/auth.md@42"` — the `edge:` prefix distinguishes edge vectors from chunk vectors (`"path.md#0"`) in the shared HNSW index. The `@line` suffix ensures uniqueness when a file links to the same target from different paragraphs.

**Extend `LinkGraph` in `src/links.rs`:**

Two new `Option` fields (backward compatible — old indices deserialize with `None`):

```rust
pub struct LinkGraph {
    pub forward: HashMap<String, Vec<LinkEntry>>,
    pub last_updated: u64,
    pub semantic_edges: Option<HashMap<String, SemanticEdge>>,  // edge_id → edge
    pub edge_cluster_state: Option<EdgeClusterState>,
}
```

**New config fields in `src/config.rs`:**

| Field | Env Var | Default | Purpose |
|---|---|---|---|
| `edge_embeddings: bool` | `MDVDB_EDGE_EMBEDDINGS` | `true` | Enable/disable semantic edge extraction |
| `edge_boost_weight: f64` | `MDVDB_EDGE_BOOST_WEIGHT` | `0.15` | Max boost multiplier for edge-weighted search |
| `edge_cluster_rebalance: usize` | `MDVDB_EDGE_CLUSTER_REBALANCE` | `50` | Rebalance threshold for edge clusters |

**New error variant in `src/error.rs`:**

```rust
#[error("semantic edge error: {0}")]
SemanticEdge(String),
```

**New search mode variant in `src/search.rs`:**

```rust
pub enum SearchMode {
    Hybrid,
    Semantic,
    Lexical,
    Edge,  // Search edge embeddings only
}
```

**New search result type:**

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct EdgeSearchResult {
    pub edge_id: String,
    pub source_path: String,
    pub target_path: String,
    pub link_text: String,
    pub context: String,
    pub relationship_type: Option<String>,
    pub cluster_id: Option<usize>,
    pub strength: Option<f64>,
}
```

### Interface Changes

**New parser function in `src/parser.rs`:**

```rust
/// Extract the surrounding paragraph for a link at a given line number.
pub fn extract_link_paragraph(body: &str, line_number: usize) -> String

/// A link with its surrounding paragraph context.
pub struct LinkContext {
    pub raw_link: RawLink,
    pub paragraph: String,
}

/// Extract paragraph context for all links in a document body.
pub fn extract_links_with_context(body: &str, links: &[RawLink]) -> Vec<LinkContext>
```

Paragraph extraction algorithm:
1. Split `body` into lines
2. For each link at `line_number` (1-based), walk backward until: empty line, heading line (`# `), or start of file
3. Walk forward until: empty line, heading line, or end of file
4. Join the range as the paragraph
5. If result is empty → fall back to the single link line

**New index methods in `src/index/state.rs`:**

```rust
/// Insert edge embeddings into the shared HNSW index.
pub fn upsert_edges(&self, edges: &[(String, Vec<f32>)]) -> Result<()>

/// Retrieve all edge embeddings (for clustering).
pub fn get_edge_vectors(&self) -> HashMap<String, Vec<f32>>

/// Search HNSW filtered to edge IDs only.
pub fn search_edges(&self, query: &[f32], limit: usize) -> Result<Vec<(String, f64)>>
```

**New clustering functions in `src/clustering.rs`:**

```rust
/// Cluster edge embeddings globally to discover relationship types.
pub fn cluster_edges(
    edge_vectors: &HashMap<String, Vec<f32>>,
    edge_contexts: &HashMap<String, String>,
) -> Result<EdgeClusterState>

/// Assign a single new edge to the nearest existing cluster (incremental).
pub fn assign_edge_to_nearest(state: &mut EdgeClusterState, edge_id: &str, embedding: &[f32])

/// Rebalance edge clusters if threshold exceeded.
pub fn maybe_rebalance_edges(
    state: &mut EdgeClusterState,
    edge_vectors: &HashMap<String, Vec<f32>>,
    edge_contexts: &HashMap<String, String>,
    threshold: usize,
) -> Result<bool>
```

### New Commands / API / Library

| Surface | Change |
|---------|--------|
| CLI | `mdvdb edges [FILE] [--json] [--relationship TYPE]` — explore semantic edges |
| CLI | `mdvdb search "query" --edge-search` — edge-first retrieval mode |
| CLI | `mdvdb graph --json` — extended output includes `edge_clusters` and per-edge `relationship_type`, `strength`, `context_text`, `edge_cluster_id` |
| Library | `vdb.edges(file: Option<&str>) -> Result<Vec<SemanticEdge>>` |
| Library | `vdb.edge_clusters() -> Result<Option<EdgeClusterState>>` |
| Search | `SearchQuery.edge_search: Option<bool>` — enable edge-first mode |
| Search | `SearchResult.edge: Option<EdgeSearchResult>` — edge metadata on results |
| Search | `GraphContextItem.edge_context: Option<String>` — WHY the link exists |
| Search | `GraphContextItem.edge_relationship: Option<String>` — cluster label |

### Architecture Flow

**Ingest pipeline (enhanced):**

```
MarkdownFile ──→ extract_links() ──→ RawLink[]
                                         │
                      ┌──────────────────┤
                      ▼                  ▼
            chunk_document()    extract_links_with_context()
                      │                  │
                      ▼                  ▼
            Chunk[]              LinkContext[]
                      │                  │
                      └───────┬──────────┘
                              ▼
                  embed_chunks() (batched together, same API calls)
                              │
                  ┌───────────┴───────────┐
                  ▼                       ▼
        chunk embeddings          edge embeddings
                  │                       │
                  └───────────┬───────────┘
                              ▼
                  Shared HNSW Index
                  (chunk IDs: "path.md#0")
                  (edge IDs:  "edge:src->tgt@42")
                              │
                              ▼
                  cluster_edges() ──→ EdgeClusterState
                  (global K-means on edge embeddings)
                  (clusters = discovered relationship types)
```

Edge contexts share the same `source_path` as the file's chunks, so the hash-based skip logic works unchanged: when a file is unmodified, both its chunks and its edges are skipped; when it changes, both are re-embedded.

**Search pipeline (enhanced):**

Edge-weighted boost (replaces flat distance boost):
```
For each result linked to a top-result via a semantic edge:
  1. Look up the edge embedding from HNSW
  2. Compute cosine(query_embedding, edge_embedding)
  3. Boost = 1.0 + edge_boost_weight × cosine_sim / 2^(hop_distance − 1)
  If cosine_sim is low (edge irrelevant to query), boost is naturally small.
  If high, boost is strong. Query-aware, not flat.
```

Edge-first retrieval (`--edge-search`):
```
Query → embed → HNSW search (edge IDs only) → ranked edges
  → for each edge: return source + target files with relationship context
```

### Edge Clustering Design

Edge clustering differs from document clustering in important ways:

| Aspect | Document Clustering | Edge Clustering |
|---|---|---|
| What's clustered | Averaged chunk vectors per file | Edge context paragraph embeddings |
| What clusters represent | Topic groups ("security", "API docs") | Relationship types ("depends on", "elaborates") |
| Label source | TF-IDF on document content | TF-IDF on edge context paragraphs |
| K range | sqrt(n/2) clamped [2, 50] | sqrt(n/2) clamped [2, 20] |
| Stored in | `IndexMetadata.cluster_state` | `LinkGraph.edge_cluster_state` |

The innovation: relationship types are **discovered**, not declared. A vault might have edges that cluster into "dependency / import / requires", "elaboration / extends / builds on", "contradiction / unlike / however", and "reference / see also / mentioned in" — all inferred from the paragraph context around links.

### Index Storage

Edge embeddings go into the **same HNSW index** as chunk embeddings. The `id_to_key: HashMap<String, u64>` in `IndexState` already maps string IDs to HNSW u64 keys; edge IDs (`"edge:..."`) coexist naturally.

Changes to `src/index/state.rs`:
- `open_with_options()` — also load edge IDs from `link_graph.semantic_edges` into `id_to_key`
- `upsert_edges()` — new method, inserts edge vectors without adding to `metadata.chunks`
- `remove_file()` — also remove edge vectors where `source == file_path`
- `save()` compaction — extend sorted key set to include edge IDs alongside chunk IDs

### Incremental Updates

When a single file is re-ingested:
1. Remove old edge embeddings for that file from HNSW (scan `semantic_edges` by `source` field)
2. Remove old `SemanticEdge` entries
3. Extract new link contexts, embed, upsert, create new `SemanticEdge` entries
4. Assign new edges to nearest existing clusters (or rebalance if threshold exceeded)

### Edge Cases

- **Bare links** (link on its own line with blank lines above/below): paragraph is just that line — short but still embedded
- **Links in fenced code blocks**: wikilink regex currently matches inside code blocks; add code-fence tracking (``` line counting) to skip these; standard markdown links are already filtered by pulldown_cmark
- **Multiple links in one paragraph**: each gets its own `SemanticEdge` with the same paragraph text; embeddings are identical but edge metadata differs; acceptable cost
- **Files with no links**: no edge contexts generated; zero overhead
- **Empty paragraphs**: fall back to the link line text itself
- **Self-links**: already excluded by existing `build_link_graph()` deduplication

### Migration Strategy

No migration needed. `LinkGraph.semantic_edges` and `LinkGraph.edge_cluster_state` are `Option` fields. Existing indices load with `None` and work unchanged. Semantic edges are generated on next ingest when `MDVDB_EDGE_EMBEDDINGS=true` (default).

---

## Acceptance Criteria

### Rust / CLI

1. `cargo test` passes with all new tests; `cargo clippy --all-targets` clean
2. `mdvdb ingest` extracts link paragraphs, embeds them alongside chunks, and stores semantic edges
3. `mdvdb edges` lists all semantic edges with relationship types and strengths
4. `mdvdb edges <file>` lists edges for a specific file
5. `mdvdb edges --relationship "depends"` filters by relationship type substring
6. `mdvdb search "query" --edge-search` returns edges ranked by semantic relevance
7. `mdvdb search "query" --boost-links` uses edge-weighted boost (not flat distance boost) when semantic edges exist
8. `mdvdb graph --json` includes `edge_clusters` array and per-edge `relationship_type`, `strength`, `context_text`, `edge_cluster_id`
9. Edge clustering auto-discovers relationship types from edge context paragraphs
10. Incremental ingest correctly re-embeds edges for changed files and removes edges for deleted files
11. Old indices without semantic edges load and work unchanged (backward compat)
12. Edge embeddings are batched with chunk embeddings — no extra API round trips

### Testing

13. Unit tests for paragraph extraction (basic, heading boundary, start/end of file, bare link, code block, multiple links)
14. Unit tests for semantic edge ID generation and format
15. Unit tests for edge clustering (basic, too few edges, label generation, incremental assignment, rebalancing)
16. Unit tests for edge HNSW storage (upsert, save/load cycle, removal, search)
17. Unit tests for edge-weighted search boost and edge-first retrieval
18. Integration test: 3+ files with inter-links → ingest → verify edges → search → verify results
19. CLI test: `mdvdb edges` and `mdvdb search --edge-search` JSON output validation

---

## Implementation Phases

| Order | Phase | Scope | Depends On |
|---|---|---|---|
| 1 | A | Data structures, config, error variant | — |
| 2 | B | Parser: paragraph extraction around links | A |
| 3 | C | Index: edge vector storage in shared HNSW | A |
| 4 | D | Ingest: edge embedding pipeline integration | B, C |
| 5 | E | Clustering: global edge clustering | D |
| 6 | F | Search: edge-weighted boost + edge-first retrieval | D, E |
| 7 | G | CLI + public API: `edges` command, `--edge-search`, graph output | F |

Phases B and C can be implemented in parallel. Tests should be written alongside each phase.
App visualization is a separate phase covered in `docs/prds/app/phase-22-semantic-edge-visualization.md`.

---

## Files Modified

### Rust (core library)
- `src/links.rs` — `SemanticEdge`, `EdgeClusterState`, `EdgeClusterInfo`, extend `LinkGraph`
- `src/parser.rs` — `extract_link_paragraph()`, `LinkContext`, `extract_links_with_context()`
- `src/config.rs` — `edge_embeddings`, `edge_boost_weight`, `edge_cluster_rebalance`
- `src/error.rs` — `SemanticEdge` error variant
- `src/index/state.rs` — `upsert_edges`, `get_edge_vectors`, `search_edges`, modify `open`, `save`, `remove_file`
- `src/clustering.rs` — `cluster_edges`, `assign_edge_to_nearest`, `maybe_rebalance_edges`
- `src/search.rs` — `SearchMode::Edge`, edge-weighted boost, `EdgeSearchResult`, enhanced `GraphContextItem`
- `src/lib.rs` — ingest pipeline changes, `edges()`, `edge_clusters()` methods, re-exports
- `src/main.rs` — `Edges` subcommand, `--edge-search` flag, extended `GraphData`/`GraphEdge`
- `src/main/format.rs` — human-readable edge output

### Rust (tests)
- `tests/edges_test.rs` — NEW: end-to-end integration tests

### App
See `docs/prds/app/phase-22-semantic-edge-visualization.md`.

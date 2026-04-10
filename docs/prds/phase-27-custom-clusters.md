# PRD: Phase 27 — User-Defined Custom Clusters

## Overview

Add support for user-defined custom clusters alongside the existing automatic K-means clusters. Users define clusters by providing a name and a set of seed words/phrases in dotenv config. During ingest, seed phrases are embedded and averaged into centroids, then every document is assigned to its nearest custom cluster by cosine similarity. Custom clusters coexist as a separate layer — each document belongs to one auto-cluster AND one custom-cluster. Definitions live in `.markdownvdb/.config`, computed state (centroids + assignments) persists in the binary index.

## Problem Statement

The existing K-means clustering is purely algorithmic — it discovers latent structure but gives users no control over how documents are grouped. Users often have domain-specific mental models for organizing their knowledge base (e.g., "AI Research", "Project Management", "Personal Notes") that don't align with what K-means discovers.

There is no mechanism to express "I want these topics to be my clusters" and have documents sorted into them. Users must accept whatever the algorithm produces. For the app's 3D graph visualization, custom clusters would allow meaningful semantic grouping that maps to the user's actual taxonomy.

## Goals

- Users can define named clusters with seed words/phrases via `MDVDB_CUSTOM_CLUSTERS` env var
- Seed phrases are embedded during ingest to create stable cluster centroids
- Every document is assigned to exactly one custom cluster (nearest centroid by cosine similarity)
- Custom clusters are a separate layer — auto K-means clusters continue to operate independently
- Definitions stored in `.markdownvdb/.config` (human-editable, version-controllable)
- Computed state (centroids, member assignments) stored in binary index via rkyv
- CLI commands for adding/removing cluster definitions without manual config editing
- Custom cluster assignments included in graph data output for app visualization
- Backward-compatible index format (no version bump, no forced re-ingest)
- Incremental ingest assigns new files without re-embedding seeds

## Non-Goals

- Replacing auto K-means clusters — custom clusters coexist, not replace
- Threshold-based assignment — every document joins exactly one custom cluster (nearest wins)
- Automatic label generation for custom clusters — the user provides the name
- Keyword extraction for custom clusters — seeds serve as the "keywords"
- Seed documents (using existing files as exemplars) — only word/phrase seeds for now
- Custom clusters influencing search scoring — purely organizational, not search-boosting
- Multi-assignment (one document in multiple custom clusters)
- YAML/TOML config format — stays dotenv-consistent

## Technical Design

### Config Format

Single env var in `.markdownvdb/.config`:

```
MDVDB_CUSTOM_CLUSTERS=AI Research:machine learning,neural networks,deep learning|Web Dev:html,css,javascript,react|DevOps:docker,kubernetes,CI/CD,deployment
```

Format: `Name1:seed1,seed2,seed3|Name2:seed4,seed5`
- Pipe `|` separates clusters
- Colon `:` separates name from seeds
- Comma `,` separates seeds within a cluster

**Restrictions**: Names cannot contain `:` or `|`. Seeds cannot contain `,` or `|`.

### Data Model Changes

**`CustomClusterDef`** — config-layer only (not persisted in index):

```rust
#[derive(Debug, Clone)]
pub struct CustomClusterDef {
    pub name: String,
    pub seeds: Vec<String>,
}
```

**`CustomClusterInfo` in `src/clustering.rs`** — persisted in index:

```rust
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, serde::Serialize)]
#[rkyv(derive(Debug))]
pub struct CustomClusterInfo {
    pub id: usize,
    pub name: String,
    pub seed_phrases: Vec<String>,
    pub centroid: Vec<f32>,
    pub members: Vec<String>,  // file paths assigned to this cluster
}
```

**`CustomClusterState` in `src/clustering.rs`** — persisted in index:

```rust
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, serde::Serialize)]
#[rkyv(derive(Debug))]
pub struct CustomClusterState {
    pub clusters: Vec<CustomClusterInfo>,
}
```

No rebalance counters needed — custom cluster centroids are anchored to seed embeddings, not document content. Only member lists change.

**`IndexMetadata` in `src/index/types.rs`** — add field:

```rust
pub custom_cluster_state: Option<CustomClusterState>,  // NEW
```

Follows the same `Option<T>` pattern as `cluster_state`, `link_graph`, `file_mtimes`. Old indices deserialize with `None`.

**`CustomClusterSummary` in `src/lib.rs`** — API output type:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct CustomClusterSummary {
    pub id: usize,
    pub name: String,
    pub seed_phrases: Vec<String>,
    pub document_count: usize,
}
```

### Config Changes

One new field in `Config` (`src/config.rs`):

| Field | Env Var | Default | Type | Validation |
|---|---|---|---|---|
| `custom_cluster_defs` | `MDVDB_CUSTOM_CLUSTERS` | `[]` (empty) | `Vec<CustomClusterDef>` | No duplicate names, names non-empty, each cluster has at least one seed |

### Centroid Computation

During full ingest, for each custom cluster definition:

1. Embed all seed phrases via `provider.embed_batch(seeds)`
2. Average the resulting vectors element-wise: `centroid[i] = sum(embeddings[j][i]) / n`
3. Normalize to unit vector (for cosine similarity consistency)

This produces one centroid per custom cluster. Centroids are stable across incremental ingests unless seed definitions change.

### Assignment Algorithm

For each document vector (average of its chunk embeddings):

```rust
fn assign_to_nearest_custom(doc_vector: &[f32], centroids: &[Vec<f32>]) -> usize {
    centroids.iter()
        .enumerate()
        .map(|(i, c)| (i, cosine_similarity(doc_vector, c)))
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map(|(i, _)| i)
        .unwrap_or(0)
}
```

Every document gets assigned — there is no threshold. The nearest custom cluster always wins.

### Ingest Pipeline Integration

**Full ingest** (after existing K-means clustering block):

```
if config.custom_cluster_defs is non-empty:
    1. Embed all seed phrases → compute centroids (async, uses provider)
    2. For each document vector, assign to nearest custom centroid
    3. Build CustomClusterState with member lists
    4. Store in index via update_custom_clusters()
else if custom_cluster_state exists in index:
    Clear it (set to None) — definitions were removed
```

**Incremental (single-file) ingest**:

```
if config.custom_cluster_defs is non-empty AND custom_cluster_state exists in index:
    1. Load existing CustomClusterState (centroids already computed)
    2. Remove file from any existing cluster membership
    3. Compute document vector for the new/changed file
    4. Assign to nearest custom centroid
    5. Save updated state
```

Incremental ingest does NOT re-embed seeds — uses existing centroids from index. If seed definitions changed in config, the next full ingest will recompute centroids.

### Graph Data Changes

**`GraphNode`** — add field:

```rust
pub custom_cluster_id: Option<usize>,  // NEW: separate from cluster_id
```

**`GraphData`** — add field:

```rust
pub custom_clusters: Vec<GraphCluster>,  // NEW: reuses existing GraphCluster type
```

The `graph_data()` and `graph_data_chunks()` methods build a `path_to_custom_cluster` map from `get_custom_clusters()` and populate both fields.

### CLI Changes

Expand the `clusters` command with subcommands and a flag:

```
mdvdb clusters              Show auto clusters (unchanged)
mdvdb clusters --custom     Show computed custom cluster assignments
mdvdb clusters add <NAME> --seeds <SEEDS>   Add a custom cluster definition to config
mdvdb clusters remove <NAME>                Remove a custom cluster definition from config
mdvdb clusters list                         Show cluster definitions from config (no index needed)
```

**`clusters add`** implementation:
1. Read `.markdownvdb/.config` file (create if missing)
2. Parse existing `MDVDB_CUSTOM_CLUSTERS` line (if any)
3. Validate: name doesn't contain `:` or `|`, seeds don't contain `,` or `|`, no duplicate name
4. Append new cluster to the encoded value
5. Write back the modified file

**`clusters remove`** implementation:
1. Read `.markdownvdb/.config` file
2. Parse `MDVDB_CUSTOM_CLUSTERS`, filter out the named cluster
3. Write back (or remove the line entirely if no clusters remain)

**`clusters list`** output (human):
```
Custom cluster definitions:
  1. AI Research
     Seeds: machine learning, neural networks, deep learning
  2. Web Dev
     Seeds: html, css, javascript, react
```

**`clusters --custom`** output (human):
```
Custom clusters (3 clusters, 47 documents):
  1. AI Research (18 docs)
  2. Web Dev (15 docs)
  3. DevOps (14 docs)
```

JSON output mirrors `CustomClusterSummary[]`.

### Library API Changes

Add to `MarkdownVdb`:

```rust
pub fn custom_clusters(&self) -> Result<Vec<CustomClusterSummary>>
```

Returns summaries from the persisted `CustomClusterState`. Returns empty vec if no custom clusters defined/computed.

### Index State Accessors

Add to index state (`src/index/state.rs`):

```rust
pub fn get_custom_clusters(&self) -> Option<CustomClusterState>
pub fn update_custom_clusters(&self, state: Option<CustomClusterState>)
```

Follows identical pattern of existing `get_clusters()` / `update_clusters()`.

### Config Mutation Helper

New function for CLI `add`/`remove` commands:

```rust
pub fn update_config_value(config_path: &Path, key: &str, value: &str) -> Result<()>
```

Reads all lines from `.markdownvdb/.config`, replaces or appends the line matching `key=...`, writes back. Preserves comments and other lines. If `value` is empty, removes the line.

## Implementation Steps

0. **Save PRD** — Write to `docs/prds/phase-27-custom-clusters.md`.

1. **Data types** — `src/clustering.rs`: Add `CustomClusterDef`, `CustomClusterInfo`, `CustomClusterState` structs with appropriate derives. `src/index/types.rs`: Add `custom_cluster_state: Option<CustomClusterState>` to `IndexMetadata`.

2. **Index accessors** — `src/index/state.rs`: Add `get_custom_clusters()` and `update_custom_clusters()` following the existing `get_clusters()`/`update_clusters()` pattern.

3. **Config parsing** — `src/config.rs`: Add `custom_cluster_defs: Vec<CustomClusterDef>` field. Add `parse_custom_clusters()` function for `MDVDB_CUSTOM_CLUSTERS` env var. Add validation for duplicate names.

4. **Clustering logic** — `src/clustering.rs`: Add methods to `Clusterer`:
   - `embed_seed_centroids(defs, provider) -> Result<Vec<Vec<f32>>>` (async helper, called from lib.rs)
   - `assign_all_to_custom(defs, centroids, doc_vectors) -> CustomClusterState`
   - `assign_single_to_custom(state, path, vector)` (incremental, centroid unchanged)

5. **Ingest integration** — `src/lib.rs`: After existing clustering block in `ingest()`, add custom cluster computation for both full and incremental paths. Pre-embed seeds (async), then assign (sync).

6. **Library API** — `src/lib.rs`: Add `custom_clusters() -> Result<Vec<CustomClusterSummary>>`. Update `graph_data()` and `graph_data_chunks()` to include `custom_cluster_id` on nodes and `custom_clusters` on `GraphData`.

7. **Config mutation** — `src/config.rs` or `src/lib.rs`: Add `update_config_value()` helper for read-modify-write of `.markdownvdb/.config`.

8. **CLI commands** — `src/main.rs`: Expand `ClustersArgs` with `--custom` flag and `ClusterAction` subcommand enum (`Add`, `Remove`, `List`). Implement handlers.

9. **Tests** — Comprehensive coverage (see Validation Criteria).

10. **CLAUDE.md update** — Add `CustomClusterDef`, `CustomClusterInfo`, `CustomClusterState`, `CustomClusterSummary` to re-exports. Document `MDVDB_CUSTOM_CLUSTERS` env var. Add `custom_clusters()` to API listing.

## Files Modified

| File | Change |
|---|---|
| `src/clustering.rs` | `CustomClusterDef`, `CustomClusterInfo`, `CustomClusterState` types; `embed_seed_centroids()`, `assign_all_to_custom()`, `assign_single_to_custom()` methods |
| `src/index/types.rs` | Add `custom_cluster_state: Option<CustomClusterState>` to `IndexMetadata` |
| `src/index/state.rs` | `get_custom_clusters()`, `update_custom_clusters()` accessors |
| `src/config.rs` | `custom_cluster_defs` field, `parse_custom_clusters()`, validation, `update_config_value()` helper |
| `src/lib.rs` | Ingest integration, `custom_clusters()` API, `GraphNode.custom_cluster_id`, `GraphData.custom_clusters` |
| `src/main.rs` | `--custom` flag, `ClusterAction` subcommand (add/remove/list), handlers |
| `tests/clustering_test.rs` | Custom cluster assignment tests, incremental assignment, centroid stability |
| `tests/config_test.rs` | Parsing valid/invalid `MDVDB_CUSTOM_CLUSTERS`, duplicate name rejection |
| `tests/cli_test.rs` | `clusters add`, `clusters remove`, `clusters list`, `clusters --custom --json` |
| `tests/api_test.rs` | Full pipeline: define clusters → ingest → verify custom cluster state |
| `CLAUDE.md` | Config table, API listing, re-exports |

## Validation Criteria

- [ ] `cargo test` passes — all existing + new tests
- [ ] `cargo clippy --all-targets` — zero warnings
- [ ] Empty `MDVDB_CUSTOM_CLUSTERS` (or unset): no custom cluster state in index, no behavior change
- [ ] `MDVDB_CUSTOM_CLUSTERS=A:x,y|B:z` → after ingest, two custom clusters in index with all docs assigned
- [ ] Every document assigned to exactly one custom cluster (no unassigned, no multi-assigned)
- [ ] Auto K-means clusters unaffected — same assignments as before
- [ ] `mdvdb clusters` still shows auto clusters (unchanged behavior)
- [ ] `mdvdb clusters --custom --json` returns `CustomClusterSummary[]` with correct counts
- [ ] `mdvdb clusters add "Test" --seeds "foo,bar"` writes to `.markdownvdb/.config` correctly
- [ ] `mdvdb clusters remove "Test"` removes from `.markdownvdb/.config`
- [ ] `mdvdb clusters list` shows definitions without needing an index
- [ ] Duplicate cluster names rejected at config validation with clear error
- [ ] Incremental ingest assigns new file to nearest custom cluster without re-embedding seeds
- [ ] Custom cluster centroids remain stable across incremental ingests (anchored to seeds)
- [ ] Removing all cluster definitions from config → next full ingest clears custom cluster state
- [ ] Graph data JSON includes `custom_cluster_id` on nodes and `custom_clusters` array
- [ ] Old indices load correctly (`custom_cluster_state` = None, no error)
- [ ] Names with `:` or `|` rejected by `clusters add` with helpful message
- [ ] Seeds with `,` or `|` rejected by `clusters add` with helpful message

## Anti-Patterns to Avoid

- **Do not update custom centroids during incremental ingest** — Centroids are anchored to seed phrase embeddings. Only full ingest recomputes them. This ensures stability.

- **Do not merge custom and auto clusters into one list** — They are separate layers with different semantics. Auto clusters have algorithm-generated labels and shifting centroids. Custom clusters have user-defined names and fixed centroids.

- **Do not require an embedding provider for `clusters add/remove/list`** — These are pure config operations. Embedding only happens during `ingest`.

- **Do not bump index version** — The `Option<CustomClusterState>` approach avoids forcing re-ingest.

- **Do not store `CustomClusterDef` in the index** — Definitions live in config (editable). Only computed state (with centroids and assignments) goes in the index.

- **Do not apply custom clusters to search scoring** — Custom clusters are organizational. Search uses auto-clusters for link boosting. Keep concerns separate.

## Patterns to Follow

- **Optional IndexMetadata fields:** `cluster_state: Option<ClusterState>`, `link_graph: Option<LinkGraph>` in `src/index/types.rs:75-79`
- **Clustering methods:** `cluster_all()`, `assign_to_nearest()`, `maybe_rebalance()` in `src/clustering.rs`
- **Config parsing:** `parse_comma_list_string()`, `parse_env_bool()` in `src/config.rs`
- **CLI subcommands:** `SearchArgs`, `IngestArgs` pattern in `src/main.rs`
- **Graph data building:** `graph_data()` at `src/lib.rs:1612` builds node/edge/cluster structures
- **Index accessors:** `get_clusters()`/`update_clusters()` in `src/index/state.rs:487-586`

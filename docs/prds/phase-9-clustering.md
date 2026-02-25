# PRD: Phase 9 — Clustering

## Overview

Implement automatic document clustering using k-means on document embeddings, with incremental nearest-centroid assignment for new documents and periodic rebalancing. Cluster assignments are stored only in the index file (never in frontmatter). Agents can browse cluster summaries to understand the topics in a knowledge base before searching.

## Problem Statement

Large markdown vaults contain documents spanning many topics. Without clustering, an agent's only discovery mechanism is search — but search requires knowing what to ask. Clustering provides a browsable topic map: "here are the 12 main topics in this vault, each with N documents." This lets agents explore the knowledge base at a high level, then drill down with targeted searches.

## Goals

- Automatically group semantically similar documents into clusters using k-means
- Assign new documents to the nearest existing cluster on ingest (no full recluster)
- Rebalance clusters after `MDVDB_CLUSTERING_REBALANCE_THRESHOLD` new documents
- Store cluster assignments only in the index file — frontmatter is never modified
- Expose cluster summaries: label, document count, representative keywords/topics
- Configurable via `MDVDB_CLUSTERING_ENABLED` (default: true)
- Cluster count auto-determined based on corpus size (heuristic: sqrt(N/2))

## Non-Goals

- No hierarchical clustering (flat clusters only)
- No user-defined cluster labels (labels are auto-generated from content)
- No manual cluster assignment or override
- No cross-vault clustering
- No visualization of clusters

## Technical Design

### Data Model Changes

**`ClusterInfo` struct** — stored in the index:

```rust
#[derive(Archive, Serialize, Deserialize, Clone, serde::Serialize)]
pub struct ClusterInfo {
    /// Cluster ID (0-based index)
    pub id: usize,
    /// Auto-generated label from top keywords
    pub label: String,
    /// Centroid vector (average of member vectors)
    pub centroid: Vec<f32>,
    /// File paths (relative) belonging to this cluster
    pub members: Vec<String>,
    /// Top keywords extracted from cluster members' content
    pub keywords: Vec<String>,
}
```

**`ClusterState` struct** — clustering metadata in the index:

```rust
#[derive(Archive, Serialize, Deserialize, Clone, serde::Serialize)]
pub struct ClusterState {
    /// All clusters
    pub clusters: Vec<ClusterInfo>,
    /// Number of documents added since last rebalance
    pub docs_since_rebalance: usize,
    /// Total documents at last rebalance
    pub docs_at_last_rebalance: usize,
}
```

**Extend `IndexMetadata`** (from Phase 5):

```rust
// Add to IndexMetadata:
pub cluster_state: Option<ClusterState>,
```

### Interface Changes

```rust
/// Clustering operations
pub struct Clusterer {
    config: Config,
}

impl Clusterer {
    pub fn new(config: &Config) -> Self;

    /// Run initial k-means clustering on all document vectors
    pub fn cluster_all(
        &self,
        document_vectors: &HashMap<String, Vec<f32>>,
    ) -> Result<ClusterState>;

    /// Assign a single new document to the nearest existing cluster
    pub fn assign_to_nearest(
        &self,
        state: &mut ClusterState,
        doc_path: &str,
        doc_vector: &[f32],
    ) -> Result<usize>; // returns cluster ID

    /// Check if rebalancing is needed and do it if so
    pub fn maybe_rebalance(
        &self,
        state: &mut ClusterState,
        all_document_vectors: &HashMap<String, Vec<f32>>,
    ) -> Result<bool>; // returns true if rebalanced

    /// Generate a label from cluster members' content
    fn generate_label(
        member_contents: &[&str],
    ) -> String;

    /// Extract top keywords from content
    fn extract_keywords(
        contents: &[&str],
        n: usize,
    ) -> Vec<String>;
}
```

**Extend `Index`** (from Phase 5):

```rust
// Add to Index:
pub fn get_clusters(&self) -> Result<Option<ClusterState>>;
pub fn update_clusters(&self, state: ClusterState) -> Result<()>;
```

### Clustering Algorithm

**Initial clustering (on full ingest):**

1. Compute document-level vectors: average all chunk vectors per file
2. Determine k: `k = max(2, min(sqrt(N/2), 50))` where N = document count
3. Run k-means via `linfa-clustering` with max 100 iterations
4. For each cluster: collect member file paths, compute centroid, extract keywords, generate label
5. Store `ClusterState` in the index

**Incremental assignment (on single file ingest/update):**

1. Compute document vector (average of file's chunk vectors)
2. For each cluster centroid, compute cosine distance
3. Assign to nearest cluster
4. Increment `docs_since_rebalance`

**Rebalancing (when `docs_since_rebalance >= MDVDB_CLUSTERING_REBALANCE_THRESHOLD`):**

1. Recompute all document vectors from the index
2. Run k-means again with potentially updated k
3. Rebuild all cluster metadata
4. Reset `docs_since_rebalance` to 0

### Keyword/Label Generation

Simple TF-IDF-like approach without external dependencies:

1. Tokenize all member documents' content (split on whitespace and punctuation)
2. Remove common stop words (hardcoded list of ~100 English stop words)
3. Count term frequency across all cluster members
4. Weight by inverse document frequency across all clusters
5. Top 5 keywords by TF-IDF score become the `keywords`
6. Label = top 3 keywords joined with " / " (e.g., "rust / async / tokio")

### Migration Strategy

The `cluster_state` field is `Option<ClusterState>` in `IndexMetadata`. Existing indexes without clustering will have `None`. First ingest with clustering enabled will populate it.

## Implementation Steps

1. **Create `src/clustering.rs`** — Implement the clustering module:
   - Define `ClusterInfo`, `ClusterState`, `Clusterer` structs
   - Derive `rkyv` traits on stored types, `serde::Serialize` on API types

2. **Implement `Clusterer::cluster_all()`:**
   - Convert `HashMap<String, Vec<f32>>` to a 2D `ndarray::Array2<f64>` (linfa uses f64)
   - Compute `k` using the heuristic: `max(2, min((N as f64 / 2.0).sqrt() as usize, 50))`
   - If N < 2 documents, return a single cluster containing everything
   - Create `linfa_clustering::KMeans::params(k)` with `max_n_iterations(100)` and `tolerance(1e-4)`
   - Fit the model on the data: `model.fit(&dataset)`
   - Extract cluster assignments and centroids
   - Group documents by cluster assignment
   - For each cluster, call `extract_keywords()` and `generate_label()`
   - Return `ClusterState`

3. **Implement `Clusterer::assign_to_nearest()`:**
   - Compute cosine similarity between `doc_vector` and each cluster centroid
   - Assign to the cluster with highest similarity
   - Add `doc_path` to that cluster's `members`
   - Increment `docs_since_rebalance`
   - Return the cluster ID

4. **Implement `Clusterer::maybe_rebalance()`:**
   - Check: `state.docs_since_rebalance >= config.clustering_rebalance_threshold`
   - If yes: call `cluster_all()` with current document vectors, replace state
   - If no: return false
   - Log at info level when rebalancing: `"Rebalancing clusters ({} docs since last rebalance)"`

5. **Implement keyword extraction and label generation:**
   - `extract_keywords(contents, n)`:
     - Tokenize: split on `[^a-zA-Z0-9]`, lowercase, filter tokens < 3 chars
     - Remove stop words (define `STOP_WORDS: &[&str]` constant with common English stop words)
     - Count term frequency per cluster
     - Compute IDF: `log(total_clusters / clusters_containing_term)`
     - Score: TF * IDF
     - Return top `n` terms by score
   - `generate_label(member_contents)`:
     - Call `extract_keywords(contents, 3)`
     - Join with " / "

6. **Integrate with ingest pipeline** — Update `src/ingest.rs`:
   - After embedding and upserting, compute document-level vectors (average of chunk vectors per file)
   - On full ingest: call `Clusterer::cluster_all()` and store in index
   - On single file ingest: call `Clusterer::assign_to_nearest()`
   - After assignment: call `Clusterer::maybe_rebalance()`
   - Skip all clustering if `config.clustering_enabled == false`

7. **Extend `Index`** — In `src/index/state.rs`:
   - Add `get_clusters()`: read lock → return `cluster_state.clone()`
   - Add `update_clusters(state)`: write lock → set `cluster_state = Some(state)`

8. **Update `src/lib.rs`** — Add `pub mod clustering;`

9. **Write clustering unit tests** — In `src/clustering.rs` `#[cfg(test)] mod tests`:
   - Test: 10 documents with 2 clear topic groups produce 2 clusters (use mock vectors: group A near [1,0,...], group B near [0,1,...])
   - Test: single document corpus produces 1 cluster
   - Test: `assign_to_nearest()` assigns to the correct cluster (closest centroid)
   - Test: `maybe_rebalance()` triggers when threshold is reached
   - Test: `maybe_rebalance()` does not trigger below threshold
   - Test: keyword extraction returns meaningful terms (not stop words)
   - Test: label generation joins top 3 keywords
   - Test: k heuristic: 100 docs → k=7, 10 docs → k=2, 5000 docs → k=50

10. **Write integration test** — Create `tests/clustering_test.rs`:
    - Test: full ingest with clustering enabled produces valid `ClusterState`
    - Test: cluster members cover all indexed files (no file left unassigned)
    - Test: incremental file addition assigns to a cluster without full recluster
    - Test: after threshold new files, rebalance runs and produces updated clusters
    - Test: `MDVDB_CLUSTERING_ENABLED=false` skips all clustering (cluster_state is None)

## Validation Criteria

- [ ] Initial clustering with 50 mock documents produces a reasonable number of clusters (between 2 and 50)
- [ ] k heuristic: `cluster_all()` with 100 documents uses k ≈ 7
- [ ] Each document is assigned to exactly one cluster
- [ ] All documents appear in some cluster's `members` list
- [ ] `assign_to_nearest()` adds the document to the closest cluster by cosine similarity
- [ ] `maybe_rebalance()` triggers after `MDVDB_CLUSTERING_REBALANCE_THRESHOLD` new documents
- [ ] `maybe_rebalance()` does NOT trigger below the threshold
- [ ] Cluster labels are human-readable strings derived from content keywords
- [ ] Cluster keywords do not contain stop words ("the", "and", "is", etc.)
- [ ] Clustering is skipped entirely when `MDVDB_CLUSTERING_ENABLED=false`
- [ ] Cluster state persists in the index file and survives reopen
- [ ] Removing a file updates its cluster's member list
- [ ] `cargo test` passes all clustering tests
- [ ] `cargo clippy` reports no warnings

## Anti-Patterns to Avoid

- **Do NOT write cluster assignments to frontmatter** — Cluster data lives only in the index. The system is strictly read-only on markdown files (PROJECT.md §11, §10).
- **Do NOT run k-means on every single file addition** — Use nearest-centroid assignment for incremental updates. K-means is only for initial clustering and rebalancing.
- **Do NOT use chunk-level vectors for clustering** — Cluster at the document level (average of chunk vectors). Chunk-level clustering would produce too many small, incoherent clusters.
- **Do NOT use a fixed k** — Use the `sqrt(N/2)` heuristic capped at 50. A fixed k is wrong for both small (10 docs) and large (10k docs) corpora.
- **Do NOT panic if linfa fails** — K-means can fail to converge. If it does, log a warning and keep the previous cluster state. Don't crash the ingest pipeline.

## Patterns to Follow

- **Algorithm choice:** K-means for batch clustering, nearest-centroid for incremental — this is the pattern specified in TECH.md §Clustering
- **Data conversion:** Convert `Vec<f32>` to `ndarray::Array2<f64>` at the linfa boundary. The rest of the system uses `f32` for memory efficiency; linfa uses `f64` for numerical stability.
- **Optional feature:** Gate on `config.clustering_enabled`. When false, `cluster_state` remains `None` in the index and all clustering functions are no-ops.
- **Error handling:** Clustering failures are logged and non-fatal. The ingest pipeline completes even if clustering fails.

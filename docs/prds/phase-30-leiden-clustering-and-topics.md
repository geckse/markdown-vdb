# PRD: Phase 30 — Leiden Auto-Clustering & Multi-Label Topics

> **Supersedes semantics of:** Phase 9 (K-means auto-clustering — K-means remains available as a config fallback, but is no longer the default) and Phase 27 (custom clusters — force-assignment is replaced by multi-label threshold assignment). Those PRDs remain in `docs/prds/` as historical record per project policy.
>
> **Companion PRD:** app repo `docs/prds/phase-40-topics-and-graph-coloring.md` (Tesseract UI for this feature).

## Overview

Replace the default auto-clustering algorithm with **Leiden community detection on a cosine k-NN graph**, keeping seeded K-means as a configurable fallback, and evolve custom clusters into a proper **topics** feature: multi-label assignment with per-topic thresholds, a global similarity floor, an explicit Unassigned bucket, persisted similarity scores, and optional natural-language topic descriptions that anchor centroids better than bare seed keywords. Cluster identity becomes **stable across re-clustering** (surviving clusters keep their ids and labels), a derived one-level hierarchy is exposed for the app, and a definitions fingerprint eliminates silent centroid staleness. All clustering is deterministic (fixed seed, sorted iteration, explicit tie-breaks) and runs on unit-normalized vectors with cosine similarity end to end.

## Problem Statement

The Phase 9 K-means clustering has structural weaknesses that surface directly in the Tesseract app (which renders clusters in its 3D graph):

1. **Wrong tool for embeddings.** K-means assumes isotropic, similar-sized, convex clusters under Euclidean distance and requires a pre-specified k (`clamp(sqrt(n·g/2), 2, 50)` heuristic). Document embeddings form non-spherical, wildly differently-sized groups on a cosine manifold. Industry topic/vault tools (BERTopic, Top2Vec, Nomic Atlas, Apple Embedding Atlas, Obsidian Smart Connections) converge on either UMAP+HDBSCAN or **cosine k-NN graphs + community detection** — nobody ships plain k-means on raw high-dimensional embeddings.
2. **Cluster identity churn.** Every full re-cluster produced arbitrary new cluster ids and labels, so the app's colors and legend reshuffled on every ingest.
3. **Metric inconsistency.** Batch clustering used L2 (linfa default) while incremental assignment used cosine — a document could be batch-assigned to one cluster and incrementally routed to another.
4. **Correctness bugs.** Re-ingesting a changed file added it to a second cluster without removing the old membership; zero-norm vectors silently vanished; the watcher never updated document clusters at all (stale under `mdvdb watch`, which the app runs continuously); empty-cluster drops left non-contiguous ids; single-document/single-cluster TF-IDF collapsed to `ln(1) = 0` for every term, producing HashMap-ordered garbage labels.

The Phase 27 custom clusters are ~70% of a topics feature but miss what makes topics feel right:

5. **Force-assignment.** Every document was assigned to its nearest custom centroid regardless of relevance — with one topic defined, *all* notes got tagged with it. No threshold, no Unassigned bucket, no multi-membership, no confidence scores.
6. **Silent staleness.** Editing seed phrases had no effect until a *full* ingest; single-file/watch ingests kept using stale centroids with no signal to the user.
7. **Weak centroids.** Bare seed keywords are a degenerate "query" against long documents. Zero-shot classification research (OpenAI cookbook, Jina, EMNLP 2023 label-description work) shows sentence-form descriptions improve assignment accuracy by 8–30%.

## Goals

- Default auto-clustering is Leiden community detection on an exact cosine k-NN graph; K-means selectable via `clustering.algorithm: kmeans`.
- Two identical `cluster_all` runs on identical input produce byte-identical serialized output (seeded RNG, sorted iteration, explicit tie-breaks).
- Cluster ids/labels are stable across re-clustering: matched by member overlap against the previous state, fresh ids minted from a persisted, never-reused counter. Consumers treat ids as **opaque — stable but non-contiguous**.
- One derived parent hierarchy level (`parent_id`, `parent_clusters`) when there are ≥ 7 clusters; additive JSON only.
- Topics: a document joins **every** topic whose cosine similarity ≥ `max(topic.threshold, clustering.topics.min_similarity)`; documents matching nothing land in an explicit sorted `unassigned` list; per-membership scores are persisted and exposed.
- Topic centroids built from `"{name}: {description}"` sentence embeddings (weight 0.6) combined with the mean of individually-normalized seed embeddings (weight 0.4); either component alone is valid.
- A SHA-256 fingerprint of `(defs, floor, embedding model, dimensions)` is persisted; any ingest (including single-file) detects definition changes and triggers a full re-embed + reassignment; unchanged definitions reuse stored centroids (zero provider calls).
- The watcher maintains both cluster layers live: incremental assignment on create/modify, membership removal on delete/rename.
- Improved TF-IDF labels: unigrams + adjacent bigrams, smoothed IDF `ln((1+N)/(1+df)) + 1` (never zero), deterministic `(score desc, term asc)` ranking, label term de-duplication, and a `representative` document per cluster.
- New CLI surface: `clusters add --description/--threshold`, `clusters update … --rename`, `clusters unassigned`, `mdvdb config set <dotted.key> <value>`.
- Everything cosine, everything unit-normalized, `cargo test` green, `cargo clippy --all-targets` clean.

## Non-Goals

- UMAP/HDBSCAN pipeline — rejected: Rust dimensionality-reduction crates are immature or drag in BLAS (`annembed`); Leiden on k-NN needs no reduction and reuses concepts the codebase already has.
- LLM-generated cluster labels — statistical TF-IDF only; an LLM layer can be added later without schema changes.
- Index format version bump — `storage::VERSION` stays **1** (project has no users; see Migration Strategy).
- Edge clustering changes — `cluster_edges` and `EdgeClusterState` (links.rs) keep K-means and their schema; they only inherit the shared seeded-kmeans helper and improved label functions.
- Renaming the `clustering.custom` YAML key to `topics:` — the raw-`Value` deep-merge pipeline would let `custom:` (project) and `topics:` (user) coexist in one merged mapping and fail deserialization. "Topics" is UX naming only.
- A `mdvdb topics` command alias — the CLI surface stays `clusters`.
- Seed-embedding cache — the fingerprint already provides centroid reuse; a text-keyed cache adds persistence complexity for negligible gain.
- Multi-level hierarchy (only one derived parent level), HNSW-accelerated k-NN build (exact brute force is fine to ~20k docs; a warning logs beyond that and the function boundary allows a future ANN swap).

## Technical Design

### Module Layout

`src/clustering.rs` (1,491 lines) becomes a directory module; the public surface is re-exported unchanged so callers (`lib.rs`, `links.rs`, `main.rs`, tests) don't move:

```
src/clustering/
├── mod.rs       # Types, Clusterer facade, algorithm dispatch, stability matching,
│                # topics (multi-label assignment, centroids, fingerprint),
│                # normalize_vectors/normalize_in_place/cosine_similarity
├── leiden.rs    # KnnGraph, build_knn_graph, leiden_partition, relabel_contiguous,
│                # merge_small_communities, aggregate_partition (hierarchy)
├── kmeans.rs    # CLUSTER_SEED, seeded run_kmeans (shared with edge clustering),
│                # compute_k, compute_edge_k
└── labels.rs    # STOP_WORDS, terms_with_bigrams, smoothed-IDF extract_keywords,
                 # cross_cluster_keywords, generate_label
```

### Dependencies (Cargo.toml)

```toml
leiden-rs = { version = "0.8.1", default-features = false }  # pure Rust, MIT/Apache-2.0, seedable
rand = "0.8"            # matches linfa 0.7's rand
rand_xoshiro = "0.6"
```

`leiden-rs` was chosen over `graphrs` because its `LeidenConfig` exposes `seed: Option<u64>` (graphrs's `leiden()` has unseedable refinement randomness). Risk of the young crate is contained: pin the exact version, isolate the call behind `leiden_partition()` so the backend is swappable (graphrs Louvain is the fallback), and pin the guarantee with a determinism integration test.

### Configuration

```yaml
clustering:
  enabled: true
  algorithm: leiden        # leiden | kmeans (default: leiden); enum ClusteringAlgorithm
  knn: 15                  # k-NN graph degree, validated [2, 64]
  resolution: 1.0          # Leiden modularity gamma, validated [0.1, 10.0]; higher = more clusters
  min_cluster_size: 2      # merge smaller communities, validated [1, 50]; 1 = off
  rebalance_threshold: 50  # unchanged
  granularity: 1.0         # K-means fallback ONLY (unchanged, [0.25, 4.0])
  topics:
    min_similarity: 0.30   # global assignment floor, validated [0.0, 1.0]
  custom:                  # key name KEPT (see Non-Goals); "Topics" is the UX name
    - name: Rust
      description: "Notes about Rust programming, cargo, crates"  # NEW, optional
      seeds: [borrow checker, async runtime]                      # now optional if description present
      threshold: 0.40                                             # NEW, optional per-topic override
```

- Env overrides: `MDVDB_CLUSTERING_ALGORITHM`, `MDVDB_CLUSTERING_KNN`, `MDVDB_CLUSTERING_RESOLUTION`, `MDVDB_CLUSTERING_MIN_CLUSTER_SIZE`, `MDVDB_TOPICS_MIN_SIMILARITY`.
- `MDVDB_CUSTOM_CLUSTERS` is **dual-format**: a value starting with `[` parses as JSON `Vec<CustomClusterDef>` (full fidelity: description/threshold; invalid JSON warns and yields nothing); otherwise the legacy pipe format `Name1:seed1,seed2|Name2:seed3` parses as before. `encode_custom_clusters` emits the legacy format when every def is seed-only (byte-identical round-trips) and JSON otherwise.
- Validation additions (`Config::validate`): each def needs a non-empty description **or** ≥ 1 seed (a bare `seeds: []` previously reached `embed_seed_centroids` and silently killed the whole custom pass); `threshold`/`min_similarity` ∈ [0, 1]; unknown algorithm strings are a config error; duplicate-name check kept.
- `Config` gains `clustering_algorithm: ClusteringAlgorithm` (`Leiden` default, `FromStr` accepts `leiden`/`kmeans`/`k-means`), `clustering_knn`, `clustering_resolution`, `clustering_min_cluster_size`, `topics_min_similarity: f32`.

### Data Model Changes (all rkyv + serde `Serialize`)

```rust
pub struct ClusterInfo {
    pub id: usize,                       // stable across re-clustering, NOT contiguous
    pub label: String,
    pub centroid: Vec<f32>,              // unit-normalized mean of member embeddings
    pub members: Vec<String>,            // sorted relative paths
    pub keywords: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<usize>,        // NEW: hierarchy parent (in parent_clusters)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub representative: Option<String>,  // NEW: member closest to centroid
}

pub struct ClusterState {
    pub clusters: Vec<ClusterInfo>,
    pub docs_since_rebalance: usize,
    pub docs_at_last_rebalance: usize,
    pub next_cluster_id: usize,          // NEW: ids are never reused
    pub algorithm: String,               // NEW: "leiden" | "kmeans"; mismatch vs config → full re-cluster
    pub unclustered: Vec<String>,        // NEW: zero-norm docs, sorted (no longer silently dropped)
    pub parent_clusters: Vec<ClusterInfo>, // NEW: derived hierarchy level (empty when skipped)
}

pub struct TopicMember { pub path: String, pub score: f32 }   // NEW

pub struct CustomClusterInfo {
    pub id: usize,                       // = definition order
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,     // NEW (persisted so summaries don't need config)
    pub seed_phrases: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,          // NEW (persisted so assign_single works without config defs)
    pub centroid: Vec<f32>,              // unit-normalized, frozen (anchored to definition)
    pub members: Vec<TopicMember>,       // CHANGED from Vec<String>; path-sorted
}

pub struct CustomClusterState {
    pub clusters: Vec<CustomClusterInfo>,
    pub unassigned: Vec<String>,         // NEW: sorted; deliberately NOT a synthetic cluster
    pub fingerprint: String,             // NEW: staleness key
}
```

`CustomClusterDef` (config layer, not persisted) gains `description: Option<String>`, `threshold: Option<f32>`, `seeds` defaulting to empty, and `serde::Deserialize` (needed for the JSON env format). Unassigned is a separate list rather than a reserved-id cluster so it cannot collide with def-index ids or pollute the app's palette/legend iteration; the app renders `custom_cluster_id: null` as Unassigned with zero special-casing.

### Leiden Pipeline (`cluster_all`, algorithm = leiden)

1. **Normalize** — `normalize_vectors(raw) -> (BTreeMap<path, unit vec>, Vec<zero-norm paths>)`. Zero-norm docs go to `ClusterState.unclustered` with a `warn!`. Deterministic sorted-key order everywhere downstream.
2. **Exact k-NN graph** — `build_knn_graph(&normalized, knn)`: node id = index into sorted paths; per node, top-k neighbors by dot product (= cosine on unit vectors) with tie-break `(sim desc, path asc)`; `k = min(k, n-1)`; drop `sim <= 0`; symmetrize by union; dedupe into `(u, v, weight)` with `u < v` via `BTreeMap`. O(n²·d) — acceptable to ~20k docs (`warn!` above `KNN_BRUTE_FORCE_WARN_THRESHOLD = 20_000`); exact and deterministic, unlike ANN.
3. **Partition** — `leiden_partition(node_count, edges, resolution, CLUSTER_SEED)`: `GraphDataBuilder` → `LeidenConfig::builder().seed(42).resolution(γ).quality(QualityType::Modularity).max_iterations(100)` → `Leiden::run`. Output relabeled contiguously by `(community size desc, smallest member index asc)`.
4. **Merge small communities** — communities `< min_cluster_size` merge into the neighbor community with the largest total connecting edge weight (process smallest first, ties by lowest label; target ties by lowest label); communities with no positive connection are left for step 5. Relabel contiguously.
5. **Fold isolated stragglers** — after building member groups + provisional centroids, any still-undersized cluster's members move to the nearest sibling centroid by cosine *only if positively similar*; otherwise they remain their own cluster (garbage is not forced into a group).
6. **Centroids & representatives** — centroid = unit-normalized mean of member vectors; `representative` = member with highest cosine to centroid (sorted members make ties deterministic).
7. **Keywords & labels** — cross-cluster TF-IDF from `labels.rs` (see below).
8. **Identity stability** — `match_to_previous(new, prev) -> next_id`: Jaccard member overlap for all (prev, new) pairs; greedy acceptance in `(jaccard desc, prev.id asc, new idx asc)`; **jaccard ≥ 0.3 inherits the previous id; ≥ 0.6 also inherits the previous label** (keywords always recomputed — label stickiness kills cosmetic churn). Unmatched new clusters mint ids from `next_id_floor(prev) = max(prev.next_cluster_id, max used id + 1)`. Fresh runs (no previous) emit contiguous `0..m`.
9. **Hierarchy** — when final cluster count ≥ `HIERARCHY_MIN_CLUSTERS = 7`: build the aggregated community graph (nodes = clusters, edge weight = summed inter-cluster k-NN weight), run `leiden_partition` at `resolution × 0.25` with the same seed. Skip if the result doesn't actually coarsen (1 group, or as many groups as clusters). Parents mint ids from the same counter, get members = sorted union of children, size-weighted normalized centroid, and cross-cluster TF-IDF labels; children get `parent_id`.

K-means path (`algorithm = kmeans`): steps 1, 6, 7, 8 identical; grouping via shared `run_kmeans(data, compute_k(n, granularity), ctx)` using `KMeans::params_with_rng(k, Xoshiro256Plus::seed_from_u64(CLUSTER_SEED))`, max 100 iterations, tolerance 1e-4; empty clusters dropped and ids re-issued contiguously. No fold pass, no hierarchy.

### Incremental Assignment & Rebalance

```rust
pub fn assign_incremental(&self, state, doc_path, vector, all_vectors) -> Result<Option<usize>>
```

- Always removes the doc from all membership first (clusters, parents, unclustered) — fixes the duplicate-membership bug. `docs_since_rebalance` increments only when the doc was genuinely new.
- Zero-norm → insert sorted into `unclustered`, return `Ok(None)`.
- **Leiden mode:** weighted k-NN neighbor vote — cosine against all other docs, top `knn` positive-similarity neighbors each vote for their cluster with weight = similarity; ties break to the lower cluster index; no votes → nearest-centroid fallback. **K-means mode:** nearest centroid by cosine.
- Winner's centroid updates as a running mean then re-normalizes; member inserts sorted; parent membership mirrors.
- `remove_document(state, path) -> bool` and `remove_document_from_topics(state, path) -> bool` clear membership on delete/rename.
- `maybe_rebalance` keeps the `docs_since_rebalance >= rebalance_threshold` trigger but re-clusters **with the outgoing state as `previous`**, so a rebalance no longer churns ids. `algorithm_changed(&state)` (string compare vs config) forces a full re-cluster on algorithm switch.

### Labels (`labels.rs`)

- Terms = filtered unigrams (≥ 3 chars, non-stopword) **plus bigrams of tokens adjacent in the raw token stream** where both parts pass the filter (stop words break adjacency: "quick and brown" yields no "quick brown").
- **Smoothed IDF** `ln((1+N)/(1+df)) + 1.0` — never zero, fixing the single-doc/single-cluster degeneracy where every score was 0 and keyword order was HashMap-arbitrary.
- Deterministic ranking `(score desc, term asc)`.
- `generate_label`: up to 3 terms in rank order, skipping unigrams already contained word-wise in a chosen bigram ("rust programming / cargo / tokio", not "rust programming / rust / cargo"); `"Unlabeled"` when empty.
- `cross_cluster_keywords` computes IDF across clusters (distinctive terms promoted); shared by doc clusters, parent clusters, and edge clusters.

### Topics

**Centroids** — `embed_topic_centroids(defs, provider)` (rename of `embed_seed_centroids`):

- Description component: embed the single sentence `"{name}: {description}"`, L2-normalize.
- Seed component: embed all seeds in one batch, **normalize each embedding first** (prevents magnitude bias), mean, re-normalize.
- Combine: `normalize(0.6·desc + 0.4·seeds)`; one component alone is used as-is. Constants `TOPIC_DESC_WEIGHT = 0.6`, `TOPIC_SEED_WEIGHT = 0.4`.
- Hard guards: empty definition → `Error::Clustering`; embedding-count mismatch → error; **dimension consistency enforced within and across defs** → error (previously, mismatched dims silently zip-truncated inside `cosine_similarity` in release builds).

**Fingerprint** — `topics_fingerprint(defs, min_similarity, model, dims) -> String`: hex SHA-256 of the canonical JSON of `{"version": "topics-v1", defs, min_similarity, model, dims}`. Sensitive to def edits, reordering, description/threshold changes, floor, model, and dimensions.

**Assignment** — `assign_all_to_custom(defs, centroids, doc_vectors, fingerprint) -> Result<CustomClusterState>`:

- Effective cutoff per topic = `topic.threshold.map_or(floor, |t| t.max(floor))` — a per-topic threshold can only tighten, never undercut the global floor.
- Sorted path iteration; per doc: zero-norm → `unassigned`; else cosine to every centroid, push `TopicMember { path, score }` into **every** qualifying topic; none → `unassigned`. Dimension mismatch is a hard error with re-ingest guidance.
- `assign_single_to_custom(state, path, vector) -> Result<()>`: removes from all members **and** unassigned, uses **persisted** thresholds + config floor, binary-search inserts to keep members path-sorted, centroids stay frozen.

**Ingest wiring** (`MarkdownVdb::ingest`, custom block — runs whenever defs are non-empty, independent of `clustering.enabled`):

| Situation | Action |
|---|---|
| Single-file ingest, fingerprint matches | `assign_single_to_custom` against stored centroids — **no provider call** |
| Full ingest, fingerprint matches | Reuse stored centroids, `assign_all_to_custom` — **no provider call** |
| Any ingest, fingerprint stale or state missing | `embed_topic_centroids` → `assign_all_to_custom` (this is the staleness fix: a definition edit takes effect on the *next ingest of any kind*) |
| Defs removed from config | Clear the persisted state |

The auto-cluster block similarly gains: single-file fast path only when a non-empty, algorithm-compatible state exists; otherwise (full ingest, missing state — previously silently skipped — or algorithm switch) a full `cluster_all(…, previous)` runs. All failures stay non-fatal `warn!`s.

**Watcher** (`watcher.rs::process_file` + delete/rename branches): after index save, `update_clusters_for_file` runs `assign_incremental` + `maybe_rebalance` (auto layer) and fingerprint-gated `assign_single_to_custom` (topics; a stale fingerprint logs a debug note and defers to the next full ingest). Deletions/renames call `remove_document` / `remove_document_from_topics` and persist.

### CLI (`src/main.rs`)

- `clusters add <name> [--seeds a,b] [--description "…"] [--threshold 0.4]` — requires seeds or description; threshold ∈ [0, 1]; name rejects `:`/`|`; writes `clustering.custom` in `config.yaml`.
- `clusters update <name> [--seeds a,b] [--description "…"] [--threshold t] [--rename NewName]` — `--description ""` clears the description; a **negative** threshold (equals form `--threshold=-1`; clap rejects a bare `-1` value) clears the threshold; rename checks duplicates.
- `clusters remove <name>` — unchanged.
- `clusters list [--json]` — JSON = `CustomClusterDef[]`; seed-only defs serialize byte-identically to the old `{name, seeds}` shape (`skip_serializing_if` on new fields).
- `clusters unassigned [--json]` — JSON `{"count": N, "paths": [...]}`.
- `clusters --custom [--json]` — JSON **stays a top-level array** of `CustomClusterSummary`; human output adds mean score, threshold chip, and an `Unassigned: N documents` footer.
- `config set <dotted.key> <value>` — writes any key into `.markdownvdb/config.yaml` via the existing `update_yaml_config_value`; value parsed as bool → i64 → f64 → string; validates the resulting config and warns on failure. (Consumed by the app GUI for `clustering.topics.min_similarity`, `clustering.algorithm`, `clustering.granularity`.)
- All mutating commands print only to **stderr** — stdout stays empty under the global `--json`, which the app's `execCommand` bridge injects unconditionally.

### Library API / JSON Contract (consumed by the app)

- `ClusterSummary` += `parent_id?`, `representative?` (skip-if-none).
- `CustomClusterSummary` += `description?`, `threshold?`, `mean_score?` (mean member similarity, `None` when empty).
- New `topic_unassigned() -> Result<Vec<String>>`.
- `GraphNode`: `custom_cluster_id` keeps its field but becomes the **primary** topic (highest score, ties → lower id; `null` = Unassigned); new `custom_cluster_ids: Vec<usize>` and `custom_cluster_scores: Vec<f32>` (parallel, score-descending, omitted when empty). Chunk nodes inherit the parent document's memberships.
- `GraphCluster` += `description?`, `threshold?` (topics) and `parent_id?` (auto clusters).
- All additions are additive/optional — old consumers keep parsing.

### Migration Strategy

**`storage::VERSION` stays 1** (explicit decision: the project has no users; do not version-bump for schema changes at this stage). The rkyv layout of `IndexMetadata` did change, so an index written by a pre-Phase-30 binary fails **validated** deserialization (`rkyv::from_bytes` with bytecheck) as `Error::IndexCorrupted` — and `Index::open_or_create_with_options` already heals `IndexNotFound | IndexVersionMismatch | IndexCorrupted` by deleting and recreating the index. Net behavior: old indexes rebuild transparently on the next ingest.

Behavioral change to expect (intentional): topics no longer swallow every document — with the 0.30 default floor, existing custom clusters shrink and an Unassigned bucket appears. The floor is per-vault tunable (YAML, env, or `config set`); per-topic `threshold` is the escape hatch for narrow topics. The floor default suits OpenAI-class embedding models; other models may need calibration.

## Implementation Steps

1. **Module split + labels** — Move `src/clustering.rs` to `src/clustering/{mod,kmeans,labels}.rs` re-exporting the existing surface; implement bigram terms, smoothed IDF, deterministic ranking, label de-dup in `labels.rs`; route doc + edge keyword paths through `cross_cluster_keywords`. No schema change.
2. **Determinism + metric fixes** — `normalize_vectors`/`normalize_in_place` in `mod.rs`; explicit `params_with_rng(k, Xoshiro256Plus::seed_from_u64(42))` in `kmeans::run_kmeans` (shared by doc fallback + `cluster_edges`); remove-before-assign in incremental paths; contiguous fresh ids; centroids re-normalized after incremental mean updates.
3. **Schema v2 shapes (no version bump)** — Add the new `ClusterInfo`/`ClusterState`/`TopicMember`/`CustomClusterInfo`/`CustomClusterState` fields in `src/clustering/mod.rs`; fix all construction sites (`lib.rs`, tests).
4. **Config** — `src/config.rs`: `ClusteringAlgorithm` enum, `YamlClustering` fields (`algorithm`, `knn`, `resolution`, `min_cluster_size`, `topics.min_similarity`), evolved `YamlCustomCluster`/`CustomClusterDef`, validation, env overrides, dual-format `parse_custom_clusters_value`/`encode_custom_clusters`, delete the dead private `parse_custom_clusters`.
5. **Leiden core** — `src/clustering/leiden.rs` per the pipeline above; dispatch in `cluster_all` on `clustering_algorithm`.
6. **Stability + incremental** — `match_to_previous`, `next_id_floor`, `assign_incremental` (neighbor vote), `remove_document`, `algorithm_changed`, rebalance-with-previous.
7. **Topics core** — `embed_topic_centroids`, `topics_fingerprint`, multi-label `assign_all_to_custom`/`assign_single_to_custom`, `remove_document_from_topics`.
8. **lib.rs wiring + API** — Rewire both ingest blocks (fast-path table above); evolve `ClusterSummary`/`CustomClusterSummary`; add `topic_unassigned()`; multi-label graph fields via a shared `topic_membership_map` helper in `graph_data` and `graph_data_chunks`.
9. **CLI** — `ClusterAction::{Add,Update,Remove,List,Unassigned}` + `ConfigAction::Set` in `src/main.rs`; shared `parse_seed_list`/`normalize_description`/`validate_topic_fields`/`parse_yaml_scalar` helpers; YAML read/write helpers carry description/threshold (omitting `None`).
10. **Watcher** — `update_clusters_for_file` + `remove_from_clusters` in `src/watcher.rs`, wired into `process_file`, missing-file, `Deleted`, and `Renamed` paths.
11. **Polish** — clippy clean, CLAUDE.md architecture/config/CLI updates, PRD table row.

## Validation Criteria

- [ ] `leiden_determinism_two_runs_identical` / `kmeans_determinism_two_runs_identical`: two `cluster_all` runs → identical `serde_json` output.
- [ ] Leiden validity: every non-zero doc in exactly one cluster; unit-length centroids; non-empty labels; representative is a member; separated synthetic groups don't mix; higher resolution never yields fewer clusters; `min_cluster_size` folds stragglers.
- [ ] Stability: re-cluster with additions preserves surviving ids (and labels at ≥ 0.6 overlap); a genuinely new group mints an id greater than any previous; `next_cluster_id` is monotone; rebalance does not churn ids.
- [ ] Incremental: neighbor vote picks the majority group; re-assignment removes prior membership (regression for the duplicate-membership bug) and does not bump `docs_since_rebalance`; zero-norm → `unclustered`; empty state errors.
- [ ] Labels: bigrams appear; single-doc keywords are deterministic and ranked by TF (smoothed-IDF regression); label skips covered unigrams.
- [ ] Topics: multi-label above floor; floor overrides a laxer per-topic threshold; a stricter per-topic threshold applies; below-floor and zero-norm docs land in `unassigned`; members path-sorted and deduped; dimension mismatch errors in both assign paths; centroids stay frozen through `assign_single`; description-only defs work; 0.6/0.4 weighting verified numerically with the mock provider.
- [ ] Fingerprint: stable for identical inputs; changes on def edit/reorder/description/threshold/floor/model/dims; single-file ingest with a stale fingerprint triggers a full recompute; matching fingerprint reuses centroids (assert provider embed-call count).
- [ ] Watcher (driven via public `handle_event`, no FS events): new file gets exactly one cluster membership; delete clears membership; topic reassignment happens when the fingerprint matches.
- [ ] CLI: add with description-only; add rejects empty defs and out-of-range thresholds; update edits/renames and rejects unknown names; `clusters list --json` byte-compatible for seed-only defs; `clusters --custom --json` stays an array with `mean_score` iff non-empty; `unassigned --json` shape; `config set` writes typed YAML scalars.
- [ ] Config: defaults, YAML parse, env overrides, validation rejections for all new keys; dual-format env parse; legacy encode round-trip byte-identical; invalid JSON env value yields empty with a warning.
- [ ] Storage: current-version header passes the gate; garbage body fails as `IndexCorrupted` (not version mismatch); pre-rework index files self-heal via recreate.
- [ ] `cargo test` fully green; `cargo clippy --all-targets` zero warnings.
- [ ] End-to-end on a scratch vault (mock provider): two full ingests produce identical `clusters --json`; `config set clustering.topics.min_similarity 0.99` + a single-file ingest moves all docs to `unassigned` (fingerprint recompute observed).

## Anti-Patterns to Avoid

- **Do not bump `storage::VERSION` for schema changes** while the project has no users — validated rkyv deserialization + the existing `open_or_create` recreate path already self-heal old files. (Explicit maintainer decision on this phase.)
- **Do not reuse cluster ids.** `next_cluster_id` exists precisely so a deleted cluster's id never comes back meaning something else; the app diffs and colors by id.
- **Do not assume cluster ids are contiguous** anywhere downstream (`0..len` indexing bugs) — they are opaque after the first stability match.
- **Do not rename or alias the `clustering.custom` YAML key.** The config deep-merge operates on raw `serde_yaml::Value` *before* deserialization; `custom:` and `topics:` from different config layers would both survive the merge and break serde.
- **Do not call `KMeans::params()` and rely on its default RNG** — seed explicitly; determinism must not depend on a dependency's default.
- **Do not re-embed topic seeds on every full ingest** (the pre-phase-30 behavior) — the fingerprint gate exists to make provider calls proportional to definition changes, not ingest frequency.
- **Do not force-assign documents below the floor** or model Unassigned as a synthetic cluster with a reserved id — both re-create the Phase 27 problems this phase removes.
- **Do not compare or cluster unnormalized vectors** — every similarity in this subsystem assumes unit vectors; mixing L2 and cosine was a real, shipped bug.
- **Do not let `leiden-rs` types leak past `leiden_partition()`** — the young-crate risk is only acceptable while the backend stays swappable behind that one function.
- **Do not print to stdout from mutating CLI commands** — the app bridge treats stdout as the JSON contract and injects `--json` unconditionally.

## Patterns to Follow

- **Deterministic iteration idiom** — sorted keys before any order-dependent step (`normalize_vectors` returns `BTreeMap`; assignment iterates sorted paths); see `src/clustering/mod.rs`. Every tie-break is written down.
- **Typed errors, non-fatal clustering** — `Error::Clustering(String)` from `thiserror` (`src/error.rs`); ingest/watcher wrap cluster failures in `warn!` and continue, matching the existing pipeline philosophy.
- **Config discipline** — all env reading and YAML parsing in `Config::load` / `apply_env_overrides` (`src/config.rs`); modules receive typed config. New keys follow the existing `env_*` helper closures and validation-error phrasing.
- **Persisted types** — `rkyv::{Archive, Serialize, Deserialize}` + serde `Serialize` with `#[rkyv(derive(Debug))]`, exactly like the existing `ClusterInfo` (`src/clustering/mod.rs`); additive JSON via `skip_serializing_if`.
- **Test layout** — unit tests in `#[cfg(test)] mod tests` beside the code (each new submodule carries its own); integration tests one-file-per-module (`tests/clustering_test.rs`, `tests/config_test.rs`, `tests/cli_test.rs` with `env!("CARGO_BIN_EXE_mdvdb")`, `tests/watcher_test.rs` driving public `handle_event`); `tempfile::TempDir` isolation; `EmbeddingProviderType::Mock` at 8 dims — no API keys.
- **Atomic writes** — index persistence stays tmp + fsync + rename (`src/index/storage.rs`); nothing in this phase writes markdown files (frontmatter remains read-only).

//! Document and edge clustering.
//!
//! Two independent layers:
//! - **Auto clusters** (`ClusterState`) — unsupervised grouping of document
//!   vectors. Default algorithm: Leiden community detection on a cosine k-NN
//!   graph; K-means remains available via `clustering.algorithm: kmeans`.
//! - **Topics / custom clusters** (`CustomClusterState`) — user-defined topic
//!   centroids built from descriptions and seed phrases; documents get
//!   multi-label assignments with per-topic thresholds and an explicit
//!   Unassigned bucket.
//!
//! All clustering runs on **unit-normalized** vectors with cosine similarity,
//! and every algorithm is explicitly seeded for deterministic output. Cluster
//! ids are **stable across re-clustering**: new results are matched to the
//! previous state by member overlap, surviving clusters keep their id (and,
//! for strong matches, their label), and genuinely new clusters mint fresh
//! ids from a persisted counter. Consumers must treat ids as opaque — stable
//! but not contiguous.

pub(crate) mod kmeans;
pub(crate) mod labels;
pub(crate) mod leiden;

use std::collections::{BTreeMap, HashMap, HashSet};

use ndarray::Array2;
use serde::Serialize;
use tracing::{debug, info, warn};

use crate::config::{ClusteringAlgorithm, Config};
use crate::links::{EdgeClusterInfo, EdgeClusterState};

pub(crate) use kmeans::{compute_edge_k, compute_k};

/// Minimum cluster count before a parent hierarchy level is derived.
const HIERARCHY_MIN_CLUSTERS: usize = 7;

/// Jaccard member-overlap threshold for a new cluster to inherit a previous
/// cluster's id.
const STABILITY_ID_JACCARD: f64 = 0.3;

/// Jaccard threshold to also inherit the previous label (prevents cosmetic
/// label churn on rebalance).
const STABILITY_LABEL_JACCARD: f64 = 0.6;

/// Weight of the description vector vs the seed vector in topic centroids.
const TOPIC_DESC_WEIGHT: f32 = 0.6;
const TOPIC_SEED_WEIGHT: f32 = 0.4;

/// Information about a single cluster, stored in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct ClusterInfo {
    /// Numeric cluster identifier — stable across re-clustering, not contiguous.
    pub id: usize,
    /// Human-readable auto-generated label.
    pub label: String,
    /// Centroid vector (unit-normalized mean of member embeddings).
    pub centroid: Vec<f32>,
    /// File paths (relative) belonging to this cluster, sorted.
    pub members: Vec<String>,
    /// Top keywords extracted via TF-IDF.
    pub keywords: Vec<String>,
    /// Id of the parent cluster in `ClusterState::parent_clusters`, if a
    /// hierarchy level was derived.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<usize>,
    /// Member path closest to the centroid — the cluster's most typical document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub representative: Option<String>,
}

/// Cluster state persisted in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct ClusterState {
    /// All clusters.
    pub clusters: Vec<ClusterInfo>,
    /// Number of documents added since last full rebalance.
    pub docs_since_rebalance: usize,
    /// Total document count at last rebalance.
    pub docs_at_last_rebalance: usize,
    /// Next id to mint for a genuinely new cluster; ids are never reused.
    pub next_cluster_id: usize,
    /// Algorithm that produced this state ("leiden" | "kmeans"); a config
    /// switch triggers a full re-cluster.
    pub algorithm: String,
    /// Documents that could not be clustered (zero-norm vectors), sorted.
    pub unclustered: Vec<String>,
    /// Derived parent hierarchy level (empty when skipped). Parent ids share
    /// the same id space as clusters.
    pub parent_clusters: Vec<ClusterInfo>,
}

impl ClusterState {
    /// An empty state carrying forward the id counter and algorithm name.
    fn empty(algorithm: &str, next_cluster_id: usize, unclustered: Vec<String>) -> Self {
        Self {
            clusters: Vec::new(),
            docs_since_rebalance: 0,
            docs_at_last_rebalance: 0,
            next_cluster_id,
            algorithm: algorithm.to_string(),
            unclustered,
            parent_clusters: Vec::new(),
        }
    }
}

/// User-defined custom cluster (topic) definition — config-layer only, not
/// persisted in the index.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct CustomClusterDef {
    /// User-provided topic name.
    pub name: String,
    /// Optional natural-language description; embedded as "{name}: {description}"
    /// it typically anchors the centroid better than bare seed keywords.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Seed words/phrases used to compute the centroid (optional when a
    /// description is present).
    #[serde(default)]
    pub seeds: Vec<String>,
    /// Optional per-topic similarity threshold; the effective cutoff is
    /// `max(threshold, topics.min_similarity)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,
}

/// One document's membership in a topic, with its cosine similarity score.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct TopicMember {
    /// File path (relative).
    pub path: String,
    /// Cosine similarity to the topic centroid at assignment time.
    pub score: f32,
}

/// Information about a single user-defined topic, stored in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct CustomClusterInfo {
    /// Numeric cluster identifier (0-based, = definition order).
    pub id: usize,
    /// User-provided topic name.
    pub name: String,
    /// User-provided description, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The seed phrases used to compute this topic's centroid.
    pub seed_phrases: Vec<String>,
    /// Per-topic similarity threshold override, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,
    /// Centroid vector (unit-normalized).
    pub centroid: Vec<f32>,
    /// Documents assigned to this topic (multi-label), sorted by path.
    pub members: Vec<TopicMember>,
}

/// Custom cluster (topics) state persisted in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct CustomClusterState {
    /// All topics.
    pub clusters: Vec<CustomClusterInfo>,
    /// Documents matching no topic (below every threshold), sorted.
    pub unassigned: Vec<String>,
    /// Fingerprint of the definitions + assignment inputs that produced this
    /// state; a mismatch means centroids/assignments are stale.
    pub fingerprint: String,
}

/// Performs clustering operations on document embeddings.
pub struct Clusterer {
    config: Config,
}

impl Clusterer {
    /// Create a new clusterer with the given configuration.
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
        }
    }

    /// Run a full clustering pass over all document vectors.
    ///
    /// `vectors` maps document path (relative) to its embedding vector.
    /// `documents` maps document path to its text content (for keyword extraction).
    /// `previous` enables cluster-identity stability: surviving clusters keep
    /// their ids/labels, new clusters mint fresh ids.
    pub fn cluster_all(
        &self,
        vectors: &HashMap<String, Vec<f32>>,
        documents: &HashMap<String, String>,
        previous: Option<&ClusterState>,
    ) -> crate::Result<ClusterState> {
        let algorithm = self.config.clustering_algorithm;
        let id_floor = previous.map(next_id_floor).unwrap_or(0);

        let (normalized, zero_norm) = normalize_vectors(vectors);
        if !zero_norm.is_empty() {
            warn!(
                "cluster_all: {} document(s) have zero-norm vectors and cannot be clustered: {:?}",
                zero_norm.len(),
                zero_norm
            );
        }

        let n = normalized.len();
        if n == 0 {
            debug!("cluster_all: no clusterable vectors, returning empty state");
            return Ok(ClusterState::empty(algorithm.as_str(), id_floor, zero_norm));
        }

        // Build member groups per algorithm.
        let mut clusters: Vec<ClusterInfo> = if n == 1 {
            let doc_id = normalized.keys().next().expect("n == 1").clone();
            vec![new_cluster(0, vec![doc_id])]
        } else {
            match algorithm {
                ClusteringAlgorithm::Leiden => {
                    let graph =
                        leiden::build_knn_graph(&normalized, self.config.clustering_knn);
                    let membership = leiden::leiden_partition(
                        n,
                        &graph.edges,
                        self.config.clustering_resolution,
                        kmeans::CLUSTER_SEED,
                    )?;
                    let membership = leiden::merge_small_communities(
                        membership,
                        &graph,
                        self.config.clustering_min_cluster_size,
                    );
                    clusters_from_membership(&normalized, &membership)
                }
                ClusteringAlgorithm::Kmeans => self.kmeans_membership(&normalized)?,
            }
        };

        // Fold isolated under-sized clusters into their nearest sibling.
        if algorithm == ClusteringAlgorithm::Leiden {
            compute_centroids(&mut clusters, &normalized);
            fold_undersized_clusters(
                &mut clusters,
                &normalized,
                self.config.clustering_min_cluster_size,
            );
        }

        // Centroids + representatives from final membership.
        compute_centroids(&mut clusters, &normalized);
        set_representatives(&mut clusters, &normalized);

        info!(
            "cluster_all: clustered {n} documents into {} clusters ({})",
            clusters.len(),
            algorithm.as_str()
        );

        // Cross-cluster TF-IDF keywords and labels.
        assign_doc_cluster_keywords(&mut clusters, documents, 5);

        // Identity stability vs the previous state.
        let mut next_cluster_id = match previous {
            Some(prev) => match_to_previous(&mut clusters, prev),
            None => clusters.len(),
        };

        // Optional one-level hierarchy (Leiden only, enough clusters).
        let parent_clusters = if algorithm == ClusteringAlgorithm::Leiden {
            self.build_parent_level(
                &mut clusters,
                &normalized,
                documents,
                &mut next_cluster_id,
            )?
        } else {
            Vec::new()
        };

        Ok(ClusterState {
            clusters,
            docs_since_rebalance: 0,
            docs_at_last_rebalance: n,
            next_cluster_id,
            algorithm: algorithm.as_str().to_string(),
            unclustered: zero_norm,
            parent_clusters,
        })
    }

    /// K-means membership grouping (fallback algorithm).
    fn kmeans_membership(
        &self,
        normalized: &BTreeMap<String, Vec<f32>>,
    ) -> crate::Result<Vec<ClusterInfo>> {
        let n = normalized.len();
        let ids: Vec<&String> = normalized.keys().collect();
        let dim = normalized.values().next().expect("n >= 1").len();
        let k = compute_k(n, self.config.clustering_granularity);

        let mut data = Array2::<f64>::zeros((n, dim));
        for (i, v) in normalized.values().enumerate() {
            for (j, &val) in v.iter().enumerate() {
                data[[i, j]] = val as f64;
            }
        }

        let (_, assignments) = kmeans::run_kmeans(data, k, "document")?;

        let mut cluster_members: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, &cluster_id) in assignments.iter().enumerate() {
            cluster_members
                .entry(cluster_id)
                .or_default()
                .push(ids[i].clone());
        }

        let mut clusters: Vec<ClusterInfo> = Vec::new();
        for raw_id in 0..k {
            let members = cluster_members.remove(&raw_id).unwrap_or_default();
            if members.is_empty() {
                continue;
            }
            clusters.push(new_cluster(clusters.len(), members));
        }
        Ok(clusters)
    }

    /// Derive one parent hierarchy level by re-clustering the aggregated
    /// community graph at a coarser resolution. No-op when there are too few
    /// clusters or aggregation doesn't actually coarsen.
    fn build_parent_level(
        &self,
        clusters: &mut [ClusterInfo],
        normalized: &BTreeMap<String, Vec<f32>>,
        documents: &HashMap<String, String>,
        next_cluster_id: &mut usize,
    ) -> crate::Result<Vec<ClusterInfo>> {
        if clusters.len() < HIERARCHY_MIN_CLUSTERS {
            return Ok(Vec::new());
        }

        // Rebuild node membership over the final cluster list.
        let path_to_cluster: HashMap<&String, usize> = clusters
            .iter()
            .enumerate()
            .flat_map(|(idx, c)| c.members.iter().map(move |m| (m, idx)))
            .collect();
        let graph = leiden::build_knn_graph(normalized, self.config.clustering_knn);
        let membership: Vec<usize> = graph
            .node_paths
            .iter()
            .map(|p| path_to_cluster.get(p).copied().unwrap_or(0))
            .collect();

        let parent_membership = leiden::aggregate_partition(
            &graph,
            &membership,
            clusters.len(),
            self.config.clustering_resolution * 0.25,
        )?;

        let group_count = parent_membership.iter().copied().max().map_or(0, |m| m + 1);
        if group_count <= 1 || group_count >= clusters.len() {
            return Ok(Vec::new()); // aggregation didn't coarsen — skip
        }

        let mut parents: Vec<ClusterInfo> = Vec::with_capacity(group_count);
        for group in 0..group_count {
            let children: Vec<usize> = (0..clusters.len())
                .filter(|&i| parent_membership[i] == group)
                .collect();
            let mut members: Vec<String> = children
                .iter()
                .flat_map(|&i| clusters[i].members.iter().cloned())
                .collect();
            members.sort();

            let mut parent = new_cluster(*next_cluster_id, members);
            *next_cluster_id += 1;
            for &child in &children {
                clusters[child].parent_id = Some(parent.id);
            }
            // Centroid: size-weighted mean of child centroids, unit-normalized.
            let dim = clusters[children[0]].centroid.len();
            let mut centroid = vec![0.0f32; dim];
            for &child in &children {
                let weight = clusters[child].members.len() as f32;
                for (i, v) in clusters[child].centroid.iter().enumerate() {
                    centroid[i] += v * weight;
                }
            }
            parent.centroid = normalize_in_place(centroid);
            parents.push(parent);
        }

        assign_doc_cluster_keywords(&mut parents, documents, 5);
        Ok(parents)
    }

    /// Assign a single new or changed document to a cluster incrementally.
    ///
    /// Removes the document from any prior membership first (a changed file
    /// must not end up in two clusters). Leiden mode assigns by weighted
    /// k-NN neighbor vote (falling back to nearest centroid); K-means mode
    /// assigns by nearest centroid. Zero-norm vectors go to `unclustered`.
    ///
    /// Returns the assigned cluster id, or `None` for unclustered documents.
    pub fn assign_incremental(
        &self,
        state: &mut ClusterState,
        doc_path: &str,
        vector: &[f32],
        all_vectors: &HashMap<String, Vec<f32>>,
    ) -> crate::Result<Option<usize>> {
        if state.clusters.is_empty() {
            return Err(crate::Error::Clustering(
                "no clusters exist for assignment".to_string(),
            ));
        }

        let was_member = self.remove_document(state, doc_path);

        // Zero-norm vectors cannot be placed.
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            insert_sorted(&mut state.unclustered, doc_path.to_string());
            debug!("assign_incremental: {doc_path} has a zero-norm vector, marked unclustered");
            return Ok(None);
        }
        let unit: Vec<f32> = vector.iter().map(|x| x / norm).collect();

        let cluster_idx = match self.config.clustering_algorithm {
            ClusteringAlgorithm::Leiden => self
                .neighbor_vote(state, doc_path, &unit, all_vectors)
                .unwrap_or_else(|| nearest_cluster_index(state, &unit)),
            ClusteringAlgorithm::Kmeans => nearest_cluster_index(state, &unit),
        };

        let cluster = &mut state.clusters[cluster_idx];
        let cluster_id = cluster.id;

        // Update centroid incrementally as a running mean, then re-normalize.
        let n = cluster.members.len() as f32;
        for (i, c) in cluster.centroid.iter_mut().enumerate() {
            *c = (*c * n + unit[i]) / (n + 1.0);
        }
        cluster.centroid = normalize_in_place(std::mem::take(&mut cluster.centroid));

        insert_sorted(&mut cluster.members, doc_path.to_string());
        if let Some(parent_id) = cluster.parent_id {
            if let Some(parent) = state
                .parent_clusters
                .iter_mut()
                .find(|p| p.id == parent_id)
            {
                insert_sorted(&mut parent.members, doc_path.to_string());
            }
        }
        if !was_member {
            state.docs_since_rebalance += 1;
        }

        debug!("assign_incremental: assigned {doc_path} to cluster {cluster_id}");
        Ok(Some(cluster_id))
    }

    /// Weighted k-NN neighbor vote: each of the document's top-k neighbors
    /// votes for its own cluster with weight = cosine similarity. Returns the
    /// winning cluster's index, or `None` when no positive-similarity
    /// neighbor has a cluster.
    fn neighbor_vote(
        &self,
        state: &ClusterState,
        doc_path: &str,
        unit: &[f32],
        all_vectors: &HashMap<String, Vec<f32>>,
    ) -> Option<usize> {
        let path_to_cluster: HashMap<&String, usize> = state
            .clusters
            .iter()
            .enumerate()
            .flat_map(|(idx, c)| c.members.iter().map(move |m| (m, idx)))
            .collect();

        let mut sims: Vec<(f32, &String)> = all_vectors
            .iter()
            .filter(|(path, _)| path.as_str() != doc_path)
            .filter_map(|(path, v)| {
                let sim = cosine_similarity(unit, v);
                (sim > 0.0).then_some((sim, path))
            })
            .collect();
        sims.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(b.1))
        });

        let mut votes: BTreeMap<usize, f64> = BTreeMap::new();
        for (sim, path) in sims.iter().take(self.config.clustering_knn) {
            if let Some(&idx) = path_to_cluster.get(path) {
                *votes.entry(idx).or_insert(0.0) += *sim as f64;
            }
        }

        votes
            .into_iter()
            .max_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    // BTreeMap iterates ascending by key; prefer the LOWER
                    // cluster index on weight ties.
                    .then_with(|| b.0.cmp(&a.0))
            })
            .map(|(idx, _)| idx)
    }

    /// Remove a document from all cluster membership (clusters, parents,
    /// unclustered). Returns `true` if it was a member anywhere.
    pub fn remove_document(&self, state: &mut ClusterState, doc_path: &str) -> bool {
        let mut removed = false;
        for cluster in &mut state.clusters {
            let before = cluster.members.len();
            cluster.members.retain(|m| m != doc_path);
            removed |= cluster.members.len() != before;
        }
        for parent in &mut state.parent_clusters {
            parent.members.retain(|m| m != doc_path);
        }
        let before = state.unclustered.len();
        state.unclustered.retain(|m| m != doc_path);
        removed |= state.unclustered.len() != before;
        removed
    }

    /// Whether the persisted state was produced by a different algorithm than
    /// the current config (requires a full re-cluster).
    pub fn algorithm_changed(&self, state: &ClusterState) -> bool {
        state.algorithm != self.config.clustering_algorithm.as_str()
    }

    /// Rebalance clusters if the number of new documents exceeds the threshold.
    ///
    /// A full re-clustering is triggered when `docs_since_rebalance` exceeds
    /// the configured `clustering_rebalance_threshold`. The outgoing state
    /// seeds identity matching, so ids/labels stay stable. Returns `true` if
    /// a rebalance was performed.
    pub fn maybe_rebalance(
        &self,
        state: &mut ClusterState,
        vectors: &HashMap<String, Vec<f32>>,
        documents: &HashMap<String, String>,
    ) -> crate::Result<bool> {
        if state.docs_since_rebalance < self.config.clustering_rebalance_threshold {
            debug!(
                "maybe_rebalance: {}/{} docs since rebalance, skipping",
                state.docs_since_rebalance, self.config.clustering_rebalance_threshold
            );
            return Ok(false);
        }

        info!(
            "maybe_rebalance: threshold reached ({} docs since last rebalance), re-clustering",
            state.docs_since_rebalance
        );

        let new_state = self.cluster_all(vectors, documents, Some(&*state))?;
        *state = new_state;

        Ok(true)
    }

    /// Extract top-N keywords from a set of documents using TF-IDF.
    pub fn extract_keywords(&self, documents: &[&str], n: usize) -> Vec<String> {
        labels::extract_keywords(documents, n)
    }

    /// Generate a human-readable label from keywords.
    pub fn generate_label(&self, keywords: &[String]) -> String {
        labels::generate_label(keywords)
    }

    /// Returns the configured rebalance threshold.
    pub fn rebalance_threshold(&self) -> usize {
        self.config.clustering_rebalance_threshold
    }

    /// Returns whether clustering is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.clustering_enabled
    }

    /// Run a full K-means clustering pass over edge embeddings.
    ///
    /// `edge_vectors` maps edge ID to its embedding vector.
    /// `edge_contexts` maps edge ID to its context paragraph text (for keyword extraction).
    ///
    /// Returns empty `EdgeClusterState` if fewer than 4 edges.
    pub fn cluster_edges(
        &self,
        edge_vectors: &HashMap<String, Vec<f32>>,
        edge_contexts: &HashMap<String, String>,
    ) -> crate::Result<EdgeClusterState> {
        if edge_vectors.len() < 4 {
            debug!(
                "cluster_edges: fewer than 4 edges ({}), returning empty state",
                edge_vectors.len()
            );
            return Ok(EdgeClusterState {
                clusters: Vec::new(),
                edges_since_rebalance: 0,
                edges_at_last_rebalance: edge_vectors.len(),
            });
        }

        let (normalized, zero_norm) = normalize_vectors(edge_vectors);
        if !zero_norm.is_empty() {
            debug!(
                "cluster_edges: skipping {} zero-norm edge vector(s)",
                zero_norm.len()
            );
        }

        let n = normalized.len();
        if n < 4 {
            return Ok(EdgeClusterState {
                clusters: Vec::new(),
                edges_since_rebalance: 0,
                edges_at_last_rebalance: n,
            });
        }

        let ids: Vec<&String> = normalized.keys().collect();
        let dim = normalized.values().next().expect("n >= 4").len();
        let k = compute_edge_k(n, self.config.clustering_granularity);

        let mut data = Array2::<f64>::zeros((n, dim));
        for (i, v) in normalized.values().enumerate() {
            for (j, &val) in v.iter().enumerate() {
                data[[i, j]] = val as f64;
            }
        }

        let (centroids, assignments) = kmeans::run_kmeans(data, k, "edge")?;

        let mut cluster_members: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, &cluster_id) in assignments.iter().enumerate() {
            cluster_members
                .entry(cluster_id)
                .or_default()
                .push(ids[i].clone());
        }

        // Build EdgeClusterInfo for each non-empty cluster with contiguous ids.
        let mut clusters: Vec<EdgeClusterInfo> = Vec::new();
        for raw_id in 0..k {
            let members = cluster_members.remove(&raw_id).unwrap_or_default();
            if members.is_empty() {
                continue;
            }

            let centroid = normalize_in_place(
                centroids.row(raw_id).iter().map(|&v| v as f32).collect(),
            );

            clusters.push(EdgeClusterInfo {
                id: clusters.len(),
                label: String::new(),
                centroid,
                members,
                keywords: Vec::new(),
            });
        }

        info!(
            "cluster_edges: clustered {n} edges into {} clusters",
            clusters.len()
        );

        // Cross-cluster TF-IDF for keyword distinctiveness.
        let cluster_texts: Vec<Vec<&str>> = clusters
            .iter()
            .map(|c| {
                c.members
                    .iter()
                    .filter_map(|m| edge_contexts.get(m).map(|s| s.as_str()))
                    .collect()
            })
            .collect();
        let keywords = labels::cross_cluster_keywords(&cluster_texts, 5);
        for (cluster, kws) in clusters.iter_mut().zip(keywords) {
            cluster.label = labels::generate_label(&kws);
            cluster.keywords = kws;
        }

        Ok(EdgeClusterState {
            clusters,
            edges_since_rebalance: 0,
            edges_at_last_rebalance: n,
        })
    }

    /// Assign a single new edge to the nearest existing edge cluster.
    ///
    /// Removes the edge from any prior membership first.
    /// Returns the cluster ID the edge was assigned to.
    pub fn assign_edge_to_nearest(
        &self,
        state: &mut EdgeClusterState,
        edge_id: &str,
        embedding: &[f32],
    ) -> crate::Result<usize> {
        if state.clusters.is_empty() {
            return Err(crate::Error::Clustering(
                "no edge clusters exist for assignment".to_string(),
            ));
        }

        let mut was_member = false;
        for cluster in &mut state.clusters {
            let before = cluster.members.len();
            cluster.members.retain(|m| m != edge_id);
            was_member |= cluster.members.len() != before;
        }

        let mut best_idx = 0;
        let mut best_sim = f32::NEG_INFINITY;
        for (i, cluster) in state.clusters.iter().enumerate() {
            let sim = cosine_similarity(embedding, &cluster.centroid);
            if sim > best_sim {
                best_sim = sim;
                best_idx = i;
            }
        }

        let cluster = &mut state.clusters[best_idx];
        let cluster_id = cluster.id;

        let n = cluster.members.len() as f32;
        for (i, c) in cluster.centroid.iter_mut().enumerate() {
            *c = (*c * n + embedding[i]) / (n + 1.0);
        }
        cluster.centroid = normalize_in_place(std::mem::take(&mut cluster.centroid));

        cluster.members.push(edge_id.to_string());
        if !was_member {
            state.edges_since_rebalance += 1;
        }

        debug!(
            "assign_edge_to_nearest: assigned {edge_id} to cluster {cluster_id} (similarity={best_sim:.4})"
        );

        Ok(cluster_id)
    }

    /// Rebalance edge clusters if the number of new edges exceeds the threshold.
    ///
    /// Returns `true` if a rebalance was performed.
    pub fn maybe_rebalance_edges(
        &self,
        state: &mut EdgeClusterState,
        edge_vectors: &HashMap<String, Vec<f32>>,
        edge_contexts: &HashMap<String, String>,
        threshold: usize,
    ) -> crate::Result<bool> {
        if state.edges_since_rebalance < threshold {
            debug!(
                "maybe_rebalance_edges: {}/{} edges since rebalance, skipping",
                state.edges_since_rebalance, threshold
            );
            return Ok(false);
        }

        info!(
            "maybe_rebalance_edges: threshold reached ({} edges since last rebalance), re-clustering",
            state.edges_since_rebalance
        );

        let new_state = self.cluster_edges(edge_vectors, edge_contexts)?;
        *state = new_state;

        Ok(true)
    }

    /// Assign all documents to topics (multi-label with thresholds).
    ///
    /// A document becomes a member of **every** topic whose cosine similarity
    /// meets `max(topic.threshold, topics.min_similarity)`; documents matching
    /// no topic (or with zero-norm vectors) land in the Unassigned bucket.
    ///
    /// `defs` provides names/descriptions/seeds/thresholds; `centroids` are the
    /// pre-computed centroid vectors (one per def, in the same order);
    /// `fingerprint` is the `topics_fingerprint` of the inputs.
    pub fn assign_all_to_custom(
        &self,
        defs: &[CustomClusterDef],
        centroids: &[Vec<f32>],
        doc_vectors: &HashMap<String, Vec<f32>>,
        fingerprint: String,
    ) -> crate::Result<CustomClusterState> {
        let floor = self.config.topics_min_similarity;

        let dims = centroids.first().map(|c| c.len()).unwrap_or(0);
        for c in centroids {
            if c.len() != dims {
                return Err(crate::Error::Clustering(format!(
                    "topic centroid dimension mismatch: {} vs {dims}",
                    c.len()
                )));
            }
        }

        let mut clusters: Vec<CustomClusterInfo> = defs
            .iter()
            .enumerate()
            .map(|(i, def)| CustomClusterInfo {
                id: i,
                name: def.name.clone(),
                description: def.description.clone(),
                seed_phrases: def.seeds.clone(),
                threshold: def.threshold,
                centroid: centroids[i].clone(),
                members: Vec::new(),
            })
            .collect();
        let mut unassigned: Vec<String> = Vec::new();

        let mut paths: Vec<&String> = doc_vectors.keys().collect();
        paths.sort();

        for path in paths {
            let vector = &doc_vectors[path];
            let norm_sq: f32 = vector.iter().map(|x| x * x).sum();
            if norm_sq == 0.0 {
                unassigned.push(path.clone());
                continue;
            }
            if !centroids.is_empty() && vector.len() != dims {
                return Err(crate::Error::Clustering(format!(
                    "dimension mismatch: document vectors are {}-d but topic centroids are {dims}-d — \
                     re-run a full ingest after changing the embedding model",
                    vector.len()
                )));
            }

            let mut matched = false;
            for cluster in &mut clusters {
                let sim = cosine_similarity(vector, &cluster.centroid);
                let cutoff = cluster.threshold.map_or(floor, |t| t.max(floor));
                if sim >= cutoff {
                    cluster.members.push(TopicMember {
                        path: path.clone(),
                        score: sim,
                    });
                    matched = true;
                }
            }
            if !matched {
                unassigned.push(path.clone());
            }
        }

        info!(
            "assign_all_to_custom: assigned {} documents across {} topics ({} unassigned)",
            doc_vectors.len(),
            clusters.len(),
            unassigned.len()
        );

        Ok(CustomClusterState {
            clusters,
            unassigned,
            fingerprint,
        })
    }

    /// Remove a document from all topic membership (members and unassigned).
    /// Returns `true` if it was present anywhere.
    pub fn remove_document_from_topics(
        &self,
        state: &mut CustomClusterState,
        doc_path: &str,
    ) -> bool {
        let mut removed = false;
        for cluster in &mut state.clusters {
            let before = cluster.members.len();
            cluster.members.retain(|m| m.path != doc_path);
            removed |= cluster.members.len() != before;
        }
        let before = state.unassigned.len();
        state.unassigned.retain(|m| m != doc_path);
        removed |= state.unassigned.len() != before;
        removed
    }

    /// Re-assign a single document against the existing topic centroids
    /// (multi-label). Removes any current memberships first; centroids are
    /// NOT updated (anchored to their definitions).
    pub fn assign_single_to_custom(
        &self,
        state: &mut CustomClusterState,
        doc_path: &str,
        vector: &[f32],
    ) -> crate::Result<()> {
        for cluster in &mut state.clusters {
            cluster.members.retain(|m| m.path != doc_path);
        }
        state.unassigned.retain(|m| m != doc_path);

        if state.clusters.is_empty() {
            return Ok(());
        }

        let dims = state.clusters[0].centroid.len();
        let norm_sq: f32 = vector.iter().map(|x| x * x).sum();
        if norm_sq == 0.0 {
            insert_sorted(&mut state.unassigned, doc_path.to_string());
            return Ok(());
        }
        if vector.len() != dims {
            return Err(crate::Error::Clustering(format!(
                "dimension mismatch: document vector is {}-d but topic centroids are {dims}-d — \
                 re-run a full ingest after changing the embedding model",
                vector.len()
            )));
        }

        let floor = self.config.topics_min_similarity;
        let mut matched = false;
        for cluster in &mut state.clusters {
            let sim = cosine_similarity(vector, &cluster.centroid);
            let cutoff = cluster.threshold.map_or(floor, |t| t.max(floor));
            if sim >= cutoff {
                let member = TopicMember {
                    path: doc_path.to_string(),
                    score: sim,
                };
                let pos = cluster
                    .members
                    .binary_search_by(|m| m.path.as_str().cmp(doc_path))
                    .unwrap_or_else(|p| p);
                cluster.members.insert(pos, member);
                matched = true;
            }
        }
        if !matched {
            insert_sorted(&mut state.unassigned, doc_path.to_string());
        }

        debug!(
            "assign_single_to_custom: {doc_path} matched {} topic(s)",
            if matched { "some" } else { "no" }
        );
        Ok(())
    }
}

/// A bare cluster with members only (centroid/keywords filled in later).
fn new_cluster(id: usize, mut members: Vec<String>) -> ClusterInfo {
    members.sort();
    ClusterInfo {
        id,
        label: String::new(),
        centroid: Vec::new(),
        members,
        keywords: Vec::new(),
        parent_id: None,
        representative: None,
    }
}

/// Group documents into clusters from a contiguous membership labeling.
fn clusters_from_membership(
    normalized: &BTreeMap<String, Vec<f32>>,
    membership: &[usize],
) -> Vec<ClusterInfo> {
    let count = membership.iter().copied().max().map_or(0, |m| m + 1);
    let mut groups: Vec<Vec<String>> = vec![Vec::new(); count];
    for (path, &community) in normalized.keys().zip(membership.iter()) {
        groups[community].push(path.clone());
    }
    groups
        .into_iter()
        .filter(|g| !g.is_empty())
        .enumerate()
        .map(|(i, members)| new_cluster(i, members))
        .collect()
}

/// Compute unit-normalized mean centroids from member vectors.
fn compute_centroids(clusters: &mut [ClusterInfo], vectors: &BTreeMap<String, Vec<f32>>) {
    let dim = vectors.values().next().map_or(0, |v| v.len());
    for cluster in clusters.iter_mut() {
        let mut centroid = vec![0.0f32; dim];
        let mut count = 0usize;
        for member in &cluster.members {
            if let Some(v) = vectors.get(member) {
                for (i, x) in v.iter().enumerate() {
                    centroid[i] += x;
                }
                count += 1;
            }
        }
        if count > 0 {
            for x in &mut centroid {
                *x /= count as f32;
            }
        }
        cluster.centroid = normalize_in_place(centroid);
    }
}

/// Set each cluster's representative: the member closest to the centroid
/// (ties break to the lexicographically smaller path via sorted members).
fn set_representatives(clusters: &mut [ClusterInfo], vectors: &BTreeMap<String, Vec<f32>>) {
    for cluster in clusters.iter_mut() {
        let mut best: Option<(f32, &String)> = None;
        for member in &cluster.members {
            if let Some(v) = vectors.get(member) {
                let sim = cosine_similarity(v, &cluster.centroid);
                if best.is_none_or(|(bs, _)| sim > bs) {
                    best = Some((sim, member));
                }
            }
        }
        cluster.representative = best.map(|(_, m)| m.clone());
    }
}

/// Fold clusters smaller than `min_size` into the nearest sibling by centroid
/// cosine (only when positively similar). Covers isolated nodes that
/// `merge_small_communities` cannot merge for lack of graph edges.
fn fold_undersized_clusters(
    clusters: &mut Vec<ClusterInfo>,
    vectors: &BTreeMap<String, Vec<f32>>,
    min_size: usize,
) {
    if min_size <= 1 {
        return;
    }
    loop {
        if clusters.len() <= 1 {
            return;
        }
        let Some(idx) = clusters.iter().position(|c| c.members.len() < min_size) else {
            return;
        };
        let small = clusters.remove(idx);
        let mut homeless: Vec<String> = Vec::new();
        for member in small.members {
            let Some(v) = vectors.get(&member) else {
                homeless.push(member);
                continue;
            };
            let mut best: Option<(f32, usize)> = None;
            for (i, c) in clusters.iter().enumerate() {
                let sim = cosine_similarity(v, &c.centroid);
                if sim > 0.0 && best.is_none_or(|(bs, _)| sim > bs) {
                    best = Some((sim, i));
                }
            }
            match best {
                Some((_, i)) => insert_sorted(&mut clusters[i].members, member),
                None => homeless.push(member),
            }
        }
        if !homeless.is_empty() {
            // Nothing positively similar — keep them as their own cluster.
            let mut kept = new_cluster(idx, homeless);
            kept.centroid = small.centroid;
            clusters.insert(idx, kept);
            // Don't loop on the same undersized cluster forever.
            if clusters.iter().all(|c| c.members.len() < min_size) {
                return;
            }
            // If this kept cluster is the only undersized one left, stop.
            if clusters
                .iter()
                .enumerate()
                .all(|(i, c)| i == idx || c.members.len() >= min_size)
            {
                return;
            }
        }
        // Recompute affected centroids for subsequent folds.
        compute_centroids(clusters, vectors);
    }
}

/// Rewrite ids/labels of `new` in place based on member overlap with `prev`.
///
/// Pairs are matched greedily by `(jaccard desc, prev id asc, new index asc)`.
/// A match with jaccard >= 0.3 inherits the previous id; >= 0.6 also inherits
/// the previous label. Unmatched new clusters mint fresh ids. Returns the
/// updated next-id counter.
pub(crate) fn match_to_previous(new: &mut [ClusterInfo], prev: &ClusterState) -> usize {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (pi, p) in prev.clusters.iter().enumerate() {
        let pset: HashSet<&String> = p.members.iter().collect();
        if pset.is_empty() {
            continue;
        }
        for (ni, n) in new.iter().enumerate() {
            let intersection = n.members.iter().filter(|m| pset.contains(m)).count();
            if intersection == 0 {
                continue;
            }
            let union = pset.len() + n.members.len() - intersection;
            let jaccard = intersection as f64 / union as f64;
            if jaccard >= STABILITY_ID_JACCARD {
                pairs.push((jaccard, pi, ni));
            }
        }
    }

    pairs.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| prev.clusters[a.1].id.cmp(&prev.clusters[b.1].id))
            .then_with(|| a.2.cmp(&b.2))
    });

    let mut prev_used = vec![false; prev.clusters.len()];
    let mut new_matched = vec![false; new.len()];
    for (jaccard, pi, ni) in pairs {
        if prev_used[pi] || new_matched[ni] {
            continue;
        }
        prev_used[pi] = true;
        new_matched[ni] = true;
        new[ni].id = prev.clusters[pi].id;
        if jaccard >= STABILITY_LABEL_JACCARD {
            new[ni].label = prev.clusters[pi].label.clone();
        }
    }

    let mut next = next_id_floor(prev);
    for (ni, matched) in new_matched.iter().enumerate() {
        if !matched {
            new[ni].id = next;
            next += 1;
        }
    }
    next
}

/// The smallest id safe to mint next given a previous state (never reuse).
fn next_id_floor(prev: &ClusterState) -> usize {
    let max_used = prev
        .clusters
        .iter()
        .chain(prev.parent_clusters.iter())
        .map(|c| c.id + 1)
        .max()
        .unwrap_or(0);
    prev.next_cluster_id.max(max_used)
}

/// Insert into a sorted Vec<String>, keeping it sorted and deduped.
fn insert_sorted(vec: &mut Vec<String>, value: String) {
    if let Err(pos) = vec.binary_search(&value) {
        vec.insert(pos, value);
    }
}

/// Compute cross-cluster TF-IDF keywords and labels for document clusters.
fn assign_doc_cluster_keywords(
    clusters: &mut [ClusterInfo],
    documents: &HashMap<String, String>,
    n: usize,
) {
    if clusters.is_empty() {
        return;
    }
    let cluster_texts: Vec<Vec<&str>> = clusters
        .iter()
        .map(|c| {
            c.members
                .iter()
                .filter_map(|m| documents.get(m).map(|s| s.as_str()))
                .collect()
        })
        .collect();
    let keywords = labels::cross_cluster_keywords(&cluster_texts, n);
    for (cluster, kws) in clusters.iter_mut().zip(keywords) {
        cluster.label = labels::generate_label(&kws);
        cluster.keywords = kws;
    }
}

/// Compute a fingerprint of everything that determines topic centroids and
/// assignments: the definitions (in order), the global floor, and the
/// embedding model/dimensions. Stored in `CustomClusterState`; a mismatch
/// means the persisted topics are stale and need a full recompute.
pub fn topics_fingerprint(
    defs: &[CustomClusterDef],
    min_similarity: f32,
    embedding_model: &str,
    embedding_dimensions: usize,
) -> String {
    use sha2::{Digest, Sha256};

    let payload = serde_json::json!({
        "version": "topics-v1",
        "defs": defs,
        "min_similarity": min_similarity,
        "model": embedding_model,
        "dims": embedding_dimensions,
    });
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_string(&payload).unwrap_or_default().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Embed topic definitions into unit-normalized centroid vectors.
///
/// Per definition: the description (embedded as the sentence
/// `"{name}: {description}"`) and the seed phrases (each embedding
/// unit-normalized, then averaged and re-normalized) are combined as
/// `normalize(0.6 * description + 0.4 * seeds)`; a definition with only one
/// component uses it alone. Errors on empty definitions or inconsistent
/// embedding dimensions.
pub async fn embed_topic_centroids(
    defs: &[CustomClusterDef],
    provider: &dyn crate::embedding::provider::EmbeddingProvider,
) -> crate::Result<Vec<Vec<f32>>> {
    let mut centroids = Vec::with_capacity(defs.len());
    let mut expected_dims: Option<usize> = None;

    for def in defs {
        let description = def
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(|d| format!("{}: {}", def.name, d));

        let mut texts: Vec<String> = Vec::with_capacity(def.seeds.len() + 1);
        if let Some(desc) = &description {
            texts.push(desc.clone());
        }
        texts.extend(def.seeds.iter().cloned());

        if texts.is_empty() {
            return Err(crate::Error::Clustering(format!(
                "topic '{}' has neither a description nor seed phrases",
                def.name
            )));
        }

        let embeddings = provider.embed_batch(&texts).await?;
        if embeddings.len() != texts.len() {
            return Err(crate::Error::Clustering(format!(
                "embedding count mismatch for topic '{}': sent {} texts, got {} embeddings",
                def.name,
                texts.len(),
                embeddings.len()
            )));
        }
        for emb in &embeddings {
            match expected_dims {
                None => expected_dims = Some(emb.len()),
                Some(d) if emb.len() != d => {
                    return Err(crate::Error::Clustering(format!(
                        "embedding dimension mismatch for topic '{}': got {}, expected {d}",
                        def.name,
                        emb.len()
                    )));
                }
                _ => {}
            }
        }

        let seed_start = usize::from(description.is_some());
        let desc_vec = description
            .is_some()
            .then(|| normalize_in_place(embeddings[0].clone()));
        let seed_vec = (embeddings.len() > seed_start).then(|| {
            let dims = embeddings[seed_start].len();
            let mut mean = vec![0.0f32; dims];
            let count = (embeddings.len() - seed_start) as f32;
            for emb in &embeddings[seed_start..] {
                let unit = normalize_in_place(emb.clone());
                for (i, v) in unit.iter().enumerate() {
                    mean[i] += v;
                }
            }
            for v in &mut mean {
                *v /= count;
            }
            normalize_in_place(mean)
        });

        let centroid = match (desc_vec, seed_vec) {
            (Some(d), Some(s)) => normalize_in_place(
                d.iter()
                    .zip(s.iter())
                    .map(|(dv, sv)| dv * TOPIC_DESC_WEIGHT + sv * TOPIC_SEED_WEIGHT)
                    .collect(),
            ),
            (Some(d), None) => d,
            (None, Some(s)) => s,
            (None, None) => unreachable!("texts checked non-empty"),
        };

        centroids.push(centroid);
    }

    Ok(centroids)
}

/// Compute cosine similarity between two vectors.
///
/// Returns 0.0 if either vector has zero magnitude.
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "vectors must have equal dimensions");

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for (ai, bi) in a.iter().zip(b.iter()) {
        dot += ai * bi;
        norm_a += ai * ai;
        norm_b += bi * bi;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

/// Unit-normalize a set of vectors in deterministic (sorted key) order.
///
/// Returns the normalized vectors plus the sorted keys of zero-norm vectors,
/// which cannot participate in cosine-based clustering.
pub(crate) fn normalize_vectors(
    raw: &HashMap<String, Vec<f32>>,
) -> (BTreeMap<String, Vec<f32>>, Vec<String>) {
    let mut normalized: BTreeMap<String, Vec<f32>> = BTreeMap::new();
    let mut zero_norm: Vec<String> = Vec::new();

    for (key, v) in raw {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            zero_norm.push(key.clone());
        } else {
            normalized.insert(key.clone(), v.iter().map(|x| x / norm).collect());
        }
    }

    zero_norm.sort();
    (normalized, zero_norm)
}

/// Scale a vector to unit length (no-op for zero vectors).
pub(crate) fn normalize_in_place(mut v: Vec<f32>) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// Index of the cluster whose centroid is most cosine-similar to `vector`.
fn nearest_cluster_index(state: &ClusterState, vector: &[f32]) -> usize {
    let mut best_idx = 0;
    let mut best_sim = f32::NEG_INFINITY;
    for (i, cluster) in state.clusters.iter().enumerate() {
        let sim = cosine_similarity(vector, &cluster.centroid);
        if sim > best_sim {
            best_sim = sim;
            best_idx = i;
        }
    }
    best_idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        let mut config = Config::load(std::path::Path::new("/nonexistent")).unwrap();
        config.clustering_enabled = true;
        config.clustering_rebalance_threshold = 50;
        config
    }

    fn kmeans_config() -> Config {
        let mut config = test_config();
        config.clustering_algorithm = ClusteringAlgorithm::Kmeans;
        config
    }

    fn bare_cluster(id: usize, centroid: Vec<f32>, members: Vec<&str>) -> ClusterInfo {
        ClusterInfo {
            id,
            label: format!("cluster-{id}"),
            centroid,
            members: members.into_iter().map(String::from).collect(),
            keywords: vec![],
            parent_id: None,
            representative: None,
        }
    }

    fn state_with(clusters: Vec<ClusterInfo>) -> ClusterState {
        let next = clusters.iter().map(|c| c.id + 1).max().unwrap_or(0);
        ClusterState {
            clusters,
            docs_since_rebalance: 0,
            docs_at_last_rebalance: 0,
            next_cluster_id: next,
            algorithm: "leiden".to_string(),
            unclustered: vec![],
            parent_clusters: vec![],
        }
    }

    #[test]
    fn cosine_similarity_identical_vectors() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![0.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn normalize_vectors_splits_zero_norm() {
        let mut raw = HashMap::new();
        raw.insert("b.md".to_string(), vec![3.0, 4.0]);
        raw.insert("a.md".to_string(), vec![0.0, 0.0]);
        let (normalized, zero) = normalize_vectors(&raw);
        assert_eq!(zero, vec!["a.md".to_string()]);
        let v = &normalized["b.md"];
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn cluster_info_serializes_to_json() {
        let info = bare_cluster(0, vec![0.1, 0.2, 0.3], vec!["doc1.md"]);
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("cluster-0"));
        // Optional fields are omitted when None (additive JSON).
        assert!(!json.contains("parent_id"));
        assert!(!json.contains("representative"));
    }

    // --- cluster_all (both algorithms) ---

    fn grouped_vectors(n_per_group: usize) -> (HashMap<String, Vec<f32>>, HashMap<String, String>) {
        let mut vectors = HashMap::new();
        let mut documents = HashMap::new();
        for i in 0..n_per_group {
            let mut v = vec![0.02f32; 8];
            v[0] = 1.0;
            v[1] = 0.05 * i as f32;
            vectors.insert(format!("rust{i}.md"), v);
            documents.insert(
                format!("rust{i}.md"),
                format!("rust cargo borrow checker systems programming {i}"),
            );
            let mut w = vec![0.02f32; 8];
            w[4] = 1.0;
            w[5] = 0.05 * i as f32;
            vectors.insert(format!("cook{i}.md"), w);
            documents.insert(
                format!("cook{i}.md"),
                format!("cooking recipe kitchen ingredients food {i}"),
            );
        }
        (vectors, documents)
    }

    #[test]
    fn leiden_cluster_all_produces_valid_state() {
        let clusterer = Clusterer::new(&test_config());
        let (vectors, documents) = grouped_vectors(5);
        let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();

        assert_eq!(state.algorithm, "leiden");
        assert!(!state.clusters.is_empty());
        let total: usize = state.clusters.iter().map(|c| c.members.len()).sum();
        assert_eq!(total, 10, "every non-zero doc is in exactly one cluster");
        for c in &state.clusters {
            assert!(!c.label.is_empty());
            assert!(!c.centroid.is_empty());
            let norm: f32 = c.centroid.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-5, "centroid must be unit length");
            assert!(c.representative.is_some());
            assert!(c.members.contains(c.representative.as_ref().unwrap()));
        }
        assert_eq!(state.next_cluster_id, state.clusters.len());
        assert!(state.unclustered.is_empty());
    }

    #[test]
    fn leiden_determinism_two_runs_identical() {
        let clusterer = Clusterer::new(&test_config());
        let (vectors, documents) = grouped_vectors(6);
        let a = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        let b = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }

    #[test]
    fn kmeans_determinism_two_runs_identical() {
        let clusterer = Clusterer::new(&kmeans_config());
        let (vectors, documents) = grouped_vectors(6);
        let a = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        let b = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        assert_eq!(a.algorithm, "kmeans");
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }

    #[test]
    fn leiden_separates_distinct_groups() {
        let clusterer = Clusterer::new(&test_config());
        let (vectors, documents) = grouped_vectors(5);
        let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();

        // rust* docs and cook* docs must not share a cluster.
        for c in &state.clusters {
            let rust = c.members.iter().filter(|m| m.starts_with("rust")).count();
            let cook = c.members.iter().filter(|m| m.starts_with("cook")).count();
            assert!(
                rust == 0 || cook == 0,
                "cluster {} mixes groups: {:?}",
                c.id,
                c.members
            );
        }
    }

    #[test]
    fn higher_resolution_no_fewer_clusters() {
        let (vectors, documents) = grouped_vectors(8);

        let mut coarse_cfg = test_config();
        coarse_cfg.clustering_resolution = 0.3;
        let coarse = Clusterer::new(&coarse_cfg)
            .cluster_all(&vectors, &documents, None)
            .unwrap();

        let mut fine_cfg = test_config();
        fine_cfg.clustering_resolution = 5.0;
        let fine = Clusterer::new(&fine_cfg)
            .cluster_all(&vectors, &documents, None)
            .unwrap();

        assert!(
            fine.clusters.len() >= coarse.clusters.len(),
            "higher resolution should not produce fewer clusters ({} vs {})",
            fine.clusters.len(),
            coarse.clusters.len()
        );
    }

    #[test]
    fn cluster_all_empty_vectors() {
        let clusterer = Clusterer::new(&test_config());
        let state = clusterer
            .cluster_all(&HashMap::new(), &HashMap::new(), None)
            .unwrap();
        assert!(state.clusters.is_empty());
        assert_eq!(state.docs_since_rebalance, 0);
        assert_eq!(state.algorithm, "leiden");
    }

    #[test]
    fn cluster_all_single_vector() {
        let clusterer = Clusterer::new(&test_config());
        let mut vectors = HashMap::new();
        vectors.insert("doc.md".to_string(), vec![1.0, 0.0, 0.0]);
        let mut documents = HashMap::new();
        documents.insert("doc.md".to_string(), "rust programming language".to_string());
        let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        assert_eq!(state.clusters.len(), 1);
        assert_eq!(state.clusters[0].members, vec!["doc.md".to_string()]);
        assert_eq!(state.clusters[0].representative.as_deref(), Some("doc.md"));
        assert_eq!(state.docs_at_last_rebalance, 1);
    }

    #[test]
    fn cluster_all_records_zero_norm_as_unclustered() {
        let clusterer = Clusterer::new(&test_config());
        let (mut vectors, documents) = grouped_vectors(3);
        vectors.insert("zero.md".to_string(), vec![0.0; 8]);
        let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        assert_eq!(state.unclustered, vec!["zero.md".to_string()]);
        for c in &state.clusters {
            assert!(!c.members.contains(&"zero.md".to_string()));
        }
    }

    #[test]
    fn cluster_all_only_zero_vectors() {
        let clusterer = Clusterer::new(&test_config());
        let mut vectors = HashMap::new();
        vectors.insert("z1.md".to_string(), vec![0.0, 0.0, 0.0]);
        vectors.insert("z2.md".to_string(), vec![0.0, 0.0, 0.0]);
        let state = clusterer.cluster_all(&vectors, &HashMap::new(), None).unwrap();
        assert!(state.clusters.is_empty());
        assert_eq!(state.unclustered.len(), 2);
    }

    #[test]
    fn min_cluster_size_folds_stragglers() {
        let mut config = test_config();
        config.clustering_min_cluster_size = 3;
        let clusterer = Clusterer::new(&config);
        let (vectors, documents) = grouped_vectors(5);
        let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        for c in &state.clusters {
            assert!(
                c.members.len() >= 3,
                "cluster {} smaller than min_cluster_size: {:?}",
                c.id,
                c.members
            );
        }
    }

    #[test]
    fn keywords_include_bigrams_from_content() {
        let clusterer = Clusterer::new(&test_config());
        let (vectors, documents) = grouped_vectors(5);
        let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        let all_keywords: Vec<&String> =
            state.clusters.iter().flat_map(|c| c.keywords.iter()).collect();
        assert!(
            all_keywords.iter().any(|k| k.contains(' ')),
            "expected at least one bigram keyword, got {all_keywords:?}"
        );
    }

    // --- identity stability ---

    #[test]
    fn stability_preserves_ids_across_recluster() {
        let clusterer = Clusterer::new(&test_config());
        let (mut vectors, mut documents) = grouped_vectors(6);
        let first = clusterer.cluster_all(&vectors, &documents, None).unwrap();

        // Add two more docs to one group and re-cluster with `previous`.
        for i in 6..8 {
            let mut v = vec![0.02f32; 8];
            v[0] = 1.0;
            v[1] = 0.05 * i as f32;
            vectors.insert(format!("rust{i}.md"), v);
            documents.insert(format!("rust{i}.md"), format!("rust cargo systems {i}"));
        }
        let second = clusterer
            .cluster_all(&vectors, &documents, Some(&first))
            .unwrap();

        // For every first-run cluster there must be a second-run cluster with
        // the same id containing (most of) the same members.
        for prev in &first.clusters {
            let survived = second.clusters.iter().find(|c| c.id == prev.id);
            assert!(
                survived.is_some(),
                "cluster id {} vanished after re-cluster",
                prev.id
            );
            let now = survived.unwrap();
            let overlap = prev
                .members
                .iter()
                .filter(|m| now.members.contains(m))
                .count();
            assert!(overlap * 2 >= prev.members.len(), "id {} drifted", prev.id);
        }
        assert!(second.next_cluster_id >= first.next_cluster_id);
    }

    #[test]
    fn stability_new_group_gets_fresh_id() {
        let clusterer = Clusterer::new(&test_config());
        let (mut vectors, mut documents) = grouped_vectors(6);
        let first = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        let max_first_id = first.clusters.iter().map(|c| c.id).max().unwrap();

        // Inject a brand-new distinct group.
        for i in 0..6 {
            let mut v = vec![0.02f32; 8];
            v[7] = 1.0;
            v[6] = 0.05 * i as f32;
            vectors.insert(format!("music{i}.md"), v);
            documents.insert(
                format!("music{i}.md"),
                format!("guitar chords melody rhythm music {i}"),
            );
        }
        let second = clusterer
            .cluster_all(&vectors, &documents, Some(&first))
            .unwrap();

        let music_cluster = second
            .clusters
            .iter()
            .find(|c| c.members.iter().any(|m| m.starts_with("music")))
            .expect("music docs must be clustered");
        assert!(
            music_cluster.id > max_first_id,
            "new cluster must mint a fresh id ({} <= {max_first_id})",
            music_cluster.id
        );
    }

    #[test]
    fn match_to_previous_inherits_label_on_strong_overlap() {
        let prev = state_with(vec![bare_cluster(
            3,
            vec![1.0, 0.0],
            vec!["a.md", "b.md", "c.md"],
        )]);
        let mut new = vec![bare_cluster(0, vec![1.0, 0.0], vec!["a.md", "b.md", "c.md"])];
        new[0].label = "fresh label".to_string();
        let next = match_to_previous(&mut new, &prev);
        assert_eq!(new[0].id, 3);
        assert_eq!(new[0].label, "cluster-3", "strong overlap inherits label");
        assert_eq!(next, 4);
    }

    #[test]
    fn match_to_previous_keeps_new_label_on_weak_overlap() {
        let prev = state_with(vec![bare_cluster(
            2,
            vec![1.0, 0.0],
            vec!["a.md", "b.md", "c.md", "d.md", "e.md"],
        )]);
        // Overlap 2/6 elements → jaccard 2/8 = 0.25 < 0.3 → no id match either.
        let mut new = vec![bare_cluster(
            0,
            vec![1.0, 0.0],
            vec!["a.md", "b.md", "x.md", "y.md", "z.md"],
        )];
        new[0].label = "fresh label".to_string();
        let next = match_to_previous(&mut new, &prev);
        assert_eq!(new[0].id, 3, "weak overlap mints a fresh id");
        assert_eq!(new[0].label, "fresh label");
        assert_eq!(next, 4);
    }

    #[test]
    fn algorithm_switch_detected() {
        let clusterer = Clusterer::new(&kmeans_config());
        let state = state_with(vec![]);
        assert!(clusterer.algorithm_changed(&state), "leiden state + kmeans config");
    }

    // --- incremental assignment ---

    #[test]
    fn assign_incremental_picks_closest_cluster() {
        let clusterer = Clusterer::new(&kmeans_config());
        let mut state = state_with(vec![
            bare_cluster(0, vec![1.0, 0.0, 0.0], vec!["a.md"]),
            bare_cluster(1, vec![0.0, 1.0, 0.0], vec!["b.md"]),
        ]);

        let assigned = clusterer
            .assign_incremental(&mut state, "new.md", &[0.9, 0.1, 0.0], &HashMap::new())
            .unwrap();
        assert_eq!(assigned, Some(0));
        assert!(state.clusters[0].members.contains(&"new.md".to_string()));
        assert_eq!(state.docs_since_rebalance, 1);
    }

    #[test]
    fn assign_incremental_updates_centroid() {
        let clusterer = Clusterer::new(&kmeans_config());
        let mut state = state_with(vec![bare_cluster(0, vec![1.0, 0.0, 0.0], vec!["a.md"])]);

        clusterer
            .assign_incremental(&mut state, "b.md", &[0.0, 1.0, 0.0], &HashMap::new())
            .unwrap();

        // Mean is (0.5, 0.5, 0), re-normalized to unit length (1/√2, 1/√2, 0).
        let c = &state.clusters[0].centroid;
        let expected = 1.0 / 2.0f32.sqrt();
        assert!((c[0] - expected).abs() < 1e-6);
        assert!((c[1] - expected).abs() < 1e-6);
    }

    #[test]
    fn assign_incremental_removes_prior_membership() {
        let clusterer = Clusterer::new(&kmeans_config());
        let mut state = state_with(vec![
            bare_cluster(0, vec![1.0, 0.0, 0.0], vec!["a.md", "doc.md"]),
            bare_cluster(1, vec![0.0, 1.0, 0.0], vec!["b.md"]),
        ]);

        let assigned = clusterer
            .assign_incremental(&mut state, "doc.md", &[0.0, 0.9, 0.1], &HashMap::new())
            .unwrap();
        assert_eq!(assigned, Some(1));
        assert!(!state.clusters[0].members.contains(&"doc.md".to_string()));
        assert!(state.clusters[1].members.contains(&"doc.md".to_string()));
        // Not a new document — the rebalance counter must not move.
        assert_eq!(state.docs_since_rebalance, 0);
    }

    #[test]
    fn assign_incremental_neighbor_vote_majority() {
        let clusterer = Clusterer::new(&test_config()); // leiden mode
        let mut state = state_with(vec![
            bare_cluster(0, vec![1.0, 0.0, 0.0], vec!["a1.md", "a2.md", "a3.md"]),
            bare_cluster(1, vec![0.0, 1.0, 0.0], vec!["b1.md", "b2.md"]),
        ]);

        let mut all_vectors = HashMap::new();
        all_vectors.insert("a1.md".to_string(), vec![1.0, 0.05, 0.0]);
        all_vectors.insert("a2.md".to_string(), vec![0.95, 0.1, 0.0]);
        all_vectors.insert("a3.md".to_string(), vec![0.9, 0.0, 0.05]);
        all_vectors.insert("b1.md".to_string(), vec![0.0, 1.0, 0.05]);
        all_vectors.insert("b2.md".to_string(), vec![0.05, 0.95, 0.0]);

        let assigned = clusterer
            .assign_incremental(&mut state, "new.md", &[0.85, 0.15, 0.0], &all_vectors)
            .unwrap();
        assert_eq!(assigned, Some(0), "neighbor vote should pick the a-group");
        assert!(state.clusters[0].members.contains(&"new.md".to_string()));
    }

    #[test]
    fn assign_incremental_zero_norm_goes_unclustered() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = state_with(vec![bare_cluster(0, vec![1.0, 0.0], vec!["a.md"])]);
        let assigned = clusterer
            .assign_incremental(&mut state, "zero.md", &[0.0, 0.0], &HashMap::new())
            .unwrap();
        assert_eq!(assigned, None);
        assert_eq!(state.unclustered, vec!["zero.md".to_string()]);
    }

    #[test]
    fn assign_incremental_empty_clusters_errors() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = state_with(vec![]);
        let result =
            clusterer.assign_incremental(&mut state, "x.md", &[1.0, 0.0], &HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn remove_document_clears_all_membership() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = state_with(vec![bare_cluster(0, vec![1.0, 0.0], vec!["a.md", "b.md"])]);
        state.unclustered.push("z.md".to_string());

        assert!(clusterer.remove_document(&mut state, "a.md"));
        assert!(clusterer.remove_document(&mut state, "z.md"));
        assert!(!clusterer.remove_document(&mut state, "missing.md"));
        assert_eq!(state.clusters[0].members, vec!["b.md".to_string()]);
        assert!(state.unclustered.is_empty());
    }

    // --- rebalance ---

    #[test]
    fn maybe_rebalance_below_threshold() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = state_with(vec![]);
        state.docs_since_rebalance = 5;
        let rebalanced = clusterer
            .maybe_rebalance(&mut state, &HashMap::new(), &HashMap::new())
            .unwrap();
        assert!(!rebalanced);
    }

    #[test]
    fn maybe_rebalance_above_threshold() {
        let mut config = test_config();
        config.clustering_rebalance_threshold = 2;
        let clusterer = Clusterer::new(&config);

        let (vectors, documents) = grouped_vectors(4);
        let mut state = state_with(vec![]);
        state.docs_since_rebalance = 3;

        let rebalanced = clusterer
            .maybe_rebalance(&mut state, &vectors, &documents)
            .unwrap();
        assert!(rebalanced);
        assert!(!state.clusters.is_empty());
        assert_eq!(state.docs_since_rebalance, 0);
    }

    #[test]
    fn rebalance_threshold_returns_config_value() {
        let config = test_config();
        let clusterer = Clusterer::new(&config);
        assert_eq!(clusterer.rebalance_threshold(), 50);
    }

    #[test]
    fn is_enabled_returns_config_value() {
        let config = test_config();
        let clusterer = Clusterer::new(&config);
        assert!(clusterer.is_enabled());
    }

    // --- hierarchy ---

    #[test]
    fn hierarchy_parent_ids_valid_when_present() {
        let mut config = test_config();
        config.clustering_resolution = 5.0; // force many fine clusters
        config.clustering_min_cluster_size = 1;
        let clusterer = Clusterer::new(&config);

        // Build 8 distinct small groups.
        let mut vectors = HashMap::new();
        let mut documents = HashMap::new();
        for g in 0..8 {
            for i in 0..3 {
                let mut v = vec![0.01f32; 16];
                v[g * 2] = 1.0;
                v[g * 2 + 1] = 0.1 * i as f32;
                vectors.insert(format!("g{g}doc{i}.md"), v);
                documents.insert(format!("g{g}doc{i}.md"), format!("group{g} topic word{i}"));
            }
        }
        let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();

        if state.parent_clusters.is_empty() {
            // Hierarchy is best-effort; when skipped, no parent_ids may dangle.
            for c in &state.clusters {
                assert!(c.parent_id.is_none());
            }
        } else {
            let parent_ids: HashSet<usize> =
                state.parent_clusters.iter().map(|p| p.id).collect();
            for c in &state.clusters {
                if let Some(pid) = c.parent_id {
                    assert!(parent_ids.contains(&pid), "dangling parent_id {pid}");
                }
            }
            // Parent members = union of children members.
            for p in &state.parent_clusters {
                let child_union: usize = state
                    .clusters
                    .iter()
                    .filter(|c| c.parent_id == Some(p.id))
                    .map(|c| c.members.len())
                    .sum();
                assert_eq!(p.members.len(), child_union);
                assert!(!p.label.is_empty());
            }
            // Parent ids don't collide with cluster ids.
            for c in &state.clusters {
                assert!(!parent_ids.contains(&c.id));
            }
        }
    }

    #[test]
    fn no_hierarchy_below_min_clusters() {
        let clusterer = Clusterer::new(&test_config());
        let (vectors, documents) = grouped_vectors(5);
        let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        assert!(state.clusters.len() < HIERARCHY_MIN_CLUSTERS);
        assert!(state.parent_clusters.is_empty());
    }

    // --- topics (custom clusters) ---

    fn topic_defs() -> Vec<CustomClusterDef> {
        vec![
            CustomClusterDef {
                name: "X Topic".to_string(),
                description: None,
                seeds: vec!["x things".to_string()],
                threshold: None,
            },
            CustomClusterDef {
                name: "Y Topic".to_string(),
                description: None,
                seeds: vec!["y things".to_string()],
                threshold: None,
            },
        ]
    }

    fn topic_centroids() -> Vec<Vec<f32>> {
        vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]]
    }

    #[test]
    fn multi_label_assigns_doc_to_all_topics_above_floor() {
        let mut config = test_config();
        config.topics_min_similarity = 0.3;
        let clusterer = Clusterer::new(&config);

        let mut doc_vectors = HashMap::new();
        // Similar to both centroids (cos ≈ 0.707 each).
        doc_vectors.insert("both.md".to_string(), vec![1.0, 1.0, 0.0]);
        let state = clusterer
            .assign_all_to_custom(&topic_defs(), &topic_centroids(), &doc_vectors, "fp".into())
            .unwrap();

        assert_eq!(state.clusters[0].members.len(), 1);
        assert_eq!(state.clusters[1].members.len(), 1);
        assert!((state.clusters[0].members[0].score - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        assert!(state.unassigned.is_empty());
        assert_eq!(state.fingerprint, "fp");
    }

    #[test]
    fn docs_below_floor_land_in_unassigned() {
        let mut config = test_config();
        config.topics_min_similarity = 0.9;
        let clusterer = Clusterer::new(&config);

        let mut doc_vectors = HashMap::new();
        doc_vectors.insert("meh.md".to_string(), vec![1.0, 1.0, 0.0]); // 0.707 < 0.9
        let state = clusterer
            .assign_all_to_custom(&topic_defs(), &topic_centroids(), &doc_vectors, "fp".into())
            .unwrap();

        assert!(state.clusters.iter().all(|c| c.members.is_empty()));
        assert_eq!(state.unassigned, vec!["meh.md".to_string()]);
    }

    #[test]
    fn per_topic_threshold_above_floor_applies() {
        let mut config = test_config();
        config.topics_min_similarity = 0.2;
        let clusterer = Clusterer::new(&config);

        let mut defs = topic_defs();
        defs[0].threshold = Some(0.9); // stricter than the floor
        let mut doc_vectors = HashMap::new();
        doc_vectors.insert("doc.md".to_string(), vec![1.0, 1.0, 0.0]); // 0.707

        let state = clusterer
            .assign_all_to_custom(&defs, &topic_centroids(), &doc_vectors, "fp".into())
            .unwrap();

        assert!(state.clusters[0].members.is_empty(), "blocked by topic threshold");
        assert_eq!(state.clusters[1].members.len(), 1, "floor still applies to topic 2");
    }

    #[test]
    fn global_floor_overrides_lower_topic_threshold() {
        let mut config = test_config();
        config.topics_min_similarity = 0.8;
        let clusterer = Clusterer::new(&config);

        let mut defs = topic_defs();
        defs[0].threshold = Some(0.1); // laxer than the floor — floor wins
        let mut doc_vectors = HashMap::new();
        doc_vectors.insert("doc.md".to_string(), vec![1.0, 1.0, 0.0]); // 0.707 < 0.8

        let state = clusterer
            .assign_all_to_custom(&defs, &topic_centroids(), &doc_vectors, "fp".into())
            .unwrap();
        assert!(state.clusters[0].members.is_empty());
        assert_eq!(state.unassigned, vec!["doc.md".to_string()]);
    }

    #[test]
    fn zero_norm_vector_goes_to_unassigned() {
        let clusterer = Clusterer::new(&test_config());
        let mut doc_vectors = HashMap::new();
        doc_vectors.insert("zero.md".to_string(), vec![0.0, 0.0, 0.0]);
        let state = clusterer
            .assign_all_to_custom(&topic_defs(), &topic_centroids(), &doc_vectors, "fp".into())
            .unwrap();
        assert_eq!(state.unassigned, vec!["zero.md".to_string()]);
        assert!(state.clusters.iter().all(|c| c.members.is_empty()));
    }

    #[test]
    fn dimension_mismatch_returns_clustering_error() {
        let clusterer = Clusterer::new(&test_config());
        let mut doc_vectors = HashMap::new();
        doc_vectors.insert("doc.md".to_string(), vec![1.0, 0.0]); // 2-d vs 3-d centroids
        let result = clusterer.assign_all_to_custom(
            &topic_defs(),
            &topic_centroids(),
            &doc_vectors,
            "fp".into(),
        );
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("dimension mismatch"));
    }

    #[test]
    fn assignment_is_deterministic_and_sorted() {
        let mut config = test_config();
        config.topics_min_similarity = 0.1;
        let clusterer = Clusterer::new(&config);

        let mut doc_vectors = HashMap::new();
        doc_vectors.insert("c.md".to_string(), vec![0.9, 0.1, 0.0]);
        doc_vectors.insert("a.md".to_string(), vec![0.8, 0.2, 0.0]);
        doc_vectors.insert("b.md".to_string(), vec![0.7, 0.3, 0.0]);

        let s1 = clusterer
            .assign_all_to_custom(&topic_defs(), &topic_centroids(), &doc_vectors, "fp".into())
            .unwrap();
        let s2 = clusterer
            .assign_all_to_custom(&topic_defs(), &topic_centroids(), &doc_vectors, "fp".into())
            .unwrap();
        assert_eq!(
            serde_json::to_string(&s1).unwrap(),
            serde_json::to_string(&s2).unwrap()
        );
        let paths: Vec<&str> = s1.clusters[0].members.iter().map(|m| m.path.as_str()).collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted, "members must be path-sorted");
    }

    #[test]
    fn assign_single_moves_between_topics_and_unassigned() {
        let mut config = test_config();
        config.topics_min_similarity = 0.5;
        let clusterer = Clusterer::new(&config);

        let mut doc_vectors = HashMap::new();
        doc_vectors.insert("doc.md".to_string(), vec![1.0, 0.0, 0.0]);
        let mut state = clusterer
            .assign_all_to_custom(&topic_defs(), &topic_centroids(), &doc_vectors, "fp".into())
            .unwrap();
        assert_eq!(state.clusters[0].members.len(), 1);

        // Re-assign with a vector now matching topic 1 instead.
        clusterer
            .assign_single_to_custom(&mut state, "doc.md", &[0.0, 1.0, 0.0])
            .unwrap();
        assert!(state.clusters[0].members.is_empty());
        assert_eq!(state.clusters[1].members.len(), 1);

        // Re-assign with a vector matching nothing → unassigned.
        clusterer
            .assign_single_to_custom(&mut state, "doc.md", &[0.0, 0.0, 1.0])
            .unwrap();
        assert!(state.clusters.iter().all(|c| c.members.is_empty()));
        assert_eq!(state.unassigned, vec!["doc.md".to_string()]);

        // Centroids stay frozen throughout.
        assert_eq!(state.clusters[0].centroid, vec![1.0, 0.0, 0.0]);
    }

    #[test]
    fn assign_single_zero_norm_goes_unassigned() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = clusterer
            .assign_all_to_custom(&topic_defs(), &topic_centroids(), &HashMap::new(), "fp".into())
            .unwrap();
        clusterer
            .assign_single_to_custom(&mut state, "zero.md", &[0.0, 0.0, 0.0])
            .unwrap();
        assert_eq!(state.unassigned, vec!["zero.md".to_string()]);
    }

    #[test]
    fn assign_single_dimension_mismatch_errors() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = clusterer
            .assign_all_to_custom(&topic_defs(), &topic_centroids(), &HashMap::new(), "fp".into())
            .unwrap();
        let result = clusterer.assign_single_to_custom(&mut state, "doc.md", &[1.0, 0.0]);
        assert!(result.is_err());
    }

    // --- fingerprint ---

    #[test]
    fn topics_fingerprint_stable_for_identical_inputs() {
        let defs = topic_defs();
        let a = topics_fingerprint(&defs, 0.3, "model-x", 1536);
        let b = topics_fingerprint(&defs, 0.3, "model-x", 1536);
        assert_eq!(a, b);
    }

    #[test]
    fn topics_fingerprint_changes_on_any_input() {
        let defs = topic_defs();
        let base = topics_fingerprint(&defs, 0.3, "model-x", 1536);

        let mut edited = defs.clone();
        edited[0].seeds.push("extra seed".to_string());
        assert_ne!(base, topics_fingerprint(&edited, 0.3, "model-x", 1536));

        let mut described = defs.clone();
        described[0].description = Some("described".to_string());
        assert_ne!(base, topics_fingerprint(&described, 0.3, "model-x", 1536));

        let mut reordered = defs.clone();
        reordered.reverse();
        assert_ne!(base, topics_fingerprint(&reordered, 0.3, "model-x", 1536));

        assert_ne!(base, topics_fingerprint(&defs, 0.4, "model-x", 1536));
        assert_ne!(base, topics_fingerprint(&defs, 0.3, "model-y", 1536));
        assert_ne!(base, topics_fingerprint(&defs, 0.3, "model-x", 768));
    }

    // --- edge clustering ---

    #[test]
    fn cluster_edges_too_few_edges() {
        let clusterer = Clusterer::new(&test_config());
        let mut vectors = HashMap::new();
        vectors.insert("e1".to_string(), vec![1.0, 0.0, 0.0]);
        vectors.insert("e2".to_string(), vec![0.0, 1.0, 0.0]);
        vectors.insert("e3".to_string(), vec![0.0, 0.0, 1.0]);
        let contexts = HashMap::new();

        let state = clusterer.cluster_edges(&vectors, &contexts).unwrap();
        assert!(state.clusters.is_empty());
        assert_eq!(state.edges_at_last_rebalance, 3);
    }

    #[test]
    fn cluster_edges_empty() {
        let clusterer = Clusterer::new(&test_config());
        let state = clusterer
            .cluster_edges(&HashMap::new(), &HashMap::new())
            .unwrap();
        assert!(state.clusters.is_empty());
    }

    #[test]
    fn cluster_edges_basic() {
        let clusterer = Clusterer::new(&test_config());
        let mut vectors = HashMap::new();
        let mut contexts = HashMap::new();

        for i in 0..5 {
            vectors.insert(format!("edge:a{i}"), vec![1.0, 0.1 * i as f32, 0.0, 0.0]);
            contexts.insert(
                format!("edge:a{i}"),
                format!("rust programming language systems {i}"),
            );
        }
        for i in 0..5 {
            vectors.insert(format!("edge:b{i}"), vec![0.0, 0.0, 1.0, 0.1 * i as f32]);
            contexts.insert(
                format!("edge:b{i}"),
                format!("cooking recipe food kitchen {i}"),
            );
        }

        let state = clusterer.cluster_edges(&vectors, &contexts).unwrap();
        assert!(!state.clusters.is_empty());

        let total: usize = state.clusters.iter().map(|c| c.members.len()).sum();
        assert_eq!(total, 10);

        for c in &state.clusters {
            assert!(!c.label.is_empty());
            assert!(!c.centroid.is_empty());
        }
    }

    #[test]
    fn cluster_edges_label_generation() {
        let clusterer = Clusterer::new(&test_config());
        let mut vectors = HashMap::new();
        let mut contexts = HashMap::new();

        for i in 0..6 {
            let mut v = vec![0.0f32; 4];
            v[i % 4] = 1.0;
            vectors.insert(format!("edge:{i}"), v);
            contexts.insert(
                format!("edge:{i}"),
                format!("documentation reference guide manual {i}"),
            );
        }

        let state = clusterer.cluster_edges(&vectors, &contexts).unwrap();
        for c in &state.clusters {
            assert!(!c.label.is_empty());
            assert_ne!(c.label, "Unlabeled");
        }
    }

    #[test]
    fn assign_edge_to_nearest_picks_closest() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = EdgeClusterState {
            clusters: vec![
                EdgeClusterInfo {
                    id: 0,
                    label: "A".to_string(),
                    centroid: vec![1.0, 0.0, 0.0],
                    members: vec!["e1".to_string()],
                    keywords: vec![],
                },
                EdgeClusterInfo {
                    id: 1,
                    label: "B".to_string(),
                    centroid: vec![0.0, 1.0, 0.0],
                    members: vec!["e2".to_string()],
                    keywords: vec![],
                },
            ],
            edges_since_rebalance: 0,
            edges_at_last_rebalance: 2,
        };

        let assigned = clusterer
            .assign_edge_to_nearest(&mut state, "e3", &[0.9, 0.1, 0.0])
            .unwrap();
        assert_eq!(assigned, 0);
        assert!(state.clusters[0].members.contains(&"e3".to_string()));
        assert_eq!(state.edges_since_rebalance, 1);
    }

    #[test]
    fn assign_edge_to_nearest_updates_centroid() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = EdgeClusterState {
            clusters: vec![EdgeClusterInfo {
                id: 0,
                label: "A".to_string(),
                centroid: vec![1.0, 0.0, 0.0],
                members: vec!["e1".to_string()],
                keywords: vec![],
            }],
            edges_since_rebalance: 0,
            edges_at_last_rebalance: 1,
        };

        clusterer
            .assign_edge_to_nearest(&mut state, "e2", &[0.0, 1.0, 0.0])
            .unwrap();

        // Mean is (0.5, 0.5, 0), re-normalized to unit length.
        let c = &state.clusters[0].centroid;
        let expected = 1.0 / 2.0f32.sqrt();
        assert!((c[0] - expected).abs() < 1e-6);
        assert!((c[1] - expected).abs() < 1e-6);
    }

    #[test]
    fn assign_edge_to_nearest_empty_clusters_errors() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = EdgeClusterState {
            clusters: vec![],
            edges_since_rebalance: 0,
            edges_at_last_rebalance: 0,
        };
        let result = clusterer.assign_edge_to_nearest(&mut state, "e1", &[1.0, 0.0]);
        assert!(result.is_err());
    }

    #[test]
    fn maybe_rebalance_edges_below_threshold() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = EdgeClusterState {
            clusters: vec![],
            edges_since_rebalance: 3,
            edges_at_last_rebalance: 10,
        };
        let rebalanced = clusterer
            .maybe_rebalance_edges(&mut state, &HashMap::new(), &HashMap::new(), 50)
            .unwrap();
        assert!(!rebalanced);
    }

    #[test]
    fn maybe_rebalance_edges_above_threshold() {
        let clusterer = Clusterer::new(&test_config());

        let mut vectors = HashMap::new();
        let mut contexts = HashMap::new();
        for i in 0..6 {
            let mut v = vec![0.0f32; 4];
            v[i % 4] = 1.0;
            vectors.insert(format!("edge:{i}"), v);
            contexts.insert(format!("edge:{i}"), format!("word{i} text content"));
        }

        let mut state = EdgeClusterState {
            clusters: vec![],
            edges_since_rebalance: 5,
            edges_at_last_rebalance: 0,
        };

        let rebalanced = clusterer
            .maybe_rebalance_edges(&mut state, &vectors, &contexts, 2)
            .unwrap();
        assert!(rebalanced);
        assert!(!state.clusters.is_empty());
        assert_eq!(state.edges_since_rebalance, 0);
    }

    // --- serialization shape ---

    #[test]
    fn cluster_state_serializes_to_json() {
        let state = ClusterState {
            clusters: vec![],
            docs_since_rebalance: 5,
            docs_at_last_rebalance: 100,
            next_cluster_id: 7,
            algorithm: "leiden".to_string(),
            unclustered: vec![],
            parent_clusters: vec![],
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("docs_since_rebalance"));
        assert!(json.contains("next_cluster_id"));
        assert!(json.contains("leiden"));
    }

    #[test]
    fn topic_member_serializes_with_named_fields() {
        let member = TopicMember {
            path: "doc.md".to_string(),
            score: 0.42,
        };
        let json = serde_json::to_string(&member).unwrap();
        assert!(json.contains("\"path\""));
        assert!(json.contains("\"score\""));
    }
}

use std::collections::HashMap;

use linfa::traits::Fit;
use linfa::DatasetBase;
use ndarray::Array2;
use serde::Serialize;
use tracing::{debug, info};

use crate::config::Config;
use crate::links::{EdgeClusterInfo, EdgeClusterState};

/// Common English stop words filtered out during keyword extraction.
const STOP_WORDS: &[&str] = &[
    "a", "about", "above", "after", "again", "against", "all", "am", "an", "and", "any", "are",
    "aren't", "as", "at", "be", "because", "been", "before", "being", "below", "between", "both",
    "but", "by", "can", "can't", "cannot", "could", "couldn't", "did", "didn't", "do", "does",
    "doesn't", "doing", "don't", "down", "during", "each", "few", "for", "from", "further", "get",
    "got", "had", "hadn't", "has", "hasn't", "have", "haven't", "having", "he", "her", "here",
    "hers", "herself", "him", "himself", "his", "how", "i", "if", "in", "into", "is", "isn't",
    "it", "its", "itself", "just", "let", "like", "ll", "me", "might", "more", "most", "must",
    "mustn't", "my", "myself", "no", "nor", "not", "now", "of", "off", "on", "once", "only",
    "or", "other", "our", "ours", "ourselves", "out", "over", "own", "re", "s", "same", "shall",
    "shan't", "she", "should", "shouldn't", "so", "some", "such", "t", "than", "that", "the",
    "their", "theirs", "them", "themselves", "then", "there", "these", "they", "this", "those",
    "through", "to", "too", "under", "until", "up", "us", "ve", "very", "was", "wasn't", "we",
    "were", "weren't", "what", "when", "where", "which", "while", "who", "whom", "why", "will",
    "with", "won't", "would", "wouldn't", "you", "your", "yours", "yourself", "yourselves",
];

/// Information about a single cluster, stored in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct ClusterInfo {
    /// Numeric cluster identifier (0-based).
    pub id: usize,
    /// Human-readable auto-generated label.
    pub label: String,
    /// Centroid vector (mean of member embeddings).
    pub centroid: Vec<f32>,
    /// File paths (relative) belonging to this cluster.
    pub members: Vec<String>,
    /// Top keywords extracted via TF-IDF.
    pub keywords: Vec<String>,
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
}

/// User-defined custom cluster definition (config-layer only, not persisted in index).
#[derive(Debug, Clone, Serialize)]
pub struct CustomClusterDef {
    /// User-provided cluster name.
    pub name: String,
    /// Seed words/phrases used to compute the centroid.
    pub seeds: Vec<String>,
}

/// Information about a single user-defined custom cluster, stored in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct CustomClusterInfo {
    /// Numeric cluster identifier (0-based).
    pub id: usize,
    /// User-provided cluster name.
    pub name: String,
    /// The seed phrases used to compute this cluster's centroid.
    pub seed_phrases: Vec<String>,
    /// Centroid vector (average of embedded seed phrases, normalized).
    pub centroid: Vec<f32>,
    /// File paths (relative) assigned to this cluster.
    pub members: Vec<String>,
}

/// Custom cluster state persisted in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct CustomClusterState {
    /// All custom clusters.
    pub clusters: Vec<CustomClusterInfo>,
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

    /// Run a full K-means clustering pass over all document vectors.
    ///
    /// `vectors` maps document path (relative) to its embedding vector.
    /// `documents` maps document path to its text content (for keyword extraction).
    pub fn cluster_all(
        &self,
        vectors: &HashMap<String, Vec<f32>>,
        documents: &HashMap<String, String>,
    ) -> crate::Result<ClusterState> {
        if vectors.is_empty() {
            debug!("cluster_all: no vectors, returning empty state");
            return Ok(ClusterState {
                clusters: Vec::new(),
                docs_since_rebalance: 0,
                docs_at_last_rebalance: 0,
            });
        }

        // Collect chunk IDs and their vectors in deterministic order
        let mut ids: Vec<String> = vectors.keys().cloned().collect();
        ids.sort();

        // Filter out zero-norm vectors
        let (ids, vecs): (Vec<String>, Vec<&Vec<f32>>) = ids
            .into_iter()
            .filter_map(|id| {
                let v = vectors.get(&id)?;
                let norm: f32 = v.iter().map(|x| x * x).sum();
                if norm == 0.0 {
                    debug!("cluster_all: skipping zero-norm vector for {id}");
                    None
                } else {
                    Some((id, v))
                }
            })
            .unzip();

        let n = ids.len();
        if n == 0 {
            return Ok(ClusterState {
                clusters: Vec::new(),
                docs_since_rebalance: 0,
                docs_at_last_rebalance: 0,
            });
        }

        // Single document: return one cluster
        if n == 1 {
            let doc_id = &ids[0];
            let doc_texts: Vec<&str> = documents
                .get(doc_id)
                .map(|s| vec![s.as_str()])
                .unwrap_or_default();
            let keywords = self.extract_keywords(&doc_texts, 5);
            let label = self.generate_label(&keywords);

            return Ok(ClusterState {
                clusters: vec![ClusterInfo {
                    id: 0,
                    label,
                    centroid: vecs[0].clone(),
                    members: vec![doc_id.clone()],
                    keywords,
                }],
                docs_since_rebalance: 0,
                docs_at_last_rebalance: n,
            });
        }

        let dim = vecs[0].len();
        let k = compute_k(n, self.config.clustering_granularity);

        // Build ndarray matrix (f64 for linfa)
        let mut data = Array2::<f64>::zeros((n, dim));
        for (i, v) in vecs.iter().enumerate() {
            for (j, &val) in v.iter().enumerate() {
                data[[i, j]] = val as f64;
            }
        }

        let dataset = DatasetBase::from(data);

        // Run K-means
        let model = linfa_clustering::KMeans::params(k)
            .max_n_iterations(100)
            .tolerance(1e-4)
            .fit(&dataset)
            .map_err(|e| crate::Error::Clustering(format!("K-means failed: {e}")))?;

        // Get cluster assignments
        let centroids = model.centroids();
        let assignments = linfa::traits::Predict::predict(&model, &dataset);

        info!("cluster_all: clustered {n} documents into {k} clusters");

        // Group members by cluster
        let mut cluster_members: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, &cluster_id) in assignments.iter().enumerate() {
            cluster_members
                .entry(cluster_id)
                .or_default()
                .push(ids[i].clone());
        }

        // Build ClusterInfo for each cluster (without keywords yet)
        let mut clusters: Vec<ClusterInfo> = Vec::new();
        for cluster_id in 0..k {
            let members = cluster_members.remove(&cluster_id).unwrap_or_default();
            if members.is_empty() {
                continue;
            }

            // Extract centroid as f32
            let centroid: Vec<f32> = centroids
                .row(cluster_id)
                .iter()
                .map(|&v| v as f32)
                .collect();

            clusters.push(ClusterInfo {
                id: cluster_id,
                label: String::new(),
                centroid,
                members,
                keywords: Vec::new(),
            });
        }

        // Compute cross-cluster TF-IDF keywords and labels
        self.assign_cross_cluster_keywords(&mut clusters, documents, 5);

        Ok(ClusterState {
            clusters,
            docs_since_rebalance: 0,
            docs_at_last_rebalance: n,
        })
    }

    /// Assign a single new document to the nearest existing cluster.
    ///
    /// Finds the cluster whose centroid has the highest cosine similarity to the
    /// given vector, adds the document to that cluster, updates the centroid, and
    /// increments `docs_since_rebalance`.
    ///
    /// Returns the cluster ID the document was assigned to.
    pub fn assign_to_nearest(
        &self,
        state: &mut ClusterState,
        doc_path: &str,
        vector: &[f32],
    ) -> crate::Result<usize> {
        if state.clusters.is_empty() {
            return Err(crate::Error::Clustering(
                "no clusters exist for assignment".to_string(),
            ));
        }

        // Find the cluster with highest cosine similarity to the vector
        let mut best_idx = 0;
        let mut best_sim = f32::NEG_INFINITY;
        for (i, cluster) in state.clusters.iter().enumerate() {
            let sim = cosine_similarity(vector, &cluster.centroid);
            if sim > best_sim {
                best_sim = sim;
                best_idx = i;
            }
        }

        let cluster = &mut state.clusters[best_idx];
        let cluster_id = cluster.id;

        // Update centroid incrementally: new_centroid = (old_centroid * n + vector) / (n + 1)
        let n = cluster.members.len() as f32;
        for (i, c) in cluster.centroid.iter_mut().enumerate() {
            *c = (*c * n + vector[i]) / (n + 1.0);
        }

        cluster.members.push(doc_path.to_string());
        state.docs_since_rebalance += 1;

        debug!(
            "assign_to_nearest: assigned {doc_path} to cluster {cluster_id} (similarity={best_sim:.4})"
        );

        Ok(cluster_id)
    }

    /// Rebalance clusters if the number of new documents exceeds the threshold.
    ///
    /// A full re-clustering is triggered when `docs_since_rebalance` exceeds
    /// the configured `clustering_rebalance_threshold`. After rebalancing,
    /// the counter is reset.
    ///
    /// Returns `true` if a rebalance was performed.
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

        let new_state = self.cluster_all(vectors, documents)?;
        *state = new_state;

        Ok(true)
    }

    /// Extract top-N keywords from a set of documents using TF-IDF.
    ///
    /// Tokenizes on non-alphanumeric boundaries, lowercases, filters tokens < 3 chars,
    /// removes stop words, then ranks by TF-IDF score.
    pub fn extract_keywords(
        &self,
        documents: &[&str],
        n: usize,
    ) -> Vec<String> {
        if documents.is_empty() || n == 0 {
            return Vec::new();
        }

        // Tokenize each document into filtered terms
        let tokenized: Vec<Vec<String>> = documents
            .iter()
            .map(|doc| tokenize_and_filter(doc))
            .collect();

        let num_docs = tokenized.len() as f64;

        // Compute TF: total term count across all documents in this cluster
        let mut tf: HashMap<String, f64> = HashMap::new();
        for doc_terms in &tokenized {
            for term in doc_terms {
                *tf.entry(term.clone()).or_insert(0.0) += 1.0;
            }
        }

        // Compute DF: number of documents containing each term
        let mut df: HashMap<String, f64> = HashMap::new();
        for doc_terms in &tokenized {
            let unique: std::collections::HashSet<&String> = doc_terms.iter().collect();
            for term in unique {
                *df.entry(term.clone()).or_insert(0.0) += 1.0;
            }
        }

        // Compute TF-IDF and sort
        let mut scores: Vec<(String, f64)> = tf
            .into_iter()
            .map(|(term, tf_val)| {
                let df_val = df.get(&term).copied().unwrap_or(1.0);
                let idf = (num_docs / df_val).ln();
                (term, tf_val * idf)
            })
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scores.into_iter().take(n).map(|(term, _)| term).collect()
    }

    /// Generate a human-readable label from keywords.
    ///
    /// Takes the top 3 keywords and joins them with " / ".
    pub fn generate_label(&self, keywords: &[String]) -> String {
        let top: Vec<&str> = keywords.iter().take(3).map(|s| s.as_str()).collect();
        if top.is_empty() {
            "Unlabeled".to_string()
        } else {
            top.join(" / ")
        }
    }

    /// Compute cross-cluster TF-IDF keywords and labels for all clusters.
    ///
    /// Uses IDF = log(total_clusters / clusters_containing_term) so that terms
    /// appearing in many clusters are down-weighted, and cluster-distinctive terms
    /// are promoted.
    fn assign_cross_cluster_keywords(
        &self,
        clusters: &mut [ClusterInfo],
        documents: &HashMap<String, String>,
        n: usize,
    ) {
        if clusters.is_empty() {
            return;
        }

        let num_clusters = clusters.len() as f64;

        // Tokenize each cluster's documents and compute per-cluster TF
        let mut cluster_tfs: Vec<HashMap<String, f64>> = Vec::with_capacity(clusters.len());
        let mut cluster_term_sets: Vec<std::collections::HashSet<String>> =
            Vec::with_capacity(clusters.len());

        for cluster in clusters.iter() {
            let mut tf: HashMap<String, f64> = HashMap::new();
            let mut terms_in_cluster: std::collections::HashSet<String> =
                std::collections::HashSet::new();

            for member in &cluster.members {
                if let Some(text) = documents.get(member) {
                    let tokens = tokenize_and_filter(text);
                    for token in &tokens {
                        *tf.entry(token.clone()).or_insert(0.0) += 1.0;
                        terms_in_cluster.insert(token.clone());
                    }
                }
            }

            cluster_tfs.push(tf);
            cluster_term_sets.push(terms_in_cluster);
        }

        // Compute cross-cluster DF: number of clusters containing each term
        let mut cross_df: HashMap<String, f64> = HashMap::new();
        for term_set in &cluster_term_sets {
            for term in term_set {
                *cross_df.entry(term.clone()).or_insert(0.0) += 1.0;
            }
        }

        // For each cluster, compute TF * IDF scores and assign top-N keywords
        for (i, cluster) in clusters.iter_mut().enumerate() {
            let tf = &cluster_tfs[i];

            let mut scores: Vec<(String, f64)> = tf
                .iter()
                .map(|(term, &tf_val)| {
                    let df_val = cross_df.get(term).copied().unwrap_or(1.0);
                    let idf = (num_clusters / df_val).ln();
                    (term.clone(), tf_val * idf)
                })
                .collect();

            scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            cluster.keywords = scores.into_iter().take(n).map(|(term, _)| term).collect();
            cluster.label = self.generate_label(&cluster.keywords);
        }
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
            debug!("cluster_edges: fewer than 4 edges ({}), returning empty state", edge_vectors.len());
            return Ok(EdgeClusterState {
                clusters: Vec::new(),
                edges_since_rebalance: 0,
                edges_at_last_rebalance: edge_vectors.len(),
            });
        }

        // Collect edge IDs in deterministic order
        let mut ids: Vec<String> = edge_vectors.keys().cloned().collect();
        ids.sort();

        // Filter out zero-norm vectors
        let (ids, vecs): (Vec<String>, Vec<&Vec<f32>>) = ids
            .into_iter()
            .filter_map(|id| {
                let v = edge_vectors.get(&id)?;
                let norm: f32 = v.iter().map(|x| x * x).sum();
                if norm == 0.0 {
                    debug!("cluster_edges: skipping zero-norm vector for {id}");
                    None
                } else {
                    Some((id, v))
                }
            })
            .unzip();

        let n = ids.len();
        if n < 4 {
            return Ok(EdgeClusterState {
                clusters: Vec::new(),
                edges_since_rebalance: 0,
                edges_at_last_rebalance: n,
            });
        }

        let dim = vecs[0].len();
        let k = compute_edge_k(n, self.config.clustering_granularity);

        // Build ndarray matrix (f64 for linfa)
        let mut data = Array2::<f64>::zeros((n, dim));
        for (i, v) in vecs.iter().enumerate() {
            for (j, &val) in v.iter().enumerate() {
                data[[i, j]] = val as f64;
            }
        }

        let dataset = DatasetBase::from(data);

        // Run K-means
        let model = linfa_clustering::KMeans::params(k)
            .max_n_iterations(100)
            .tolerance(1e-4)
            .fit(&dataset)
            .map_err(|e| crate::Error::Clustering(format!("Edge K-means failed: {e}")))?;

        let centroids = model.centroids();
        let assignments = linfa::traits::Predict::predict(&model, &dataset);

        info!("cluster_edges: clustered {n} edges into {k} clusters");

        // Group members by cluster
        let mut cluster_members: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, &cluster_id) in assignments.iter().enumerate() {
            cluster_members
                .entry(cluster_id)
                .or_default()
                .push(ids[i].clone());
        }

        // Build EdgeClusterInfo for each cluster
        let mut clusters: Vec<EdgeClusterInfo> = Vec::new();
        for cluster_id in 0..k {
            let members = cluster_members.remove(&cluster_id).unwrap_or_default();
            if members.is_empty() {
                continue;
            }

            let centroid: Vec<f32> = centroids
                .row(cluster_id)
                .iter()
                .map(|&v| v as f32)
                .collect();

            // Extract keywords from edge contexts for this cluster
            let member_texts: Vec<&str> = members
                .iter()
                .filter_map(|m| edge_contexts.get(m).map(|s| s.as_str()))
                .collect();
            let keywords = self.extract_keywords(&member_texts, 5);
            let label = self.generate_label(&keywords);

            clusters.push(EdgeClusterInfo {
                id: cluster_id,
                label,
                centroid,
                members,
                keywords,
            });
        }

        // Apply cross-cluster TF-IDF for better keyword distinctiveness
        self.assign_cross_cluster_edge_keywords(&mut clusters, edge_contexts, 5);

        Ok(EdgeClusterState {
            clusters,
            edges_since_rebalance: 0,
            edges_at_last_rebalance: n,
        })
    }

    /// Assign a single new edge to the nearest existing edge cluster.
    ///
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

        // Update centroid incrementally
        let n = cluster.members.len() as f32;
        for (i, c) in cluster.centroid.iter_mut().enumerate() {
            *c = (*c * n + embedding[i]) / (n + 1.0);
        }

        cluster.members.push(edge_id.to_string());
        state.edges_since_rebalance += 1;

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

    /// Compute cross-cluster TF-IDF keywords for edge clusters.
    fn assign_cross_cluster_edge_keywords(
        &self,
        clusters: &mut [EdgeClusterInfo],
        edge_contexts: &HashMap<String, String>,
        n: usize,
    ) {
        if clusters.is_empty() {
            return;
        }

        let num_clusters = clusters.len() as f64;

        let mut cluster_tfs: Vec<HashMap<String, f64>> = Vec::with_capacity(clusters.len());
        let mut cluster_term_sets: Vec<std::collections::HashSet<String>> =
            Vec::with_capacity(clusters.len());

        for cluster in clusters.iter() {
            let mut tf: HashMap<String, f64> = HashMap::new();
            let mut terms_in_cluster: std::collections::HashSet<String> =
                std::collections::HashSet::new();

            for member in &cluster.members {
                if let Some(text) = edge_contexts.get(member) {
                    let tokens = tokenize_and_filter(text);
                    for token in &tokens {
                        *tf.entry(token.clone()).or_insert(0.0) += 1.0;
                        terms_in_cluster.insert(token.clone());
                    }
                }
            }

            cluster_tfs.push(tf);
            cluster_term_sets.push(terms_in_cluster);
        }

        let mut cross_df: HashMap<String, f64> = HashMap::new();
        for term_set in &cluster_term_sets {
            for term in term_set {
                *cross_df.entry(term.clone()).or_insert(0.0) += 1.0;
            }
        }

        for (i, cluster) in clusters.iter_mut().enumerate() {
            let tf = &cluster_tfs[i];

            let mut scores: Vec<(String, f64)> = tf
                .iter()
                .map(|(term, &tf_val)| {
                    let df_val = cross_df.get(term).copied().unwrap_or(1.0);
                    let idf = (num_clusters / df_val).ln();
                    (term.clone(), tf_val * idf)
                })
                .collect();

            scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            cluster.keywords = scores.into_iter().take(n).map(|(term, _)| term).collect();
            cluster.label = self.generate_label(&cluster.keywords);
        }
    }

    /// Assign all documents to their nearest custom cluster centroid.
    ///
    /// `defs` provides cluster names and seed phrases. `centroids` are the
    /// pre-computed centroid vectors (one per def, in the same order).
    /// `doc_vectors` maps document path to its embedding vector.
    pub fn assign_all_to_custom(
        &self,
        defs: &[CustomClusterDef],
        centroids: &[Vec<f32>],
        doc_vectors: &HashMap<String, Vec<f32>>,
    ) -> CustomClusterState {
        let mut clusters: Vec<CustomClusterInfo> = defs
            .iter()
            .enumerate()
            .map(|(i, def)| CustomClusterInfo {
                id: i,
                name: def.name.clone(),
                seed_phrases: def.seeds.clone(),
                centroid: centroids[i].clone(),
                members: Vec::new(),
            })
            .collect();

        // Assign each document to the nearest centroid.
        for (path, vec) in doc_vectors {
            let best_idx = find_nearest_centroid(vec, centroids);
            clusters[best_idx].members.push(path.clone());
        }

        // Sort members within each cluster for determinism.
        for cluster in &mut clusters {
            cluster.members.sort();
        }

        info!(
            "assign_all_to_custom: assigned {} documents to {} custom clusters",
            doc_vectors.len(),
            clusters.len()
        );

        CustomClusterState { clusters }
    }

    /// Assign a single document to the nearest existing custom cluster.
    ///
    /// Removes the document from any current membership first, then assigns
    /// to the nearest centroid. The centroid is NOT updated (anchored to seeds).
    pub fn assign_single_to_custom(
        &self,
        state: &mut CustomClusterState,
        doc_path: &str,
        vector: &[f32],
    ) {
        // Remove from any existing cluster.
        for cluster in &mut state.clusters {
            cluster.members.retain(|m| m != doc_path);
        }

        // Find nearest centroid.
        let centroids: Vec<&Vec<f32>> = state.clusters.iter().map(|c| &c.centroid).collect();
        if centroids.is_empty() {
            return;
        }

        let centroid_vecs: Vec<Vec<f32>> = centroids.iter().map(|c| (*c).clone()).collect();
        let best_idx = find_nearest_centroid(vector, &centroid_vecs);
        state.clusters[best_idx].members.push(doc_path.to_string());

        debug!(
            "assign_single_to_custom: assigned {doc_path} to custom cluster '{}'",
            state.clusters[best_idx].name
        );
    }
}

/// Embed seed phrases and compute normalized centroid vectors for custom clusters.
///
/// For each cluster definition, embeds all seed phrases and averages the resulting
/// vectors, then normalizes to unit length. Returns one centroid per definition.
pub async fn embed_seed_centroids(
    defs: &[CustomClusterDef],
    provider: &dyn crate::embedding::provider::EmbeddingProvider,
) -> crate::Result<Vec<Vec<f32>>> {
    let mut centroids = Vec::with_capacity(defs.len());

    for def in defs {
        let seeds: Vec<String> = def.seeds.clone();
        let embeddings = provider.embed_batch(&seeds).await?;

        if embeddings.is_empty() {
            return Err(crate::Error::Clustering(format!(
                "no embeddings returned for custom cluster '{}' seeds",
                def.name
            )));
        }

        let dims = embeddings[0].len();
        let mut centroid = vec![0.0f32; dims];
        let n = embeddings.len() as f32;

        for emb in &embeddings {
            for (i, v) in emb.iter().enumerate() {
                centroid[i] += v;
            }
        }
        for v in &mut centroid {
            *v /= n;
        }

        // Normalize to unit vector.
        let norm: f32 = centroid.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut centroid {
                *v /= norm;
            }
        }

        centroids.push(centroid);
    }

    Ok(centroids)
}

/// Find the index of the nearest centroid to a given vector by cosine similarity.
fn find_nearest_centroid(vector: &[f32], centroids: &[Vec<f32>]) -> usize {
    let mut best_idx = 0;
    let mut best_sim = f32::NEG_INFINITY;
    for (i, centroid) in centroids.iter().enumerate() {
        let sim = cosine_similarity(vector, centroid);
        if sim > best_sim {
            best_sim = sim;
            best_idx = i;
        }
    }
    best_idx
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

/// Compute the optimal number of clusters (k) for a given document count.
///
/// Uses the heuristic: `clamp(sqrt(n * granularity / 2), 2, 50)`.
/// `granularity` is a multiplier (default 1.0): higher = more clusters.
pub(crate) fn compute_k(n: usize, granularity: f64) -> usize {
    let k = (n as f64 * granularity / 2.0).sqrt() as usize;
    k.clamp(2, 50)
}

/// Compute the optimal number of clusters for edges, clamped to [2, 20].
/// `granularity` is a multiplier (default 1.0): higher = more clusters.
pub(crate) fn compute_edge_k(n: usize, granularity: f64) -> usize {
    let k = (n as f64 * granularity / 2.0).sqrt() as usize;
    k.clamp(2, 20)
}

/// Check if a word is a stop word.
pub(crate) fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(&word)
}

/// Tokenize text into filtered terms: split on non-alphanumeric, lowercase,
/// remove tokens < 3 chars, and remove stop words.
fn tokenize_and_filter(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 3 && !is_stop_word(w))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn compute_k_small() {
        // n=4 -> sqrt(4*1/2) = sqrt(2) ≈ 1.4 -> clamped to 2
        assert_eq!(compute_k(4, 1.0), 2);
    }

    #[test]
    fn compute_k_medium() {
        // n=200 -> sqrt(200*1/2) = sqrt(100) = 10
        assert_eq!(compute_k(200, 1.0), 10);
    }

    #[test]
    fn compute_k_large() {
        // n=10000 -> sqrt(5000) ≈ 70 -> clamped to 50
        assert_eq!(compute_k(10000, 1.0), 50);
    }

    #[test]
    fn compute_k_minimum() {
        assert_eq!(compute_k(0, 1.0), 2);
        assert_eq!(compute_k(1, 1.0), 2);
    }

    #[test]
    fn compute_k_high_granularity() {
        // n=200, g=4.0 -> sqrt(200*4/2) = sqrt(400) = 20
        assert_eq!(compute_k(200, 4.0), 20);
    }

    #[test]
    fn compute_k_low_granularity() {
        // n=200, g=0.25 -> sqrt(200*0.25/2) = sqrt(25) = 5
        assert_eq!(compute_k(200, 0.25), 5);
    }

    #[test]
    fn stop_words_contains_common_words() {
        assert!(is_stop_word("the"));
        assert!(is_stop_word("and"));
        assert!(is_stop_word("is"));
        assert!(!is_stop_word("clustering"));
        assert!(!is_stop_word("vector"));
    }

    #[test]
    fn cluster_info_serializes_to_json() {
        let info = ClusterInfo {
            id: 0,
            label: "Test Cluster".to_string(),
            centroid: vec![0.1, 0.2, 0.3],
            members: vec!["doc1.md#0".to_string()],
            keywords: vec!["test".to_string()],
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("Test Cluster"));
    }

    fn test_config() -> Config {
        let mut config = Config::load(std::path::Path::new("/nonexistent")).unwrap();
        config.clustering_enabled = true;
        config.clustering_rebalance_threshold = 50;
        config
    }

    #[test]
    fn extract_keywords_no_stopwords() {
        let clusterer = Clusterer::new(&test_config());
        let docs = vec![
            "The quick brown fox jumps over the lazy dog",
            "A brown fox is quick and nimble",
            "Foxes are brown animals that jump quickly",
        ];
        let keywords = clusterer.extract_keywords(&docs, 5);
        for kw in &keywords {
            assert!(!is_stop_word(kw), "keyword '{kw}' is a stop word");
        }
        assert!(!keywords.is_empty());
    }

    #[test]
    fn extract_keywords_empty_docs() {
        let clusterer = Clusterer::new(&test_config());
        let keywords = clusterer.extract_keywords(&[], 5);
        assert!(keywords.is_empty());
    }

    #[test]
    fn extract_keywords_respects_n() {
        let clusterer = Clusterer::new(&test_config());
        let docs = vec!["rust programming language systems performance memory safety"];
        let keywords = clusterer.extract_keywords(&docs, 3);
        assert!(keywords.len() <= 3);
    }

    #[test]
    fn generate_label_format() {
        let clusterer = Clusterer::new(&test_config());
        let keywords = vec![
            "rust".to_string(),
            "programming".to_string(),
            "systems".to_string(),
            "extra".to_string(),
        ];
        let label = clusterer.generate_label(&keywords);
        assert_eq!(label, "rust / programming / systems");
    }

    #[test]
    fn generate_label_fewer_than_three() {
        let clusterer = Clusterer::new(&test_config());
        let keywords = vec!["rust".to_string()];
        let label = clusterer.generate_label(&keywords);
        assert_eq!(label, "rust");
    }

    #[test]
    fn generate_label_empty() {
        let clusterer = Clusterer::new(&test_config());
        let label = clusterer.generate_label(&[]);
        assert_eq!(label, "Unlabeled");
    }

    #[test]
    fn tokenize_filters_short_and_stopwords() {
        let tokens = tokenize_and_filter("The big cat is on a mat");
        assert!(!tokens.contains(&"the".to_string()));
        assert!(!tokens.contains(&"is".to_string()));
        assert!(!tokens.contains(&"on".to_string()));
        assert!(tokens.contains(&"big".to_string()));
        assert!(tokens.contains(&"cat".to_string()));
        assert!(tokens.contains(&"mat".to_string()));
    }

    #[test]
    fn assign_to_nearest_picks_closest_cluster() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = ClusterState {
            clusters: vec![
                ClusterInfo {
                    id: 0,
                    label: "A".to_string(),
                    centroid: vec![1.0, 0.0, 0.0],
                    members: vec!["a#0".to_string()],
                    keywords: vec![],
                },
                ClusterInfo {
                    id: 1,
                    label: "B".to_string(),
                    centroid: vec![0.0, 1.0, 0.0],
                    members: vec!["b#0".to_string()],
                    keywords: vec![],
                },
            ],
            docs_since_rebalance: 0,
            docs_at_last_rebalance: 2,
        };

        // Vector close to cluster 0
        let assigned = clusterer
            .assign_to_nearest(&mut state, "new#0", &[0.9, 0.1, 0.0])
            .unwrap();
        assert_eq!(assigned, 0);
        assert!(state.clusters[0].members.contains(&"new#0".to_string()));
        assert_eq!(state.docs_since_rebalance, 1);
    }

    #[test]
    fn assign_to_nearest_updates_centroid() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = ClusterState {
            clusters: vec![ClusterInfo {
                id: 0,
                label: "A".to_string(),
                centroid: vec![1.0, 0.0, 0.0],
                members: vec!["a#0".to_string()],
                keywords: vec![],
            }],
            docs_since_rebalance: 0,
            docs_at_last_rebalance: 1,
        };

        clusterer
            .assign_to_nearest(&mut state, "b#0", &[0.0, 1.0, 0.0])
            .unwrap();

        // Centroid should be average: (1+0)/2, (0+1)/2, 0
        let c = &state.clusters[0].centroid;
        assert!((c[0] - 0.5).abs() < 1e-6);
        assert!((c[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn assign_to_nearest_empty_clusters_errors() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = ClusterState {
            clusters: vec![],
            docs_since_rebalance: 0,
            docs_at_last_rebalance: 0,
        };
        let result = clusterer.assign_to_nearest(&mut state, "x#0", &[1.0, 0.0]);
        assert!(result.is_err());
    }

    #[test]
    fn maybe_rebalance_below_threshold() {
        let clusterer = Clusterer::new(&test_config());
        let mut state = ClusterState {
            clusters: vec![],
            docs_since_rebalance: 5,
            docs_at_last_rebalance: 10,
        };
        let vectors = HashMap::new();
        let documents = HashMap::new();
        let rebalanced = clusterer.maybe_rebalance(&mut state, &vectors, &documents).unwrap();
        assert!(!rebalanced);
    }

    #[test]
    fn maybe_rebalance_above_threshold() {
        let mut config = test_config();
        config.clustering_rebalance_threshold = 2;
        let clusterer = Clusterer::new(&config);

        let mut vectors = HashMap::new();
        vectors.insert("a#0".to_string(), vec![1.0, 0.0, 0.0]);
        vectors.insert("b#0".to_string(), vec![0.0, 1.0, 0.0]);
        vectors.insert("c#0".to_string(), vec![0.0, 0.0, 1.0]);

        let mut documents = HashMap::new();
        documents.insert("a#0".to_string(), "alpha docs".to_string());
        documents.insert("b#0".to_string(), "beta docs".to_string());
        documents.insert("c#0".to_string(), "gamma docs".to_string());

        let mut state = ClusterState {
            clusters: vec![],
            docs_since_rebalance: 3,
            docs_at_last_rebalance: 0,
        };

        let rebalanced = clusterer.maybe_rebalance(&mut state, &vectors, &documents).unwrap();
        assert!(rebalanced);
        assert!(!state.clusters.is_empty());
        assert_eq!(state.docs_since_rebalance, 0);
    }

    #[test]
    fn cluster_all_empty_vectors() {
        let clusterer = Clusterer::new(&test_config());
        let vectors = HashMap::new();
        let documents = HashMap::new();
        let state = clusterer.cluster_all(&vectors, &documents).unwrap();
        assert!(state.clusters.is_empty());
        assert_eq!(state.docs_since_rebalance, 0);
    }

    #[test]
    fn cluster_all_single_vector() {
        let clusterer = Clusterer::new(&test_config());
        let mut vectors = HashMap::new();
        vectors.insert("doc#0".to_string(), vec![1.0, 0.0, 0.0]);
        let mut documents = HashMap::new();
        documents.insert("doc#0".to_string(), "rust programming language".to_string());
        let state = clusterer.cluster_all(&vectors, &documents).unwrap();
        assert_eq!(state.clusters.len(), 1);
        assert_eq!(state.clusters[0].members, vec!["doc#0".to_string()]);
        assert_eq!(state.docs_at_last_rebalance, 1);
    }

    #[test]
    fn cluster_all_skips_zero_norm_vectors() {
        let clusterer = Clusterer::new(&test_config());
        let mut vectors = HashMap::new();
        vectors.insert("good#0".to_string(), vec![1.0, 0.0, 0.0]);
        vectors.insert("zero#0".to_string(), vec![0.0, 0.0, 0.0]);
        let documents = HashMap::new();
        let state = clusterer.cluster_all(&vectors, &documents).unwrap();
        assert_eq!(state.clusters.len(), 1);
        assert!(state.clusters[0].members.contains(&"good#0".to_string()));
        assert!(!state.clusters[0].members.contains(&"zero#0".to_string()));
    }

    #[test]
    fn cluster_all_multiple_vectors() {
        let clusterer = Clusterer::new(&test_config());
        let mut vectors = HashMap::new();
        let mut documents = HashMap::new();
        for i in 0..10 {
            let mut v = vec![0.0f32; 8];
            v[i % 8] = 1.0;
            vectors.insert(format!("doc#{i}"), v);
            documents.insert(format!("doc#{i}"), format!("document number {i}"));
        }
        let state = clusterer.cluster_all(&vectors, &documents).unwrap();
        assert!(!state.clusters.is_empty());
        // All docs should be assigned
        let total_members: usize = state.clusters.iter().map(|c| c.members.len()).sum();
        assert_eq!(total_members, 10);
        // Each cluster should have a non-empty label
        for c in &state.clusters {
            assert!(!c.label.is_empty());
            assert!(!c.centroid.is_empty());
        }
    }

    #[test]
    fn cluster_all_only_zero_vectors() {
        let clusterer = Clusterer::new(&test_config());
        let mut vectors = HashMap::new();
        vectors.insert("z1#0".to_string(), vec![0.0, 0.0, 0.0]);
        vectors.insert("z2#0".to_string(), vec![0.0, 0.0, 0.0]);
        let state = clusterer.cluster_all(&vectors, &HashMap::new()).unwrap();
        assert!(state.clusters.is_empty());
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

    // --- Edge clustering tests ---

    #[test]
    fn compute_edge_k_clamped_to_20() {
        // n=10000 -> sqrt(5000) ≈ 70 -> clamped to 20
        assert_eq!(compute_edge_k(10000, 1.0), 20);
        // n=4 -> sqrt(2) ≈ 1 -> clamped to 2
        assert_eq!(compute_edge_k(4, 1.0), 2);
        // n=200 -> sqrt(100) = 10
        assert_eq!(compute_edge_k(200, 1.0), 10);
    }

    #[test]
    fn compute_edge_k_with_granularity() {
        // n=200, g=4.0 -> sqrt(400) = 20 (hits cap)
        assert_eq!(compute_edge_k(200, 4.0), 20);
        // n=200, g=0.25 -> sqrt(25) = 5
        assert_eq!(compute_edge_k(200, 0.25), 5);
    }

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
        let state = clusterer.cluster_edges(&HashMap::new(), &HashMap::new()).unwrap();
        assert!(state.clusters.is_empty());
    }

    #[test]
    fn cluster_edges_basic() {
        let clusterer = Clusterer::new(&test_config());
        let mut vectors = HashMap::new();
        let mut contexts = HashMap::new();

        // Create two groups of edges with distinct vectors
        for i in 0..5 {
            vectors.insert(format!("edge:a{i}"), vec![1.0, 0.1 * i as f32, 0.0, 0.0]);
            contexts.insert(format!("edge:a{i}"), format!("rust programming language systems {i}"));
        }
        for i in 0..5 {
            vectors.insert(format!("edge:b{i}"), vec![0.0, 0.0, 1.0, 0.1 * i as f32]);
            contexts.insert(format!("edge:b{i}"), format!("cooking recipe food kitchen {i}"));
        }

        let state = clusterer.cluster_edges(&vectors, &contexts).unwrap();
        assert!(!state.clusters.is_empty());

        // All edges should be assigned
        let total: usize = state.clusters.iter().map(|c| c.members.len()).sum();
        assert_eq!(total, 10);

        // Each cluster should have labels
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
            contexts.insert(format!("edge:{i}"), format!("documentation reference guide manual {i}"));
        }

        let state = clusterer.cluster_edges(&vectors, &contexts).unwrap();
        for c in &state.clusters {
            // Labels should be keyword-based, not empty
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

        let c = &state.clusters[0].centroid;
        assert!((c[0] - 0.5).abs() < 1e-6);
        assert!((c[1] - 0.5).abs() < 1e-6);
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

    #[test]
    fn cluster_state_serializes_to_json() {
        let state = ClusterState {
            clusters: vec![],
            docs_since_rebalance: 5,
            docs_at_last_rebalance: 100,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("docs_since_rebalance"));
    }
}

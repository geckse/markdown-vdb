use std::collections::HashMap;

use serde::Serialize;
use tracing::debug;

use crate::config::Config;

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
    /// Chunk IDs belonging to this cluster.
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
    /// `vectors` maps chunk ID to its embedding vector.
    /// `documents` maps chunk ID to its text content (for keyword extraction).
    pub fn cluster_all(
        &self,
        _vectors: &HashMap<String, Vec<f32>>,
        _documents: &HashMap<String, String>,
    ) -> crate::Result<ClusterState> {
        todo!("cluster_all: full K-means implementation in Phase 2")
    }

    /// Assign a single new document to the nearest existing cluster.
    pub fn assign_to_nearest(
        &self,
        _state: &mut ClusterState,
        _chunk_id: &str,
        _vector: &[f32],
    ) -> crate::Result<usize> {
        todo!("assign_to_nearest: incremental assignment in Phase 2")
    }

    /// Rebalance clusters if the number of new documents exceeds the threshold.
    ///
    /// Returns `true` if a rebalance was performed.
    pub fn maybe_rebalance(
        &self,
        _state: &mut ClusterState,
        _vectors: &HashMap<String, Vec<f32>>,
        _documents: &HashMap<String, String>,
    ) -> crate::Result<bool> {
        todo!("maybe_rebalance: threshold-based rebalancing in Phase 2")
    }

    /// Extract top-N keywords from a set of documents using TF-IDF.
    pub fn extract_keywords(
        &self,
        _documents: &[&str],
        _n: usize,
    ) -> Vec<String> {
        todo!("extract_keywords: TF-IDF extraction in Phase 2")
    }

    /// Generate a human-readable label from keywords.
    pub fn generate_label(&self, _keywords: &[String]) -> String {
        todo!("generate_label: label generation in Phase 2")
    }

    /// Returns the configured rebalance threshold.
    pub fn rebalance_threshold(&self) -> usize {
        self.config.clustering_rebalance_threshold
    }

    /// Returns whether clustering is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.clustering_enabled
    }
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
/// Uses the heuristic: `clamp(sqrt(n / 2), 2, 50)`.
pub(crate) fn compute_k(n: usize) -> usize {
    let k = (n as f64 / 2.0).sqrt() as usize;
    k.max(2).min(50)
}

/// Check if a word is a stop word.
pub(crate) fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(&word)
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
        // n=4 -> sqrt(2) ≈ 1.4 -> clamped to 2
        assert_eq!(compute_k(4), 2);
    }

    #[test]
    fn compute_k_medium() {
        // n=200 -> sqrt(100) = 10
        assert_eq!(compute_k(200), 10);
    }

    #[test]
    fn compute_k_large() {
        // n=10000 -> sqrt(5000) ≈ 70 -> clamped to 50
        assert_eq!(compute_k(10000), 50);
    }

    #[test]
    fn compute_k_minimum() {
        assert_eq!(compute_k(0), 2);
        assert_eq!(compute_k(1), 2);
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

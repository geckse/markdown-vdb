//! Leiden community detection over a cosine k-NN graph — the default
//! document-clustering algorithm.
//!
//! Pipeline: unit-normalized document vectors → exact k-NN graph (cosine
//! weights) → seeded Leiden partition → small-community merge → optional
//! one-level hierarchy via re-clustering the aggregated community graph.
//!
//! Everything here is deterministic: sorted node order, explicit tie-breaks,
//! and a fixed RNG seed.

use std::collections::BTreeMap;

use leiden_rs::{GraphDataBuilder, Leiden, LeidenConfig, QualityType};
use tracing::{debug, warn};

use super::kmeans::CLUSTER_SEED;

/// Above this document count the exact O(n²) k-NN build gets slow; log a
/// warning (an ANN-backed build is the designated future optimization).
const KNN_BRUTE_FORCE_WARN_THRESHOLD: usize = 20_000;

/// An undirected, weighted k-NN similarity graph over documents.
pub(crate) struct KnnGraph {
    /// Sorted document paths; the index in this vec is the graph node id.
    pub node_paths: Vec<String>,
    /// Undirected deduped edges `(u, v, weight)` with `u < v`; weight = cosine similarity.
    pub edges: Vec<(u32, u32, f64)>,
}

impl KnnGraph {
    /// Total edge weight between each pair of the given node communities.
    /// Returns a map `(community_a, community_b) -> summed weight` with a < b.
    fn community_edge_weights(&self, membership: &[usize]) -> BTreeMap<(usize, usize), f64> {
        let mut weights: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        for &(u, v, w) in &self.edges {
            let (cu, cv) = (membership[u as usize], membership[v as usize]);
            if cu == cv {
                continue;
            }
            let key = (cu.min(cv), cu.max(cv));
            *weights.entry(key).or_insert(0.0) += w;
        }
        weights
    }
}

/// Build an exact cosine k-NN graph over unit-normalized vectors.
///
/// `vectors` must be unit-normalized (dot product == cosine similarity) and is
/// keyed deterministically (BTreeMap). Rules: k is clamped to `n - 1`; each
/// node links to its top-k neighbors by `(similarity desc, path asc)`;
/// non-positive similarities are dropped; the union of directed picks forms
/// the undirected edge set.
pub(crate) fn build_knn_graph(vectors: &BTreeMap<String, Vec<f32>>, k: usize) -> KnnGraph {
    let node_paths: Vec<String> = vectors.keys().cloned().collect();
    let vecs: Vec<&Vec<f32>> = vectors.values().collect();
    let n = node_paths.len();

    if n > KNN_BRUTE_FORCE_WARN_THRESHOLD {
        warn!(
            "build_knn_graph: {n} documents — exact k-NN build is O(n²) and may take a while"
        );
    }

    let k = k.min(n.saturating_sub(1));
    let mut edge_map: BTreeMap<(u32, u32), f64> = BTreeMap::new();

    for i in 0..n {
        let mut sims: Vec<(f32, usize)> = (0..n)
            .filter(|&j| j != i)
            .map(|j| (dot(vecs[i], vecs[j]), j))
            .collect();
        sims.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| node_paths[a.1].cmp(&node_paths[b.1]))
        });
        for &(sim, j) in sims.iter().take(k) {
            if sim <= 0.0 {
                break; // sorted desc — nothing positive follows
            }
            let (u, v) = if i < j {
                (i as u32, j as u32)
            } else {
                (j as u32, i as u32)
            };
            edge_map.entry((u, v)).or_insert(sim as f64);
        }
    }

    KnnGraph {
        node_paths,
        edges: edge_map
            .into_iter()
            .map(|((u, v), w)| (u, v, w))
            .collect(),
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Run seeded Leiden community detection over an edge list.
///
/// Returns one community label per node, relabeled contiguously `0..m` in
/// deterministic order: communities sorted by `(size desc, smallest member asc)`.
pub(crate) fn leiden_partition(
    node_count: usize,
    edges: &[(u32, u32, f64)],
    resolution: f64,
    seed: u64,
) -> crate::Result<Vec<usize>> {
    if node_count == 0 {
        return Ok(Vec::new());
    }

    let mut builder = GraphDataBuilder::new(node_count);
    for &(u, v, w) in edges {
        builder
            .add_edge(u as usize, v as usize, w)
            .map_err(|e| crate::Error::Clustering(format!("leiden graph build failed: {e}")))?;
    }
    let graph = builder
        .build()
        .map_err(|e| crate::Error::Clustering(format!("leiden graph build failed: {e}")))?;

    let config = LeidenConfig::builder()
        .seed(seed)
        .resolution(resolution)
        .quality(QualityType::Modularity)
        .max_iterations(100)
        .build();
    config
        .validate()
        .map_err(|e| crate::Error::Clustering(format!("leiden config invalid: {e}")))?;

    let output = Leiden::new(config)
        .run(&graph)
        .map_err(|e| crate::Error::Clustering(format!("leiden failed: {e}")))?;

    Ok(relabel_contiguous(output.partition.as_slice()))
}

/// Relabel arbitrary community ids contiguously by `(size desc, smallest member asc)`.
fn relabel_contiguous(membership: &[usize]) -> Vec<usize> {
    let mut first_member: BTreeMap<usize, usize> = BTreeMap::new();
    let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
    for (node, &community) in membership.iter().enumerate() {
        first_member.entry(community).or_insert(node);
        *sizes.entry(community).or_insert(0) += 1;
    }

    let mut communities: Vec<usize> = sizes.keys().copied().collect();
    communities.sort_by(|a, b| {
        sizes[b]
            .cmp(&sizes[a])
            .then_with(|| first_member[a].cmp(&first_member[b]))
    });

    let mapping: BTreeMap<usize, usize> = communities
        .into_iter()
        .enumerate()
        .map(|(new_id, old_id)| (old_id, new_id))
        .collect();

    membership.iter().map(|c| mapping[c]).collect()
}

/// Merge communities smaller than `min_size` into their strongest-connected
/// neighbor community (by total k-NN edge weight). Communities with no
/// positive connection to any other community are left as-is (the caller's
/// nearest-centroid fallback covers isolated nodes). Deterministic: smallest
/// community first, ties broken by lowest label; target ties by lowest label.
///
/// Returns the membership relabeled contiguously.
pub(crate) fn merge_small_communities(
    membership: Vec<usize>,
    graph: &KnnGraph,
    min_size: usize,
) -> Vec<usize> {
    if min_size <= 1 || membership.is_empty() {
        return relabel_contiguous(&membership);
    }

    let mut membership = membership;
    loop {
        let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
        for &c in &membership {
            *sizes.entry(c).or_insert(0) += 1;
        }
        if sizes.len() <= 1 {
            break;
        }

        // Smallest under-sized community, ties by lowest label.
        let candidate = sizes
            .iter()
            .filter(|(_, &size)| size < min_size)
            .min_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)))
            .map(|(&c, _)| c);
        let Some(small) = candidate else { break };

        // Strongest-connected other community.
        let weights = graph.community_edge_weights(&membership);
        let mut best: Option<(f64, usize)> = None;
        for ((a, b), w) in &weights {
            let other = if *a == small {
                Some(*b)
            } else if *b == small {
                Some(*a)
            } else {
                None
            };
            if let Some(other) = other {
                let better = match best {
                    None => true,
                    Some((bw, bc)) => *w > bw || (*w == bw && other < bc),
                };
                if better {
                    best = Some((*w, other));
                }
            }
        }

        let Some((_, target)) = best else {
            // No connections to any other community — leave it (caller handles
            // isolated nodes via nearest-centroid fallback if desired).
            break;
        };

        debug!("merge_small_communities: merging community {small} into {target}");
        for c in &mut membership {
            if *c == small {
                *c = target;
            }
        }
    }

    relabel_contiguous(&membership)
}

/// Build the aggregated community graph (nodes = communities, edge weight =
/// summed inter-community k-NN weight) and partition it with Leiden at a
/// coarser resolution. Returns one parent label per community, contiguous.
pub(crate) fn aggregate_partition(
    graph: &KnnGraph,
    membership: &[usize],
    community_count: usize,
    resolution: f64,
) -> crate::Result<Vec<usize>> {
    let weights = graph.community_edge_weights(membership);
    let edges: Vec<(u32, u32, f64)> = weights
        .into_iter()
        .map(|((a, b), w)| (a as u32, b as u32, w))
        .collect();
    leiden_partition(community_count, &edges, resolution, CLUSTER_SEED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: Vec<f32>) -> Vec<f32> {
        super::super::normalize_in_place(v)
    }

    fn two_group_vectors() -> BTreeMap<String, Vec<f32>> {
        let mut m = BTreeMap::new();
        // Group A around x-axis, group B around y-axis.
        m.insert("a1.md".to_string(), unit(vec![1.0, 0.05, 0.0]));
        m.insert("a2.md".to_string(), unit(vec![0.95, 0.1, 0.0]));
        m.insert("a3.md".to_string(), unit(vec![0.9, 0.0, 0.05]));
        m.insert("b1.md".to_string(), unit(vec![0.0, 1.0, 0.05]));
        m.insert("b2.md".to_string(), unit(vec![0.05, 0.95, 0.0]));
        m.insert("b3.md".to_string(), unit(vec![0.0, 0.9, 0.1]));
        m
    }

    #[test]
    fn knn_graph_deterministic_and_symmetric() {
        let vectors = two_group_vectors();
        let g1 = build_knn_graph(&vectors, 2);
        let g2 = build_knn_graph(&vectors, 2);
        assert_eq!(g1.node_paths, g2.node_paths);
        assert_eq!(g1.edges.len(), g2.edges.len());
        for (e1, e2) in g1.edges.iter().zip(g2.edges.iter()) {
            assert_eq!(e1.0, e2.0);
            assert_eq!(e1.1, e2.1);
        }
        // Edges deduped with u < v.
        for &(u, v, _) in &g1.edges {
            assert!(u < v);
        }
    }

    #[test]
    fn knn_graph_clamps_k() {
        let mut vectors = BTreeMap::new();
        vectors.insert("a.md".to_string(), unit(vec![1.0, 0.0]));
        vectors.insert("b.md".to_string(), unit(vec![0.9, 0.1]));
        // k = 10 with n = 2 must clamp to 1.
        let g = build_knn_graph(&vectors, 10);
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn knn_graph_drops_non_positive_similarity() {
        let mut vectors = BTreeMap::new();
        vectors.insert("a.md".to_string(), unit(vec![1.0, 0.0]));
        vectors.insert("b.md".to_string(), unit(vec![-1.0, 0.0]));
        let g = build_knn_graph(&vectors, 1);
        assert!(g.edges.is_empty());
    }

    #[test]
    fn leiden_partition_separates_groups_and_is_deterministic() {
        let vectors = two_group_vectors();
        let graph = build_knn_graph(&vectors, 2);
        let m1 = leiden_partition(graph.node_paths.len(), &graph.edges, 1.0, CLUSTER_SEED).unwrap();
        let m2 = leiden_partition(graph.node_paths.len(), &graph.edges, 1.0, CLUSTER_SEED).unwrap();
        assert_eq!(m1, m2);

        // node_paths sorted: a1, a2, a3, b1, b2, b3
        assert_eq!(m1[0], m1[1]);
        assert_eq!(m1[1], m1[2]);
        assert_eq!(m1[3], m1[4]);
        assert_eq!(m1[4], m1[5]);
        assert_ne!(m1[0], m1[3]);
        // Contiguous labels starting at 0.
        let max = *m1.iter().max().unwrap();
        assert_eq!(max, 1);
    }

    #[test]
    fn relabel_orders_by_size_then_first_member() {
        // Community 7 has 3 members (first member 0), community 2 has 1 member.
        let membership = vec![7, 7, 2, 7];
        let relabeled = relabel_contiguous(&membership);
        assert_eq!(relabeled, vec![0, 0, 1, 0]);
    }

    #[test]
    fn merge_small_communities_folds_singletons() {
        let vectors = two_group_vectors();
        let graph = build_knn_graph(&vectors, 2);
        // Force a bad partition: node 0 alone, rest by group.
        let membership = vec![2, 0, 0, 1, 1, 1];
        let merged = merge_small_communities(membership, &graph, 2);
        // Node 0 (a1) must join the community of a2/a3.
        assert_eq!(merged[0], merged[1]);
        assert_eq!(merged[1], merged[2]);
        // Groups remain distinct.
        assert_ne!(merged[0], merged[3]);
    }

    #[test]
    fn merge_disabled_when_min_size_one() {
        let vectors = two_group_vectors();
        let graph = build_knn_graph(&vectors, 2);
        let membership = vec![2, 0, 0, 1, 1, 1];
        let merged = merge_small_communities(membership, &graph, 1);
        // Only relabeled, not merged: still 3 communities.
        let distinct: std::collections::HashSet<usize> = merged.iter().copied().collect();
        assert_eq!(distinct.len(), 3);
    }
}

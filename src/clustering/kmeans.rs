//! Seeded K-means helpers — the fallback document-clustering algorithm and the
//! backend for edge clustering.

use linfa::traits::Fit;
use linfa::DatasetBase;
use ndarray::Array2;
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256Plus;

/// Fixed RNG seed for all clustering runs — keeps output deterministic.
pub(crate) const CLUSTER_SEED: u64 = 42;

/// Run seeded K-means over an `n × dim` matrix.
///
/// Returns `(centroids, assignments)` where `centroids` is `k × dim` and
/// `assignments[i]` is the cluster index of row i. Deterministic for a given
/// input (explicitly seeded k-means++ init, sorted caller-side row order).
pub(crate) fn run_kmeans(
    data: Array2<f64>,
    k: usize,
    context: &str,
) -> crate::Result<(Array2<f64>, Vec<usize>)> {
    let dataset = DatasetBase::from(data);

    let model =
        linfa_clustering::KMeans::params_with_rng(k, Xoshiro256Plus::seed_from_u64(CLUSTER_SEED))
            .max_n_iterations(100)
            .tolerance(1e-4)
            .fit(&dataset)
            .map_err(|e| crate::Error::Clustering(format!("{context} K-means failed: {e}")))?;

    let centroids = model.centroids().clone();
    let assignments = linfa::traits::Predict::predict(&model, &dataset);

    Ok((centroids, assignments.iter().copied().collect()))
}

/// Compute the number of clusters (k) for a given document count.
///
/// Uses the heuristic: `clamp(sqrt(n * granularity / 2), 2, 50)`.
/// `granularity` is a multiplier (default 1.0): higher = more clusters.
pub(crate) fn compute_k(n: usize, granularity: f64) -> usize {
    let k = (n as f64 * granularity / 2.0).sqrt() as usize;
    k.clamp(2, 50)
}

/// Compute the number of clusters for edges, clamped to [2, 20].
/// `granularity` is a multiplier (default 1.0): higher = more clusters.
pub(crate) fn compute_edge_k(n: usize, granularity: f64) -> usize {
    let k = (n as f64 * granularity / 2.0).sqrt() as usize;
    k.clamp(2, 20)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn run_kmeans_deterministic() {
        let mut data = Array2::<f64>::zeros((6, 3));
        for i in 0..3 {
            data[[i, 0]] = 1.0 + 0.01 * i as f64;
        }
        for i in 3..6 {
            data[[i, 1]] = 1.0 + 0.01 * i as f64;
        }
        let (c1, a1) = run_kmeans(data.clone(), 2, "test").unwrap();
        let (c2, a2) = run_kmeans(data, 2, "test").unwrap();
        assert_eq!(a1, a2);
        assert_eq!(c1, c2);
    }
}

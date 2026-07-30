//! Derived, disposable graph-analysis state for Shards.
//!
//! The cache intentionally lives outside the index wire format. It contains
//! only derived cluster/topic state and centroids; document vectors and graph
//! topology continue to have one collection-wide source of truth.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::clustering::{ClusterState, CustomClusterDef, CustomClusterState};
use crate::config::Config;
use crate::error::Error;
use crate::path_util;
use crate::Result;

pub(crate) const SHARD_ANALYSIS_CACHE_FORMAT: &str = "mdvdb.shard-analysis";
pub(crate) const SHARD_ANALYSIS_CACHE_VERSION: u32 = 1;
/// Internal Shard clustering recipe revision.
///
/// This deliberately lives in the derived-state fingerprint instead of the
/// public cache/index compatibility versions: old Shard cluster states are
/// disposable and should simply be recomputed with the current recipe.
const SHARD_CLUSTERING_ALGORITHM_REVISION: u32 = 2;

/// Whether graph analysis is collection-wide or derived for a Shard.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphAnalysisContext {
    Collection,
    Shard,
}

/// Availability of automatic clustering for a graph response.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClusterAnalysisStatus {
    Ready,
    Disabled,
    TooSmall,
    Error,
}

/// Availability of local topic assignments for a graph response.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TopicAnalysisStatus {
    Ready,
    None,
    NeedsIngest,
    Error,
}

/// Additive metadata describing the analysis source used by a graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphAnalysisInfo {
    pub context: GraphAnalysisContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shard_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shard_path: Option<String>,
    pub clusters: ClusterAnalysisStatus,
    pub topics: TopicAnalysisStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// On-disk analysis cache. This is a disposable JSON sidecar, not part of the
/// index or compact-graph compatibility contracts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardAnalysisCache {
    pub format: String,
    pub version: u32,
    pub shard_id: String,
    pub shard_path: String,
    /// Fingerprint covering every cluster/topic analysis input.
    pub fingerprint: String,
    /// Shard path and sorted in-scope `(path, content hash)` pairs.
    pub corpus_fingerprint: String,
    /// Corpus fingerprint plus embedding and auto-clustering settings.
    pub cluster_fingerprint: String,
    /// Embedding and auto-clustering settings without corpus identity.
    pub cluster_settings_fingerprint: String,
    /// Topic definitions, similarity floor, embedding model, and dimensions.
    pub topic_fingerprint: String,
    /// Corpus fingerprint that the persisted Topic assignments cover.
    pub topic_corpus_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clusters: Option<ClusterState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topics: Option<CustomClusterState>,
}

/// Locked, atomic access to `.markdownvdb/cache/shards/<ID>.json`.
#[derive(Debug, Clone)]
pub(crate) struct ShardAnalysisCacheStore {
    path: PathBuf,
    shard_id: String,
}

impl ShardAnalysisCacheStore {
    pub(crate) fn new(root: &Path, shard_id: &str) -> Self {
        Self {
            path: root
                .join(".markdownvdb")
                .join("cache")
                .join("shards")
                .join(format!("{shard_id}.json")),
            shard_id: shard_id.to_string(),
        }
    }

    pub(crate) fn load(&self) -> Result<Option<ShardAnalysisCache>> {
        let _lock = self.acquire_lock()?;
        self.load_unlocked()
    }

    /// Hold the cache lock across a complete lazy read/compute/write cycle.
    pub(crate) fn update<T>(
        &self,
        update: impl FnOnce(Option<ShardAnalysisCache>) -> Result<(ShardAnalysisCache, T)>,
    ) -> Result<T> {
        let _lock = self.acquire_lock()?;
        let current = self.load_unlocked()?;
        let (cache, output) = update(current)?;
        self.write_unlocked(&cache)?;
        Ok(output)
    }

    fn load_unlocked(&self) -> Result<Option<ShardAnalysisCache>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path).map_err(|error| {
            shard_cache_error(format!(
                "failed to read Shard analysis cache '{}': {error}",
                self.path.display()
            ))
        })?;
        match serde_json::from_slice::<ShardAnalysisCache>(&bytes) {
            Ok(cache)
                if cache.format == SHARD_ANALYSIS_CACHE_FORMAT
                    && cache.version == SHARD_ANALYSIS_CACHE_VERSION
                    && cache.shard_id == self.shard_id =>
            {
                Ok(Some(cache))
            }
            Ok(_) => {
                warn!(
                    path = %self.path.display(),
                    "ignoring incompatible Shard analysis cache"
                );
                Ok(None)
            }
            Err(error) => {
                warn!(
                    path = %self.path.display(),
                    %error,
                    "ignoring malformed Shard analysis cache"
                );
                Ok(None)
            }
        }
    }

    fn write_unlocked(&self, cache: &ShardAnalysisCache) -> Result<()> {
        let Some(parent) = self.path.parent() else {
            return Err(shard_cache_error("Shard cache path has no parent"));
        };
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec(cache).map_err(|error| {
            Error::Serialization(format!("failed to serialize Shard analysis cache: {error}"))
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| {
            shard_cache_error(format!(
                "failed to atomically replace Shard analysis cache '{}': {}",
                self.path.display(),
                error.error
            ))
        })?;
        Ok(())
    }

    fn acquire_lock(&self) -> Result<ShardAnalysisLock> {
        const ATTEMPTS: usize = 40;
        const RETRY_DELAY: Duration = Duration::from_millis(25);

        let Some(parent) = self.path.parent() else {
            return Err(shard_cache_error("Shard cache path has no parent"));
        };
        fs::create_dir_all(parent)?;
        let lock_path = self.path.with_extension("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        for attempt in 0..ATTEMPTS {
            match file.try_lock() {
                Ok(()) => return Ok(ShardAnalysisLock { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) if attempt + 1 < ATTEMPTS => {
                    std::thread::sleep(RETRY_DELAY);
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(shard_cache_error(format!(
                        "another mdvdb process is updating Shard analysis cache '{}'",
                        self.path.display()
                    )));
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(Error::Io(error)),
            }
        }
        unreachable!("Shard cache lock retry loop always returns")
    }
}

pub(crate) fn remove_cache(root: &Path, shard_id: &str) -> Result<()> {
    let store = ShardAnalysisCacheStore::new(root, shard_id);
    let _lock = store.acquire_lock()?;
    match fs::remove_file(&store.path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

#[must_use]
struct ShardAnalysisLock {
    _file: std::fs::File,
}

pub(crate) fn scoped_maps(
    scope: &str,
    vectors: &HashMap<String, Vec<f32>>,
    documents: &HashMap<String, String>,
) -> (HashMap<String, Vec<f32>>, HashMap<String, String>) {
    let scoped_vectors = vectors
        .iter()
        .filter(|(path, _)| path_util::path_is_in_scope(path, scope))
        .map(|(path, vector)| (path.clone(), vector.clone()))
        .collect();
    let scoped_documents = documents
        .iter()
        .filter(|(path, _)| path_util::path_is_in_scope(path, scope))
        .map(|(path, content)| (path.clone(), content.clone()))
        .collect();
    (scoped_vectors, scoped_documents)
}

pub(crate) fn corpus_fingerprint(
    shard_path: &str,
    file_hashes: &HashMap<String, String>,
) -> String {
    let files: BTreeMap<&str, &str> = file_hashes
        .iter()
        .filter(|(path, _)| path_util::path_is_in_scope(path, shard_path))
        .map(|(path, hash)| (path.as_str(), hash.as_str()))
        .collect();
    hash_json(&serde_json::json!({
        "shard_path": shard_path,
        "files": files,
    }))
}

/// Count vectors that can participate in cosine-based clustering.
pub(crate) fn clusterable_document_count(vectors: &HashMap<String, Vec<f32>>) -> usize {
    vectors
        .values()
        .filter(|vector| vector.iter().map(|value| value * value).sum::<f32>() != 0.0)
        .count()
}

/// Bound a Shard's Leiden neighborhood to its local corpus size.
///
/// Collection clustering continues to use the configured value unchanged.
/// On a small Shard, the normal default (`knn = 15`) would otherwise clamp to
/// `n - 1` and create an effectively complete graph, erasing useful local
/// community structure.
pub(crate) fn effective_shard_knn(
    configured_knn: usize,
    clusterable_document_count: usize,
) -> usize {
    let adaptive_cap = (clusterable_document_count as f64).sqrt().ceil().max(2.0) as usize;
    configured_knn.min(adaptive_cap)
}

pub(crate) fn cluster_settings_fingerprint(config: &Config, effective_knn: usize) -> String {
    hash_json(&serde_json::json!({
        "shard_clustering_algorithm_revision": SHARD_CLUSTERING_ALGORITHM_REVISION,
        "model": config.embedding_model,
        "dims": config.embedding_dimensions,
        "enabled": config.clustering_enabled,
        "algorithm": config.clustering_algorithm,
        "knn": config.clustering_knn,
        "effective_shard_knn": effective_knn,
        "resolution": config.clustering_resolution,
        "min_cluster_size": config.clustering_min_cluster_size,
        "granularity": config.clustering_granularity,
    }))
}

pub(crate) fn cluster_fingerprint(corpus_fingerprint: &str, settings_fingerprint: &str) -> String {
    hash_json(&serde_json::json!({
        "corpus": corpus_fingerprint,
        "settings": settings_fingerprint,
    }))
}

pub(crate) fn full_fingerprint(
    shard_id: &str,
    shard_path: &str,
    corpus_fingerprint: &str,
    cluster_fingerprint: &str,
    topic_fingerprint: &str,
) -> String {
    hash_json(&serde_json::json!({
        "shard_id": shard_id,
        "shard_path": shard_path,
        "corpus": corpus_fingerprint,
        "clusters": cluster_fingerprint,
        "topics": topic_fingerprint,
    }))
}

pub(crate) fn topic_fingerprint(config: &Config, definitions: &[CustomClusterDef]) -> String {
    crate::clustering::topics_fingerprint(
        definitions,
        config.topics_min_similarity,
        &config.embedding_model,
        config.embedding_dimensions,
    )
}

fn hash_json(value: &serde_json::Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(value).unwrap_or_default());
    format!("{:x}", hasher.finalize())
}

fn shard_cache_error(message: impl Into<String>) -> Error {
    Error::Shard(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clustering::Clusterer;
    use crate::config::ClusteringAlgorithm;
    use std::fs;

    fn clustering_config() -> Config {
        let mut config = Config::load(Path::new("/nonexistent")).unwrap();
        config.clustering_enabled = true;
        config.clustering_algorithm = ClusteringAlgorithm::Leiden;
        config.clustering_knn = 15;
        config.clustering_resolution = 1.0;
        config.clustering_min_cluster_size = 2;
        config.embedding_model = "test-model".to_string();
        config.embedding_dimensions = 3;
        config
    }

    #[test]
    fn corpus_fingerprint_is_segment_safe_and_order_independent() {
        let left = HashMap::from([
            ("docs-old/no.md".to_string(), "ignored".to_string()),
            ("docs/b.md".to_string(), "b".to_string()),
            ("docs/a.md".to_string(), "a".to_string()),
        ]);
        let right = HashMap::from([
            ("docs/a.md".to_string(), "a".to_string()),
            ("docs/b.md".to_string(), "b".to_string()),
        ]);
        assert_eq!(
            corpus_fingerprint("docs", &left),
            corpus_fingerprint("docs", &right)
        );
    }

    #[test]
    fn shard_knn_cap_is_adaptive_and_respects_lower_configuration() {
        assert_eq!(effective_shard_knn(15, 16), 4);
        assert_eq!(effective_shard_knn(15, 17), 5);
        assert_eq!(effective_shard_knn(15, 0), 2);
        assert_eq!(effective_shard_knn(3, 16), 3);
        assert_eq!(effective_shard_knn(2, 100), 2);
    }

    #[test]
    fn shard_cluster_fingerprint_invalidates_the_legacy_recipe_and_effective_knn() {
        let config = clustering_config();
        let current = cluster_settings_fingerprint(&config, 4);
        let different_effective_knn = cluster_settings_fingerprint(&config, 3);
        assert_ne!(current, different_effective_knn);

        // This is the exact pre-adaptive-KNN fingerprint payload. The public
        // cache version stays at 1, but its old one-cluster state must miss.
        let legacy = hash_json(&serde_json::json!({
            "model": config.embedding_model,
            "dims": config.embedding_dimensions,
            "enabled": config.clustering_enabled,
            "algorithm": config.clustering_algorithm,
            "knn": config.clustering_knn,
            "resolution": config.clustering_resolution,
            "min_cluster_size": config.clustering_min_cluster_size,
            "granularity": config.clustering_granularity,
        }));
        assert_ne!(current, legacy);
        assert_eq!(SHARD_ANALYSIS_CACHE_VERSION, 1);
    }

    #[test]
    fn adaptive_shard_knn_finds_three_local_communities_in_sixteen_documents() {
        let mut config = clustering_config();
        let mut vectors = HashMap::new();
        let mut documents = HashMap::new();
        let groups = [
            ("alpha", 8, [1.0, 0.15, 0.15]),
            ("beta", 5, [0.15, 1.0, 0.15]),
            ("gamma", 3, [0.15, 0.15, 1.0]),
        ];
        for (group, count, center) in groups {
            for index in 0..count {
                let path = format!("shard/{group}-{index}.md");
                let offset = index as f32 * 0.0001;
                vectors.insert(path.clone(), vec![center[0] + offset, center[1], center[2]]);
                documents.insert(path, format!("{group} local community {index}"));
            }
        }

        let clusterable = clusterable_document_count(&vectors);
        config.clustering_knn = effective_shard_knn(config.clustering_knn, clusterable);
        assert_eq!(config.clustering_knn, 4);

        let first = Clusterer::new(&config)
            .cluster_all(&vectors, &documents, None)
            .unwrap();
        let second = Clusterer::new(&config)
            .cluster_all(&vectors, &documents, None)
            .unwrap();
        let mut member_counts: Vec<usize> = first
            .clusters
            .iter()
            .map(|cluster| cluster.members.len())
            .collect();
        member_counts.sort_unstable_by(|left, right| right.cmp(left));
        assert_eq!(member_counts, vec![8, 5, 3]);
        assert_eq!(
            first
                .clusters
                .iter()
                .map(|cluster| &cluster.members)
                .collect::<Vec<_>>(),
            second
                .clusters
                .iter()
                .map(|cluster| &cluster.members)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn corrupt_or_incompatible_cache_is_ignored_and_atomically_rebuilt() {
        let temp = tempfile::tempdir().unwrap();
        let store = ShardAnalysisCacheStore::new(temp.path(), "docs");
        let cache_path = temp.path().join(".markdownvdb/cache/shards/docs.json");
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(&cache_path, b"{not-json").unwrap();

        assert!(store.load().unwrap().is_none());

        let replacement = ShardAnalysisCache {
            format: SHARD_ANALYSIS_CACHE_FORMAT.to_string(),
            version: SHARD_ANALYSIS_CACHE_VERSION,
            shard_id: "docs".to_string(),
            shard_path: "docs".to_string(),
            fingerprint: "full".to_string(),
            corpus_fingerprint: "corpus".to_string(),
            cluster_fingerprint: "cluster".to_string(),
            cluster_settings_fingerprint: "settings".to_string(),
            topic_fingerprint: "topics".to_string(),
            topic_corpus_fingerprint: "corpus".to_string(),
            clusters: None,
            topics: None,
        };
        store
            .update(|current| {
                assert!(current.is_none());
                Ok((replacement.clone(), ()))
            })
            .unwrap();
        assert_eq!(
            store.load().unwrap().unwrap().fingerprint,
            replacement.fingerprint
        );

        let mut incompatible = replacement;
        incompatible.version += 1;
        fs::write(&cache_path, serde_json::to_vec(&incompatible).unwrap()).unwrap();
        assert!(store.load().unwrap().is_none());

        incompatible.version = SHARD_ANALYSIS_CACHE_VERSION;
        incompatible.shard_id = "other".to_string();
        fs::write(&cache_path, serde_json::to_vec(&incompatible).unwrap()).unwrap();
        assert!(store.load().unwrap().is_none());
    }
}

use std::collections::HashMap;
use std::path::Path;

use mdvdb::clustering::{
    ClusterInfo, ClusterState, Clusterer, CustomClusterDef, CustomClusterState, TopicMember,
};
use mdvdb::config::{ClusteringAlgorithm, Config};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_config() -> Config {
    let mut config = Config::load(Path::new("/nonexistent")).unwrap();
    config.clustering_enabled = true;
    config.clustering_rebalance_threshold = 50;
    config
}

fn kmeans_config() -> Config {
    let mut config = test_config();
    config.clustering_algorithm = ClusteringAlgorithm::Kmeans;
    config
}

fn empty_state() -> ClusterState {
    ClusterState {
        clusters: vec![],
        docs_since_rebalance: 0,
        docs_at_last_rebalance: 0,
        next_cluster_id: 0,
        algorithm: "leiden".to_string(),
        unclustered: vec![],
        parent_clusters: vec![],
    }
}

fn make_vectors(count: usize, dims: usize) -> HashMap<String, Vec<f32>> {
    (0..count)
        .map(|i| {
            let mut v = vec![0.0f32; dims];
            v[i % dims] = 1.0;
            // Add slight variation so vectors aren't identical
            v[(i + 1) % dims] = 0.1 * (i as f32);
            (format!("doc#{i}"), v)
        })
        .collect()
}

fn make_documents(count: usize) -> HashMap<String, String> {
    let topics = [
        "rust programming language systems performance memory safety concurrency",
        "python machine learning data science numpy pandas tensorflow",
        "javascript react frontend components hooks state management",
        "database postgresql indexing queries optimization sql joins",
        "networking tcp http protocols sockets connections routing",
    ];
    (0..count)
        .map(|i| (format!("doc#{i}"), topics[i % topics.len()].to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Integration Tests — auto clustering (Leiden default + kmeans fallback)
// ---------------------------------------------------------------------------

#[test]
fn cluster_all_produces_valid_state() {
    let clusterer = Clusterer::new(&test_config());
    let vectors = make_vectors(20, 8);
    let documents = make_documents(20);

    let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();

    // Should produce clusters
    assert!(!state.clusters.is_empty(), "should produce at least one cluster");
    assert_eq!(state.algorithm, "leiden");

    // Every document should be assigned to exactly one cluster
    let total_members: usize = state.clusters.iter().map(|c| c.members.len()).sum();
    assert_eq!(total_members, 20, "all documents should be assigned");

    // Each cluster should have valid data
    for cluster in &state.clusters {
        assert!(!cluster.members.is_empty(), "no empty clusters");
        assert!(!cluster.centroid.is_empty(), "centroid should be populated");
        assert_eq!(cluster.centroid.len(), 8, "centroid dimension should match");
        assert!(!cluster.label.is_empty(), "label should not be empty");
        assert!(cluster.representative.is_some(), "representative should be set");
    }

    // Counters should be reset
    assert_eq!(state.docs_since_rebalance, 0);
    assert_eq!(state.docs_at_last_rebalance, 20);
}

#[test]
fn cluster_all_kmeans_fallback_produces_valid_state() {
    let clusterer = Clusterer::new(&kmeans_config());
    let vectors = make_vectors(20, 8);
    let documents = make_documents(20);

    let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();

    assert_eq!(state.algorithm, "kmeans");
    let total_members: usize = state.clusters.iter().map(|c| c.members.len()).sum();
    assert_eq!(total_members, 20);
    for cluster in &state.clusters {
        assert!(!cluster.label.is_empty());
    }
}

#[test]
fn cluster_all_deterministic_both_algorithms() {
    let vectors = make_vectors(20, 8);
    let documents = make_documents(20);

    for config in [test_config(), kmeans_config()] {
        let clusterer = Clusterer::new(&config);
        let a = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        let b = clusterer.cluster_all(&vectors, &documents, None).unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap(),
            "{} must be deterministic",
            config.clustering_algorithm.as_str()
        );
    }
}

#[test]
fn cluster_all_keywords_are_meaningful() {
    let clusterer = Clusterer::new(&test_config());

    // Single document so it lands in one cluster
    let mut vectors = HashMap::new();
    vectors.insert("doc#0".to_string(), vec![1.0, 0.0, 0.0, 0.0]);

    let mut documents = HashMap::new();
    documents.insert(
        "doc#0".to_string(),
        "rust programming language systems performance memory safety".to_string(),
    );

    let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();
    assert_eq!(state.clusters.len(), 1);

    let keywords = &state.clusters[0].keywords;
    assert!(!keywords.is_empty(), "should extract keywords");
    // Keywords should not contain stop words
    for kw in keywords {
        assert!(kw.len() >= 3, "keyword '{kw}' should be at least 3 chars");
    }
}

#[test]
fn assign_then_rebalance_workflow() {
    let mut config = test_config();
    config.clustering_rebalance_threshold = 3;
    let clusterer = Clusterer::new(&config);

    // Start with initial clustering
    let vectors = make_vectors(6, 4);
    let documents = make_documents(6);
    let mut state = clusterer.cluster_all(&vectors, &documents, None).unwrap();

    let initial_cluster_count = state.clusters.len();
    assert!(initial_cluster_count > 0);

    // Assign new documents one at a time
    let mut all_vectors = vectors.clone();
    for i in 0..3 {
        let mut v = vec![0.0f32; 4];
        v[i % 4] = 0.8;
        clusterer
            .assign_incremental(&mut state, &format!("new#{i}"), &v, &all_vectors)
            .unwrap();
        all_vectors.insert(format!("new#{i}"), v);
    }

    assert_eq!(state.docs_since_rebalance, 3);

    // Now rebalance should trigger since threshold is 3
    let mut all_documents = documents.clone();
    for i in 0..3 {
        all_documents.insert(format!("new#{i}"), format!("new document {i}"));
    }

    let rebalanced = clusterer
        .maybe_rebalance(&mut state, &all_vectors, &all_documents)
        .unwrap();
    assert!(rebalanced, "should trigger rebalance");
    assert_eq!(state.docs_since_rebalance, 0, "counter should reset");

    // All documents should still be accounted for
    let total: usize = state.clusters.iter().map(|c| c.members.len()).sum();
    assert_eq!(total, 9);
}

#[test]
fn rebalance_preserves_cluster_identity() {
    let mut config = test_config();
    config.clustering_rebalance_threshold = 1;
    let clusterer = Clusterer::new(&config);

    // Two clear groups.
    let mut vectors = HashMap::new();
    let mut documents = HashMap::new();
    for i in 0..6 {
        let mut v = vec![0.01f32; 8];
        v[0] = 1.0;
        v[1] = 0.05 * i as f32;
        vectors.insert(format!("rust{i}.md"), v);
        documents.insert(format!("rust{i}.md"), format!("rust cargo systems {i}"));
        let mut w = vec![0.01f32; 8];
        w[4] = 1.0;
        w[5] = 0.05 * i as f32;
        vectors.insert(format!("cook{i}.md"), w);
        documents.insert(format!("cook{i}.md"), format!("cooking recipe food {i}"));
    }
    let mut state = clusterer.cluster_all(&vectors, &documents, None).unwrap();
    let ids_before: Vec<usize> = state.clusters.iter().map(|c| c.id).collect();

    // Add one more doc, assign incrementally, then rebalance (threshold 1).
    let mut v = vec![0.01f32; 8];
    v[0] = 0.9;
    vectors.insert("rust9.md".to_string(), v.clone());
    documents.insert("rust9.md".to_string(), "rust tokio async".to_string());
    clusterer
        .assign_incremental(&mut state, "rust9.md", &v, &vectors)
        .unwrap();
    let rebalanced = clusterer
        .maybe_rebalance(&mut state, &vectors, &documents)
        .unwrap();
    assert!(rebalanced);

    // Surviving clusters keep their ids.
    let ids_after: Vec<usize> = state.clusters.iter().map(|c| c.id).collect();
    for id in &ids_before {
        assert!(
            ids_after.contains(id),
            "cluster id {id} churned on rebalance (before={ids_before:?}, after={ids_after:?})"
        );
    }
}

#[test]
fn cluster_state_json_serialization() {
    let state = ClusterState {
        clusters: vec![
            ClusterInfo {
                id: 0,
                label: "rust / programming / systems".to_string(),
                centroid: vec![1.0, 0.0, 0.0],
                members: vec!["a.md#0".to_string(), "b.md#0".to_string()],
                keywords: vec!["rust".to_string(), "programming".to_string()],
                parent_id: None,
                representative: Some("a.md#0".to_string()),
            },
            ClusterInfo {
                id: 1,
                label: "python / data / science".to_string(),
                centroid: vec![0.0, 1.0, 0.0],
                members: vec!["c.md#0".to_string()],
                keywords: vec!["python".to_string(), "data".to_string()],
                parent_id: None,
                representative: None,
            },
        ],
        docs_since_rebalance: 5,
        docs_at_last_rebalance: 10,
        next_cluster_id: 2,
        algorithm: "leiden".to_string(),
        unclustered: vec![],
        parent_clusters: vec![],
    };

    let json = serde_json::to_string_pretty(&state).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["clusters"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["docs_since_rebalance"], 5);
    assert_eq!(parsed["clusters"][0]["label"], "rust / programming / systems");
    assert_eq!(parsed["clusters"][0]["representative"], "a.md#0");
    assert_eq!(parsed["algorithm"], "leiden");
    // Omitted when None (additive JSON for consumers).
    assert!(parsed["clusters"][1].get("representative").is_none());
    assert!(parsed["clusters"][0].get("parent_id").is_none());
}

#[test]
fn cluster_all_no_duplicate_members() {
    let clusterer = Clusterer::new(&test_config());
    let vectors = make_vectors(15, 8);
    let documents = make_documents(15);

    let state = clusterer.cluster_all(&vectors, &documents, None).unwrap();

    // Collect all members across clusters and check for duplicates
    let mut all_members: Vec<&str> = Vec::new();
    for cluster in &state.clusters {
        for member in &cluster.members {
            assert!(
                !all_members.contains(&member.as_str()),
                "duplicate member: {member}"
            );
            all_members.push(member);
        }
    }
}

#[test]
fn assign_incremental_error_on_empty_state() {
    let clusterer = Clusterer::new(&test_config());
    let mut state = empty_state();

    let result =
        clusterer.assign_incremental(&mut state, "doc#0", &[1.0, 0.0], &HashMap::new());
    assert!(result.is_err());
}

#[test]
fn maybe_rebalance_skips_below_threshold() {
    let clusterer = Clusterer::new(&test_config()); // threshold = 50
    let mut state = empty_state();
    state.clusters = vec![ClusterInfo {
        id: 0,
        label: "test".to_string(),
        centroid: vec![1.0, 0.0],
        members: vec!["a#0".to_string()],
        keywords: vec![],
        parent_id: None,
        representative: None,
    }];
    state.docs_since_rebalance = 10;
    state.docs_at_last_rebalance = 5;
    state.next_cluster_id = 1;

    let rebalanced = clusterer
        .maybe_rebalance(&mut state, &HashMap::new(), &HashMap::new())
        .unwrap();
    assert!(!rebalanced);
    // State should be unchanged
    assert_eq!(state.docs_since_rebalance, 10);
}

#[test]
fn clusterer_respects_enabled_flag() {
    let mut config = test_config();
    config.clustering_enabled = false;
    let clusterer = Clusterer::new(&config);
    assert!(!clusterer.is_enabled());

    config.clustering_enabled = true;
    let clusterer = Clusterer::new(&config);
    assert!(clusterer.is_enabled());
}

#[test]
fn high_granularity_produces_more_clusters_kmeans() {
    // Granularity applies to the kmeans fallback.
    let mut config = kmeans_config();
    config.clustering_granularity = 4.0;
    let clusterer_fine = Clusterer::new(&config);

    config.clustering_granularity = 0.25;
    let clusterer_coarse = Clusterer::new(&config);

    let vectors = make_vectors(50, 8);
    let documents = make_documents(50);

    let fine_state = clusterer_fine
        .cluster_all(&vectors, &documents, None)
        .unwrap();
    let coarse_state = clusterer_coarse
        .cluster_all(&vectors, &documents, None)
        .unwrap();

    assert!(
        fine_state.clusters.len() >= coarse_state.clusters.len(),
        "higher granularity should produce >= clusters: fine={} coarse={}",
        fine_state.clusters.len(),
        coarse_state.clusters.len()
    );
}

#[test]
fn algorithm_switch_detected_for_full_recluster() {
    let leiden_clusterer = Clusterer::new(&test_config());
    let kmeans_clusterer = Clusterer::new(&kmeans_config());
    let vectors = make_vectors(10, 8);
    let documents = make_documents(10);

    let leiden_state = leiden_clusterer
        .cluster_all(&vectors, &documents, None)
        .unwrap();
    assert!(kmeans_clusterer.algorithm_changed(&leiden_state));
    assert!(!leiden_clusterer.algorithm_changed(&leiden_state));
}

// ---------------------------------------------------------------------------
// Topic (Custom Cluster) Tests — multi-label with thresholds
// ---------------------------------------------------------------------------

fn seed_only_def(name: &str, seed: &str) -> CustomClusterDef {
    CustomClusterDef {
        name: name.to_string(),
        description: None,
        seeds: vec![seed.to_string()],
        threshold: None,
    }
}

#[test]
fn assign_all_to_custom_assigns_matching_documents() {
    let mut config = test_config();
    config.topics_min_similarity = 0.5;
    let clusterer = Clusterer::new(&config);

    let defs = vec![seed_only_def("Cluster A", "alpha"), seed_only_def("Cluster B", "beta")];

    // Centroid A points in x direction, centroid B in y direction
    let centroids = vec![vec![1.0, 0.0, 0.0, 0.0], vec![0.0, 1.0, 0.0, 0.0]];

    // Documents: some closer to A, some closer to B
    let mut doc_vectors = HashMap::new();
    doc_vectors.insert("close_to_a.md".to_string(), vec![0.9, 0.1, 0.0, 0.0]);
    doc_vectors.insert("also_a.md".to_string(), vec![0.8, 0.2, 0.0, 0.0]);
    doc_vectors.insert("close_to_b.md".to_string(), vec![0.1, 0.9, 0.0, 0.0]);

    let state = clusterer
        .assign_all_to_custom(&defs, &centroids, &doc_vectors, "fp".to_string())
        .unwrap();

    let member_paths = |i: usize| -> Vec<&str> {
        state.clusters[i].members.iter().map(|m| m.path.as_str()).collect()
    };
    assert!(member_paths(0).contains(&"close_to_a.md"));
    assert!(member_paths(0).contains(&"also_a.md"));
    assert!(member_paths(1).contains(&"close_to_b.md"));

    // Scores are recorded and plausible.
    for cluster in &state.clusters {
        for m in &cluster.members {
            assert!(m.score >= 0.5, "member below threshold: {} {}", m.path, m.score);
        }
    }

    // Verify metadata
    assert_eq!(state.clusters[0].name, "Cluster A");
    assert_eq!(state.clusters[1].name, "Cluster B");
    assert_eq!(state.clusters[0].id, 0);
    assert_eq!(state.clusters[1].id, 1);
    assert_eq!(state.fingerprint, "fp");
    assert!(state.unassigned.is_empty());
}

#[test]
fn multi_label_document_joins_multiple_topics() {
    let mut config = test_config();
    config.topics_min_similarity = 0.5;
    let clusterer = Clusterer::new(&config);

    let defs = vec![seed_only_def("A", "alpha"), seed_only_def("B", "beta")];
    let centroids = vec![vec![1.0, 0.0], vec![0.0, 1.0]];

    let mut doc_vectors = HashMap::new();
    doc_vectors.insert("both.md".to_string(), vec![1.0, 1.0]); // cos ≈ 0.707 to each

    let state = clusterer
        .assign_all_to_custom(&defs, &centroids, &doc_vectors, "fp".to_string())
        .unwrap();

    assert_eq!(state.clusters[0].members.len(), 1);
    assert_eq!(state.clusters[1].members.len(), 1);
    assert!(state.unassigned.is_empty());
}

#[test]
fn documents_below_floor_are_unassigned() {
    let mut config = test_config();
    config.topics_min_similarity = 0.95;
    let clusterer = Clusterer::new(&config);

    let defs = vec![seed_only_def("A", "alpha")];
    let centroids = vec![vec![1.0, 0.0]];

    let mut doc_vectors = HashMap::new();
    doc_vectors.insert("weak.md".to_string(), vec![1.0, 1.0]); // 0.707 < 0.95

    let state = clusterer
        .assign_all_to_custom(&defs, &centroids, &doc_vectors, "fp".to_string())
        .unwrap();

    assert!(state.clusters[0].members.is_empty());
    assert_eq!(state.unassigned, vec!["weak.md".to_string()]);
}

#[test]
fn per_topic_threshold_tightens_assignment() {
    let mut config = test_config();
    config.topics_min_similarity = 0.2;
    let clusterer = Clusterer::new(&config);

    let mut defs = vec![seed_only_def("Strict", "alpha"), seed_only_def("Lax", "beta")];
    defs[0].threshold = Some(0.95);
    let centroids = vec![vec![1.0, 0.0], vec![0.0, 1.0]];

    let mut doc_vectors = HashMap::new();
    doc_vectors.insert("doc.md".to_string(), vec![1.0, 1.0]); // 0.707 to each

    let state = clusterer
        .assign_all_to_custom(&defs, &centroids, &doc_vectors, "fp".to_string())
        .unwrap();

    assert!(state.clusters[0].members.is_empty(), "strict topic rejects 0.707");
    assert_eq!(state.clusters[1].members.len(), 1, "lax topic accepts 0.707");
}

#[test]
fn assign_single_to_custom_moves_document() {
    let mut config = test_config();
    config.topics_min_similarity = 0.5;
    let clusterer = Clusterer::new(&config);

    let mut state = CustomClusterState {
        clusters: vec![
            mdvdb::clustering::CustomClusterInfo {
                id: 0,
                name: "A".to_string(),
                description: None,
                seed_phrases: vec!["alpha".to_string()],
                threshold: None,
                centroid: vec![1.0, 0.0, 0.0, 0.0],
                members: vec![TopicMember {
                    path: "doc.md".to_string(),
                    score: 0.9,
                }],
            },
            mdvdb::clustering::CustomClusterInfo {
                id: 1,
                name: "B".to_string(),
                description: None,
                seed_phrases: vec!["beta".to_string()],
                threshold: None,
                centroid: vec![0.0, 1.0, 0.0, 0.0],
                members: vec![],
            },
        ],
        unassigned: vec![],
        fingerprint: "fp".to_string(),
    };

    // Re-assign doc.md — now closer to B
    let new_vector = vec![0.1, 0.9, 0.0, 0.0];
    clusterer
        .assign_single_to_custom(&mut state, "doc.md", &new_vector)
        .unwrap();

    // Should have moved from A to B
    assert!(!state.clusters[0].members.iter().any(|m| m.path == "doc.md"));
    assert!(state.clusters[1].members.iter().any(|m| m.path == "doc.md"));
}

#[test]
fn assign_single_to_custom_centroid_stable() {
    let clusterer = Clusterer::new(&test_config());

    let original_centroid = vec![1.0, 0.0, 0.0, 0.0];
    let mut state = CustomClusterState {
        clusters: vec![mdvdb::clustering::CustomClusterInfo {
            id: 0,
            name: "A".to_string(),
            description: None,
            seed_phrases: vec!["alpha".to_string()],
            threshold: None,
            centroid: original_centroid.clone(),
            members: vec![],
        }],
        unassigned: vec![],
        fingerprint: "fp".to_string(),
    };

    // Assign a document — centroid should NOT change
    clusterer
        .assign_single_to_custom(&mut state, "doc.md", &[0.5, 0.5, 0.0, 0.0])
        .unwrap();

    assert_eq!(state.clusters[0].centroid, original_centroid);
}

#[test]
fn custom_cluster_no_duplicate_members() {
    let mut config = test_config();
    config.topics_min_similarity = 0.5;
    let clusterer = Clusterer::new(&config);

    let defs = vec![seed_only_def("Only", "only")];
    let centroids = vec![vec![1.0, 0.0]];

    let mut doc_vectors = HashMap::new();
    for i in 0..10 {
        doc_vectors.insert(format!("doc{i}.md"), vec![1.0, 0.0]);
    }

    let state = clusterer
        .assign_all_to_custom(&defs, &centroids, &doc_vectors, "fp".to_string())
        .unwrap();

    let total: usize = state.clusters.iter().map(|c| c.members.len()).sum();
    assert_eq!(total, 10);

    // No duplicates, and sorted by path.
    let mut all: Vec<&str> = state.clusters[0].members.iter().map(|m| m.path.as_str()).collect();
    let sorted = all.clone();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 10);
    assert_eq!(sorted, all, "members must be path-sorted");
}

#[test]
fn custom_cluster_state_json_shape() {
    let state = CustomClusterState {
        clusters: vec![mdvdb::clustering::CustomClusterInfo {
            id: 0,
            name: "AI".to_string(),
            description: Some("machine learning notes".to_string()),
            seed_phrases: vec!["neural nets".to_string()],
            threshold: Some(0.4),
            centroid: vec![1.0, 0.0],
            members: vec![TopicMember {
                path: "doc.md".to_string(),
                score: 0.83,
            }],
        }],
        unassigned: vec!["other.md".to_string()],
        fingerprint: "abc123".to_string(),
    };

    let json = serde_json::to_string(&state).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["clusters"][0]["members"][0]["path"], "doc.md");
    assert!(parsed["clusters"][0]["members"][0]["score"].as_f64().unwrap() > 0.8);
    assert_eq!(parsed["clusters"][0]["description"], "machine learning notes");
    assert_eq!(parsed["unassigned"][0], "other.md");
    assert_eq!(parsed["fingerprint"], "abc123");
}

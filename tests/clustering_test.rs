use std::collections::HashMap;
use std::path::Path;

use mdvdb::clustering::{ClusterInfo, ClusterState, Clusterer};
use mdvdb::config::Config;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_config() -> Config {
    let mut config = Config::load(Path::new("/nonexistent")).unwrap();
    config.clustering_enabled = true;
    config.clustering_rebalance_threshold = 50;
    config
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
// Integration Tests
// ---------------------------------------------------------------------------

#[test]
fn cluster_all_produces_valid_state() {
    let clusterer = Clusterer::new(&test_config());
    let vectors = make_vectors(20, 8);
    let documents = make_documents(20);

    let state = clusterer.cluster_all(&vectors, &documents).unwrap();

    // Should produce clusters
    assert!(!state.clusters.is_empty(), "should produce at least one cluster");

    // Every document should be assigned to exactly one cluster
    let total_members: usize = state.clusters.iter().map(|c| c.members.len()).sum();
    assert_eq!(total_members, 20, "all documents should be assigned");

    // Each cluster should have valid data
    for cluster in &state.clusters {
        assert!(!cluster.members.is_empty(), "no empty clusters");
        assert!(!cluster.centroid.is_empty(), "centroid should be populated");
        assert_eq!(cluster.centroid.len(), 8, "centroid dimension should match");
        assert!(!cluster.label.is_empty(), "label should not be empty");
    }

    // Counters should be reset
    assert_eq!(state.docs_since_rebalance, 0);
    assert_eq!(state.docs_at_last_rebalance, 20);
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

    let state = clusterer.cluster_all(&vectors, &documents).unwrap();
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
    let mut state = clusterer.cluster_all(&vectors, &documents).unwrap();

    let initial_cluster_count = state.clusters.len();
    assert!(initial_cluster_count > 0);

    // Assign new documents one at a time
    for i in 0..3 {
        let mut v = vec![0.0f32; 4];
        v[i % 4] = 0.8;
        clusterer
            .assign_to_nearest(&mut state, &format!("new#{i}"), &v)
            .unwrap();
    }

    assert_eq!(state.docs_since_rebalance, 3);

    // Now rebalance should trigger since threshold is 3
    let mut all_vectors = vectors.clone();
    let mut all_documents = documents.clone();
    for i in 0..3 {
        let mut v = vec![0.0f32; 4];
        v[i % 4] = 0.8;
        all_vectors.insert(format!("new#{i}"), v);
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
fn cluster_state_json_serialization() {
    let state = ClusterState {
        clusters: vec![
            ClusterInfo {
                id: 0,
                label: "rust / programming / systems".to_string(),
                centroid: vec![1.0, 0.0, 0.0],
                members: vec!["a.md#0".to_string(), "b.md#0".to_string()],
                keywords: vec!["rust".to_string(), "programming".to_string()],
            },
            ClusterInfo {
                id: 1,
                label: "python / data / science".to_string(),
                centroid: vec![0.0, 1.0, 0.0],
                members: vec!["c.md#0".to_string()],
                keywords: vec!["python".to_string(), "data".to_string()],
            },
        ],
        docs_since_rebalance: 5,
        docs_at_last_rebalance: 10,
    };

    let json = serde_json::to_string_pretty(&state).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["clusters"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["docs_since_rebalance"], 5);
    assert_eq!(parsed["clusters"][0]["label"], "rust / programming / systems");
}

#[test]
fn cluster_all_no_duplicate_members() {
    let clusterer = Clusterer::new(&test_config());
    let vectors = make_vectors(15, 8);
    let documents = make_documents(15);

    let state = clusterer.cluster_all(&vectors, &documents).unwrap();

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
fn assign_to_nearest_error_on_empty_state() {
    let clusterer = Clusterer::new(&test_config());
    let mut state = ClusterState {
        clusters: vec![],
        docs_since_rebalance: 0,
        docs_at_last_rebalance: 0,
    };

    let result = clusterer.assign_to_nearest(&mut state, "doc#0", &[1.0, 0.0]);
    assert!(result.is_err());
}

#[test]
fn maybe_rebalance_skips_below_threshold() {
    let clusterer = Clusterer::new(&test_config()); // threshold = 50
    let mut state = ClusterState {
        clusters: vec![ClusterInfo {
            id: 0,
            label: "test".to_string(),
            centroid: vec![1.0, 0.0],
            members: vec!["a#0".to_string()],
            keywords: vec![],
        }],
        docs_since_rebalance: 10,
        docs_at_last_rebalance: 5,
    };

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

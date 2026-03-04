use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::{IngestOptions, MarkdownVdb};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const DIMS: usize = 8;

fn mock_config() -> Config {
    Config {
        embedding_provider: EmbeddingProviderType::Mock,
        embedding_model: "mock-model".into(),
        embedding_dimensions: DIMS,
        embedding_batch_size: 100,
        openai_api_key: None,
        ollama_host: "http://localhost:11434".into(),
        embedding_endpoint: None,
        source_dirs: vec![PathBuf::from(".")],
        ignore_patterns: vec![],
        watch_enabled: false,
        watch_debounce_ms: 300,
        chunk_max_tokens: 512,
        chunk_overlap_tokens: 50,
        clustering_enabled: false,
        clustering_rebalance_threshold: 50,
        search_default_limit: 10,
        search_min_score: 0.0,
        search_default_mode: mdvdb::SearchMode::Hybrid,
        search_rrf_k: 60.0,
        bm25_norm_k: 1.5,
        search_decay_enabled: false,
        search_decay_half_life: 90.0,
        vector_quantization: mdvdb::VectorQuantization::F16,
        index_compression: true,
    }
}

fn setup_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();
    dir
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_graph_data_empty_index() {
    let dir = setup_dir();
    let root = dir.path();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    let graph = vdb.graph_data().unwrap();

    assert!(graph.nodes.is_empty(), "empty index should have no nodes");
    assert!(graph.edges.is_empty(), "empty index should have no edges");
    assert!(graph.clusters.is_empty(), "empty index should have no clusters");
}

#[tokio::test]
async fn test_graph_data_with_files() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nContent of A.\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent of B.\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nContent of C.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph_data().unwrap();

    assert_eq!(graph.nodes.len(), 3, "should have 3 nodes");
    let paths: Vec<&str> = graph.nodes.iter().map(|n| n.path.as_str()).collect();
    assert!(paths.contains(&"a.md"));
    assert!(paths.contains(&"b.md"));
    assert!(paths.contains(&"c.md"));

    // No links between files, so no edges
    assert!(graph.edges.is_empty(), "no links means no edges");
}

#[tokio::test]
async fn test_graph_data_with_links() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nLink to [B](b.md) and [C](c.md).\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nLink to [C](c.md).\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nContent of C.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph_data().unwrap();

    assert_eq!(graph.nodes.len(), 3);
    assert_eq!(graph.edges.len(), 3, "should have 3 edges: a->b, a->c, b->c");

    let edge_pairs: Vec<(&str, &str)> = graph
        .edges
        .iter()
        .map(|e| (e.source.as_str(), e.target.as_str()))
        .collect();
    assert!(edge_pairs.contains(&("a.md", "b.md")));
    assert!(edge_pairs.contains(&("a.md", "c.md")));
    assert!(edge_pairs.contains(&("b.md", "c.md")));
}

#[tokio::test]
async fn test_graph_data_filters_broken_edges() {
    let dir = setup_dir();
    let root = dir.path();

    // a links to b (exists) and nonexistent (doesn't exist)
    fs::write(
        root.join("a.md"),
        "# A\n\nLink to [B](b.md) and [missing](nonexistent.md).\n",
    )
    .unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent of B.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph_data().unwrap();

    assert_eq!(graph.nodes.len(), 2);
    // Only a->b edge should exist; a->nonexistent should be filtered out
    assert_eq!(
        graph.edges.len(),
        1,
        "broken edge to nonexistent.md should be filtered"
    );
    assert_eq!(graph.edges[0].source, "a.md");
    assert_eq!(graph.edges[0].target, "b.md");
}

#[tokio::test]
async fn test_graph_data_no_clusters() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nContent.\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent.\n").unwrap();

    // clustering_enabled is false in mock_config
    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph_data().unwrap();

    assert_eq!(graph.nodes.len(), 2);
    assert!(graph.clusters.is_empty(), "no clusters when clustering disabled");
    for node in &graph.nodes {
        assert!(
            node.cluster_id.is_none(),
            "node {} should have no cluster_id",
            node.path
        );
    }
}

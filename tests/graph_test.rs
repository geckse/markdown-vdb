use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::{GraphLevel, IngestOptions, MarkdownVdb};
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
    let graph = vdb.graph_data(None).unwrap();

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

    let graph = vdb.graph_data(None).unwrap();

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

    let graph = vdb.graph_data(None).unwrap();

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

    let graph = vdb.graph_data(None).unwrap();

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

    let graph = vdb.graph_data(None).unwrap();

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

// ---------------------------------------------------------------------------
// Chunk graph tests
// ---------------------------------------------------------------------------

#[test]
fn test_chunk_graph_empty_index() {
    let dir = setup_dir();
    let root = dir.path();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    let graph = vdb.graph(GraphLevel::Chunk, None).unwrap();

    assert!(graph.nodes.is_empty(), "empty index should have no nodes");
    assert!(graph.edges.is_empty(), "empty index should have no edges");
    assert_eq!(graph.level, "chunk");
}

#[tokio::test]
async fn test_chunk_graph_cross_file_edges() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# Alpha\n\nAlpha content here.\n").unwrap();
    fs::write(root.join("b.md"), "# Beta\n\nBeta content here.\n").unwrap();
    fs::write(root.join("c.md"), "# Gamma\n\nGamma content here.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph(GraphLevel::Chunk, None).unwrap();

    assert_eq!(graph.level, "chunk");
    assert!(graph.nodes.len() >= 3, "should have at least 3 chunk nodes");

    // All edges must connect chunks from different files
    for edge in &graph.edges {
        let src_node = graph.nodes.iter().find(|n| n.id == edge.source).unwrap();
        let tgt_node = graph.nodes.iter().find(|n| n.id == edge.target).unwrap();
        assert_ne!(
            src_node.path, tgt_node.path,
            "edge {}->{} connects chunks from the same file {}",
            edge.source, edge.target, src_node.path
        );
    }
}

#[tokio::test]
async fn test_chunk_graph_no_intra_file_edges() {
    let dir = setup_dir();
    let root = dir.path();

    // Single file with multiple headings → multiple chunks
    fs::write(
        root.join("multi.md"),
        "# Section One\n\nFirst section content.\n\n# Section Two\n\nSecond section content.\n\n# Section Three\n\nThird section content.\n",
    )
    .unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph(GraphLevel::Chunk, None).unwrap();

    // Multiple chunks from the same file
    assert!(
        graph.nodes.len() >= 2,
        "multi-heading file should produce multiple chunk nodes, got {}",
        graph.nodes.len()
    );

    // No edges since all chunks are from the same file
    assert!(
        graph.edges.is_empty(),
        "single-file chunks should produce no edges, got {}",
        graph.edges.len()
    );
}

#[tokio::test]
async fn test_chunk_graph_heading_labels() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(
        root.join("doc.md"),
        "# Main Title\n\nIntro.\n\n## Sub Section\n\nDetails.\n",
    )
    .unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph(GraphLevel::Chunk, None).unwrap();

    assert!(!graph.nodes.is_empty(), "should have chunk nodes");

    for node in &graph.nodes {
        // Every chunk node should have a label (heading hierarchy or fallback)
        assert!(
            node.label.is_some(),
            "chunk node {} should have a label",
            node.id
        );
        let label = node.label.as_ref().unwrap();
        assert!(!label.is_empty(), "label should not be empty for {}", node.id);
    }
}

#[tokio::test]
async fn test_chunk_graph_no_heading_labels() {
    let dir = setup_dir();
    let root = dir.path();

    // Content without any headings — chunks should have label = None
    fs::write(root.join("plain.md"), "Just some plain text without headings.\n").unwrap();
    fs::write(root.join("other.md"), "Another file with no headings at all.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph(GraphLevel::Chunk, None).unwrap();

    assert!(!graph.nodes.is_empty(), "should have chunk nodes");

    for node in &graph.nodes {
        // Without headings, label should be None
        assert!(
            node.label.is_none(),
            "chunk node {} from file without headings should have label = None",
            node.id
        );
        // chunk_index should still be set
        assert!(
            node.chunk_index.is_some(),
            "chunk node {} should have chunk_index",
            node.id
        );
    }
}

#[tokio::test]
async fn test_chunk_graph_edge_weights() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("x.md"), "# X\n\nContent about X.\n").unwrap();
    fs::write(root.join("y.md"), "# Y\n\nContent about Y.\n").unwrap();
    fs::write(root.join("z.md"), "# Z\n\nContent about Z.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph(GraphLevel::Chunk, None).unwrap();

    for edge in &graph.edges {
        assert!(
            edge.weight.is_some(),
            "chunk edge {}->{} should have a weight",
            edge.source, edge.target
        );
        let w = edge.weight.unwrap();
        assert!(
            (0.0..=1.0).contains(&w),
            "weight {} should be in 0.0-1.0 for edge {}->{}",
            w, edge.source, edge.target
        );
    }
}

#[tokio::test]
async fn test_chunk_graph_single_file() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("only.md"), "# Only File\n\nSome content.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph(GraphLevel::Chunk, None).unwrap();

    assert!(
        !graph.nodes.is_empty(),
        "single file should still produce chunk nodes"
    );
    assert!(
        graph.edges.is_empty(),
        "single file should have zero edges (no cross-file connections)"
    );
}

#[tokio::test]
async fn test_graph_dispatcher() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nContent of A.\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent of B.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Document level should match graph_data()
    let doc_graph = vdb.graph(GraphLevel::Document, None).unwrap();
    let direct_graph = vdb.graph_data(None).unwrap();
    assert_eq!(doc_graph.level, "document");
    assert_eq!(doc_graph.nodes.len(), direct_graph.nodes.len());
    assert_eq!(doc_graph.edges.len(), direct_graph.edges.len());

    // Chunk level should return chunk-level data
    let chunk_graph = vdb.graph(GraphLevel::Chunk, None).unwrap();
    assert_eq!(chunk_graph.level, "chunk");
    // Chunk nodes have chunk_index set
    for node in &chunk_graph.nodes {
        assert!(
            node.chunk_index.is_some(),
            "chunk-level node {} should have chunk_index",
            node.id
        );
    }
}

#[tokio::test]
async fn test_graph_data_backward_compat() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nLink to [B](b.md).\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph_data(None).unwrap();

    // New fields should have backward-compatible defaults
    assert_eq!(graph.level, "document");

    for node in &graph.nodes {
        // id should be set to the path
        assert_eq!(node.id, node.path, "document node id should equal path");
        // label and chunk_index should be None for document-level
        assert!(node.label.is_none(), "document node should have no label");
        assert!(
            node.chunk_index.is_none(),
            "document node should have no chunk_index"
        );
    }

    for edge in &graph.edges {
        // weight should be None for link-based document edges
        assert!(
            edge.weight.is_none(),
            "document edge should have no weight"
        );
    }
}

// ---------------------------------------------------------------------------
// Cluster inheritance tests
// ---------------------------------------------------------------------------

fn mock_config_clustering() -> Config {
    let mut cfg = mock_config();
    cfg.clustering_enabled = true;
    cfg
}

#[tokio::test]
async fn test_chunk_graph_cluster_inheritance() {
    let dir = setup_dir();
    let root = dir.path();

    // Need enough files for clustering to produce clusters
    fs::write(root.join("a.md"), "# A\n\nRust programming language.\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nPython data science.\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nJavaScript frontend.\n").unwrap();
    fs::write(root.join("d.md"), "# D\n\nDatabase optimization.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config_clustering()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let chunk_graph = vdb.graph(GraphLevel::Chunk, None).unwrap();
    let doc_graph = vdb.graph_data(None).unwrap();

    // Chunk nodes should inherit cluster_id from their parent document
    for chunk_node in &chunk_graph.nodes {
        let doc_node = doc_graph.nodes.iter().find(|n| n.path == chunk_node.path);
        if let Some(doc) = doc_node {
            assert_eq!(
                chunk_node.cluster_id, doc.cluster_id,
                "chunk {} should inherit cluster_id from parent doc {}",
                chunk_node.id, chunk_node.path
            );
        }
    }
}

#[tokio::test]
async fn test_chunk_graph_clusters_populated() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nRust programming.\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nPython science.\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nJavaScript frontend.\n").unwrap();
    fs::write(root.join("d.md"), "# D\n\nDatabase SQL.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config_clustering()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let chunk_graph = vdb.graph(GraphLevel::Chunk, None).unwrap();

    // clusters vec should be non-empty when clustering is enabled
    assert!(
        !chunk_graph.clusters.is_empty(),
        "chunk graph should return populated clusters"
    );

    // Each cluster should have valid fields
    for cluster in &chunk_graph.clusters {
        assert!(!cluster.label.is_empty(), "cluster label should not be empty");
        assert!(cluster.member_count > 0, "cluster should have members");
    }
}

#[tokio::test]
async fn test_chunk_graph_no_clusters_fallback() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nContent.\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent.\n").unwrap();

    // clustering_enabled is false in mock_config
    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let chunk_graph = vdb.graph(GraphLevel::Chunk, None).unwrap();

    assert!(
        chunk_graph.clusters.is_empty(),
        "clusters should be empty when clustering disabled"
    );
    for node in &chunk_graph.nodes {
        assert!(
            node.cluster_id.is_none(),
            "chunk node {} should have no cluster_id when clustering disabled",
            node.id
        );
    }
}

// ---------------------------------------------------------------------------
// Path filter tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_graph_data_path_filter() {
    let dir = setup_dir();
    let root = dir.path();

    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/guide.md"), "# Guide\n\nGuide content.\n").unwrap();
    fs::write(root.join("docs/api.md"), "# API\n\nAPI reference.\n").unwrap();
    fs::write(root.join("readme.md"), "# Readme\n\nTop-level readme.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph_data(Some("docs/")).unwrap();

    assert_eq!(graph.nodes.len(), 2, "only docs/ files should be included");
    for node in &graph.nodes {
        assert!(
            node.path.starts_with("docs/"),
            "node {} should start with docs/",
            node.path
        );
    }
}

#[tokio::test]
async fn test_chunk_graph_path_filter() {
    let dir = setup_dir();
    let root = dir.path();

    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/guide.md"), "# Guide\n\nGuide content here.\n").unwrap();
    fs::write(root.join("docs/api.md"), "# API\n\nAPI reference docs.\n").unwrap();
    fs::write(root.join("readme.md"), "# Readme\n\nTop-level readme.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph(GraphLevel::Chunk, Some("docs/")).unwrap();

    assert!(!graph.nodes.is_empty(), "should have chunk nodes from docs/");
    for node in &graph.nodes {
        assert!(
            node.path.starts_with("docs/"),
            "chunk node {} has path {} outside docs/",
            node.id, node.path
        );
    }
}

#[tokio::test]
async fn test_graph_path_filter_edges() {
    let dir = setup_dir();
    let root = dir.path();

    fs::create_dir_all(root.join("docs")).unwrap();
    // docs/a links to docs/b (both in filter) and to root.md (outside filter)
    fs::write(
        root.join("docs/a.md"),
        "# A\n\nLink to [B](../docs/b.md) and [root](../root.md).\n",
    )
    .unwrap();
    fs::write(root.join("docs/b.md"), "# B\n\nContent.\n").unwrap();
    fs::write(root.join("root.md"), "# Root\n\nRoot content.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph = vdb.graph_data(Some("docs/")).unwrap();

    // Only nodes from docs/
    assert_eq!(graph.nodes.len(), 2);
    // Edges should only include those where both endpoints are in docs/
    for edge in &graph.edges {
        assert!(
            edge.source.starts_with("docs/"),
            "edge source {} should be in docs/",
            edge.source
        );
        assert!(
            edge.target.starts_with("docs/"),
            "edge target {} should be in docs/",
            edge.target
        );
    }
}

#[tokio::test]
async fn test_graph_path_filter_none() {
    let dir = setup_dir();
    let root = dir.path();

    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/guide.md"), "# Guide\n\nGuide.\n").unwrap();
    fs::write(root.join("readme.md"), "# Readme\n\nReadme.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let graph_all = vdb.graph_data(None).unwrap();
    assert_eq!(graph_all.nodes.len(), 2, "None filter should return all files");

    let paths: Vec<&str> = graph_all.nodes.iter().map(|n| n.path.as_str()).collect();
    assert!(paths.contains(&"docs/guide.md"));
    assert!(paths.contains(&"readme.md"));
}

#[tokio::test]
async fn test_graph_dispatcher_path_filter() {
    let dir = setup_dir();
    let root = dir.path();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.md"), "# Main\n\nMain module.\n").unwrap();
    fs::write(root.join("src/lib.md"), "# Lib\n\nLib module.\n").unwrap();
    fs::write(root.join("readme.md"), "# Readme\n\nTop level.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Document level with path filter
    let doc_graph = vdb.graph(GraphLevel::Document, Some("src/")).unwrap();
    assert_eq!(doc_graph.nodes.len(), 2, "document graph should have 2 src/ nodes");
    for node in &doc_graph.nodes {
        assert!(node.path.starts_with("src/"));
    }

    // Chunk level with path filter
    let chunk_graph = vdb.graph(GraphLevel::Chunk, Some("src/")).unwrap();
    assert!(!chunk_graph.nodes.is_empty(), "chunk graph should have src/ nodes");
    for node in &chunk_graph.nodes {
        assert!(
            node.path.starts_with("src/"),
            "chunk node {} should be in src/",
            node.id
        );
    }
}

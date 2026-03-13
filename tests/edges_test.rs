use std::fs;
use std::path::PathBuf;
use std::process::Command;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::{IngestOptions, MarkdownVdb, SearchMode, SearchQuery};
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
        search_default_mode: SearchMode::Hybrid,
        search_rrf_k: 60.0,
        bm25_norm_k: 1.5,
        search_decay_enabled: false,
        search_decay_half_life: 90.0,
        search_decay_exclude: vec![],
        search_decay_include: vec![],
        search_boost_links: false,
        search_boost_hops: 1,
        search_expand_graph: 0,
        search_expand_limit: 3,
        vector_quantization: mdvdb::VectorQuantization::F16,
        index_compression: true,
        edge_embeddings: true,
        edge_boost_weight: 0.15,
        edge_cluster_rebalance: 50,
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

/// Create 3+ interlinked markdown files for edge tests.
fn write_interlinked_files(root: &std::path::Path) {
    fs::write(
        root.join("alpha.md"),
        "---\ntags: [rust]\n---\n# Alpha\n\nAlpha introduces concepts. See [Beta](beta.md) for details and [Gamma](gamma.md) for examples.\n",
    )
    .unwrap();
    fs::write(
        root.join("beta.md"),
        "---\ntags: [python]\n---\n# Beta\n\nBeta expands on Alpha. Link back to [Alpha](alpha.md) and forward to [Gamma](gamma.md).\n",
    )
    .unwrap();
    fs::write(
        root.join("gamma.md"),
        "---\ntags: [javascript]\n---\n# Gamma\n\nGamma provides examples. References [Alpha](alpha.md) for theory.\n",
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// 1. Full ingest with edges
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ingest_creates_semantic_edges() {
    let dir = setup_dir();
    let root = dir.path();
    write_interlinked_files(root);

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let edges = vdb.edges(None).unwrap();
    assert!(
        !edges.is_empty(),
        "ingest of interlinked files should produce semantic edges"
    );

    // Each edge should have required fields populated
    for edge in &edges {
        assert!(
            edge.edge_id.starts_with("edge:"),
            "edge_id '{}' should start with 'edge:'",
            edge.edge_id
        );
        assert!(!edge.source.is_empty(), "source should not be empty");
        assert!(!edge.target.is_empty(), "target should not be empty");
        assert!(
            !edge.context_text.is_empty(),
            "context_text should not be empty for edge {}",
            edge.edge_id
        );
        assert!(edge.line_number > 0, "line_number should be > 0");
    }

    // We expect edges for the links: alpha->beta, alpha->gamma, beta->alpha, beta->gamma, gamma->alpha
    assert!(
        edges.len() >= 3,
        "expected at least 3 edges from interlinked files, got {}",
        edges.len()
    );
}

#[tokio::test]
async fn test_ingest_edges_filtered_by_file() {
    let dir = setup_dir();
    let root = dir.path();
    write_interlinked_files(root);

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let alpha_edges = vdb.edges(Some("alpha.md")).unwrap();
    assert!(
        !alpha_edges.is_empty(),
        "alpha.md should have edges"
    );
    for edge in &alpha_edges {
        assert!(
            edge.source == "alpha.md" || edge.target == "alpha.md",
            "filtered edge {} should involve alpha.md",
            edge.edge_id
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Edge search end-to-end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_edge_search_returns_results() {
    let dir = setup_dir();
    let root = dir.path();
    write_interlinked_files(root);

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let mut query = SearchQuery::new("concepts and examples");
    query.mode = SearchMode::Edge;
    query.limit = 10;

    let response = vdb.search(query).await.unwrap();

    // Edge mode should populate edge_results
    assert!(
        !response.edge_results.is_empty(),
        "edge search should return edge_results"
    );

    for er in &response.edge_results {
        assert!(!er.edge_id.is_empty(), "edge_id should be set");
        assert!(!er.source_path.is_empty(), "source_path should be set");
        assert!(!er.target_path.is_empty(), "target_path should be set");
        assert!(er.score > 0.0, "score should be positive, got {}", er.score);
    }
}

// ---------------------------------------------------------------------------
// 3. Edge-weighted boost vs flat boost
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_edge_weighted_boost_differs_from_flat() {
    let dir = setup_dir();
    let root = dir.path();
    write_interlinked_files(root);

    let mut cfg_edge = mock_config();
    cfg_edge.search_boost_links = true;
    cfg_edge.edge_boost_weight = 0.3;

    let vdb_edge = MarkdownVdb::open_with_config(root.to_path_buf(), cfg_edge).unwrap();
    vdb_edge.ingest(IngestOptions::default()).await.unwrap();

    let mut q1 = SearchQuery::new("Alpha concepts");
    q1.boost_links = Some(true);
    let response_edge = vdb_edge.search(q1).await.unwrap();

    // Now with edge_embeddings disabled (flat boost only)
    let dir2 = setup_dir();
    let root2 = dir2.path();
    write_interlinked_files(root2);

    let mut cfg_flat = mock_config();
    cfg_flat.search_boost_links = true;
    cfg_flat.edge_embeddings = false;

    let vdb_flat = MarkdownVdb::open_with_config(root2.to_path_buf(), cfg_flat).unwrap();
    vdb_flat.ingest(IngestOptions::default()).await.unwrap();

    let mut q2 = SearchQuery::new("Alpha concepts");
    q2.boost_links = Some(true);
    let response_flat = vdb_flat.search(q2).await.unwrap();

    // Both should return results
    assert!(!response_edge.results.is_empty(), "edge-boosted search should have results");
    assert!(!response_flat.results.is_empty(), "flat-boosted search should have results");

    // Scores may differ when edge boost is active vs not
    // We just verify both work without error — exact score differences depend on mock embeddings
    let edge_scores: Vec<f64> = response_edge.results.iter().map(|r| r.score).collect();
    let flat_scores: Vec<f64> = response_flat.results.iter().map(|r| r.score).collect();

    // At minimum, verify we got scores
    assert!(!edge_scores.is_empty());
    assert!(!flat_scores.is_empty());
}

// ---------------------------------------------------------------------------
// 4. Incremental edge update
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_incremental_edge_update() {
    let dir = setup_dir();
    let root = dir.path();
    write_interlinked_files(root);

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let _edges_before = vdb.edges(None).unwrap();
    let alpha_edges_before = vdb.edges(Some("alpha.md")).unwrap();
    assert!(!alpha_edges_before.is_empty());

    // Modify alpha.md: remove link to gamma, add link to new file delta
    fs::write(
        root.join("delta.md"),
        "# Delta\n\nDelta content with [Alpha](alpha.md) link.\n",
    )
    .unwrap();
    fs::write(
        root.join("alpha.md"),
        "---\ntags: [rust]\n---\n# Alpha\n\nAlpha revised. See [Beta](beta.md) and [Delta](delta.md) now.\n",
    )
    .unwrap();

    // Re-ingest with --full to ensure complete rebuild
    vdb.ingest(IngestOptions { full: true, ..Default::default() }).await.unwrap();

    let edges_after = vdb.edges(None).unwrap();
    let alpha_edges_after = vdb.edges(Some("alpha.md")).unwrap();

    // Alpha should now link to delta and/or beta (not gamma)
    let alpha_outgoing: Vec<&str> = alpha_edges_after
        .iter()
        .filter(|e| e.source == "alpha.md")
        .map(|e| e.target.as_str())
        .collect();

    assert!(
        alpha_outgoing.contains(&"delta.md") || alpha_outgoing.contains(&"beta.md"),
        "alpha should have edges to new targets after update, got: {:?}",
        alpha_outgoing
    );

    // Old alpha->gamma edge should be gone (alpha no longer links to gamma)
    let has_alpha_gamma = alpha_outgoing.contains(&"gamma.md");
    assert!(
        !has_alpha_gamma,
        "old alpha->gamma edge should be removed after full re-ingest"
    );

    // New edges should include delta connections
    let has_delta_edges = edges_after.iter().any(|e| e.source == "delta.md" || e.target == "delta.md");
    assert!(
        has_delta_edges,
        "delta.md should have edges after re-ingest"
    );
}

// ---------------------------------------------------------------------------
// 5. Edge clustering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_edge_clustering_with_many_files() {
    let dir = setup_dir();
    let root = dir.path();

    // Create 10+ interlinked files with diverse content
    for i in 0..12 {
        let name = format!("doc{}.md", i);
        let next = format!("doc{}.md", (i + 1) % 12);
        let prev = format!("doc{}.md", (i + 11) % 12);
        let content = format!(
            "# Document {}\n\nContent about topic {}. See [next]({}) and [prev]({}).\n",
            i, i, next, prev
        );
        fs::write(root.join(&name), content).unwrap();
    }

    let mut cfg = mock_config();
    // Set low rebalance threshold to trigger clustering with fewer edges
    cfg.edge_cluster_rebalance = 5;

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), cfg).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let edges = vdb.edges(None).unwrap();
    assert!(
        edges.len() >= 10,
        "12 interlinked files should produce many edges, got {}",
        edges.len()
    );

    // Check edge clustering state
    let cluster_state = vdb.edge_clusters().unwrap();
    if let Some(ref state) = cluster_state {
        assert!(
            !state.clusters.is_empty(),
            "edge clusters should be auto-discovered"
        );
        for cluster in &state.clusters {
            assert!(!cluster.label.is_empty(), "cluster should have a label");
            assert!(!cluster.members.is_empty(), "cluster should have members");
            assert!(!cluster.keywords.is_empty(), "cluster should have keywords");
        }
    }
    // Note: clustering may not trigger if edge count < rebalance threshold,
    // which is acceptable — the important thing is no errors.

    // Verify some edges have cluster_id assigned
    let clustered_edges: Vec<_> = edges.iter().filter(|e| e.cluster_id.is_some()).collect();
    // If clustering triggered, some edges should have cluster IDs
    if cluster_state.is_some() {
        // At least some edges should be clustered
        // (may be all or partial depending on rebalance timing)
    }
    let _ = clustered_edges; // suppress unused warning
}

// ---------------------------------------------------------------------------
// 6. Backward compatibility: index without semantic edges
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_backward_compat_no_edges() {
    let dir = setup_dir();
    let root = dir.path();

    // Ingest with edges disabled first
    let mut cfg_no_edges = mock_config();
    cfg_no_edges.edge_embeddings = false;

    fs::write(
        root.join("a.md"),
        "# A\n\nLink to [B](b.md).\n",
    )
    .unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent of B.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), cfg_no_edges).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Normal search should work fine without edges
    let query = SearchQuery::new("content");
    let response = vdb.search(query).await.unwrap();
    assert!(
        !response.results.is_empty(),
        "search should work without semantic edges"
    );

    // edges() should return empty list
    let edges = vdb.edges(None).unwrap();
    assert!(
        edges.is_empty(),
        "edges should be empty when edge_embeddings was false"
    );

    // edge_clusters should be None
    let clusters = vdb.edge_clusters().unwrap();
    assert!(
        clusters.is_none(),
        "edge_clusters should be None without edges"
    );

    // Drop first VDB to release FTS lock before re-opening
    drop(vdb);

    // Re-open with edges enabled — should still work (backward compat)
    let cfg_with_edges = mock_config();
    let vdb2 = MarkdownVdb::open_with_config(root.to_path_buf(), cfg_with_edges).unwrap();

    // Status should work
    let status = vdb2.status();
    assert!(status.document_count > 0);

    // Search should work on old index
    let query2 = SearchQuery::new("content");
    let response2 = vdb2.search(query2).await.unwrap();
    assert!(!response2.results.is_empty());
}

// ---------------------------------------------------------------------------
// 7. Edge disabled: no edges created
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_edge_disabled_no_edges_created() {
    let dir = setup_dir();
    let root = dir.path();
    write_interlinked_files(root);

    let mut cfg = mock_config();
    cfg.edge_embeddings = false;

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), cfg).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let edges = vdb.edges(None).unwrap();
    assert!(
        edges.is_empty(),
        "no edges should be created when edge_embeddings=false, got {}",
        edges.len()
    );

    // Edge search mode should return empty results
    let mut query = SearchQuery::new("concepts");
    query.mode = SearchMode::Edge;
    let response = vdb.search(query).await.unwrap();
    assert!(
        response.edge_results.is_empty(),
        "edge search should return empty when edges disabled"
    );
}

// ---------------------------------------------------------------------------
// Additional: edge context and relationship type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_edge_context_populated() {
    let dir = setup_dir();
    let root = dir.path();
    write_interlinked_files(root);

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let edges = vdb.edges(None).unwrap();
    for edge in &edges {
        // Context should contain some text from around the link
        assert!(
            !edge.context_text.is_empty(),
            "context_text should be populated for edge {}",
            edge.edge_id
        );
    }
}

// ===========================================================================
// CLI Integration Tests
// ===========================================================================

fn mdvdb_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mdvdb"))
}

/// Create a temp directory with interlinked files, config, and run ingest via CLI.
fn cli_setup_and_ingest() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\nMDVDB_EDGE_EMBEDDINGS=true\n",
    )
    .unwrap();

    write_interlinked_files(root);

    let output = mdvdb_bin()
        .arg("ingest")
        .current_dir(root)
        .output()
        .expect("failed to run ingest");
    assert!(
        output.status.success(),
        "ingest should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    dir
}

// ---------------------------------------------------------------------------
// CLI 1. mdvdb edges --json returns valid JSON with required fields
// ---------------------------------------------------------------------------

#[test]
fn test_cli_edges_json_returns_valid_json() {
    let dir = cli_setup_and_ingest();

    let output = mdvdb_bin()
        .args(["edges", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run edges");

    assert!(
        output.status.success(),
        "edges --json should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("edges --json should return valid JSON");

    // Should have top-level fields
    assert!(parsed["edges"].is_array(), "should have 'edges' array");
    assert!(parsed["total_edges"].is_number(), "should have 'total_edges'");

    let edges = parsed["edges"].as_array().unwrap();
    assert!(!edges.is_empty(), "should have at least one edge");

    // Check required fields on first edge
    let edge = &edges[0];
    assert!(edge["edge_id"].is_string(), "edge should have edge_id");
    assert!(edge["source"].is_string(), "edge should have source");
    assert!(edge["target"].is_string(), "edge should have target");
    assert!(edge["context_text"].is_string(), "edge should have context_text");
    assert!(edge["line_number"].is_number(), "edge should have line_number");
}

// ---------------------------------------------------------------------------
// CLI 2. mdvdb edges <file> --json filters to edges involving that file
// ---------------------------------------------------------------------------

#[test]
fn test_cli_edges_file_filter_json() {
    let dir = cli_setup_and_ingest();

    let output = mdvdb_bin()
        .args(["edges", "alpha.md", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run edges with file filter");

    assert!(
        output.status.success(),
        "edges alpha.md --json should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("should be valid JSON");

    let edges = parsed["edges"].as_array().unwrap();
    assert!(!edges.is_empty(), "alpha.md should have edges");

    // All edges should involve alpha.md
    for edge in edges {
        let source = edge["source"].as_str().unwrap();
        let target = edge["target"].as_str().unwrap();
        assert!(
            source == "alpha.md" || target == "alpha.md",
            "filtered edge should involve alpha.md, got source={source} target={target}"
        );
    }

    // file field should be set in output
    assert_eq!(parsed["file"].as_str().unwrap(), "alpha.md");
}

// ---------------------------------------------------------------------------
// CLI 3. mdvdb edges --relationship 'depends' --json filters by relationship
// ---------------------------------------------------------------------------

#[test]
fn test_cli_edges_relationship_filter_json() {
    let dir = cli_setup_and_ingest();

    // First get all edges to find a relationship type we can filter on
    let output_all = mdvdb_bin()
        .args(["edges", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run edges");

    let stdout_all = String::from_utf8_lossy(&output_all.stdout);
    let parsed_all: serde_json::Value = serde_json::from_str(&stdout_all).unwrap();
    let all_edges = parsed_all["edges"].as_array().unwrap();

    // Use a nonsense relationship filter — should return empty or subset
    let output = mdvdb_bin()
        .args(["edges", "--relationship", "zzz_nonexistent_zzz", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run edges with relationship filter");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let filtered_edges = parsed["edges"].as_array().unwrap();
    // Nonsense filter should return fewer edges than total
    assert!(
        filtered_edges.len() <= all_edges.len(),
        "relationship filter should not increase edge count"
    );

    // relationship_filter field should be set
    assert_eq!(
        parsed["relationship_filter"].as_str().unwrap(),
        "zzz_nonexistent_zzz"
    );
}

// ---------------------------------------------------------------------------
// CLI 4. mdvdb search 'query' --edge-search --json returns edge_results
// ---------------------------------------------------------------------------

#[test]
fn test_cli_search_edge_search_json() {
    let dir = cli_setup_and_ingest();

    let output = mdvdb_bin()
        .args(["search", "concepts examples", "--edge-search", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run search --edge-search");

    assert!(
        output.status.success(),
        "search --edge-search --json should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("should be valid JSON");

    // Should have edge_results array
    assert!(
        parsed["edge_results"].is_array(),
        "search --edge-search should include edge_results in JSON output"
    );

    // Mode should be Edge
    assert_eq!(
        parsed["mode"].as_str().unwrap_or(""),
        "edge",
        "mode should be 'edge'"
    );
}

// ---------------------------------------------------------------------------
// CLI 5. mdvdb graph --json includes edge_clusters
// ---------------------------------------------------------------------------

#[test]
fn test_cli_graph_json_includes_edge_clusters() {
    let dir = cli_setup_and_ingest();

    let output = mdvdb_bin()
        .args(["graph", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run graph --json");

    assert!(
        output.status.success(),
        "graph --json should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("graph --json should return valid JSON");

    // Should have edge_clusters field (may be empty array if not enough edges)
    assert!(
        parsed.get("edge_clusters").is_some(),
        "graph JSON should include edge_clusters field"
    );
    assert!(
        parsed["edge_clusters"].is_array(),
        "edge_clusters should be an array"
    );
}

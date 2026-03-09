use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::links::{self, LinkState, NeighborhoodResult};
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
        search_decay_exclude: vec![],
        search_decay_include: vec![],
        search_boost_links: false,
        search_boost_hops: 1,
        search_expand_graph: 0,
        search_expand_limit: 3,
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

#[tokio::test]
async fn test_links_after_ingest() {
    let dir = setup_dir();
    let root = dir.path();

    // A links to B and C
    fs::write(
        root.join("a.md"),
        "# A\n\nLink to [B](b.md) and [C](c.md).\n",
    )
    .unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent of B.\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nContent of C.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.links("a.md").unwrap();
    assert_eq!(result.outgoing.len(), 2, "A should have 2 outgoing links");

    let targets: Vec<&str> = result.outgoing.iter().map(|r| r.entry.target.as_str()).collect();
    assert!(targets.contains(&"b.md"));
    assert!(targets.contains(&"c.md"));
}

#[tokio::test]
async fn test_backlinks_after_ingest() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nLink to [B](b.md).\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent of B.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let backlinks = vdb.backlinks("b.md").unwrap();
    assert_eq!(backlinks.len(), 1, "B should have 1 backlink");
    assert_eq!(backlinks[0].entry.source, "a.md");
}

#[tokio::test]
async fn test_broken_links() {
    let dir = setup_dir();
    let root = dir.path();

    // A links to nonexistent file
    fs::write(
        root.join("a.md"),
        "# A\n\nLink to [missing](nonexistent.md).\n",
    )
    .unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.links("a.md").unwrap();
    assert_eq!(result.outgoing.len(), 1);
    assert_eq!(result.outgoing[0].state, LinkState::Broken);
    assert_eq!(result.outgoing[0].entry.target, "nonexistent.md");
}

#[tokio::test]
async fn test_orphans() {
    let dir = setup_dir();
    let root = dir.path();

    // A links to B; C has no links at all
    fs::write(root.join("a.md"), "# A\n\nLink to [B](b.md).\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent.\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nOrphan content.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let orphans = vdb.orphans().unwrap();
    let orphan_paths: Vec<&str> = orphans.iter().map(|o| o.path.as_str()).collect();
    assert!(orphan_paths.contains(&"c.md"), "C should be an orphan, got: {orphan_paths:?}");
    assert!(!orphan_paths.contains(&"a.md"), "A should not be an orphan");
    assert!(!orphan_paths.contains(&"b.md"), "B should not be an orphan (has incoming)");
}

#[tokio::test]
async fn test_wikilinks() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(
        root.join("a.md"),
        "# A\n\nLink to [[page-name]].\n",
    )
    .unwrap();
    fs::write(root.join("page-name.md"), "# Page Name\n\nContent.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.links("a.md").unwrap();
    assert_eq!(result.outgoing.len(), 1);
    assert_eq!(result.outgoing[0].entry.target, "page-name.md");
    assert_eq!(result.outgoing[0].state, LinkState::Valid);
    assert!(result.outgoing[0].entry.is_wikilink);
}

#[tokio::test]
async fn test_link_graph_persistence() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nLink to [B](b.md).\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent.\n").unwrap();

    // Ingest, then drop the VDB
    {
        let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
        vdb.ingest(IngestOptions::default()).await.unwrap();
        let result = vdb.links("a.md").unwrap();
        assert_eq!(result.outgoing.len(), 1);
    }

    // Reopen and verify links are still available
    let vdb2 = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    let result = vdb2.links("a.md").unwrap();
    assert_eq!(result.outgoing.len(), 1, "link graph should persist across reopen");
    assert_eq!(result.outgoing[0].entry.target, "b.md");
}

#[tokio::test]
async fn test_incremental_link_update() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nLink to [B](b.md).\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent.\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nContent.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Verify initial state
    let result = vdb.links("a.md").unwrap();
    assert_eq!(result.outgoing.len(), 1);
    assert_eq!(result.outgoing[0].entry.target, "b.md");

    // Modify a.md to link to C instead
    fs::write(root.join("a.md"), "# A\n\nLink to [C](c.md).\n").unwrap();

    // Single-file incremental ingest
    let opts = IngestOptions {
        full: false,
        file: Some(PathBuf::from("a.md")),
        ..Default::default()
    };
    vdb.ingest(opts).await.unwrap();

    let result = vdb.links("a.md").unwrap();
    assert_eq!(result.outgoing.len(), 1, "should have 1 link after update");
    assert_eq!(result.outgoing[0].entry.target, "c.md", "link should point to c.md now");
}

#[test]
fn test_empty_link_graph_before_ingest() {
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nContent.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();

    // Before ingest, links should return an error
    let result = vdb.links("a.md");
    assert!(result.is_err(), "should error when no link graph exists");
}

#[tokio::test]
async fn test_bidirectional_links() {
    let dir = setup_dir();
    let root = dir.path();

    // A links to B, B links to A
    fs::write(root.join("a.md"), "# A\n\nLink to [B](b.md).\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nLink to [A](a.md).\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // A has outgoing to B and incoming from B
    let result_a = vdb.links("a.md").unwrap();
    assert_eq!(result_a.outgoing.len(), 1);
    assert_eq!(result_a.outgoing[0].entry.target, "b.md");
    assert_eq!(result_a.incoming.len(), 1);
    assert_eq!(result_a.incoming[0].source, "b.md");

    // B has outgoing to A and incoming from A
    let result_b = vdb.links("b.md").unwrap();
    assert_eq!(result_b.outgoing.len(), 1);
    assert_eq!(result_b.outgoing[0].entry.target, "a.md");
    assert_eq!(result_b.incoming.len(), 1);
    assert_eq!(result_b.incoming[0].source, "a.md");
}

#[tokio::test]
async fn test_self_link_excluded() {
    let dir = setup_dir();
    let root = dir.path();

    // A links to itself
    fs::write(root.join("a.md"), "# A\n\nLink to [self](a.md).\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.links("a.md").unwrap();
    assert!(result.outgoing.is_empty(), "self-links should be excluded");
}

// ---------------------------------------------------------------------------
// BFS & Neighborhood integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bfs_neighbors_integration() {
    // Build a chain A -> B -> C -> D and verify BFS at depth 1/2/3
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nLink to [B](b.md).\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nLink to [C](c.md).\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nLink to [D](d.md).\n").unwrap();
    fs::write(root.join("d.md"), "# D\n\nContent.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Prove the link graph exists after ingest
    let _graph = vdb.links("a.md").unwrap();

    // Reconstruct graph for BFS via parsing (same structure as ingest builds)
    let link_graph = {
        let files = vec![
            mdvdb::parser::parse_markdown_file(root, &PathBuf::from("a.md")).unwrap(),
            mdvdb::parser::parse_markdown_file(root, &PathBuf::from("b.md")).unwrap(),
            mdvdb::parser::parse_markdown_file(root, &PathBuf::from("c.md")).unwrap(),
            mdvdb::parser::parse_markdown_file(root, &PathBuf::from("d.md")).unwrap(),
        ];
        links::build_link_graph(&files)
    };
    let backlinks = links::compute_backlinks(&link_graph);

    // Depth 1: only B reachable from A
    let n1 = links::bfs_neighbors(&link_graph, &backlinks, &["a.md".to_string()], 1);
    assert_eq!(n1.len(), 1, "depth 1: only B reachable");
    assert_eq!(n1["b.md"], 1);

    // Depth 2: B and C reachable
    let n2 = links::bfs_neighbors(&link_graph, &backlinks, &["a.md".to_string()], 2);
    assert_eq!(n2.len(), 2, "depth 2: B and C reachable");
    assert_eq!(n2["b.md"], 1);
    assert_eq!(n2["c.md"], 2);

    // Depth 3: B, C, and D reachable
    let n3 = links::bfs_neighbors(&link_graph, &backlinks, &["a.md".to_string()], 3);
    assert_eq!(n3.len(), 3, "depth 3: B, C, and D reachable");
    assert_eq!(n3["b.md"], 1);
    assert_eq!(n3["c.md"], 2);
    assert_eq!(n3["d.md"], 3);
}

#[tokio::test]
async fn bfs_bidirectional_integration() {
    // Verify backward edges (backlinks) are traversed during BFS.
    // A -> B, C -> A. From A: B is forward hop 1, C is backlink hop 1.
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nLink to [B](b.md).\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nContent.\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nLink to [A](a.md).\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let files = vec![
        mdvdb::parser::parse_markdown_file(root, &PathBuf::from("a.md")).unwrap(),
        mdvdb::parser::parse_markdown_file(root, &PathBuf::from("b.md")).unwrap(),
        mdvdb::parser::parse_markdown_file(root, &PathBuf::from("c.md")).unwrap(),
    ];
    let link_graph = links::build_link_graph(&files);
    let backlinks = links::compute_backlinks(&link_graph);

    let neighbors = links::bfs_neighbors(&link_graph, &backlinks, &["a.md".to_string()], 1);

    // B via forward link, C via backlink (C links to A)
    assert_eq!(neighbors.len(), 2, "should find B (forward) and C (backlink)");
    assert_eq!(neighbors["b.md"], 1, "B at hop 1 via forward link");
    assert_eq!(neighbors["c.md"], 1, "C at hop 1 via backlink");
}

#[tokio::test]
async fn neighborhood_depth_1_integration() {
    // Verify tree output at depth 1 matches outgoing links via MarkdownVdb API.
    let dir = setup_dir();
    let root = dir.path();

    fs::write(
        root.join("hub.md"),
        "# Hub\n\nLink to [X](x.md) and [Y](y.md).\n",
    )
    .unwrap();
    fs::write(root.join("x.md"), "# X\n\nLink to [Z](z.md).\n").unwrap();
    fs::write(root.join("y.md"), "# Y\n\nContent.\n").unwrap();
    fs::write(root.join("z.md"), "# Z\n\nContent.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.links_neighborhood("hub.md", 1).unwrap();

    assert_eq!(result.file, "hub.md");

    // Outgoing at depth 1: x.md and y.md (no children since depth=1)
    assert_eq!(result.outgoing.len(), 2, "hub has 2 outgoing links");
    let out_paths: Vec<&str> = result.outgoing.iter().map(|n| n.path.as_str()).collect();
    assert!(out_paths.contains(&"x.md"), "should include x.md");
    assert!(out_paths.contains(&"y.md"), "should include y.md");

    for node in &result.outgoing {
        assert_eq!(node.state, LinkState::Valid);
        assert!(node.children.is_empty(), "depth 1 should have no children");
    }

    assert_eq!(result.outgoing_count, 2);
    assert_eq!(result.outgoing_depth_count, 1);
}

#[tokio::test]
async fn neighborhood_depth_2_integration() {
    // Verify tree at depth 2 has correct children.
    // hub -> x -> z, hub -> y (leaf)
    let dir = setup_dir();
    let root = dir.path();

    fs::write(
        root.join("hub.md"),
        "# Hub\n\nLink to [X](x.md) and [Y](y.md).\n",
    )
    .unwrap();
    fs::write(root.join("x.md"), "# X\n\nLink to [Z](z.md).\n").unwrap();
    fs::write(root.join("y.md"), "# Y\n\nContent of Y.\n").unwrap();
    fs::write(root.join("z.md"), "# Z\n\nContent of Z.\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.links_neighborhood("hub.md", 2).unwrap();

    assert_eq!(result.file, "hub.md");
    assert_eq!(result.outgoing.len(), 2, "hub has 2 direct outgoing links");

    // Find x.md node — it should have z.md as a child
    let x_node = result.outgoing.iter().find(|n| n.path == "x.md").unwrap();
    assert_eq!(x_node.state, LinkState::Valid);
    assert_eq!(
        x_node.children.len(),
        1,
        "x.md at depth 2 should have z.md as child"
    );
    assert_eq!(x_node.children[0].path, "z.md");
    assert_eq!(x_node.children[0].state, LinkState::Valid);
    assert!(
        x_node.children[0].children.is_empty(),
        "z.md has no further children"
    );

    // Find y.md node — it's a leaf, no outgoing links
    let y_node = result.outgoing.iter().find(|n| n.path == "y.md").unwrap();
    assert_eq!(y_node.state, LinkState::Valid);
    assert!(y_node.children.is_empty(), "y.md is a leaf node");

    // 3 total nodes: x, y, z
    assert_eq!(result.outgoing_count, 3);
    assert_eq!(result.outgoing_depth_count, 2);
}

#[tokio::test]
async fn neighborhood_handles_cycles() {
    // A -> B -> C -> A cycle should not cause infinite recursion.
    let dir = setup_dir();
    let root = dir.path();

    fs::write(root.join("a.md"), "# A\n\nLink to [B](b.md).\n").unwrap();
    fs::write(root.join("b.md"), "# B\n\nLink to [C](c.md).\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nLink to [A](a.md).\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // This should complete without hanging (cycle-safe)
    let result = vdb.links_neighborhood("a.md", 3).unwrap();

    assert_eq!(result.file, "a.md");

    // Outgoing: a -> b -> c (c -> a is cycle, skipped because 'a' is root)
    assert_eq!(result.outgoing.len(), 1, "a has 1 outgoing link (b)");
    assert_eq!(result.outgoing[0].path, "b.md");
    assert_eq!(result.outgoing[0].children.len(), 1, "b has 1 child (c)");
    assert_eq!(result.outgoing[0].children[0].path, "c.md");
    // c -> a would be a cycle, so c has no children
    assert!(
        result.outgoing[0].children[0].children.is_empty(),
        "c -> a cycle should be broken"
    );

    // Incoming: c -> a (backlink), then c's incoming: b -> c, then b's incoming: a -> b (cycle)
    assert_eq!(result.incoming.len(), 1, "a has 1 incoming link (c)");
    assert_eq!(result.incoming[0].path, "c.md");
    assert_eq!(result.incoming[0].children.len(), 1, "c's incoming: b");
    assert_eq!(result.incoming[0].children[0].path, "b.md");
    // b's incoming backlink is a -> b, but a is on the branch, so skipped
    assert!(
        result.incoming[0].children[0].children.is_empty(),
        "b -> a cycle should be broken on incoming side"
    );
}

#[tokio::test]
async fn neighborhood_serialization() {
    // NeighborhoodResult should serialize to JSON correctly.
    let dir = setup_dir();
    let root = dir.path();

    fs::write(
        root.join("a.md"),
        "# A\n\nLink to [B](b.md) and [C](c.md).\n",
    )
    .unwrap();
    fs::write(root.join("b.md"), "# B\n\nLink to [D](d.md).\n").unwrap();
    fs::write(root.join("c.md"), "# C\n\nContent.\n").unwrap();
    fs::write(root.join("d.md"), "# D\n\nContent.\n").unwrap();
    // e links to a so it appears in incoming
    fs::write(root.join("e.md"), "# E\n\nLink to [A](a.md).\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.links_neighborhood("a.md", 2).unwrap();

    // Serialize to JSON
    let json_str = serde_json::to_string(&result).expect("NeighborhoodResult should serialize");
    let json: serde_json::Value =
        serde_json::from_str(&json_str).expect("serialized JSON should be valid");

    // Verify top-level structure
    assert_eq!(json["file"], "a.md");
    assert!(json["outgoing"].is_array(), "outgoing should be an array");
    assert!(json["incoming"].is_array(), "incoming should be an array");
    assert!(
        json["outgoing_count"].is_number(),
        "outgoing_count should be a number"
    );
    assert!(
        json["incoming_count"].is_number(),
        "incoming_count should be a number"
    );
    assert!(
        json["outgoing_depth_count"].is_number(),
        "outgoing_depth_count should be a number"
    );
    assert!(
        json["incoming_depth_count"].is_number(),
        "incoming_depth_count should be a number"
    );

    // Verify node structure
    let outgoing = json["outgoing"].as_array().unwrap();
    assert!(!outgoing.is_empty(), "should have outgoing nodes");
    for node in outgoing {
        assert!(node["path"].is_string(), "node should have 'path' string");
        assert!(node["state"].is_string(), "node should have 'state' string");
        assert!(
            node["children"].is_array(),
            "node should have 'children' array"
        );
    }

    // Verify that at least one outgoing node has children (b.md -> d.md at depth 2)
    let b_node = outgoing.iter().find(|n| n["path"] == "b.md");
    assert!(b_node.is_some(), "b.md should be in outgoing nodes");
    let b_children = b_node.unwrap()["children"].as_array().unwrap();
    assert_eq!(
        b_children.len(),
        1,
        "b.md should have 1 child (d.md) at depth 2"
    );
    assert_eq!(b_children[0]["path"], "d.md");

    // Verify incoming has e.md
    let incoming = json["incoming"].as_array().unwrap();
    assert!(!incoming.is_empty(), "should have incoming nodes");
    let e_node = incoming.iter().find(|n| n["path"] == "e.md");
    assert!(e_node.is_some(), "e.md should be in incoming nodes");

    // Verify state values are valid strings
    for node in outgoing {
        let state = node["state"].as_str().unwrap();
        assert!(
            state == "Valid" || state == "Broken",
            "state should be Valid or Broken, got: {}",
            state
        );
    }

    // Roundtrip: deserialize back and check equality
    let deserialized: NeighborhoodResult =
        serde_json::from_str(&json_str).expect("should deserialize back to NeighborhoodResult");
    assert_eq!(deserialized.file, result.file);
    assert_eq!(deserialized.outgoing_count, result.outgoing_count);
    assert_eq!(deserialized.incoming_count, result.incoming_count);
    assert_eq!(deserialized.outgoing_depth_count, result.outgoing_depth_count);
    assert_eq!(deserialized.incoming_depth_count, result.incoming_depth_count);
}

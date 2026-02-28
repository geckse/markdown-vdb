use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::links::LinkState;
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

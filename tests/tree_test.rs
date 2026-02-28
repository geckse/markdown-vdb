use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::tree::FileState;
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
        index_file: PathBuf::from(".markdownvdb.index"),
        ignore_patterns: vec![],
        watch_enabled: false,
        watch_debounce_ms: 300,
        chunk_max_tokens: 512,
        chunk_overlap_tokens: 50,
        clustering_enabled: false,
        clustering_rebalance_threshold: 50,
        search_default_limit: 10,
        search_min_score: 0.0,
        fts_index_dir: PathBuf::from(".markdownvdb.fts"),
        search_default_mode: mdvdb::SearchMode::Hybrid,
        search_rrf_k: 60.0,
    }
}

fn setup_project() -> (TempDir, MarkdownVdb) {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(
        root.join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    fs::write(
        root.join("hello.md"),
        "---\ntitle: Hello World\n---\n\n# Hello\n\nThis is a greeting document.\n",
    )
    .unwrap();

    fs::write(
        root.join("rust.md"),
        "---\ntitle: Rust Guide\n---\n\n# Rust\n\nRust is a systems programming language.\n",
    )
    .unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    (dir, vdb)
}

fn setup_nested_project() -> (TempDir, MarkdownVdb) {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(
        root.join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    fs::write(
        root.join("readme.md"),
        "---\ntitle: README\n---\n\n# README\n\nProject overview.\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("docs/api")).unwrap();
    fs::write(
        root.join("docs/guide.md"),
        "---\ntitle: Guide\n---\n\n# Guide\n\nUser guide content.\n",
    )
    .unwrap();

    fs::write(
        root.join("docs/api/auth.md"),
        "---\ntitle: Auth API\n---\n\n# Auth\n\nAuthentication API docs.\n",
    )
    .unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    (dir, vdb)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_file_tree_empty_index() {
    let (_dir, vdb) = setup_project();

    let tree = vdb.file_tree().unwrap();

    // Before ingest, all files should be New
    assert_eq!(tree.new_count, tree.total_files);
    assert_eq!(tree.indexed_count, 0);
    assert_eq!(tree.modified_count, 0);
    assert_eq!(tree.deleted_count, 0);
    assert!(tree.total_files > 0, "should discover files on disk");

    // Verify all leaf nodes have New state
    fn assert_all_new(node: &mdvdb::FileTreeNode) {
        if !node.is_dir {
            assert_eq!(
                node.state,
                Some(FileState::New),
                "file {} should be New before ingest",
                node.name,
            );
        }
        for child in &node.children {
            assert_all_new(child);
        }
    }
    assert_all_new(&tree.root);
}

#[tokio::test]
async fn test_file_tree_after_ingest() {
    let (_dir, vdb) = setup_project();

    vdb.ingest(IngestOptions::default()).await.unwrap();

    let tree = vdb.file_tree().unwrap();

    assert_eq!(tree.indexed_count, tree.total_files);
    assert_eq!(tree.new_count, 0);
    assert_eq!(tree.modified_count, 0);
    assert_eq!(tree.deleted_count, 0);
}

#[tokio::test]
async fn test_file_tree_modified_file() {
    let (dir, vdb) = setup_project();

    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Modify a file after ingest
    fs::write(
        dir.path().join("hello.md"),
        "---\ntitle: Hello Updated\n---\n\n# Hello\n\nThis content has been modified.\n",
    )
    .unwrap();

    let tree = vdb.file_tree().unwrap();

    assert_eq!(tree.modified_count, 1);
    assert_eq!(tree.indexed_count, tree.total_files - 1);
}

#[tokio::test]
async fn test_file_tree_deleted_file() {
    let (dir, vdb) = setup_project();

    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Delete a file after ingest
    fs::remove_file(dir.path().join("hello.md")).unwrap();

    let tree = vdb.file_tree().unwrap();

    assert_eq!(tree.deleted_count, 1);
    // The deleted file is still counted in total
    assert!(tree.total_files >= 2);

    // Find the deleted node
    fn find_deleted(node: &mdvdb::FileTreeNode) -> bool {
        if node.state == Some(FileState::Deleted) {
            return true;
        }
        node.children.iter().any(find_deleted)
    }
    assert!(find_deleted(&tree.root), "should have a deleted file node");
}

#[tokio::test]
async fn test_file_tree_new_file() {
    let (dir, vdb) = setup_project();

    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Add a new file after ingest
    fs::write(
        dir.path().join("new_doc.md"),
        "---\ntitle: New Document\n---\n\n# New\n\nBrand new content.\n",
    )
    .unwrap();

    let tree = vdb.file_tree().unwrap();

    assert_eq!(tree.new_count, 1);
    assert!(tree.total_files > 2);
}

#[tokio::test]
async fn test_file_tree_nested_dirs() {
    let (_dir, vdb) = setup_nested_project();

    vdb.ingest(IngestOptions::default()).await.unwrap();

    let tree = vdb.file_tree().unwrap();

    // Should have docs/ directory with children
    let docs = tree
        .root
        .children
        .iter()
        .find(|c| c.is_dir && c.name == "docs");
    assert!(docs.is_some(), "should have docs/ directory");

    let docs = docs.unwrap();
    // docs should contain api/ dir and guide.md
    let api = docs.children.iter().find(|c| c.is_dir && c.name == "api");
    assert!(api.is_some(), "should have docs/api/ directory");

    let guide = docs.children.iter().find(|c| c.name == "guide.md");
    assert!(guide.is_some(), "should have docs/guide.md");

    // api should contain auth.md
    let api = api.unwrap();
    let auth = api.children.iter().find(|c| c.name == "auth.md");
    assert!(auth.is_some(), "should have docs/api/auth.md");

    // Verify sorting: dirs before files
    let first_dir_idx = docs
        .children
        .iter()
        .position(|c| c.is_dir)
        .unwrap_or(usize::MAX);
    let first_file_idx = docs
        .children
        .iter()
        .position(|c| !c.is_dir)
        .unwrap_or(usize::MAX);
    assert!(
        first_dir_idx < first_file_idx,
        "directories should sort before files"
    );
}

#[tokio::test]
async fn test_file_tree_json_serialization() {
    let (_dir, vdb) = setup_project();

    vdb.ingest(IngestOptions::default()).await.unwrap();

    let tree = vdb.file_tree().unwrap();

    // Serialize to JSON and verify structure
    let json = serde_json::to_value(&tree).unwrap();

    assert!(json.get("root").is_some(), "JSON should have 'root' field");
    assert!(json.get("total_files").is_some(), "JSON should have 'total_files'");
    assert!(json.get("indexed_count").is_some(), "JSON should have 'indexed_count'");
    assert!(json.get("modified_count").is_some(), "JSON should have 'modified_count'");
    assert!(json.get("new_count").is_some(), "JSON should have 'new_count'");
    assert!(json.get("deleted_count").is_some(), "JSON should have 'deleted_count'");

    let root = json.get("root").unwrap();
    assert_eq!(root.get("name").unwrap(), ".");
    assert_eq!(root.get("is_dir").unwrap(), true);
    assert!(root.get("children").unwrap().is_array());

    // Verify file node has state
    let children = root.get("children").unwrap().as_array().unwrap();
    assert!(!children.is_empty());
    let file_node = children.iter().find(|c| !c.get("is_dir").unwrap().as_bool().unwrap()).unwrap();
    assert!(file_node.get("state").is_some());
    assert_eq!(file_node.get("state").unwrap(), "indexed");

    // Round-trip: parse back to string and ensure it's valid JSON
    let json_str = serde_json::to_string(&tree).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["total_files"], tree.total_files);
}

use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::error::Error;
use mdvdb::search::SearchQuery;
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
    }
}

/// Create a temp directory with a `.markdownvdb` config and some markdown files.
fn setup_project() -> (TempDir, MarkdownVdb) {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Write .markdownvdb config (needed for Config::load to find project root)
    fs::write(
        root.join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    // Create some markdown files
    fs::write(
        root.join("hello.md"),
        "---\ntitle: Hello World\nstatus: published\n---\n\n# Hello\n\nThis is a test document about greetings.\n",
    )
    .unwrap();

    fs::write(
        root.join("rust.md"),
        "---\ntitle: Rust Guide\nstatus: draft\n---\n\n# Rust\n\nRust is a systems programming language.\n\n## Memory Safety\n\nRust ensures memory safety without garbage collection.\n",
    )
    .unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    (dir, vdb)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_open_with_mock_config() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(".markdownvdb"), "MDVDB_EMBEDDING_PROVIDER=mock\n").unwrap();

    let vdb = MarkdownVdb::open_with_config(dir.path().to_path_buf(), mock_config());
    assert!(vdb.is_ok(), "should open with mock config: {:?}", vdb.err());
}

#[test]
fn test_status_returns_counts() {
    let (_dir, vdb) = setup_project();
    let status = vdb.status();

    // Before ingest, should be empty
    assert_eq!(status.document_count, 0);
    assert_eq!(status.chunk_count, 0);
    assert_eq!(status.vector_count, 0);
    assert_eq!(status.embedding_config.dimensions, DIMS);
}

#[tokio::test]
async fn test_ingest_populates_index() {
    let (_dir, vdb) = setup_project();

    let result = vdb.ingest(IngestOptions::default()).await.unwrap();

    assert!(result.files_indexed > 0, "should have indexed files");
    assert_eq!(result.files_failed, 0, "no files should fail");

    let status = vdb.status();
    assert!(status.document_count > 0, "should have documents after ingest");
    assert!(status.chunk_count > 0, "should have chunks after ingest");
    assert!(status.vector_count > 0, "should have vectors after ingest");
}

#[tokio::test]
async fn test_search_returns_results() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("rust programming");
    let results = vdb.search(query).await.unwrap();

    assert!(!results.is_empty(), "search should return results after ingest");
    let r = &results[0];
    assert!(r.score > 0.0, "results should have positive scores");
    assert!(!r.chunk.content.is_empty(), "results should have content");
}

#[tokio::test]
async fn test_get_document_returns_info() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let doc = vdb.get_document("hello.md").unwrap();
    assert_eq!(doc.path, "hello.md");
    assert!(doc.chunk_count > 0, "indexed doc should have chunks");
    assert!(doc.file_size > 0, "should have file size");
    assert!(!doc.content_hash.is_empty(), "should have content hash");
}

#[tokio::test]
async fn test_get_document_missing_returns_error() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.get_document("nonexistent.md");
    assert!(result.is_err(), "should return error for missing file");

    match result.unwrap_err() {
        Error::FileNotInIndex { path } => {
            assert_eq!(path, PathBuf::from("nonexistent.md"));
        }
        other => panic!("expected FileNotInIndex error, got: {other}"),
    }
}

#[tokio::test]
async fn test_schema_returns_fields_after_ingest() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let schema = vdb.schema().unwrap();
    let fields = &schema.fields;

    // Our markdown files have "title" and "status" frontmatter
    assert!(!fields.is_empty(), "schema should have fields after ingest");

    let field_names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    assert!(
        field_names.contains(&"title"),
        "schema should contain 'title' field, got: {field_names:?}"
    );
    assert!(
        field_names.contains(&"status"),
        "schema should contain 'status' field, got: {field_names:?}"
    );
}

/// Setup project with clustering enabled.
fn setup_project_with_clustering() -> (TempDir, MarkdownVdb) {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(
        root.join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    // Create several markdown files to get meaningful clusters.
    for i in 0..5 {
        fs::write(
            root.join(format!("doc{i}.md")),
            format!(
                "---\ntitle: Document {i}\n---\n\n# Document {i}\n\nContent of document number {i} with some text.\n"
            ),
        )
        .unwrap();
    }

    let mut config = mock_config();
    config.clustering_enabled = true;
    config.clustering_rebalance_threshold = 50;

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), config).unwrap();
    (dir, vdb)
}

#[tokio::test]
async fn test_clusters_returns_data_after_ingest() {
    let (_dir, vdb) = setup_project_with_clustering();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let clusters = vdb.clusters().unwrap();
    assert!(!clusters.is_empty(), "should have clusters after ingest with clustering enabled");

    // All documents should be distributed across clusters.
    let total_docs: usize = clusters.iter().map(|c| c.document_count).sum();
    assert_eq!(total_docs, 5, "all 5 docs should be in clusters");

    // Each cluster should have a label.
    for cluster in &clusters {
        assert!(cluster.label.is_some(), "cluster should have a label");
    }
}

#[tokio::test]
async fn test_get_document_returns_frontmatter() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let doc = vdb.get_document("hello.md").unwrap();
    assert!(doc.frontmatter.is_some(), "should have frontmatter");
    let fm = doc.frontmatter.unwrap();
    assert_eq!(fm["title"], "Hello World");
    assert_eq!(fm["status"], "published");
}

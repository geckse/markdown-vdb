use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::error::Error;
use mdvdb::search::SearchQuery;
use mdvdb::{CheckStatus, IngestOptions, MarkdownVdb, SearchMode};
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

/// Create a temp directory with a `.markdownvdb` config and some markdown files.
fn setup_project() -> (TempDir, MarkdownVdb) {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Write .markdownvdb/.config (needed for Config::load to find project root)
    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
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
    fs::create_dir_all(dir.path().join(".markdownvdb")).unwrap();
    fs::write(dir.path().join(".markdownvdb").join(".config"), "MDVDB_EMBEDDING_PROVIDER=mock\n").unwrap();

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

// ---------------------------------------------------------------------------
// Link graph API tests
// ---------------------------------------------------------------------------

/// Setup project with files that link to each other.
fn setup_project_with_links() -> (TempDir, MarkdownVdb) {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(
        root.join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    fs::write(
        root.join("alpha.md"),
        "---\ntitle: Alpha\n---\n\n# Alpha\n\nLinks to [Beta](beta.md) and [Gamma](gamma.md).\n",
    )
    .unwrap();

    fs::write(
        root.join("beta.md"),
        "---\ntitle: Beta\n---\n\n# Beta\n\nLinks back to [Alpha](alpha.md).\n",
    )
    .unwrap();

    fs::write(
        root.join("gamma.md"),
        "---\ntitle: Gamma\n---\n\n# Gamma\n\nNo outgoing links here.\n",
    )
    .unwrap();

    fs::write(
        root.join("orphan.md"),
        "---\ntitle: Orphan\n---\n\n# Orphan\n\nThis file has no links at all.\n",
    )
    .unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    (dir, vdb)
}

#[tokio::test]
async fn test_links_api() {
    let (_dir, vdb) = setup_project_with_links();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.links("alpha.md").unwrap();
    assert_eq!(result.file, "alpha.md");
    assert!(result.outgoing.len() >= 2, "alpha.md should have at least 2 outgoing links");

    // Check that beta.md and gamma.md are among targets
    let targets: Vec<&str> = result.outgoing.iter().map(|l| l.entry.target.as_str()).collect();
    assert!(targets.contains(&"beta.md"), "should link to beta.md, got: {targets:?}");
    assert!(targets.contains(&"gamma.md"), "should link to gamma.md, got: {targets:?}");
}

#[tokio::test]
async fn test_orphans_api() {
    let (_dir, vdb) = setup_project_with_links();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let orphans = vdb.orphans().unwrap();
    let paths: Vec<&str> = orphans.iter().map(|o| o.path.as_str()).collect();
    assert!(paths.contains(&"orphan.md"), "orphan.md should be in orphans list, got: {paths:?}");
}

// ---------------------------------------------------------------------------
// Clustering tests
// ---------------------------------------------------------------------------

/// Setup project with clustering enabled.
fn setup_project_with_clustering() -> (TempDir, MarkdownVdb) {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
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

// ---------------------------------------------------------------------------
// Hybrid / FTS search API tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_hybrid_mode_via_api() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("rust programming").with_mode(SearchMode::Hybrid);
    let results = vdb.search(query).await.unwrap();

    assert!(!results.is_empty(), "hybrid search should return results");
    assert!(results[0].score > 0.0, "results should have positive scores");
}

#[tokio::test]
async fn test_search_semantic_mode_via_api() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("rust").with_mode(SearchMode::Semantic);
    let results = vdb.search(query).await.unwrap();

    assert!(!results.is_empty(), "semantic search should return results");
}

#[tokio::test]
async fn test_search_lexical_mode_via_api() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("systems programming language").with_mode(SearchMode::Lexical);
    let results = vdb.search(query).await.unwrap();

    assert!(!results.is_empty(), "lexical search should return results for matching terms");
}

#[tokio::test]
async fn test_fts_index_populated_after_ingest() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let fts = vdb.fts_index();
    let num = fts.num_docs().unwrap();
    assert!(num > 0, "FTS index should have documents after ingest, got {num}");
}

#[tokio::test]
async fn test_search_default_mode_is_hybrid() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Default SearchQuery should use hybrid (from config)
    let query = SearchQuery::new("rust");
    let results = vdb.search(query).await.unwrap();

    assert!(!results.is_empty(), "default mode search should return results");
}

#[tokio::test]
async fn test_fts_auto_rebuild_from_rkyv() {
    let (_dir, vdb) = setup_project();

    // First ingest: populates both vector and FTS indexes.
    vdb.ingest(IngestOptions::default()).await.unwrap();
    let fts_docs_before = vdb.fts_index().num_docs().unwrap();
    assert!(fts_docs_before > 0, "FTS should have docs after initial ingest");

    // Simulate a stale FTS index by deleting all FTS docs.
    vdb.fts_index().delete_all().unwrap();
    vdb.fts_index().commit().unwrap();
    assert_eq!(vdb.fts_index().num_docs().unwrap(), 0, "FTS should be empty after delete_all");

    // Re-ingest (incremental — files unchanged, so vector skips them).
    // Consistency guard should detect FTS=0 + vector>0 and rebuild FTS.
    vdb.ingest(IngestOptions::default()).await.unwrap();
    let fts_docs_after = vdb.fts_index().num_docs().unwrap();
    assert!(
        fts_docs_after > 0,
        "FTS should have been rebuilt from rkyv metadata, got {fts_docs_after}"
    );
}

#[tokio::test]
async fn test_full_ingest_rebuilds_fts() {
    let (_dir, vdb) = setup_project();

    // Initial ingest.
    vdb.ingest(IngestOptions::default()).await.unwrap();
    let fts_before = vdb.fts_index().num_docs().unwrap();
    assert!(fts_before > 0, "FTS should have docs after ingest");

    // Full ingest: should clear and rebuild FTS.
    let opts = IngestOptions { full: true, file: None };
    vdb.ingest(opts).await.unwrap();
    let fts_after = vdb.fts_index().num_docs().unwrap();
    assert!(
        fts_after > 0,
        "FTS should have docs after full re-ingest, got {fts_after}"
    );
}

// ---------------------------------------------------------------------------
// File tree and path prefix API tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_file_tree_returns_structure() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    // Create files in subdirectories
    fs::create_dir_all(root.join("docs/guides")).unwrap();
    fs::write(
        root.join("readme.md"),
        "---\ntitle: Readme\n---\n\n# Readme\n\nTop-level readme.\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/overview.md"),
        "---\ntitle: Overview\n---\n\n# Overview\n\nDocs overview.\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/guides/start.md"),
        "---\ntitle: Getting Started\n---\n\n# Getting Started\n\nA guide.\n",
    )
    .unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let tree = vdb.file_tree().unwrap();

    // Should have entries covering our files
    assert!(tree.total_files > 0, "file tree should have files");
    assert!(tree.total_files >= 3, "should have at least 3 files, got {}", tree.total_files);
}

#[tokio::test]
async fn test_search_with_path_prefix() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(
        root.join("top.md"),
        "---\ntitle: Top\n---\n\n# Top\n\nTop-level content about programming.\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/guide.md"),
        "---\ntitle: Guide\n---\n\n# Guide\n\nA guide about programming.\n",
    )
    .unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Search scoped to docs/ directory
    let query = SearchQuery::new("programming").with_path_prefix("docs/");
    let results = vdb.search(query).await.unwrap();

    // All results should be within docs/
    for r in &results {
        assert!(
            r.file.path.starts_with("docs/"),
            "path-scoped result should be under docs/, got: {}",
            r.file.path
        );
    }
}

// ---------------------------------------------------------------------------
// Doctor tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_doctor_with_mock_provider() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.doctor().await.unwrap();

    // Mock provider should produce no Fail checks (no API key needed, provider always reachable).
    for check in &result.checks {
        assert_ne!(
            check.status,
            CheckStatus::Fail,
            "mock provider doctor should not have failures, but '{}' failed: {}",
            check.name,
            check.detail
        );
    }
    assert!(result.passed > 0, "should have passing checks");
    assert_eq!(result.passed + (result.total - result.passed), result.total);
}

#[tokio::test]
async fn test_doctor_reports_correct_counts() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.doctor().await.unwrap();

    // Find the Index check.
    let index_check = result
        .checks
        .iter()
        .find(|c| c.name == "Index")
        .expect("should have Index check");
    assert_eq!(index_check.status, CheckStatus::Pass);
    // Detail should mention document and chunk counts.
    assert!(
        index_check.detail.contains("docs") || index_check.detail.contains("chunks"),
        "Index detail should mention counts: {}",
        index_check.detail
    );
}

#[tokio::test]
async fn test_doctor_source_dirs_check() {
    let (_dir, vdb) = setup_project();

    let result = vdb.doctor().await.unwrap();

    let src_check = result
        .checks
        .iter()
        .find(|c| c.name == "Source directories")
        .expect("should have Source directories check");
    assert_eq!(src_check.status, CheckStatus::Pass);
    assert!(
        src_check.detail.contains(".md files"),
        "should mention .md files: {}",
        src_check.detail
    );
}

#[tokio::test]
async fn test_init_global_creates_and_rejects() {
    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config");

    // First call should succeed.
    MarkdownVdb::init_global(&config_path).unwrap();
    assert!(config_path.exists(), "config file should be created");

    let content = fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("OPENAI_API_KEY"), "template should mention API key");

    // Second call should fail.
    let result = MarkdownVdb::init_global(&config_path);
    assert!(matches!(result, Err(Error::ConfigAlreadyExists { .. })));
}

// ---------------------------------------------------------------------------
// HNSW key mismatch regression tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_works_after_full_reindex() {
    let (_dir, vdb) = setup_project();

    // Initial ingest and search
    vdb.ingest(IngestOptions::default()).await.unwrap();
    let query = SearchQuery::new("rust programming");
    let results1 = vdb.search(query).await.unwrap();
    assert!(!results1.is_empty(), "search should return results after first ingest");

    // Full reindex
    let opts = IngestOptions { full: true, file: None };
    vdb.ingest(opts).await.unwrap();

    // Search again — should still work
    let query2 = SearchQuery::new("rust programming");
    let results2 = vdb.search(query2).await.unwrap();
    assert!(!results2.is_empty(), "search should return results after full reindex");
    assert!(results2[0].score > 0.0, "results should have positive scores after reindex");
}

#[tokio::test]
async fn test_multiple_reindex_cycles() {
    let (_dir, vdb) = setup_project();

    // Initial ingest
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Run 3 consecutive full reindexes
    for cycle in 0..3 {
        let opts = IngestOptions { full: true, file: None };
        vdb.ingest(opts).await.unwrap();

        let status = vdb.status();
        assert!(status.document_count > 0, "cycle {cycle}: should have documents");
        assert!(status.chunk_count > 0, "cycle {cycle}: should have chunks");
        assert!(status.vector_count > 0, "cycle {cycle}: should have vectors");

        let query = SearchQuery::new("test document");
        let results = vdb.search(query).await.unwrap();
        assert!(!results.is_empty(), "cycle {cycle}: search should return results");
    }
}

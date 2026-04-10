use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::error::Error;
use mdvdb::search::SearchQuery;
use mdvdb::{CheckStatus, IngestOptions, MarkdownVdb, SearchMode, SearchResponse};
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
        clustering_granularity: 1.0,
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
            edge_embeddings: true,
            edge_boost_weight: 0.15,
            edge_cluster_rebalance: 50,
            custom_cluster_defs: Vec::new(),
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
    let results = vdb.search(query).await.unwrap().results;

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
    let results = vdb.search(query).await.unwrap().results;

    assert!(!results.is_empty(), "hybrid search should return results");
    assert!(results[0].score > 0.0, "results should have positive scores");
}

#[tokio::test]
async fn test_search_semantic_mode_via_api() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("rust").with_mode(SearchMode::Semantic);
    let results = vdb.search(query).await.unwrap().results;

    assert!(!results.is_empty(), "semantic search should return results");
}

#[tokio::test]
async fn test_search_lexical_mode_via_api() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("systems programming language").with_mode(SearchMode::Lexical);
    let results = vdb.search(query).await.unwrap().results;

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
    let results = vdb.search(query).await.unwrap().results;

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
    let opts = IngestOptions { full: true, ..Default::default() };
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
    let results = vdb.search(query).await.unwrap().results;

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
async fn test_doctor_empty_index_warns_to_ingest() {
    let (_dir, vdb) = setup_project();
    // No ingest — index is empty.

    let result = vdb.doctor().await.unwrap();

    let index_check = result
        .checks
        .iter()
        .find(|c| c.name == "Index")
        .expect("should have Index check");
    assert_eq!(index_check.status, CheckStatus::Warn);
    assert!(
        index_check.detail.contains("ingest"),
        "empty index warning should hint to run ingest: {}",
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
// Progress callback, preview, and cancellation tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ingest_with_progress_callback() {
    let (_dir, vdb) = setup_project();

    let phases = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let phases_clone = phases.clone();

    let opts = IngestOptions {
        progress: Some(Box::new(move |phase: &mdvdb::IngestPhase| {
            let label = match phase {
                mdvdb::IngestPhase::Discovering => "Discovering".to_string(),
                mdvdb::IngestPhase::Parsing { .. } => "Parsing".to_string(),
                mdvdb::IngestPhase::Skipped { .. } => "Skipped".to_string(),
                mdvdb::IngestPhase::Embedding { .. } => "Embedding".to_string(),
                mdvdb::IngestPhase::Saving => "Saving".to_string(),
                mdvdb::IngestPhase::Clustering => "Clustering".to_string(),
                mdvdb::IngestPhase::Cleaning => "Cleaning".to_string(),
                mdvdb::IngestPhase::Done => "Done".to_string(),
            };
            phases_clone.lock().unwrap().push(label);
        })),
        ..Default::default()
    };

    vdb.ingest(opts).await.unwrap();

    let collected = phases.lock().unwrap();
    assert!(collected.contains(&"Discovering".to_string()), "should have Discovering phase, got: {collected:?}");
    assert!(
        collected.contains(&"Parsing".to_string()) || collected.contains(&"Skipped".to_string()),
        "should have Parsing or Skipped phase, got: {collected:?}"
    );
    assert!(collected.contains(&"Saving".to_string()), "should have Saving phase, got: {collected:?}");
    assert!(collected.contains(&"Done".to_string()), "should have Done phase, got: {collected:?}");
}

#[test]
fn test_preview_returns_correct_counts() {
    let (_dir, vdb) = setup_project();

    let preview = vdb.preview(false, None).unwrap();

    // We have 2 markdown files (hello.md, rust.md)
    assert_eq!(preview.total_files, 2, "should discover 2 files");
    assert_eq!(preview.files_to_process, 2, "all files should be new");
    assert_eq!(preview.files_unchanged, 0, "no files should be unchanged");
    assert!(preview.total_chunks > 0, "should have chunks");
    assert!(preview.estimated_tokens > 0, "should have estimated tokens");
}

#[tokio::test]
async fn test_preview_no_api_calls() {
    let (_dir, vdb) = setup_project();

    // Preview should not call the embedding provider
    let _preview = vdb.preview(false, None).unwrap();

    // After preview, status should still be empty (no actual indexing)
    let status = vdb.status();
    assert_eq!(status.document_count, 0, "preview should not modify index");
    assert_eq!(status.chunk_count, 0, "preview should not create chunks");
}

#[tokio::test]
async fn test_preview_reindex_marks_all_changed() {
    let (_dir, vdb) = setup_project();

    // First ingest
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Preview with reindex=true — all files should be marked Changed
    let preview = vdb.preview(true, None).unwrap();
    assert_eq!(preview.files_to_process, preview.total_files, "reindex should process all files");
    assert_eq!(preview.files_unchanged, 0, "reindex should have no unchanged files");

    for file in &preview.files {
        assert_eq!(
            file.status,
            mdvdb::PreviewFileStatus::Changed,
            "file {} should be Changed with reindex=true, got {:?}",
            file.path, file.status
        );
    }
}

#[test]
fn test_ingest_options_default() {
    let opts = IngestOptions::default();
    assert!(!opts.full, "default full should be false");
    assert!(opts.file.is_none(), "default file should be None");
    assert!(opts.progress.is_none(), "default progress should be None");
    assert!(opts.cancel.is_none(), "default cancel should be None");
}

#[tokio::test]
async fn test_ingest_cancellation() {
    let (_dir, vdb) = setup_project();

    let token = tokio_util::sync::CancellationToken::new();
    token.cancel(); // Cancel immediately

    let opts = IngestOptions {
        cancel: Some(token),
        ..Default::default()
    };

    let result = vdb.ingest(opts).await.unwrap();
    assert!(result.cancelled, "result should indicate cancellation");
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
    let results1 = vdb.search(query).await.unwrap().results;
    assert!(!results1.is_empty(), "search should return results after first ingest");

    // Full reindex
    let opts = IngestOptions { full: true, ..Default::default() };
    vdb.ingest(opts).await.unwrap();

    // Search again — should still work
    let query2 = SearchQuery::new("rust programming");
    let results2 = vdb.search(query2).await.unwrap().results;
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
        let opts = IngestOptions { full: true, ..Default::default() };
        vdb.ingest(opts).await.unwrap();

        let status = vdb.status();
        assert!(status.document_count > 0, "cycle {cycle}: should have documents");
        assert!(status.chunk_count > 0, "cycle {cycle}: should have chunks");
        assert!(status.vector_count > 0, "cycle {cycle}: should have vectors");

        let query = SearchQuery::new("test document");
        let results = vdb.search(query).await.unwrap().results;
        assert!(!results.is_empty(), "cycle {cycle}: search should return results");
    }
}

// ---------------------------------------------------------------------------
// Time Decay API tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_with_decay_enabled() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("test document").with_decay(true);
    let results = vdb.search(query).await.unwrap().results;

    // Decay should not break search.
    assert!(!results.is_empty(), "search with decay should return results");
    for r in &results {
        assert!(r.score >= 0.0, "score should be non-negative");
        assert!(r.score <= 1.0, "score should be <= 1");
        assert!(r.file.modified_at.is_some(), "modified_at should be populated");
    }
}

#[tokio::test]
async fn test_search_with_decay_disabled() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("test document").with_decay(false);
    let results = vdb.search(query).await.unwrap().results;

    assert!(!results.is_empty());
    for r in &results {
        assert!(r.file.modified_at.is_some(), "modified_at should still be populated");
    }
}

#[tokio::test]
async fn test_get_document_includes_modified_at() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let doc = vdb.get_document("hello.md").unwrap();
    assert!(doc.modified_at.is_some(), "modified_at should be populated after ingest");
    assert!(doc.modified_at.unwrap() > 0, "modified_at should be non-zero");
}

#[tokio::test]
async fn test_search_decay_with_custom_half_life() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("test document")
        .with_decay(true)
        .with_decay_half_life(7.0);
    let results = vdb.search(query).await.unwrap().results;

    assert!(!results.is_empty());
    for r in &results {
        assert!(r.score >= 0.0);
    }
}

#[tokio::test]
async fn test_decay_config_enabled_via_config() {
    let (_dir, root_dir) = {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join(".markdownvdb")).unwrap();
        fs::write(root.join(".markdownvdb").join(".config"), "").unwrap();
        fs::write(root.join("doc.md"), "# Doc\n\nContent.\n").unwrap();
        (dir, root)
    };

    let mut config = mock_config();
    config.search_decay_enabled = true;
    config.search_decay_half_life = 30.0;

    let vdb = MarkdownVdb::open_with_config(root_dir, config).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Without per-query override, should use config's decay.
    let query = SearchQuery::new("doc");
    let results = vdb.search(query).await.unwrap().results;
    assert!(!results.is_empty());
}

// ---------------------------------------------------------------------------
// SearchResponse / graph traversal API tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_returns_search_response() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("rust programming");
    let response: SearchResponse = vdb.search(query).await.unwrap();

    // Verify the SearchResponse struct has all expected fields.
    assert!(!response.results.is_empty(), "search should return results");
    // Verify timings are populated (total_secs is f64, so always >= 0; just check it exists).
    let _total = response.timings.total_secs;
    let _search = response.timings.vector_search_secs;
    // graph_context should be empty when expand_graph is 0 (default).
    assert!(response.graph_context.is_empty(), "graph_context should be empty without expansion");
}

#[tokio::test]
async fn test_search_response_json_serialization() {
    let (_dir, vdb) = setup_project();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let query = SearchQuery::new("rust");
    let response = vdb.search(query).await.unwrap();

    // Serialize to JSON.
    let json_str = serde_json::to_string(&response).expect("SearchResponse should serialize to JSON");

    // Deserialize back to serde_json::Value and verify structure.
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("serialized JSON should parse back");
    assert!(parsed.is_object(), "top-level should be an object");
    assert!(parsed.get("results").is_some(), "should have 'results' key");
    assert!(parsed["results"].is_array(), "'results' should be an array");
    assert!(parsed.get("timings").is_some(), "should have 'timings' key");
    assert!(parsed["timings"].is_object(), "'timings' should be an object");
    assert!(parsed["timings"].get("total_secs").is_some(), "timings should have 'total_secs'");
    assert!(parsed["timings"].get("embed_secs").is_some(), "timings should have 'embed_secs'");

    // graph_context is empty, so it should be skipped due to skip_serializing_if.
    assert!(
        parsed.get("graph_context").is_none(),
        "empty graph_context should be omitted from JSON"
    );
}

#[test]
fn test_with_boost_hops_builder() {
    let query = SearchQuery::new("test query").with_boost_hops(2);
    assert_eq!(query.boost_hops, Some(2), "boost_hops should be set to 2");

    let query0 = SearchQuery::new("test query").with_boost_hops(0);
    assert_eq!(query0.boost_hops, Some(0), "boost_hops should accept 0");

    let query3 = SearchQuery::new("test query").with_boost_hops(3);
    assert_eq!(query3.boost_hops, Some(3), "boost_hops should accept 3");

    // Default should be None.
    let query_default = SearchQuery::new("test query");
    assert_eq!(query_default.boost_hops, None, "default boost_hops should be None");
}

#[test]
fn test_with_expand_graph_builder() {
    let query = SearchQuery::new("test query").with_expand_graph(1);
    assert_eq!(query.expand_graph, Some(1), "expand_graph should be set to 1");

    let query0 = SearchQuery::new("test query").with_expand_graph(0);
    assert_eq!(query0.expand_graph, Some(0), "expand_graph should accept 0");

    let query3 = SearchQuery::new("test query").with_expand_graph(3);
    assert_eq!(query3.expand_graph, Some(3), "expand_graph should accept 3");

    // Default should be None.
    let query_default = SearchQuery::new("test query");
    assert_eq!(query_default.expand_graph, None, "default expand_graph should be None");
}

#[tokio::test]
async fn test_search_with_expansion() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    // Create files that link to each other.
    fs::write(
        root.join("main.md"),
        "---\ntitle: Main Doc\n---\n\n# Main Document\n\nThis is the main document about programming.\nSee also [reference](reference.md).\n",
    )
    .unwrap();

    fs::write(
        root.join("reference.md"),
        "---\ntitle: Reference Guide\n---\n\n# Reference\n\nThis is the reference guide with details.\n",
    )
    .unwrap();

    fs::write(
        root.join("unlinked.md"),
        "---\ntitle: Unlinked\n---\n\n# Unlinked\n\nThis file has no links.\n",
    )
    .unwrap();

    let mut config = mock_config();
    config.search_expand_graph = 1; // Enable graph expansion by default.
    config.search_expand_limit = 5;

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), config).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Search with graph expansion enabled.
    let query = SearchQuery::new("main document programming").with_expand_graph(1);
    let response = vdb.search(query).await.unwrap();

    assert!(!response.results.is_empty(), "search should return results");

    // Check if any result file matches main.md — if so, graph_context may contain
    // chunks from reference.md (linked from main.md).
    let has_main = response.results.iter().any(|r| r.file.path == "main.md");
    if has_main {
        // With expand_graph=1, we expect graph_context to include chunks from linked files.
        // The mock provider produces deterministic embeddings, so results depend on similarity.
        // At minimum, verify graph_context items have valid structure.
        for ctx in &response.graph_context {
            assert!(!ctx.chunk.content.is_empty(), "graph context chunk should have content");
            assert!(!ctx.linked_from.is_empty(), "graph context should have linked_from");
            assert!(ctx.hop_distance >= 1, "hop_distance should be >= 1");
        }
    }

    // Also test that JSON serialization includes graph_context when non-empty.
    if !response.graph_context.is_empty() {
        let json_str = serde_json::to_string(&response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(
            parsed.get("graph_context").is_some(),
            "non-empty graph_context should be present in JSON"
        );
        assert!(
            parsed["graph_context"].is_array(),
            "graph_context should be an array"
        );
    }
}

#[tokio::test]
async fn custom_clusters_full_pipeline() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Create config dir and markdown files
    std::fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    std::fs::write(root.join("ai.md"), "# AI\nMachine learning and neural networks").unwrap();
    std::fs::write(root.join("web.md"), "# Web\nHTML CSS and JavaScript frontend").unwrap();
    std::fs::write(root.join("ops.md"), "# Ops\nDocker and Kubernetes deployments").unwrap();

    let mut config = mock_config();
    config.custom_cluster_defs = vec![
        mdvdb::CustomClusterDef {
            name: "AI".to_string(),
            seeds: vec!["machine learning".to_string(), "neural networks".to_string()],
        },
        mdvdb::CustomClusterDef {
            name: "Web".to_string(),
            seeds: vec!["html".to_string(), "css".to_string(), "javascript".to_string()],
        },
    ];

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), config).unwrap();
    vdb.ingest(mdvdb::IngestOptions::default()).await.unwrap();

    // Custom clusters should be populated
    let custom = vdb.custom_clusters().unwrap();
    assert_eq!(custom.len(), 2);
    assert_eq!(custom[0].name, "AI");
    assert_eq!(custom[1].name, "Web");

    // All 3 documents should be assigned
    let total_docs: usize = custom.iter().map(|c| c.document_count).sum();
    assert_eq!(total_docs, 3);

    // Graph data should include custom cluster info
    let graph = vdb.graph_data(None).unwrap();
    assert_eq!(graph.custom_clusters.len(), 2);
    for node in &graph.nodes {
        assert!(node.custom_cluster_id.is_some(), "every node should have a custom_cluster_id");
    }
}

#[tokio::test]
async fn custom_clusters_cleared_when_defs_removed() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    std::fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    std::fs::write(root.join("doc.md"), "# Hello\nSome content here").unwrap();

    // First ingest with custom clusters defined
    let mut config = mock_config();
    config.custom_cluster_defs = vec![mdvdb::CustomClusterDef {
        name: "Test".to_string(),
        seeds: vec!["hello".to_string()],
    }];

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), config).unwrap();
    vdb.ingest(mdvdb::IngestOptions::default()).await.unwrap();
    assert_eq!(vdb.custom_clusters().unwrap().len(), 1);
    drop(vdb);

    // Second ingest with empty defs — should clear
    let mut config2 = mock_config();
    config2.custom_cluster_defs = Vec::new();

    let vdb2 = MarkdownVdb::open_with_config(root.to_path_buf(), config2).unwrap();
    vdb2.ingest(mdvdb::IngestOptions::default()).await.unwrap();
    assert!(vdb2.custom_clusters().unwrap().is_empty());
}

#[tokio::test]
async fn custom_clusters_incremental_ingest() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    std::fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    std::fs::write(root.join("doc1.md"), "# First\nContent one").unwrap();

    let mut config = mock_config();
    config.custom_cluster_defs = vec![
        mdvdb::CustomClusterDef {
            name: "A".to_string(),
            seeds: vec!["first".to_string()],
        },
        mdvdb::CustomClusterDef {
            name: "B".to_string(),
            seeds: vec!["second".to_string()],
        },
    ];

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), config.clone()).unwrap();
    vdb.ingest(mdvdb::IngestOptions::default()).await.unwrap();

    // Should have 1 doc assigned
    let custom = vdb.custom_clusters().unwrap();
    let total: usize = custom.iter().map(|c| c.document_count).sum();
    assert_eq!(total, 1);

    // Add a new file and do incremental ingest
    std::fs::write(root.join("doc2.md"), "# Second\nContent two").unwrap();
    let opts = mdvdb::IngestOptions {
        file: Some(PathBuf::from("doc2.md")),
        ..Default::default()
    };
    vdb.ingest(opts).await.unwrap();

    // Now should have 2 docs assigned
    let custom = vdb.custom_clusters().unwrap();
    let total: usize = custom.iter().map(|c| c.document_count).sum();
    assert_eq!(total, 2);
}

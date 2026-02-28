use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::embedding::mock::MockProvider;
use mdvdb::index::{EmbeddingConfig, Index};
use mdvdb::ingest::{ingest_file, ingest_full};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const DIMS: usize = 8;

fn test_embedding_config() -> EmbeddingConfig {
    EmbeddingConfig {
        provider: "mock".to_string(),
        model: "mock-model".to_string(),
        dimensions: DIMS,
    }
}

fn test_config() -> Config {
    Config {
        embedding_provider: EmbeddingProviderType::OpenAI,
        embedding_model: "mock-model".into(),
        embedding_dimensions: DIMS,
        embedding_batch_size: 100,
        openai_api_key: Some("sk-test".into()),
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
    }
}

/// Create a temp directory with markdown files and an index.
fn setup_project(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    for (path, content) in files {
        let full = dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, content).unwrap();
    }
    fs::create_dir_all(dir.path().join(".markdownvdb")).unwrap();
    let idx_path = dir.path().join(".markdownvdb").join("index");
    (dir, idx_path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_full_ingest_indexes_all_files() {
    let (dir, idx_path) = setup_project(&[
        ("alpha.md", "# Alpha\n\nAlpha content here."),
        ("beta.md", "# Beta\n\nBeta content here."),
        ("docs/gamma.md", "# Gamma\n\nGamma content."),
    ]);

    let config = test_config();
    let index = Index::create(&idx_path, &test_embedding_config()).unwrap();
    let provider = MockProvider::new(DIMS);

    let result = ingest_full(dir.path(), &config, &index, &provider, 512, 50, 100)
        .await
        .unwrap();

    assert_eq!(result.files_discovered, 3);
    assert_eq!(result.files_ingested, 3);
    assert_eq!(result.files_skipped, 0);
    assert_eq!(result.files_removed, 0);
    assert!(result.chunks_total > 0);
    assert_eq!(result.chunks_embedded, result.chunks_total);

    let status = index.status();
    assert_eq!(status.document_count, 3);
    assert!(status.chunk_count > 0);
}

#[tokio::test]
async fn test_second_ingest_skips_unchanged() {
    let (dir, idx_path) = setup_project(&[
        ("one.md", "# One\n\nContent one."),
        ("two.md", "# Two\n\nContent two."),
    ]);

    let config = test_config();
    let index = Index::create(&idx_path, &test_embedding_config()).unwrap();
    let provider = MockProvider::new(DIMS);

    // First ingest
    let r1 = ingest_full(dir.path(), &config, &index, &provider, 512, 50, 100)
        .await
        .unwrap();
    assert_eq!(r1.files_ingested, 2);
    assert_eq!(r1.files_skipped, 0);

    let calls_after_first = provider.call_count();

    // Second ingest — nothing changed
    let r2 = ingest_full(dir.path(), &config, &index, &provider, 512, 50, 100)
        .await
        .unwrap();
    assert_eq!(r2.files_discovered, 2);
    assert_eq!(r2.files_ingested, 0);
    assert_eq!(r2.files_skipped, 2);
    assert_eq!(r2.chunks_embedded, 0);

    // No additional API calls
    assert_eq!(provider.call_count(), calls_after_first);
}

#[tokio::test]
async fn test_modified_files_re_embedded() {
    let (dir, idx_path) = setup_project(&[
        ("doc.md", "# Doc\n\nOriginal content."),
    ]);

    let config = test_config();
    let index = Index::create(&idx_path, &test_embedding_config()).unwrap();
    let provider = MockProvider::new(DIMS);

    // First ingest
    let r1 = ingest_full(dir.path(), &config, &index, &provider, 512, 50, 100)
        .await
        .unwrap();
    assert_eq!(r1.files_ingested, 1);

    // Modify the file
    fs::write(dir.path().join("doc.md"), "# Doc\n\nModified content with more text.").unwrap();

    // Second ingest — should re-embed
    let r2 = ingest_full(dir.path(), &config, &index, &provider, 512, 50, 100)
        .await
        .unwrap();
    assert_eq!(r2.files_ingested, 1);
    assert_eq!(r2.files_skipped, 0);
    assert!(r2.chunks_embedded > 0);
}

#[tokio::test]
async fn test_deleted_files_removed_as_stale() {
    let (dir, idx_path) = setup_project(&[
        ("keep.md", "# Keep\n\nKeep this file."),
        ("remove.md", "# Remove\n\nThis will be deleted."),
    ]);

    let config = test_config();
    let index = Index::create(&idx_path, &test_embedding_config()).unwrap();
    let provider = MockProvider::new(DIMS);

    // First ingest
    let r1 = ingest_full(dir.path(), &config, &index, &provider, 512, 50, 100)
        .await
        .unwrap();
    assert_eq!(r1.files_ingested, 2);
    assert_eq!(index.status().document_count, 2);

    // Delete a file
    fs::remove_file(dir.path().join("remove.md")).unwrap();

    // Second ingest — should remove stale entry
    let r2 = ingest_full(dir.path(), &config, &index, &provider, 512, 50, 100)
        .await
        .unwrap();
    assert_eq!(r2.files_discovered, 1);
    assert_eq!(r2.files_removed, 1);
    assert_eq!(index.status().document_count, 1);
}

#[tokio::test]
async fn test_ingest_result_counts_accurate() {
    let (dir, idx_path) = setup_project(&[
        ("a.md", "# A\n\nContent A."),
        ("b.md", "# B\n\nContent B."),
        ("c.md", "# C\n\nContent C."),
    ]);

    let config = test_config();
    let index = Index::create(&idx_path, &test_embedding_config()).unwrap();
    let provider = MockProvider::new(DIMS);

    // First full ingest
    let r1 = ingest_full(dir.path(), &config, &index, &provider, 512, 50, 100)
        .await
        .unwrap();

    // Counts should be consistent
    assert_eq!(r1.files_discovered, r1.files_ingested + r1.files_skipped);
    assert_eq!(r1.chunks_total, r1.chunks_embedded + r1.chunks_skipped);
    assert_eq!(r1.results.len(), r1.files_discovered);

    // Per-file results should sum to totals
    let sum_chunks_total: usize = r1.results.iter().map(|r| r.chunks_total).sum();
    let sum_chunks_embedded: usize = r1.results.iter().map(|r| r.chunks_embedded).sum();
    assert_eq!(sum_chunks_total, r1.chunks_total);
    assert_eq!(sum_chunks_embedded, r1.chunks_embedded);
}

#[tokio::test]
async fn test_single_file_ingest() {
    let (dir, idx_path) = setup_project(&[
        ("solo.md", "# Solo\n\nSolo file content for testing."),
    ]);

    let index = Index::create(&idx_path, &test_embedding_config()).unwrap();
    let provider = MockProvider::new(DIMS);

    let result = ingest_file(
        dir.path(),
        &PathBuf::from("solo.md"),
        &index,
        &provider,
        512,
        50,
        100,
    )
    .await
    .unwrap();

    assert!(!result.skipped);
    assert!(result.chunks_total > 0);
    assert_eq!(result.chunks_embedded, result.chunks_total);
    assert_eq!(result.path, PathBuf::from("solo.md"));
    assert_eq!(index.status().document_count, 1);

    // Second call with same content should skip
    let result2 = ingest_file(
        dir.path(),
        &PathBuf::from("solo.md"),
        &index,
        &provider,
        512,
        50,
        100,
    )
    .await
    .unwrap();

    assert!(result2.skipped);
    assert_eq!(result2.chunks_embedded, 0);
    assert_eq!(result2.chunks_skipped, result.chunks_total);
}

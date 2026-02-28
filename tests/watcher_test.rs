use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::embedding::mock::MockProvider;
use mdvdb::embedding::provider::EmbeddingProvider;
use mdvdb::fts::FtsIndex;
use mdvdb::index::{EmbeddingConfig, Index};
use mdvdb::watcher::Watcher;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_config(source_dir: &str) -> Config {
    Config {
        embedding_provider: EmbeddingProviderType::OpenAI,
        embedding_model: "test-model".into(),
        embedding_dimensions: 8,
        embedding_batch_size: 100,
        openai_api_key: None,
        ollama_host: String::new(),
        embedding_endpoint: None,
        source_dirs: vec![PathBuf::from(source_dir)],
        index_file: PathBuf::from(".markdownvdb.index"),
        ignore_patterns: vec![],
        watch_enabled: true,
        watch_debounce_ms: 200,
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

/// Create a temp directory under the current working directory so that macOS
/// FSEvents can reliably deliver file-system notifications. Temp dirs under
/// /private/tmp are problematic in sandboxed environments.
fn setup() -> (TempDir, PathBuf, Arc<Index>, Arc<FtsIndex>, Arc<dyn EmbeddingProvider>) {
    let dir = TempDir::new_in(".").unwrap();
    let project_root = dir.path().canonicalize().unwrap();

    // Create a docs subdirectory as the source dir.
    let docs_dir = project_root.join("docs");
    fs::create_dir_all(&docs_dir).unwrap();

    let index_path = project_root.join("test.idx");
    let embedding_config = EmbeddingConfig {
        provider: "MockProvider".to_string(),
        model: "test-model".to_string(),
        dimensions: 8,
    };
    let index = Arc::new(Index::create(&index_path, &embedding_config).unwrap());
    let fts_index = Arc::new(FtsIndex::open_or_create(&project_root.join(".markdownvdb.fts")).unwrap());
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(MockProvider::new(8));

    (dir, project_root, index, fts_index, provider)
}

/// Wait for the watcher to process events. macOS FSEvents can have a latency
/// of 1-2 seconds. We poll the index to detect when processing is done, with
/// a maximum timeout.
async fn wait_for_condition<F: Fn() -> bool>(check: F, timeout_ms: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    while tokio::time::Instant::now() < deadline {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    check()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Note: This test relies on OS-level filesystem events (FSEvents on macOS).
/// It may fail in sandboxed environments that restrict FS event delivery.
/// Run with `cargo test --test watcher_test -- --ignored` to include these.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires OS filesystem event delivery (may fail in sandbox)"]
async fn watcher_detects_new_file() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let config = test_config("docs");
    let cancel = CancellationToken::new();

    let watcher = Watcher::new(config, &project_root, index.clone(), fts_index, provider);

    let cancel_clone = cancel.clone();
    let root_clone = project_root.clone();
    let watch_handle = tokio::spawn(async move {
        watcher.watch(cancel_clone).await
    });

    // Give the watcher time to start.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create a new markdown file.
    let new_file = root_clone.join("docs/new_file.md");
    fs::write(&new_file, "# New File\n\nSome content here.").unwrap();

    let idx = index.clone();
    let detected = wait_for_condition(
        move || idx.status().document_count == 1,
        10_000,
    ).await;
    assert!(detected, "watcher should have indexed the new file");
    assert!(index.status().chunk_count > 0, "should have at least one chunk");

    cancel.cancel();
    let result = watch_handle.await.unwrap();
    assert!(result.is_ok(), "watcher should shut down cleanly");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires OS filesystem event delivery (may fail in sandbox)"]
async fn watcher_detects_modification() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let config = test_config("docs");
    let cancel = CancellationToken::new();

    // Pre-create a file before starting the watcher.
    let file_path = project_root.join("docs/existing.md");
    fs::write(&file_path, "# Original\n\nOriginal content.").unwrap();

    let watcher = Watcher::new(config, &project_root, index.clone(), fts_index, provider);

    let cancel_clone = cancel.clone();
    let watch_handle = tokio::spawn(async move {
        watcher.watch(cancel_clone).await
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Modify the file with different content.
    fs::write(&file_path, "# Updated\n\nUpdated content with more text.\n\n## Section 2\n\nAnother section.").unwrap();

    let idx = index.clone();
    let detected = wait_for_condition(
        move || idx.status().document_count == 1,
        10_000,
    ).await;
    assert!(detected, "should have one document after modification");
    assert!(index.status().chunk_count > 0, "should have chunks after modification");

    cancel.cancel();
    let result = watch_handle.await.unwrap();
    assert!(result.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires OS filesystem event delivery (may fail in sandbox)"]
async fn watcher_detects_deletion() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let config = test_config("docs");
    let cancel = CancellationToken::new();

    // Pre-create a file before the watcher starts so that the Create event
    // from initial write doesn't race with the subsequent delete.
    let file_path = project_root.join("docs/to_delete.md");
    fs::write(&file_path, "# To Delete\n\nThis will be deleted.").unwrap();

    let watcher = Watcher::new(config, &project_root, index.clone(), fts_index, provider);

    let cancel_clone = cancel.clone();
    let watch_handle = tokio::spawn(async move {
        watcher.watch(cancel_clone).await
    });

    // Wait for watcher to start and pick up the initial Create event from
    // the file that existed before watching started. On macOS, FSEvents may
    // deliver a synthetic event for recently-created files.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Ensure the file is indexed by triggering a content change.
    fs::write(&file_path, "# To Delete\n\nModified content to trigger re-index.").unwrap();

    let idx = index.clone();
    let indexed = wait_for_condition(
        move || idx.status().document_count == 1,
        10_000,
    ).await;
    assert!(indexed, "file should be indexed before deletion");

    // Wait for any in-flight events to settle before deleting.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Delete the file.
    fs::remove_file(&file_path).unwrap();

    let idx2 = index.clone();
    let deleted = wait_for_condition(
        move || idx2.status().document_count == 0,
        10_000,
    ).await;
    assert!(deleted, "watcher should have removed deleted file from index");
    assert_eq!(index.status().chunk_count, 0, "no chunks should remain");

    cancel.cancel();
    let result = watch_handle.await.unwrap();
    assert!(result.is_ok());
}

#[tokio::test]
async fn watcher_graceful_shutdown_via_cancellation_token() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let config = test_config("docs");
    let cancel = CancellationToken::new();

    let watcher = Watcher::new(config, &project_root, index, fts_index, provider);

    let cancel_clone = cancel.clone();
    let watch_handle = tokio::spawn(async move {
        watcher.watch(cancel_clone).await
    });

    // Let the watcher start up.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Cancel immediately — should shut down promptly.
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(5), watch_handle)
        .await
        .expect("watcher should shut down within 5 seconds")
        .expect("task should not panic");

    assert!(result.is_ok(), "watcher should return Ok on graceful shutdown");
}

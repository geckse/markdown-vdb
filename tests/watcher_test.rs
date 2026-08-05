use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::embedding::mock::MockProvider;
use mdvdb::embedding::provider::EmbeddingProvider;
use mdvdb::fts::FtsIndex;
use mdvdb::index::{EmbeddingConfig, Index};
use mdvdb::watcher::{FileEvent, Watcher};
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
        embedding_options: Default::default(),
        source_dirs: vec![PathBuf::from(source_dir)],
        ignore_patterns: vec![],
        watch_enabled: true,
        watch_debounce_ms: 200,
        chunk_max_tokens: 512,
        chunk_overlap_tokens: 50,
        clustering_enabled: false,
        clustering_algorithm: mdvdb::config::ClusteringAlgorithm::Leiden,
        clustering_knn: 15,
        clustering_resolution: 1.0,
        clustering_min_cluster_size: 2,
        topics_min_similarity: 0.30,
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

/// Create a temp directory under the current working directory so that macOS
/// FSEvents can reliably deliver file-system notifications. Temp dirs under
/// /private/tmp are problematic in sandboxed environments.
fn setup() -> (
    TempDir,
    PathBuf,
    Arc<Index>,
    Arc<FtsIndex>,
    Arc<dyn EmbeddingProvider>,
) {
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
    let fts_index =
        Arc::new(FtsIndex::open_or_create(&project_root.join(".markdownvdb").join("fts")).unwrap());
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

#[tokio::test]
async fn watcher_rejects_an_unsaved_partial_index_branch() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let pending_path = project_root.join("docs/pending.md");
    fs::write(&pending_path, "---\nvalue: 1\n---\n").unwrap();
    let parsed =
        mdvdb::parser::parse_markdown_file(&project_root, std::path::Path::new("docs/pending.md"))
            .unwrap();
    index.upsert(&parsed, &[], &[]).unwrap();

    let watcher = Watcher::new(
        test_config("docs"),
        &project_root,
        index,
        fts_index,
        provider,
        None,
    );
    let result = watcher
        .handle_event(&FileEvent::SchemaChanged(PathBuf::from(
            ".markdownvdb.schema.yml",
        )))
        .await;
    assert!(matches!(result, Err(mdvdb::Error::IndexDirty { .. })));
    assert!(Index::open(&project_root.join("test.idx"))
        .unwrap()
        .get_file("docs/pending.md")
        .is_none());
}

/// Note: This test relies on OS-level filesystem events (FSEvents on macOS).
/// It may fail in sandboxed environments that restrict FS event delivery.
/// Run with `cargo test --test watcher_test -- --ignored` to include these.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires OS filesystem event delivery (may fail in sandbox)"]
async fn watcher_detects_new_file() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let config = test_config("docs");
    let cancel = CancellationToken::new();

    let watcher = Watcher::new(
        config,
        &project_root,
        index.clone(),
        fts_index,
        provider,
        None,
    );

    let cancel_clone = cancel.clone();
    let root_clone = project_root.clone();
    let watch_handle = tokio::spawn(async move { watcher.watch(cancel_clone).await });

    // Give the watcher time to start.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create a new markdown file.
    let new_file = root_clone.join("docs/new_file.md");
    fs::write(&new_file, "# New File\n\nSome content here.").unwrap();

    let idx = index.clone();
    let detected = wait_for_condition(move || idx.status().document_count == 1, 10_000).await;
    assert!(detected, "watcher should have indexed the new file");
    assert!(
        index.status().chunk_count > 0,
        "should have at least one chunk"
    );

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

    let watcher = Watcher::new(
        config,
        &project_root,
        index.clone(),
        fts_index,
        provider,
        None,
    );

    let cancel_clone = cancel.clone();
    let watch_handle = tokio::spawn(async move { watcher.watch(cancel_clone).await });

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Modify the file with different content.
    fs::write(
        &file_path,
        "# Updated\n\nUpdated content with more text.\n\n## Section 2\n\nAnother section.",
    )
    .unwrap();

    let idx = index.clone();
    let detected = wait_for_condition(move || idx.status().document_count == 1, 10_000).await;
    assert!(detected, "should have one document after modification");
    assert!(
        index.status().chunk_count > 0,
        "should have chunks after modification"
    );

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

    let watcher = Watcher::new(
        config,
        &project_root,
        index.clone(),
        fts_index,
        provider,
        None,
    );

    let cancel_clone = cancel.clone();
    let watch_handle = tokio::spawn(async move { watcher.watch(cancel_clone).await });

    // Wait for watcher to start and pick up the initial Create event from
    // the file that existed before watching started. On macOS, FSEvents may
    // deliver a synthetic event for recently-created files.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Ensure the file is indexed by triggering a content change.
    fs::write(
        &file_path,
        "# To Delete\n\nModified content to trigger re-index.",
    )
    .unwrap();

    let idx = index.clone();
    let indexed = wait_for_condition(move || idx.status().document_count == 1, 10_000).await;
    assert!(indexed, "file should be indexed before deletion");

    // Wait for any in-flight events to settle before deleting.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Delete the file.
    fs::remove_file(&file_path).unwrap();

    let idx2 = index.clone();
    let deleted = wait_for_condition(move || idx2.status().document_count == 0, 10_000).await;
    assert!(
        deleted,
        "watcher should have removed deleted file from index"
    );
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

    let watcher = Watcher::new(config, &project_root, index, fts_index, provider, None);

    let cancel_clone = cancel.clone();
    let watch_handle = tokio::spawn(async move { watcher.watch(cancel_clone).await });

    // Let the watcher start up.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Cancel immediately — should shut down promptly.
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(5), watch_handle)
        .await
        .expect("watcher should shut down within 5 seconds")
        .expect("task should not panic");

    assert!(
        result.is_ok(),
        "watcher should return Ok on graceful shutdown"
    );
}

#[tokio::test]
async fn unchanged_source_event_is_a_true_no_op() {
    let (_dir, project_root, index, fts_index, _provider) = setup();
    let mock = Arc::new(MockProvider::new(8));
    let provider: Arc<dyn EmbeddingProvider> = mock.clone();
    let reports = Arc::new(Mutex::new(Vec::new()));
    let callback_reports = Arc::clone(&reports);
    let callback = Box::new(move |report: &mdvdb::WatchEventReport| {
        callback_reports.lock().unwrap().push(report.clone());
    });
    let watcher = Watcher::new(
        test_config("docs"),
        &project_root,
        Arc::clone(&index),
        fts_index,
        provider,
        Some(callback),
    );
    let relative = PathBuf::from("docs/unchanged.md");

    fs::write(
        project_root.join(&relative),
        "---\nstatus: open\n---\n# Unchanged\n\nBody text.\n",
    )
    .unwrap();
    watcher
        .handle_event(&FileEvent::Created(relative.clone()))
        .await
        .unwrap();
    assert_eq!(mock.call_count(), 1);

    watcher
        .handle_event(&FileEvent::Modified(relative))
        .await
        .unwrap();

    assert_eq!(
        mock.call_count(),
        1,
        "an exact watcher echo must not call the embedding provider"
    );
    let reports = reports.lock().unwrap();
    let created = reports.first().expect("created callback report");
    assert!(created.estimated_input_tokens > 0);
    assert_eq!(created.api_calls, 1);
    let echo = reports.last().expect("echo callback report");
    assert_eq!(echo.chunks_processed, 0);
    assert_eq!(echo.estimated_input_tokens, 0);
    assert_eq!(echo.api_calls, 0);
    assert!(
        echo.module_reports.is_empty(),
        "an exact watcher echo must not rerun modules"
    );
}

#[tokio::test]
async fn delete_event_for_existing_target_reconciles_atomic_replacement() {
    let (_dir, project_root, index, fts_index, _provider) = setup();
    let mock = Arc::new(MockProvider::new(8));
    let provider: Arc<dyn EmbeddingProvider> = mock.clone();
    let watcher = Watcher::new(
        test_config("docs"),
        &project_root,
        Arc::clone(&index),
        fts_index,
        provider,
        None,
    );
    let relative = PathBuf::from("docs/replaced.md");

    fs::write(
        project_root.join(&relative),
        "---\nstatus: open\n---\n# Replaced\n\nStable body.\n",
    )
    .unwrap();
    watcher
        .handle_event(&FileEvent::Created(relative.clone()))
        .await
        .unwrap();
    assert_eq!(mock.call_count(), 1);

    // Simulate the remove half of an atomic replacement. The final target is
    // already present when the debounced event reaches the watcher.
    watcher
        .handle_event(&FileEvent::Deleted(relative))
        .await
        .unwrap();

    assert!(
        index.get_file("docs/replaced.md").is_some(),
        "an atomic replacement event must not remove the live target"
    );
    assert_eq!(
        mock.call_count(),
        1,
        "reconciling an unchanged replacement must not re-embed"
    );
}

#[tokio::test]
async fn frontmatter_only_change_refreshes_metadata_without_embedding() {
    let (_dir, project_root, index, fts_index, _provider) = setup();
    let mock = Arc::new(MockProvider::new(8));
    let provider: Arc<dyn EmbeddingProvider> = mock.clone();
    let watcher = Watcher::new(
        test_config("docs"),
        &project_root,
        Arc::clone(&index),
        fts_index,
        provider,
        None,
    );
    let relative = PathBuf::from("docs/metadata.md");
    let absolute = project_root.join(&relative);

    fs::write(
        &absolute,
        "---\nstatus: open\n---\n# Metadata\n\nBody text stays identical.\n",
    )
    .unwrap();
    watcher
        .handle_event(&FileEvent::Created(relative.clone()))
        .await
        .unwrap();
    assert_eq!(mock.call_count(), 1);
    let original = index.get_file("docs/metadata.md").unwrap();

    fs::write(
        &absolute,
        "---\nstatus: closed\n---\n# Metadata\n\nBody text stays identical.\n",
    )
    .unwrap();
    watcher
        .handle_event(&FileEvent::Modified(relative.clone()))
        .await
        .unwrap();

    assert_eq!(
        mock.call_count(),
        1,
        "frontmatter-only changes must reuse body embeddings"
    );
    let parsed = mdvdb::parser::parse_markdown_file(&project_root, &relative).unwrap();
    let refreshed = index.get_file("docs/metadata.md").unwrap();
    assert_eq!(refreshed.content_hash, parsed.content_hash);
    assert_eq!(
        refreshed.embedding_body_hash, original.embedding_body_hash,
        "metadata refresh must retain the hash represented by stored vectors"
    );
    let frontmatter: serde_json::Value =
        serde_json::from_str(refreshed.frontmatter.as_deref().unwrap()).unwrap();
    assert_eq!(frontmatter["status"], "closed");
}

#[tokio::test]
async fn matching_source_hash_does_not_hide_a_stale_body_embedding() {
    let (_dir, project_root, index, fts_index, _provider) = setup();
    let mock = Arc::new(MockProvider::new(8));
    let provider: Arc<dyn EmbeddingProvider> = mock.clone();
    let watcher = Watcher::new(
        test_config("docs"),
        &project_root,
        Arc::clone(&index),
        fts_index,
        provider,
        None,
    );
    let relative = PathBuf::from("docs/stale-body.md");
    let absolute = project_root.join(&relative);

    fs::write(&absolute, "# Original\n\nOriginal body.\n").unwrap();
    watcher
        .handle_event(&FileEvent::Created(relative.clone()))
        .await
        .unwrap();
    assert_eq!(mock.call_count(), 1);

    fs::write(&absolute, "# Changed\n\nA genuinely changed body.\n").unwrap();
    let changed = mdvdb::parser::parse_markdown_file(&project_root, &relative).unwrap();
    index.refresh_source_metadata(&changed).unwrap();
    index.save().unwrap();
    assert_eq!(
        index.get_file("docs/stale-body.md").unwrap().content_hash,
        changed.content_hash,
        "simulate a metadata refresh performed before the watcher event"
    );

    watcher
        .handle_event(&FileEvent::Modified(relative))
        .await
        .unwrap();

    assert_eq!(
        mock.call_count(),
        2,
        "a stale embedding body must be rebuilt even when the full source hash matches"
    );
    let stored = index.get_file("docs/stale-body.md").unwrap();
    assert_eq!(
        stored.embedding_body_hash,
        mdvdb::parser::compute_content_hash(&changed.body)
    );
}

#[tokio::test]
async fn empty_body_file_is_upserted_without_provider_call() {
    let (_dir, project_root, index, fts_index, _provider) = setup();
    let mock = Arc::new(MockProvider::new(8));
    let provider: Arc<dyn EmbeddingProvider> = mock.clone();
    let watcher = Watcher::new(
        test_config("docs"),
        &project_root,
        Arc::clone(&index),
        fts_index,
        provider,
        None,
    );
    let relative = PathBuf::from("docs/frontmatter-only.md");

    fs::write(project_root.join(&relative), "---\nprice: 10\n---\n").unwrap();
    let parsed = mdvdb::parser::parse_markdown_file(&project_root, &relative).unwrap();
    assert!(parsed.body.trim().is_empty());
    watcher
        .handle_event(&FileEvent::Created(relative))
        .await
        .unwrap();

    assert_eq!(
        mock.call_count(),
        0,
        "an empty embedding batch must never reach the provider"
    );
    let stored = index
        .get_file("docs/frontmatter-only.md")
        .expect("frontmatter-only documents remain indexable");
    assert!(stored.chunk_ids.is_empty());
    let frontmatter: serde_json::Value =
        serde_json::from_str(stored.frontmatter.as_deref().unwrap()).unwrap();
    assert_eq!(frontmatter["price"], 10);
}

#[tokio::test]
async fn watcher_delete_refreshes_raw_schema_counts() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let watcher = Watcher::new(
        test_config("docs"),
        &project_root,
        Arc::clone(&index),
        fts_index,
        provider,
        None,
    );

    for name in ["a.md", "b.md"] {
        fs::write(
            project_root.join("docs").join(name),
            "---\nprice: 10\n---\n# Invoice\n",
        )
        .unwrap();
        watcher
            .handle_event(&FileEvent::Created(PathBuf::from(format!("docs/{name}"))))
            .await
            .unwrap();
    }
    assert_eq!(
        index
            .get_scoped_schema("docs")
            .unwrap()
            .schema
            .get_field("price")
            .unwrap()
            .occurrence_count,
        2
    );

    fs::remove_file(project_root.join("docs/a.md")).unwrap();
    watcher
        .handle_event(&FileEvent::Deleted(PathBuf::from("docs/a.md")))
        .await
        .unwrap();
    assert_eq!(
        index
            .get_scoped_schema("docs")
            .unwrap()
            .schema
            .get_field("price")
            .unwrap()
            .occurrence_count,
        1
    );
}

#[tokio::test]
async fn schema_change_recomputes_formulas_without_embedding() {
    let (_dir, project_root, index, fts_index, _provider) = setup();
    let config = test_config("docs");
    let mock = Arc::new(MockProvider::new(8));
    let provider: Arc<dyn EmbeddingProvider> = mock.clone();
    let reports = Arc::new(Mutex::new(Vec::new()));
    let callback_reports = Arc::clone(&reports);
    let callback = Box::new(move |report: &mdvdb::WatchEventReport| {
        callback_reports.lock().unwrap().push(report.clone());
    });
    let watcher = Watcher::new(
        config,
        &project_root,
        Arc::clone(&index),
        fts_index,
        provider,
        Some(callback),
    );

    fs::write(
        project_root.join("docs/invoice.md"),
        "---\nprice: 0.1\nquantity: 3\n---\n# Invoice\n",
    )
    .unwrap();
    fs::write(
        project_root.join(".markdownvdb.schema.yml"),
        "scopes:\n  docs:\n    fields:\n      total:\n        field_type: formula\n        formula: price * quantity\n        result_type: number\n",
    )
    .unwrap();

    watcher
        .handle_event(&FileEvent::Created(PathBuf::from("docs/invoice.md")))
        .await
        .unwrap();
    assert_eq!(mock.call_count(), 1);
    let first = index
        .get_computed_fields("docs/invoice.md")
        .unwrap()
        .remove("total")
        .unwrap();
    assert_eq!(
        first
            .value_json
            .as_deref()
            .map(serde_json::from_str::<serde_json::Value>)
            .transpose()
            .unwrap(),
        Some(serde_json::json!(0.3))
    );
    assert_eq!(
        mdvdb::parser::parse_markdown_file(&project_root, &PathBuf::from("docs/invoice.md"))
            .unwrap()
            .frontmatter
            .unwrap()["total"],
        serde_json::json!(0.3)
    );

    fs::write(
        project_root.join(".markdownvdb.schema.yml"),
        "scopes:\n  docs:\n    fields:\n      total:\n        field_type: formula\n        formula: price * quantity + 1\n        result_type: number\n",
    )
    .unwrap();
    watcher
        .handle_event(&FileEvent::SchemaChanged(PathBuf::from(
            ".markdownvdb.schema.yml",
        )))
        .await
        .unwrap();

    assert_eq!(
        mock.call_count(),
        1,
        "schema-only recomputation must not request embeddings"
    );
    let second = index
        .get_computed_fields("docs/invoice.md")
        .unwrap()
        .remove("total")
        .unwrap();
    assert_eq!(
        second
            .value_json
            .as_deref()
            .map(serde_json::from_str::<serde_json::Value>)
            .transpose()
            .unwrap(),
        Some(serde_json::json!(1.3))
    );
    assert_eq!(
        mdvdb::parser::parse_markdown_file(&project_root, &PathBuf::from("docs/invoice.md"))
            .unwrap()
            .frontmatter
            .unwrap()["total"],
        serde_json::json!(1.3)
    );

    // Reconcile the filesystem event generated by the atomic Formula write.
    // The final source hash was already committed, so this is a true no-op.
    watcher
        .handle_event(&FileEvent::Modified(PathBuf::from("docs/invoice.md")))
        .await
        .unwrap();
    assert_eq!(mock.call_count(), 1);

    let reports = reports.lock().unwrap();
    let schema_report = reports
        .iter()
        .rev()
        .find(|report| report.path == ".markdownvdb.schema.yml")
        .expect("schema callback report");
    assert_eq!(schema_report.chunks_processed, 0);
    assert_eq!(schema_report.estimated_input_tokens, 0);
    assert_eq!(schema_report.api_calls, 0);
    assert_eq!(schema_report.path, ".markdownvdb.schema.yml");
    assert_eq!(schema_report.module_reports.len(), 2);
    assert_eq!(schema_report.module_reports[0].module, "formula");
    assert_eq!(schema_report.module_reports[0].event, "schema_changed");
    assert_eq!(schema_report.module_reports[1].module, "lookup_rollup");
    assert_eq!(schema_report.module_reports[1].event, "schema_changed");
}

#[tokio::test]
async fn malformed_and_deleted_schema_clear_formula_cache_without_embedding() {
    let (_dir, project_root, index, fts_index, _provider) = setup();
    let config = test_config("docs");
    let mock = Arc::new(MockProvider::new(8));
    let provider: Arc<dyn EmbeddingProvider> = mock.clone();
    let watcher = Watcher::new(
        config,
        &project_root,
        Arc::clone(&index),
        fts_index,
        provider,
        None,
    );
    let schema_path = project_root.join(".markdownvdb.schema.yml");

    fs::write(
        project_root.join("docs/invoice.md"),
        "---\nprice: 10\nquantity: 2\n---\n# Invoice\n",
    )
    .unwrap();
    fs::write(
        &schema_path,
        "scopes:\n  docs:\n    fields:\n      total:\n        field_type: formula\n        formula: price * quantity\n        result_type: number\n",
    )
    .unwrap();
    watcher
        .handle_event(&FileEvent::Created(PathBuf::from("docs/invoice.md")))
        .await
        .unwrap();
    assert_eq!(mock.call_count(), 1);

    fs::write(&schema_path, "scopes: [this is not a mapping\n").unwrap();
    watcher
        .handle_event(&FileEvent::SchemaChanged(PathBuf::from(
            ".markdownvdb.schema.yml",
        )))
        .await
        .unwrap();
    let malformed = index
        .get_computed_fields("docs/invoice.md")
        .unwrap()
        .remove("total")
        .unwrap();
    assert!(malformed.value_json.is_none());
    assert_eq!(
        malformed
            .diagnostic
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("invalid_schema")
    );
    assert!(
        !fs::read_to_string(project_root.join("docs/invoice.md"))
            .unwrap()
            .contains("total:"),
        "a failed definition must remove its stale materialized value"
    );

    fs::remove_file(schema_path).unwrap();
    watcher
        .handle_event(&FileEvent::SchemaChanged(PathBuf::from(
            ".markdownvdb.schema.yml",
        )))
        .await
        .unwrap();
    let removed = index
        .get_computed_fields("docs/invoice.md")
        .unwrap()
        .remove("total")
        .unwrap();
    assert!(
        removed.value_json.is_none(),
        "deleting the overlay must clear the cached result"
    );
    assert_eq!(
        removed.diagnostic.as_ref().map(|error| error.code.as_str()),
        Some("schema_overlay_missing")
    );
    assert!(!fs::read_to_string(project_root.join("docs/invoice.md"))
        .unwrap()
        .contains("total:"));
    assert!(
        index
            .get_scoped_schema("docs")
            .unwrap()
            .schema
            .get_field("total")
            .is_none(),
        "a removed definition must not survive as an inferred raw field"
    );
    assert_eq!(
        mock.call_count(),
        1,
        "schema recovery must not request embeddings"
    );
}

#[tokio::test]
async fn rename_drops_old_computed_state_and_calculates_new_path() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let config = test_config("docs");
    let reports = Arc::new(Mutex::new(Vec::new()));
    let callback_reports = Arc::clone(&reports);
    let watcher = Watcher::new(
        config,
        &project_root,
        Arc::clone(&index),
        fts_index,
        provider,
        Some(Box::new(move |report: &mdvdb::WatchEventReport| {
            callback_reports.lock().unwrap().push(report.clone());
        })),
    );

    fs::write(
        project_root.join(".markdownvdb.schema.yml"),
        "scopes:\n  docs:\n    fields:\n      total:\n        field_type: formula\n        formula: price * quantity\n        result_type: number\n",
    )
    .unwrap();
    let old_path = project_root.join("docs/old.md");
    let new_path = project_root.join("docs/new.md");
    fs::write(&old_path, "---\nprice: 4\nquantity: 5\n---\n# Invoice\n").unwrap();
    watcher
        .handle_event(&FileEvent::Created(PathBuf::from("docs/old.md")))
        .await
        .unwrap();
    fs::rename(old_path, new_path).unwrap();

    watcher
        .handle_event(&FileEvent::Renamed {
            from: PathBuf::from("docs/old.md"),
            to: PathBuf::from("docs/new.md"),
        })
        .await
        .unwrap();

    assert!(index.get_file("docs/old.md").is_none());
    assert!(index.get_computed_fields("docs/old.md").is_none());
    let new_total = index
        .get_computed_fields("docs/new.md")
        .unwrap()
        .remove("total")
        .unwrap();
    assert_eq!(
        new_total.value_json.as_deref(),
        Some("20"),
        "the renamed row must receive fresh computed state"
    );
    let reports = reports.lock().unwrap();
    let rename = reports.last().expect("rename callback report");
    assert_eq!(rename.previous_path.as_deref(), Some("docs/old.md"));
    assert_eq!(rename.path, "docs/new.md");
}

// ---------------------------------------------------------------------------
// Cluster maintenance under watch (driven via handle_event, no FS events)
// ---------------------------------------------------------------------------

use mdvdb::clustering::{Clusterer, CustomClusterInfo, CustomClusterState};

fn clustering_config(source_dir: &str) -> Config {
    let mut config = test_config(source_dir);
    config.clustering_enabled = true;
    config
}

async fn index_initial_docs(watcher: &Watcher, docs_dir: &std::path::Path, names: &[(&str, &str)]) {
    for (name, content) in names {
        fs::write(docs_dir.join(name), content).unwrap();
        watcher
            .handle_event(&FileEvent::Created(PathBuf::from(format!("docs/{name}"))))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn watcher_assigns_new_file_to_existing_cluster() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let docs_dir = project_root.join("docs");
    let config = clustering_config("docs");

    let watcher = Watcher::new(
        config.clone(),
        &project_root,
        Arc::clone(&index),
        Arc::clone(&fts_index),
        Arc::clone(&provider),
        None,
    );

    index_initial_docs(
        &watcher,
        &docs_dir,
        &[
            ("a.md", "# Alpha\nRust systems programming content"),
            ("b.md", "# Beta\nCooking recipes and kitchen notes"),
            ("c.md", "# Gamma\nMore rust cargo content here"),
        ],
    )
    .await;

    // Bootstrap cluster state (normally done by a full ingest).
    let clusterer = Clusterer::new(&config);
    let state = clusterer
        .cluster_all(
            &index.get_document_vectors(),
            &index.get_document_contents(),
            None,
        )
        .unwrap();
    assert!(!state.clusters.is_empty());
    index.update_clusters(Some(state));
    index.save().unwrap();

    // A new file arriving under watch must get a cluster assignment.
    fs::write(
        docs_dir.join("d.md"),
        "# Delta\nFresh document about testing",
    )
    .unwrap();
    watcher
        .handle_event(&FileEvent::Created(PathBuf::from("docs/d.md")))
        .await
        .unwrap();

    let state = index.get_clusters().expect("cluster state persisted");
    let memberships: usize = state
        .clusters
        .iter()
        .map(|c| c.members.iter().filter(|m| *m == "docs/d.md").count())
        .sum();
    assert_eq!(memberships, 1, "new file must be in exactly one cluster");
}

#[tokio::test]
async fn watcher_delete_removes_from_cluster_members() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let docs_dir = project_root.join("docs");
    let config = clustering_config("docs");

    let watcher = Watcher::new(
        config.clone(),
        &project_root,
        Arc::clone(&index),
        Arc::clone(&fts_index),
        Arc::clone(&provider),
        None,
    );

    index_initial_docs(
        &watcher,
        &docs_dir,
        &[
            ("a.md", "# Alpha\nRust systems content"),
            ("b.md", "# Beta\nCooking recipes content"),
        ],
    )
    .await;

    let clusterer = Clusterer::new(&config);
    let state = clusterer
        .cluster_all(
            &index.get_document_vectors(),
            &index.get_document_contents(),
            None,
        )
        .unwrap();
    index.update_clusters(Some(state));
    index.save().unwrap();

    fs::remove_file(docs_dir.join("a.md")).unwrap();
    watcher
        .handle_event(&FileEvent::Deleted(PathBuf::from("docs/a.md")))
        .await
        .unwrap();

    let state = index.get_clusters().expect("cluster state persisted");
    for cluster in &state.clusters {
        assert!(
            !cluster.members.contains(&"docs/a.md".to_string()),
            "deleted file must leave cluster membership"
        );
    }
}

#[tokio::test]
async fn watcher_reassigns_topics_when_fingerprint_matches() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let docs_dir = project_root.join("docs");
    let mut config = clustering_config("docs");
    config.custom_cluster_defs = vec![mdvdb::CustomClusterDef {
        name: "Everything".to_string(),
        description: None,
        seeds: vec!["notes".to_string()],
        threshold: None,
    }];
    config.topics_min_similarity = 0.0; // accept any non-negative match

    let fingerprint = mdvdb::clustering::topics_fingerprint(
        &config.custom_cluster_defs,
        config.topics_min_similarity,
        &config.embedding_model,
        config.embedding_dimensions,
    );

    // Seed a matching topic state (normally produced by a full ingest).
    index.update_custom_clusters(Some(CustomClusterState {
        clusters: vec![CustomClusterInfo {
            id: 0,
            name: "Everything".to_string(),
            description: None,
            seed_phrases: vec!["notes".to_string()],
            threshold: None,
            centroid: vec![0.35; 8],
            members: vec![],
        }],
        unassigned: vec![],
        fingerprint,
    }));
    index.save().unwrap();

    let watcher = Watcher::new(
        config.clone(),
        &project_root,
        Arc::clone(&index),
        Arc::clone(&fts_index),
        Arc::clone(&provider),
        None,
    );

    fs::write(docs_dir.join("new.md"), "# New\nSome notes content").unwrap();
    watcher
        .handle_event(&FileEvent::Created(PathBuf::from("docs/new.md")))
        .await
        .unwrap();

    let state = index.get_custom_clusters().expect("topic state persisted");
    let in_topic = state.clusters[0]
        .members
        .iter()
        .any(|m| m.path == "docs/new.md");
    let in_unassigned = state.unassigned.contains(&"docs/new.md".to_string());
    assert!(
        in_topic != in_unassigned,
        "new file must be topic-assigned XOR unassigned (topic={in_topic}, unassigned={in_unassigned})"
    );
}

#[tokio::test]
async fn failed_event_before_mutation_leaves_no_reconcile_marker() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let marker = project_root.join(".markdownvdb/fts-reconcile-required");
    // Non-UTF-8 bytes fail markdown parsing before any store mutation.
    fs::write(
        project_root.join("docs/broken.md"),
        [0xffu8, 0xfe, 0x00, 0x81],
    )
    .unwrap();

    let watcher = Watcher::new(
        test_config("docs"),
        &project_root,
        index,
        fts_index,
        provider,
        None,
    );
    let result = watcher
        .handle_event(&FileEvent::Modified(PathBuf::from("docs/broken.md")))
        .await;

    assert!(result.is_err(), "non-UTF-8 input must fail parsing");
    assert!(
        !marker.exists(),
        "a pre-mutation failure must not orphan the reconciliation marker"
    );
}

#[tokio::test]
async fn successful_event_retires_the_reconcile_marker() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let marker = project_root.join(".markdownvdb/fts-reconcile-required");
    fs::write(
        project_root.join("docs/note.md"),
        "# Note\n\nSome body text.\n",
    )
    .unwrap();

    let watcher = Watcher::new(
        test_config("docs"),
        &project_root,
        index,
        fts_index,
        provider,
        None,
    );
    watcher
        .handle_event(&FileEvent::Created(PathBuf::from("docs/note.md")))
        .await
        .unwrap();

    assert!(
        !marker.exists(),
        "a successful mutating event must retire the reconciliation marker"
    );
}

#[tokio::test]
async fn orphaned_marker_is_repaired_by_the_next_successful_event() {
    let (_dir, project_root, index, fts_index, provider) = setup();
    let marker = project_root.join(".markdownvdb/fts-reconcile-required");
    fs::create_dir_all(project_root.join(".markdownvdb")).unwrap();
    fs::write(&marker, "1\n").unwrap();
    fs::write(
        project_root.join("docs/note.md"),
        "# Note\n\nSome body text.\n",
    )
    .unwrap();

    let watcher = Watcher::new(
        test_config("docs"),
        &project_root,
        index,
        fts_index,
        provider,
        None,
    );
    watcher
        .handle_event(&FileEvent::Created(PathBuf::from("docs/note.md")))
        .await
        .unwrap();

    assert!(
        !marker.exists(),
        "an orphaned marker must be repaired and retired by the next event"
    );
}

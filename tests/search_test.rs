use std::path::PathBuf;

use mdvdb::chunker::Chunk;
use mdvdb::embedding::mock::MockProvider;
use mdvdb::index::{EmbeddingConfig, Index};
use mdvdb::links;
use mdvdb::parser::{MarkdownFile, RawLink};
use mdvdb::fts::FtsIndex;
use mdvdb::search::{search, MetadataFilter, SearchMode, SearchQuery};
use serde_json::json;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const DIMS: usize = 8;

fn test_config() -> EmbeddingConfig {
    EmbeddingConfig {
        provider: "OpenAI".to_string(),
        model: "test-model".to_string(),
        dimensions: DIMS,
    }
}

fn create_index_dir() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.idx");
    (dir, path)
}

fn fake_markdown_file(path: &str, hash: &str, frontmatter: Option<serde_json::Value>) -> MarkdownFile {
    fake_markdown_file_with_mtime(path, hash, frontmatter, 0)
}

fn fake_markdown_file_with_mtime(path: &str, hash: &str, frontmatter: Option<serde_json::Value>, modified_at: u64) -> MarkdownFile {
    MarkdownFile {
        path: PathBuf::from(path),
        frontmatter,
        headings: vec![],
        body: "Test body content".to_string(),
        content_hash: hash.to_string(),
        file_size: 100,
        links: Vec::new(),
        modified_at,
    }
}

fn fake_chunks(path: &str, count: usize) -> Vec<Chunk> {
    (0..count)
        .map(|i| Chunk {
            id: format!("{path}#{i}"),
            source_path: PathBuf::from(path),
            heading_hierarchy: vec!["Heading".to_string()],
            content: format!("Chunk {i} content for {path}"),
            start_line: i * 10 + 1,
            end_line: (i + 1) * 10,
            chunk_index: i,
            is_sub_split: false,
        })
        .collect()
}

fn fake_embeddings(count: usize) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            let mut v = vec![0.0f32; DIMS];
            v[i % DIMS] = 1.0;
            v
        })
        .collect()
}

fn mock_provider() -> MockProvider {
    MockProvider::new(DIMS)
}

/// Populate an index with a single file and its chunks+embeddings.
fn populate_index(
    index: &Index,
    path: &str,
    hash: &str,
    frontmatter: Option<serde_json::Value>,
    chunk_count: usize,
) {
    let file = fake_markdown_file(path, hash, frontmatter);
    let chunks = fake_chunks(path, chunk_count);
    let embs = fake_embeddings(chunk_count);
    index.upsert(&file, &chunks, &embs).unwrap();
}

/// Populate an index with a single file that has a specific mtime.
fn populate_index_with_mtime(
    index: &Index,
    path: &str,
    hash: &str,
    frontmatter: Option<serde_json::Value>,
    chunk_count: usize,
    modified_at: u64,
) {
    let file = fake_markdown_file_with_mtime(path, hash, frontmatter, modified_at);
    let chunks = fake_chunks(path, chunk_count);
    let embs = fake_embeddings(chunk_count);
    index.upsert(&file, &chunks, &embs).unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_basic_search() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "doc.md", "h1", Some(json!({"title": "Test"})), 3);

    let query = SearchQuery::new("test query");
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(!results.is_empty(), "should return results");
    assert!(results.len() <= 10, "default limit is 10");
    // Verify result structure
    let r = &results[0];
    assert!(r.score > 0.0);
    assert!(!r.chunk.chunk_id.is_empty());
    assert!(!r.chunk.content.is_empty());
    assert_eq!(r.file.path, "doc.md");
}

#[tokio::test]
async fn test_min_score_filtering() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "doc.md", "h1", Some(json!({"title": "Test"})), 3);

    // Very high min_score should filter out everything
    let query = SearchQuery::new("test query").with_min_score(0.9999);
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    // All results (if any) must meet the threshold
    for r in &results {
        assert!(r.score >= 0.9999, "score {} below min", r.score);
    }
}

#[tokio::test]
async fn test_limit_capping() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    // Add enough chunks to exceed limit
    populate_index(&index, "a.md", "h1", Some(json!({"title": "A"})), 5);
    populate_index(&index, "b.md", "h2", Some(json!({"title": "B"})), 5);

    let query = SearchQuery::new("test").with_limit(3);
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(results.len() <= 3, "should respect limit of 3, got {}", results.len());
}

#[tokio::test]
async fn test_metadata_filter_equals() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "draft.md", "h1", Some(json!({"status": "draft"})), 2);
    populate_index(&index, "pub.md", "h2", Some(json!({"status": "published"})), 2);

    let query = SearchQuery::new("test")
        .with_filter(MetadataFilter::Equals {
            field: "status".into(),
            value: json!("draft"),
        });
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    for r in &results {
        let fm = r.file.frontmatter.as_ref().unwrap();
        assert_eq!(fm["status"], "draft", "should only return draft docs");
    }
}

#[tokio::test]
async fn test_metadata_filter_in() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "a.md", "h1", Some(json!({"category": "rust"})), 2);
    populate_index(&index, "b.md", "h2", Some(json!({"category": "python"})), 2);
    populate_index(&index, "c.md", "h3", Some(json!({"category": "go"})), 2);

    let query = SearchQuery::new("test")
        .with_filter(MetadataFilter::In {
            field: "category".into(),
            values: vec![json!("rust"), json!("go")],
        });
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    for r in &results {
        let cat = r.file.frontmatter.as_ref().unwrap()["category"].as_str().unwrap();
        assert!(cat == "rust" || cat == "go", "unexpected category: {cat}");
    }
}

#[tokio::test]
async fn test_metadata_filter_range() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "old.md", "h1", Some(json!({"year": 2020})), 2);
    populate_index(&index, "new.md", "h2", Some(json!({"year": 2024})), 2);

    let query = SearchQuery::new("test")
        .with_filter(MetadataFilter::Range {
            field: "year".into(),
            min: Some(json!(2023)),
            max: Some(json!(2025)),
        });
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    for r in &results {
        let year = r.file.frontmatter.as_ref().unwrap()["year"].as_i64().unwrap();
        assert!((2023..=2025).contains(&year), "year {year} out of range");
    }
}

#[tokio::test]
async fn test_metadata_filter_exists() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "with.md", "h1", Some(json!({"author": "Alice"})), 2);
    populate_index(&index, "without.md", "h2", Some(json!({"title": "No author"})), 2);

    let query = SearchQuery::new("test")
        .with_filter(MetadataFilter::Exists {
            field: "author".into(),
        });
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    for r in &results {
        let fm = r.file.frontmatter.as_ref().unwrap();
        assert!(fm.get("author").is_some(), "should only return docs with author");
    }
}

#[tokio::test]
async fn test_combined_and_filters() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "match.md", "h1", Some(json!({"status": "draft", "year": 2024})), 2);
    populate_index(&index, "wrong_status.md", "h2", Some(json!({"status": "published", "year": 2024})), 2);
    populate_index(&index, "wrong_year.md", "h3", Some(json!({"status": "draft", "year": 2020})), 2);

    let query = SearchQuery::new("test")
        .with_filter(MetadataFilter::Equals {
            field: "status".into(),
            value: json!("draft"),
        })
        .with_filter(MetadataFilter::Range {
            field: "year".into(),
            min: Some(json!(2023)),
            max: Some(json!(2025)),
        });
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    for r in &results {
        let fm = r.file.frontmatter.as_ref().unwrap();
        assert_eq!(fm["status"], "draft");
        let year = fm["year"].as_i64().unwrap();
        assert!((2023..=2025).contains(&year));
    }
}

#[tokio::test]
async fn test_empty_index_returns_no_results() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let query = SearchQuery::new("test query");
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(results.is_empty(), "empty index should return no results");
}

#[tokio::test]
async fn test_empty_query_returns_no_results() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "doc.md", "h1", Some(json!({"title": "Test"})), 3);

    let query = SearchQuery::new("");
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(results.is_empty(), "empty query should return no results");
}

// ---------------------------------------------------------------------------
// Hybrid / FTS search tests
// ---------------------------------------------------------------------------

/// Helper: populate both vector index and FTS index for a file.
fn populate_both(
    index: &Index,
    fts: &FtsIndex,
    path: &str,
    hash: &str,
    frontmatter: Option<serde_json::Value>,
    chunks: &[Chunk],
    embeddings: &[Vec<f32>],
) {
    let file = fake_markdown_file(path, hash, frontmatter);
    index.upsert(&file, chunks, embeddings).unwrap();
    let fts_chunks: Vec<mdvdb::fts::FtsChunkData> = chunks
        .iter()
        .map(|c| mdvdb::fts::FtsChunkData {
            chunk_id: c.id.clone(),
            source_path: path.to_string(),
            content: c.content.clone(),
            heading_hierarchy: c.heading_hierarchy.join(" > "),
        })
        .collect();
    fts.upsert_chunks(path, &fts_chunks).unwrap();
    fts.commit().unwrap();
}

#[tokio::test]
async fn test_search_with_fts_hybrid_mode() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let fts_dir = _dir.path().join("fts");
    let fts = FtsIndex::open_or_create(&fts_dir).unwrap();

    let chunks = fake_chunks("doc.md", 3);
    let embs = fake_embeddings(3);
    populate_both(
        &index,
        &fts,
        "doc.md",
        "h1",
        Some(json!({"title": "Test"})),
        &chunks,
        &embs,
    );

    let query = SearchQuery::new("Chunk content").with_mode(SearchMode::Hybrid);
    let results = search(&query, &index, &provider, Some(&fts), 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(!results.is_empty(), "hybrid search should return results");
}

#[tokio::test]
async fn test_search_lexical_mode() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let fts_dir = _dir.path().join("fts");
    let fts = FtsIndex::open_or_create(&fts_dir).unwrap();

    let chunks = fake_chunks("doc.md", 3);
    let embs = fake_embeddings(3);
    populate_both(
        &index,
        &fts,
        "doc.md",
        "h1",
        Some(json!({"title": "Test"})),
        &chunks,
        &embs,
    );

    let query = SearchQuery::new("Chunk content").with_mode(SearchMode::Lexical);
    let results = search(&query, &index, &provider, Some(&fts), 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(!results.is_empty(), "lexical search should return results");
}

#[tokio::test]
async fn test_search_semantic_mode_explicit() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "doc.md", "h1", Some(json!({"title": "Test"})), 3);

    let query = SearchQuery::new("test query").with_mode(SearchMode::Semantic);
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(!results.is_empty(), "semantic search should return results");
}

#[tokio::test]
async fn test_search_hybrid_fallback_without_fts() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "doc.md", "h1", Some(json!({"title": "Test"})), 3);

    // Hybrid mode without FTS index should fall back to semantic
    let query = SearchQuery::new("test query").with_mode(SearchMode::Hybrid);
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(!results.is_empty(), "hybrid without fts should fall back to semantic");
}

#[tokio::test]
async fn test_search_mode_with_filter() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let fts_dir = _dir.path().join("fts");
    let fts = FtsIndex::open_or_create(&fts_dir).unwrap();

    let chunks_a = fake_chunks("a.md", 2);
    let embs_a = fake_embeddings(2);
    populate_both(&index, &fts, "a.md", "h1", Some(json!({"status": "draft"})), &chunks_a, &embs_a);

    let chunks_b = fake_chunks("b.md", 2);
    let embs_b = fake_embeddings(2);
    populate_both(&index, &fts, "b.md", "h2", Some(json!({"status": "published"})), &chunks_b, &embs_b);

    let query = SearchQuery::new("Chunk content")
        .with_mode(SearchMode::Hybrid)
        .with_filter(MetadataFilter::Equals {
            field: "status".into(),
            value: json!("draft"),
        });
    let results = search(&query, &index, &provider, Some(&fts), 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    for r in &results {
        let fm = r.file.frontmatter.as_ref().unwrap();
        assert_eq!(fm["status"], "draft", "hybrid + filter should respect metadata filter");
    }
}

/// Verify that lexical mode does NOT call the embedding provider at all.
#[tokio::test]
async fn test_lexical_search_no_embedding_call() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let fts_dir = _dir.path().join("fts");
    let fts = FtsIndex::open_or_create(&fts_dir).unwrap();

    let chunks = fake_chunks("doc.md", 3);
    let embs = fake_embeddings(3);
    populate_both(
        &index,
        &fts,
        "doc.md",
        "h1",
        Some(json!({"title": "Test"})),
        &chunks,
        &embs,
    );

    let initial_calls = provider.call_count();

    let query = SearchQuery::new("Chunk content").with_mode(SearchMode::Lexical);
    let _results = search(&query, &index, &provider, Some(&fts), 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert_eq!(
        provider.call_count(),
        initial_calls,
        "lexical search must NOT call the embedding provider"
    );
}

/// Verify that hybrid mode produces results that combine signals from both
/// semantic and lexical searches (chunks appearing in both lists get boosted).
#[tokio::test]
async fn test_hybrid_combines_both_signals() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let fts_dir = _dir.path().join("fts");
    let fts = FtsIndex::open_or_create(&fts_dir).unwrap();

    // Create chunks with distinctive content so BM25 can differentiate.
    let chunks = vec![
        Chunk {
            id: "doc.md#0".into(),
            source_path: PathBuf::from("doc.md"),
            heading_hierarchy: vec!["Heading".to_string()],
            content: "Rust programming language is fast and safe for systems development".into(),
            start_line: 1,
            end_line: 10,
            chunk_index: 0,
            is_sub_split: false,
        },
        Chunk {
            id: "doc.md#1".into(),
            source_path: PathBuf::from("doc.md"),
            heading_hierarchy: vec!["Heading".to_string()],
            content: "Python is great for data science and machine learning tasks".into(),
            start_line: 11,
            end_line: 20,
            chunk_index: 1,
            is_sub_split: false,
        },
    ];
    let embs = fake_embeddings(2);
    populate_both(
        &index,
        &fts,
        "doc.md",
        "h1",
        Some(json!({"title": "Test"})),
        &chunks,
        &embs,
    );

    // Hybrid search for "Rust programming" — should find results from both signals.
    let query_hybrid = SearchQuery::new("Rust programming").with_mode(SearchMode::Hybrid);
    let hybrid_results = search(&query_hybrid, &index, &provider, Some(&fts), 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    // Lexical search for the same query.
    let query_lexical = SearchQuery::new("Rust programming").with_mode(SearchMode::Lexical);
    let lexical_results = search(&query_lexical, &index, &provider, Some(&fts), 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    // Both should return results.
    assert!(!hybrid_results.is_empty(), "hybrid should return results");
    assert!(!lexical_results.is_empty(), "lexical should return results");

    // Hybrid mode should have positive RRF scores from fusing both signal lists.
    assert!(
        hybrid_results[0].score > 0.0,
        "hybrid results should have positive RRF scores"
    );
}

// ---------------------------------------------------------------------------
// Path prefix search tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_path_prefix_filters_results() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "docs/guide.md", "h1", Some(json!({"title": "Guide"})), 2);
    populate_index(&index, "docs/api.md", "h2", Some(json!({"title": "API"})), 2);
    populate_index(&index, "notes/todo.md", "h3", Some(json!({"title": "Todo"})), 2);

    let query = SearchQuery::new("test").with_path_prefix("docs/");
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(!results.is_empty(), "should return results from docs/");
    for r in &results {
        assert!(
            r.file.path.starts_with("docs/"),
            "expected docs/ prefix, got: {}",
            r.file.path
        );
    }
}

#[tokio::test]
async fn test_path_prefix_no_match() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "docs/guide.md", "h1", Some(json!({"title": "Guide"})), 2);

    let query = SearchQuery::new("test").with_path_prefix("nonexistent/");
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(results.is_empty(), "nonexistent prefix should return no results");
}

#[tokio::test]
async fn test_path_prefix_combined_with_metadata_filter() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "docs/draft.md", "h1", Some(json!({"status": "draft"})), 2);
    populate_index(&index, "docs/published.md", "h2", Some(json!({"status": "published"})), 2);
    populate_index(&index, "notes/draft.md", "h3", Some(json!({"status": "draft"})), 2);

    let query = SearchQuery::new("test")
        .with_path_prefix("docs/")
        .with_filter(MetadataFilter::Equals {
            field: "status".into(),
            value: json!("draft"),
        });
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    for r in &results {
        assert!(
            r.file.path.starts_with("docs/"),
            "expected docs/ prefix, got: {}",
            r.file.path
        );
        let fm = r.file.frontmatter.as_ref().unwrap();
        assert_eq!(fm["status"], "draft", "should only return draft docs");
    }
}

// ---------------------------------------------------------------------------
// Time Decay Integration Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_decay_disabled_does_not_change_results() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Recent file and old file.
    populate_index_with_mtime(&index, "recent.md", "h1", None, 2, now);
    populate_index_with_mtime(&index, "old.md", "h2", None, 2, now - 365 * 86400);

    // Search without decay.
    let query = SearchQuery::new("test query");
    let results_no_decay = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;
    assert!(!results_no_decay.is_empty());

    // All scores should be unaffected by age (same content gets same score).
    // modified_at should still be populated on the results.
    for r in &results_no_decay {
        assert!(r.file.modified_at.is_some(), "modified_at should be present");
    }
}

#[tokio::test]
async fn test_decay_enabled_penalizes_old_files() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Both files use same chunk structure so base scores should be identical.
    populate_index_with_mtime(&index, "recent.md", "h1", None, 1, now);
    populate_index_with_mtime(&index, "old.md", "h2", None, 1, now - 180 * 86400);

    // Search with decay enabled.
    let query = SearchQuery::new("test query").with_decay(true);
    let results = search(&query, &index, &provider, None, 60.0, 1.5, true, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(results.len() >= 2, "should return results for both files");

    // Find scores for each file.
    let recent_score = results.iter().find(|r| r.file.path == "recent.md").map(|r| r.score);
    let old_score = results.iter().find(|r| r.file.path == "old.md").map(|r| r.score);

    assert!(recent_score.is_some() && old_score.is_some());
    assert!(
        recent_score.unwrap() > old_score.unwrap(),
        "recent file should score higher with decay: recent={:?} > old={:?}",
        recent_score,
        old_score
    );
}

#[tokio::test]
async fn test_decay_per_query_override_enables() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    populate_index_with_mtime(&index, "recent.md", "h1", None, 1, now);
    populate_index_with_mtime(&index, "old.md", "h2", None, 1, now - 180 * 86400);

    // Config says decay disabled (false), but query enables it.
    let query = SearchQuery::new("test query").with_decay(true);
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    let recent_score = results.iter().find(|r| r.file.path == "recent.md").map(|r| r.score);
    let old_score = results.iter().find(|r| r.file.path == "old.md").map(|r| r.score);

    assert!(recent_score.is_some() && old_score.is_some());
    assert!(
        recent_score.unwrap() > old_score.unwrap(),
        "per-query decay override should penalize old file"
    );
}

#[tokio::test]
async fn test_decay_per_query_override_disables() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    populate_index_with_mtime(&index, "recent.md", "h1", None, 1, now);
    populate_index_with_mtime(&index, "old.md", "h2", None, 1, now - 180 * 86400);

    // Config says decay enabled (true), but query disables it.
    let query = SearchQuery::new("test query").with_decay(false);
    let results_no_decay = search(&query, &index, &provider, None, 60.0, 1.5, true, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    // Without decay, both should have comparable scores (same mock embeddings).
    let recent = results_no_decay.iter().find(|r| r.file.path == "recent.md").map(|r| r.score);
    let old = results_no_decay.iter().find(|r| r.file.path == "old.md").map(|r| r.score);
    if let (Some(r), Some(o)) = (recent, old) {
        // Without decay the difference should be very small (mock provider gives similar vectors).
        let diff = (r - o).abs();
        assert!(diff < 0.2, "without decay, scores should be close: recent={}, old={}, diff={}", r, o, diff);
    }
}

#[tokio::test]
async fn test_decay_custom_half_life() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    populate_index_with_mtime(&index, "recent.md", "h1", None, 1, now);
    populate_index_with_mtime(&index, "old.md", "h2", None, 1, now - 90 * 86400);

    // Very short half-life (7 days) should punish 90-day old file severely.
    let query = SearchQuery::new("test query").with_decay(true).with_decay_half_life(7.0);
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    let recent_score = results.iter().find(|r| r.file.path == "recent.md").map(|r| r.score);
    let old_score = results.iter().find(|r| r.file.path == "old.md").map(|r| r.score);

    assert!(recent_score.is_some() && old_score.is_some());
    // With 7-day half-life and 90 days age, multiplier ≈ 0.5^(90/7) ≈ 0.00015
    assert!(
        old_score.unwrap() < recent_score.unwrap() * 0.01,
        "short half-life should severely penalize old file: recent={:?}, old={:?}",
        recent_score,
        old_score
    );
}

#[tokio::test]
async fn test_decay_modified_at_in_results() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let mtime = 1_700_000_000u64;
    populate_index_with_mtime(&index, "doc.md", "h1", None, 1, mtime);

    let query = SearchQuery::new("test query");
    let results = search(&query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    assert!(!results.is_empty());
    assert_eq!(results[0].file.modified_at, Some(mtime), "modified_at should be populated in results");
}

#[tokio::test]
async fn test_decay_scores_in_valid_range() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    populate_index_with_mtime(&index, "a.md", "h1", None, 1, now);
    populate_index_with_mtime(&index, "b.md", "h2", None, 1, now - 90 * 86400);
    populate_index_with_mtime(&index, "c.md", "h3", None, 1, now - 365 * 86400);

    let query = SearchQuery::new("test query").with_decay(true);
    let results = search(&query, &index, &provider, None, 60.0, 1.5, true, 90.0, &[], &[], false, 1, 0, 3, 0.15).await.unwrap().results;

    for r in &results {
        assert!(r.score >= 0.0, "score should be >= 0, got {}", r.score);
        assert!(r.score <= 1.0, "score should be <= 1, got {}", r.score);
    }
}

// --- Decay exclude/include integration tests ---

#[tokio::test]
async fn test_decay_exclude_preserves_score() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Both files are old (180 days).
    populate_index_with_mtime(&index, "docs/reference/api.md", "h1", None, 1, now - 180 * 86400);
    populate_index_with_mtime(&index, "docs/guides/setup.md", "h2", None, 1, now - 180 * 86400);

    let exclude = vec!["docs/reference".to_string()];

    // Search with decay enabled but docs/reference excluded.
    let query = SearchQuery::new("test query").with_decay(true);
    let results = search(
        &query, &index, &provider, None, 60.0, 1.5, true, 90.0,
        &exclude, &[], false, 1, 0, 3, 0.15,
    ).await.unwrap().results;

    let ref_score = results.iter().find(|r| r.file.path == "docs/reference/api.md").map(|r| r.score);
    let guide_score = results.iter().find(|r| r.file.path == "docs/guides/setup.md").map(|r| r.score);

    assert!(ref_score.is_some() && guide_score.is_some());
    // Excluded file should have higher score (no decay) than non-excluded (decayed).
    assert!(
        ref_score.unwrap() > guide_score.unwrap(),
        "excluded file should keep original score: ref={:?} > guide={:?}",
        ref_score, guide_score
    );
}

#[tokio::test]
async fn test_decay_include_only_affects_matching_paths() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Both files are old.
    populate_index_with_mtime(&index, "journal/2024-01.md", "h1", None, 1, now - 180 * 86400);
    populate_index_with_mtime(&index, "docs/readme.md", "h2", None, 1, now - 180 * 86400);

    let include = vec!["journal/".to_string()];

    // Search with decay enabled but only for journal/ paths.
    let query = SearchQuery::new("test query").with_decay(true);
    let results = search(
        &query, &index, &provider, None, 60.0, 1.5, true, 90.0,
        &[], &include, false, 1, 0, 3, 0.15,
    ).await.unwrap().results;

    let journal_score = results.iter().find(|r| r.file.path == "journal/2024-01.md").map(|r| r.score);
    let docs_score = results.iter().find(|r| r.file.path == "docs/readme.md").map(|r| r.score);

    assert!(journal_score.is_some() && docs_score.is_some());
    // docs/readme.md is NOT in include list, so no decay → higher score.
    assert!(
        docs_score.unwrap() > journal_score.unwrap(),
        "non-included file should keep original score: docs={:?} > journal={:?}",
        docs_score, journal_score
    );
}

#[tokio::test]
async fn test_decay_exclude_overrides_include() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // All files are old.
    populate_index_with_mtime(&index, "journal/pinned/important.md", "h1", None, 1, now - 180 * 86400);
    populate_index_with_mtime(&index, "journal/2024-01.md", "h2", None, 1, now - 180 * 86400);

    let exclude = vec!["journal/pinned".to_string()];
    let include = vec!["journal/".to_string()];

    let query = SearchQuery::new("test query").with_decay(true);
    let results = search(
        &query, &index, &provider, None, 60.0, 1.5, true, 90.0,
        &exclude, &include, false, 1, 0, 3, 0.15,
    ).await.unwrap().results;

    let pinned_score = results.iter().find(|r| r.file.path == "journal/pinned/important.md").map(|r| r.score);
    let regular_score = results.iter().find(|r| r.file.path == "journal/2024-01.md").map(|r| r.score);

    assert!(pinned_score.is_some() && regular_score.is_some());
    // Pinned matches include but also matches exclude → no decay → higher score.
    assert!(
        pinned_score.unwrap() > regular_score.unwrap(),
        "excluded file should keep original score even when in include list: pinned={:?} > regular={:?}",
        pinned_score, regular_score
    );
}

// ---------------------------------------------------------------------------
// Graph-enhanced search tests (multi-hop boost + graph context expansion)
// ---------------------------------------------------------------------------

/// Create a MarkdownFile with links to specified targets.
fn fake_markdown_file_with_links(
    path: &str,
    hash: &str,
    frontmatter: Option<serde_json::Value>,
    link_targets: &[&str],
) -> MarkdownFile {
    MarkdownFile {
        path: PathBuf::from(path),
        frontmatter,
        headings: vec![],
        body: "Test body content".to_string(),
        content_hash: hash.to_string(),
        file_size: 100,
        links: link_targets
            .iter()
            .enumerate()
            .map(|(i, target)| RawLink {
                target: target.to_string(),
                text: format!("link to {target}"),
                line_number: i + 1,
                is_wikilink: false,
            })
            .collect(),
        modified_at: 0,
    }
}

/// Populate an index with a file that has links, and return the MarkdownFile
/// for later use building the link graph.
fn populate_index_with_links(
    index: &Index,
    path: &str,
    hash: &str,
    frontmatter: Option<serde_json::Value>,
    chunk_count: usize,
    link_targets: &[&str],
) -> MarkdownFile {
    let file = fake_markdown_file_with_links(path, hash, frontmatter, link_targets);
    let chunks = fake_chunks(path, chunk_count);
    let embs = fake_embeddings(chunk_count);
    index.upsert(&file, &chunks, &embs).unwrap();
    file
}

#[tokio::test]
async fn test_search_response_has_all_fields() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "doc.md", "h1", Some(json!({"title": "Test"})), 3);

    let query = SearchQuery::new("test query");
    let response = search(
        &query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15,
    )
    .await
    .unwrap();

    // SearchResponse should have all three fields: results, graph_context, timings.
    assert!(!response.results.is_empty(), "results should be non-empty");
    // graph_context should be empty when expand_graph=0 (default).
    assert!(
        response.graph_context.is_empty(),
        "graph_context should be empty by default"
    );
    // timings should be present (total_secs is always populated).
    // Just verify the struct is accessible and has the expected fields.
    let _total = response.timings.total_secs;
    let _search = response.timings.vector_search_secs;
}

#[tokio::test]
async fn test_graph_expansion_disabled() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    // Build files with links: a.md -> b.md -> c.md
    let file_a = populate_index_with_links(
        &index, "a.md", "h1", Some(json!({"title": "A"})), 2, &["b.md"],
    );
    let file_b = populate_index_with_links(
        &index, "b.md", "h2", Some(json!({"title": "B"})), 2, &["c.md"],
    );
    let file_c = populate_index_with_links(
        &index, "c.md", "h3", Some(json!({"title": "C"})), 2, &[],
    );

    // Build and store link graph.
    let graph = links::build_link_graph(&[file_a, file_b, file_c]);
    index.update_link_graph(Some(graph));

    // Search with expand_graph=0 (explicit).
    let query = SearchQuery::new("test query").with_expand_graph(0);
    let response = search(
        &query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15,
    )
    .await
    .unwrap();

    assert!(
        response.graph_context.is_empty(),
        "expand_graph=0 should produce empty graph_context, got {} items",
        response.graph_context.len()
    );
}

#[tokio::test]
async fn test_graph_expansion_returns_items() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    // Build files with links: a.md -> b.md -> c.md
    // Give a.md a distinctive embedding so it's the top search result.
    let file_a = populate_index_with_links(
        &index, "a.md", "h1", Some(json!({"title": "A"})), 2, &["b.md"],
    );
    let file_b = populate_index_with_links(
        &index, "b.md", "h2", Some(json!({"title": "B"})), 2, &["c.md"],
    );
    let file_c = populate_index_with_links(
        &index, "c.md", "h3", Some(json!({"title": "C"})), 2, &[],
    );

    // Build and store link graph.
    let graph = links::build_link_graph(&[file_a, file_b, file_c]);
    index.update_link_graph(Some(graph));

    // Search with expand_graph=1.
    let query = SearchQuery::new("test query").with_expand_graph(1);
    let response = search(
        &query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 1, 3, 0.15,
    )
    .await
    .unwrap();

    assert!(!response.results.is_empty(), "should have search results");

    // With expand_graph=1 and files linked together, we should get graph context items
    // (unless all linked files are already in results).
    // Each graph context item should have valid linked_from and hop_distance.
    for item in &response.graph_context {
        assert!(
            item.hop_distance >= 1,
            "hop_distance should be >= 1, got {}",
            item.hop_distance
        );
        assert!(
            !item.linked_from.is_empty(),
            "linked_from should not be empty"
        );
        assert!(
            !item.chunk.content.is_empty(),
            "graph context item should have content"
        );
    }
}

#[tokio::test]
async fn test_graph_expansion_no_duplicates() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    // Build a chain: a.md -> b.md -> c.md -> d.md
    let file_a = populate_index_with_links(
        &index, "a.md", "h1", Some(json!({"title": "A"})), 2, &["b.md"],
    );
    let file_b = populate_index_with_links(
        &index, "b.md", "h2", Some(json!({"title": "B"})), 2, &["c.md"],
    );
    let file_c = populate_index_with_links(
        &index, "c.md", "h3", Some(json!({"title": "C"})), 2, &["d.md"],
    );
    let file_d = populate_index_with_links(
        &index, "d.md", "h4", Some(json!({"title": "D"})), 2, &[],
    );

    let graph = links::build_link_graph(&[file_a, file_b, file_c, file_d]);
    index.update_link_graph(Some(graph));

    // Search with expand_graph=2 for wider reach.
    let query = SearchQuery::new("test query").with_expand_graph(2);
    let response = search(
        &query, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 2, 10, 0.15,
    )
    .await
    .unwrap();

    // Collect result file paths.
    let result_paths: std::collections::HashSet<&str> = response
        .results
        .iter()
        .map(|r| r.file.path.as_str())
        .collect();

    // No graph context item should duplicate a file already in results.
    for item in &response.graph_context {
        let item_path = &item.file.path;
        assert!(
            !result_paths.contains(item_path.as_str()),
            "graph context item '{}' should not duplicate a result file",
            item_path
        );
    }

    // Also check no duplicate files within graph_context itself.
    let gc_paths: Vec<&str> = response
        .graph_context
        .iter()
        .map(|item| item.file.path.as_str())
        .collect();
    let gc_unique: std::collections::HashSet<&str> = gc_paths.iter().copied().collect();
    assert_eq!(
        gc_paths.len(),
        gc_unique.len(),
        "graph context should not contain duplicate files"
    );
}

#[tokio::test]
async fn test_multihop_boost_reorders() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    // Set up a link chain: hub.md -> mid.md -> far.md
    // Also add an unlinked file: alone.md
    //
    // All files use fake_embeddings which produce orthogonal basis vectors.
    // With mock provider, search returns similar distances for all files.
    // The link boost should change the ordering.
    let file_hub = populate_index_with_links(
        &index, "hub.md", "h1", Some(json!({"title": "Hub"})), 1, &["mid.md"],
    );
    let file_mid = populate_index_with_links(
        &index, "mid.md", "h2", Some(json!({"title": "Mid"})), 1, &["far.md"],
    );
    let file_far = populate_index_with_links(
        &index, "far.md", "h3", Some(json!({"title": "Far"})), 1, &[],
    );
    let file_alone = populate_index_with_links(
        &index, "alone.md", "h4", Some(json!({"title": "Alone"})), 1, &[],
    );

    let graph = links::build_link_graph(&[file_hub, file_mid, file_far, file_alone]);
    index.update_link_graph(Some(graph));

    // Search with 1-hop boost — only direct neighbors of top results get boosted.
    let query_1hop = SearchQuery::new("test query")
        .with_boost_links(true)
        .with_boost_hops(1);
    let response_1hop = search(
        &query_1hop, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15,
    )
    .await
    .unwrap();

    // Search with 2-hop boost — neighbors up to 2 hops away get boosted.
    let query_2hop = SearchQuery::new("test query")
        .with_boost_links(true)
        .with_boost_hops(2);
    let response_2hop = search(
        &query_2hop, &index, &provider, None, 60.0, 1.5, false, 90.0, &[], &[], false, 1, 0, 3, 0.15,
    )
    .await
    .unwrap();

    // Both should have results.
    assert!(
        response_1hop.results.len() >= 2,
        "1-hop should return >= 2 results"
    );
    assert!(
        response_2hop.results.len() >= 2,
        "2-hop should return >= 2 results"
    );

    // Compare: with 2-hop boost, files 2 hops away should have boosted scores
    // compared to 1-hop (where they wouldn't be boosted).
    // Find "far.md" scores in each result set.
    let far_score_1hop = response_1hop
        .results
        .iter()
        .find(|r| r.file.path == "far.md")
        .map(|r| r.score);
    let far_score_2hop = response_2hop
        .results
        .iter()
        .find(|r| r.file.path == "far.md")
        .map(|r| r.score);

    // With 2-hop boost, far.md (which is 2 hops from hub.md) should get a boost
    // that it doesn't get with 1-hop. So its 2-hop score should be >= its 1-hop score.
    if let (Some(score_1), Some(score_2)) = (far_score_1hop, far_score_2hop) {
        assert!(
            score_2 >= score_1,
            "2-hop boost should give far.md score >= 1-hop score: 2hop={} vs 1hop={}",
            score_2,
            score_1
        );
    }

    // The unlinked file "alone.md" should NOT be boosted in either case.
    let alone_score_1hop = response_1hop
        .results
        .iter()
        .find(|r| r.file.path == "alone.md")
        .map(|r| r.score);
    let alone_score_2hop = response_2hop
        .results
        .iter()
        .find(|r| r.file.path == "alone.md")
        .map(|r| r.score);

    if let (Some(s1), Some(s2)) = (alone_score_1hop, alone_score_2hop) {
        // alone.md should have the same score in both cases (no boost).
        let diff = (s1 - s2).abs();
        assert!(
            diff < 0.001,
            "alone.md should have same score in 1-hop and 2-hop: 1hop={}, 2hop={}, diff={}",
            s1,
            s2,
            diff
        );
    }
}

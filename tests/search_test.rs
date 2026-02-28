use std::path::PathBuf;

use mdvdb::chunker::Chunk;
use mdvdb::embedding::mock::MockProvider;
use mdvdb::index::{EmbeddingConfig, Index};
use mdvdb::parser::MarkdownFile;
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
    MarkdownFile {
        path: PathBuf::from(path),
        frontmatter,
        headings: vec![],
        body: "Test body content".to_string(),
        content_hash: hash.to_string(),
        file_size: 100,
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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

    assert!(results.is_empty(), "empty index should return no results");
}

#[tokio::test]
async fn test_empty_query_returns_no_results() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "doc.md", "h1", Some(json!({"title": "Test"})), 3);

    let query = SearchQuery::new("");
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, Some(&fts), 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, Some(&fts), 60.0).await.unwrap();

    assert!(!results.is_empty(), "lexical search should return results");
}

#[tokio::test]
async fn test_search_semantic_mode_explicit() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let provider = mock_provider();

    populate_index(&index, "doc.md", "h1", Some(json!({"title": "Test"})), 3);

    let query = SearchQuery::new("test query").with_mode(SearchMode::Semantic);
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, Some(&fts), 60.0).await.unwrap();

    for r in &results {
        let fm = r.file.frontmatter.as_ref().unwrap();
        assert_eq!(fm["status"], "draft", "hybrid + filter should respect metadata filter");
    }
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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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
    let results = search(&query, &index, &provider, None, 60.0).await.unwrap();

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

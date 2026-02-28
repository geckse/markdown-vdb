use std::collections::HashMap;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::embedding::batch::{embed_chunks, Chunk};
use mdvdb::embedding::mock::MockProvider;
use mdvdb::embedding::provider::{create_provider, EmbeddingProvider};

fn make_chunk(id: &str, path: &str, content: &str) -> Chunk {
    Chunk {
        id: id.to_string(),
        source_path: PathBuf::from(path),
        content: content.to_string(),
    }
}

fn base_config() -> Config {
    Config {
        embedding_provider: EmbeddingProviderType::OpenAI,
        embedding_model: "text-embedding-3-small".into(),
        embedding_dimensions: 1536,
        embedding_batch_size: 100,
        openai_api_key: Some("sk-test-key".into()),
        ollama_host: "http://localhost:11434".into(),
        embedding_endpoint: None,
        source_dirs: vec![PathBuf::from(".")],
        ignore_patterns: vec![],
        watch_enabled: true,
        watch_debounce_ms: 300,
        chunk_max_tokens: 512,
        chunk_overlap_tokens: 50,
        clustering_enabled: true,
        clustering_rebalance_threshold: 50,
        search_default_limit: 10,
        search_min_score: 0.0,
        search_default_mode: mdvdb::SearchMode::Hybrid,
        search_rrf_k: 60.0,
        bm25_norm_k: 1.5,
    }
}

#[tokio::test]
async fn test_embed_chunks_with_mock() {
    let provider = MockProvider::new(128);
    let chunks = vec![
        make_chunk(
            "docs/intro.md#0",
            "docs/intro.md",
            "# Introduction\n\nWelcome to the project.",
        ),
        make_chunk(
            "docs/intro.md#1",
            "docs/intro.md",
            "## Getting Started\n\nFollow these steps.",
        ),
        make_chunk(
            "docs/guide.md#0",
            "docs/guide.md",
            "# User Guide\n\nThis guide covers usage.",
        ),
    ];

    let existing: HashMap<PathBuf, String> = HashMap::new();
    let mut current = HashMap::new();
    current.insert(PathBuf::from("docs/intro.md"), "hash_intro".into());
    current.insert(PathBuf::from("docs/guide.md"), "hash_guide".into());

    let result = embed_chunks(&provider, &chunks, &existing, &current, 10)
        .await
        .unwrap();

    // All chunks should be embedded (no existing hashes to match)
    assert_eq!(result.embeddings.len(), 3);
    assert!(result.skipped.is_empty());
    assert!(result.api_calls > 0);

    // Each chunk ID is present
    assert!(result.embeddings.contains_key("docs/intro.md#0"));
    assert!(result.embeddings.contains_key("docs/intro.md#1"));
    assert!(result.embeddings.contains_key("docs/guide.md#0"));

    // Correct dimensions
    for vector in result.embeddings.values() {
        assert_eq!(
            vector.len(),
            128,
            "embedding vector should have 128 dimensions"
        );
    }
}

#[tokio::test]
async fn test_embed_chunks_hash_skip() {
    let provider = MockProvider::new(64);
    let chunks = vec![
        make_chunk("unchanged.md#0", "unchanged.md", "old content"),
        make_chunk("unchanged.md#1", "unchanged.md", "more old content"),
        make_chunk("changed.md#0", "changed.md", "new content here"),
    ];

    // unchanged.md has matching hashes -> should be skipped
    let mut existing = HashMap::new();
    existing.insert(PathBuf::from("unchanged.md"), "same_hash".into());
    existing.insert(PathBuf::from("changed.md"), "old_hash".into());

    let mut current = HashMap::new();
    current.insert(PathBuf::from("unchanged.md"), "same_hash".into());
    current.insert(PathBuf::from("changed.md"), "new_hash".into());

    let result = embed_chunks(&provider, &chunks, &existing, &current, 10)
        .await
        .unwrap();

    // Only changed.md#0 should be embedded
    assert_eq!(result.embeddings.len(), 1);
    assert!(result.embeddings.contains_key("changed.md#0"));

    // unchanged.md chunks should be skipped
    assert_eq!(result.skipped.len(), 2);
    assert!(result.skipped.contains(&"unchanged.md#0".to_string()));
    assert!(result.skipped.contains(&"unchanged.md#1".to_string()));

    // Provider should have been called once (for the changed file batch)
    assert_eq!(provider.call_count(), 1);
}

#[test]
fn test_provider_factory_from_config() {
    let mut config = base_config();
    config.embedding_provider = EmbeddingProviderType::Ollama;

    let provider = create_provider(&config).unwrap();
    assert_eq!(provider.name(), "ollama");
    assert_eq!(provider.dimensions(), 1536);
}

#[tokio::test]
async fn test_mock_provider_integration() {
    let provider = MockProvider::new(256);
    let texts = vec!["Hello world".to_string(), "Rust programming".to_string()];

    // First call
    let first = provider.embed_batch(&texts).await.unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].len(), 256);
    assert_eq!(first[1].len(), 256);

    // Second call with same input — must produce identical results
    let second = provider.embed_batch(&texts).await.unwrap();
    assert_eq!(
        first, second,
        "MockProvider must return consistent results across calls"
    );

    // Different input produces different vectors
    let different = provider
        .embed_batch(&["Something else".to_string()])
        .await
        .unwrap();
    assert_ne!(
        first[0], different[0],
        "different text should produce different vectors"
    );

    // Call count tracks correctly
    assert_eq!(provider.call_count(), 3);
}

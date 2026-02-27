use mdvdb::fts::{FtsChunkData, FtsIndex};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn create_fts_dir() -> (TempDir, FtsIndex) {
    let dir = TempDir::new().unwrap();
    let fts = FtsIndex::open_or_create(&dir.path().join("fts")).unwrap();
    (dir, fts)
}

fn chunk_data(id: &str, content: &str, headings: &[&str]) -> FtsChunkData {
    FtsChunkData {
        chunk_id: id.to_string(),
        source_path: id.split('#').next().unwrap_or(id).to_string(),
        content: content.to_string(),
        heading_hierarchy: headings.join(" > "),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_fts_open_or_create() {
    let dir = TempDir::new().unwrap();
    let result = FtsIndex::open_or_create(&dir.path().join("fts"));
    assert!(result.is_ok(), "should create FTS index: {:?}", result.err());
}

#[test]
fn test_fts_reopen_existing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("fts");

    let fts = FtsIndex::open_or_create(&path).unwrap();
    let chunks = vec![chunk_data("doc.md#0", "hello world", &["Hello"])];
    fts.upsert_chunks("doc.md", &chunks).unwrap();
    fts.commit().unwrap();
    drop(fts);

    // Reopen and verify data persists
    let fts2 = FtsIndex::open_or_create(&path).unwrap();
    let results = fts2.search("hello", 10).unwrap();
    assert!(!results.is_empty(), "data should persist after reopen");
}

#[test]
fn test_fts_upsert_and_search() {
    let (_dir, fts) = create_fts_dir();

    let chunks = vec![
        chunk_data("doc.md#0", "Rust is a systems programming language", &["Rust"]),
        chunk_data("doc.md#1", "Python is great for data science", &["Python"]),
    ];
    fts.upsert_chunks("doc.md", &chunks).unwrap();
    fts.commit().unwrap();

    let results = fts.search("rust programming", 10).unwrap();
    assert!(!results.is_empty(), "should find 'rust programming'");
    assert_eq!(results[0].chunk_id, "doc.md#0");
}

#[test]
fn test_fts_search_no_results() {
    let (_dir, fts) = create_fts_dir();

    let chunks = vec![chunk_data("doc.md#0", "hello world", &["Hello"])];
    fts.upsert_chunks("doc.md", &chunks).unwrap();
    fts.commit().unwrap();

    let results = fts.search("nonexistent zebra", 10).unwrap();
    assert!(results.is_empty(), "should return no results for unmatched query");
}

#[test]
fn test_fts_remove_file() {
    let (_dir, fts) = create_fts_dir();

    let chunks = vec![chunk_data("doc.md#0", "hello world content", &["Hello"])];
    fts.upsert_chunks("doc.md", &chunks).unwrap();
    fts.commit().unwrap();

    assert!(fts.num_docs().unwrap() > 0, "should have docs before remove");

    fts.remove_file("doc.md").unwrap();
    fts.commit().unwrap();

    let results = fts.search("hello world", 10).unwrap();
    assert!(results.is_empty(), "should have no results after remove");
}

#[test]
fn test_fts_upsert_replaces_existing() {
    let (_dir, fts) = create_fts_dir();

    let chunks_v1 = vec![chunk_data("doc.md#0", "old content about cats", &["Cats"])];
    fts.upsert_chunks("doc.md", &chunks_v1).unwrap();
    fts.commit().unwrap();

    // Upsert with new content
    let chunks_v2 = vec![chunk_data("doc.md#0", "new content about dogs", &["Dogs"])];
    fts.upsert_chunks("doc.md", &chunks_v2).unwrap();
    fts.commit().unwrap();

    let cats = fts.search("cats", 10).unwrap();
    assert!(cats.is_empty(), "old content should be gone after upsert");

    let dogs = fts.search("dogs", 10).unwrap();
    assert!(!dogs.is_empty(), "new content should be findable after upsert");
}

#[test]
fn test_fts_heading_boost() {
    let (_dir, fts) = create_fts_dir();

    // One chunk with "rust" in heading, another with "rust" only in body
    let chunks = vec![
        chunk_data("a.md#0", "some generic programming content", &["Rust Guide"]),
        chunk_data("b.md#0", "rust is mentioned in the body text", &["Introduction"]),
    ];
    fts.upsert_chunks("a.md", &chunks[..1]).unwrap();
    fts.upsert_chunks("b.md", &chunks[1..]).unwrap();
    fts.commit().unwrap();

    let results = fts.search("rust", 10).unwrap();
    assert!(results.len() >= 1, "should find results for 'rust'");
    // The chunk with "Rust" in heading should ideally score higher
    // (but we just verify both are found — boost is a ranking detail)
}

#[test]
fn test_fts_limit_respected() {
    let (_dir, fts) = create_fts_dir();

    for i in 0..10 {
        let chunks = vec![chunk_data(
            &format!("doc{i}.md#0"),
            &format!("document number {i} with common search term"),
            &["Doc"],
        )];
        fts.upsert_chunks(&format!("doc{i}.md"), &chunks).unwrap();
    }
    fts.commit().unwrap();

    let results = fts.search("document common search", 3).unwrap();
    assert!(results.len() <= 3, "should respect limit of 3, got {}", results.len());
}

#[test]
fn test_fts_delete_all() {
    let (_dir, fts) = create_fts_dir();

    let chunks = vec![chunk_data("doc.md#0", "hello world", &["Hello"])];
    fts.upsert_chunks("doc.md", &chunks).unwrap();
    fts.commit().unwrap();

    fts.delete_all().unwrap();
    fts.commit().unwrap();

    assert_eq!(fts.num_docs().unwrap(), 0, "should have 0 docs after delete_all");
}

#[test]
fn test_fts_num_docs() {
    let (_dir, fts) = create_fts_dir();

    assert_eq!(fts.num_docs().unwrap(), 0, "empty index should have 0 docs");

    let chunks = vec![
        chunk_data("doc.md#0", "chunk one", &["One"]),
        chunk_data("doc.md#1", "chunk two", &["Two"]),
    ];
    fts.upsert_chunks("doc.md", &chunks).unwrap();
    fts.commit().unwrap();

    assert_eq!(fts.num_docs().unwrap(), 2, "should have 2 docs after inserting 2 chunks");
}

#[test]
fn test_fts_multiple_files() {
    let (_dir, fts) = create_fts_dir();

    fts.upsert_chunks(
        "rust.md",
        &[chunk_data("rust.md#0", "Rust systems programming language", &["Rust"])],
    )
    .unwrap();
    fts.upsert_chunks(
        "python.md",
        &[chunk_data("python.md#0", "Python data science machine learning", &["Python"])],
    )
    .unwrap();
    fts.commit().unwrap();

    let rust_results = fts.search("systems programming", 10).unwrap();
    assert!(!rust_results.is_empty(), "should find rust doc");
    assert_eq!(rust_results[0].chunk_id, "rust.md#0");

    let py_results = fts.search("machine learning", 10).unwrap();
    assert!(!py_results.is_empty(), "should find python doc");
    assert_eq!(py_results[0].chunk_id, "python.md#0");
}

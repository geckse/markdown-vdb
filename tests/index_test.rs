use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use mdvdb::chunker::Chunk;
use mdvdb::error::Error;
use mdvdb::index::{EmbeddingConfig, Index};
use mdvdb::parser::MarkdownFile;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_config() -> EmbeddingConfig {
    EmbeddingConfig {
        provider: "OpenAI".to_string(),
        model: "test-model".to_string(),
        dimensions: 8,
    }
}

fn test_embedding_config() -> EmbeddingConfig {
    test_config()
}

fn fake_markdown_file(path: &str, hash: &str) -> MarkdownFile {
    MarkdownFile {
        path: PathBuf::from(path),
        frontmatter: Some(serde_json::json!({"title": "Test"})),
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
            content: format!("Chunk {i} content"),
            start_line: i * 10 + 1,
            end_line: (i + 1) * 10,
            chunk_index: i,
            is_sub_split: false,
        })
        .collect()
}

fn fake_embeddings(count: usize, dims: usize) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            let mut v = vec![0.0f32; dims];
            // Put a distinguishing value so vectors differ
            v[i % dims] = 1.0;
            v
        })
        .collect()
}

fn create_index_dir() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.idx");
    (dir, path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_create_empty_index() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    let status = index.status();

    assert_eq!(status.document_count, 0);
    assert_eq!(status.chunk_count, 0);
    assert_eq!(status.vector_count, 0);
}

#[test]
fn test_create_save_reopen() {
    let (_dir, path) = create_index_dir();
    let config = test_config();

    let index = Index::create(&path, &config).unwrap();
    index.save().unwrap();

    let reopened = Index::open(&path).unwrap();
    let status = reopened.status();

    assert_eq!(status.embedding_config.model, "test-model");
    assert_eq!(status.embedding_config.dimensions, 8);
    assert!(status.last_updated > 0);
}

#[test]
fn test_upsert_chunks_with_embeddings() {
    let (_dir, path) = create_index_dir();
    let config = test_config();
    let index = Index::create(&path, &config).unwrap();

    let file = fake_markdown_file("docs/test.md", "hash123");
    let chunks = fake_chunks("docs/test.md", 5);
    let embeddings = fake_embeddings(5, 8);

    index.upsert(&file, &chunks, &embeddings).unwrap();
    index.save().unwrap();

    // Verify in-memory
    let status = index.status();
    assert_eq!(status.document_count, 1);
    assert_eq!(status.chunk_count, 5);
    assert_eq!(status.vector_count, 5);

    // Reopen and verify persistence
    let reopened = Index::open(&path).unwrap();
    let status2 = reopened.status();
    assert_eq!(status2.document_count, 1);
    assert_eq!(status2.chunk_count, 5);

    // Search should find results
    let query = {
        let mut v = vec![0.0f32; 8];
        v[0] = 1.0;
        v
    };
    let results = reopened.search(&query, 3).unwrap();
    assert!(!results.is_empty(), "search should return results");
}

#[test]
fn test_upsert_idempotent() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();

    let file = fake_markdown_file("notes/a.md", "hash_v1");
    let chunks_v1 = fake_chunks("notes/a.md", 3);
    let emb_v1 = fake_embeddings(3, 8);
    index.upsert(&file, &chunks_v1, &emb_v1).unwrap();

    // Upsert same file again with different chunk count
    let file_v2 = fake_markdown_file("notes/a.md", "hash_v2");
    let chunks_v2 = fake_chunks("notes/a.md", 2);
    let emb_v2 = fake_embeddings(2, 8);
    index.upsert(&file_v2, &chunks_v2, &emb_v2).unwrap();

    let status = index.status();
    assert_eq!(status.document_count, 1, "should still be 1 file");
    assert_eq!(status.chunk_count, 2, "should have only latest chunks");
    assert_eq!(status.vector_count, 2);
}

#[test]
fn test_remove_file_chunks() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();

    let file = fake_markdown_file("docs/remove.md", "hash1");
    let chunks = fake_chunks("docs/remove.md", 3);
    let emb = fake_embeddings(3, 8);
    index.upsert(&file, &chunks, &emb).unwrap();

    assert_eq!(index.status().document_count, 1);

    index.remove_file("docs/remove.md").unwrap();
    assert_eq!(index.status().document_count, 0);
    assert_eq!(index.status().chunk_count, 0);

    // Save and reopen to verify persistence
    index.save().unwrap();
    let reopened = Index::open(&path).unwrap();
    assert_eq!(reopened.status().document_count, 0);
    assert_eq!(reopened.status().chunk_count, 0);
}

#[test]
fn test_remove_nonexistent_file() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();

    // Should be a no-op, not an error
    let result = index.remove_file("nonexistent/path.md");
    assert!(result.is_ok());
}

#[test]
fn test_get_file_hashes() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();

    let file_a = fake_markdown_file("a.md", "hash_aaa");
    let file_b = fake_markdown_file("b.md", "hash_bbb");
    index
        .upsert(&file_a, &fake_chunks("a.md", 1), &fake_embeddings(1, 8))
        .unwrap();
    index
        .upsert(&file_b, &fake_chunks("b.md", 1), &fake_embeddings(1, 8))
        .unwrap();

    let hashes = index.get_file_hashes();
    assert_eq!(hashes.len(), 2);
    assert_eq!(hashes.get("a.md").unwrap(), "hash_aaa");
    assert_eq!(hashes.get("b.md").unwrap(), "hash_bbb");
}

#[test]
fn test_status_counts() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();

    index
        .upsert(
            &fake_markdown_file("x.md", "h1"),
            &fake_chunks("x.md", 3),
            &fake_embeddings(3, 8),
        )
        .unwrap();
    index
        .upsert(
            &fake_markdown_file("y.md", "h2"),
            &fake_chunks("y.md", 2),
            &fake_embeddings(2, 8),
        )
        .unwrap();

    let status = index.status();
    assert_eq!(status.document_count, 2);
    assert_eq!(status.chunk_count, 5);
    assert_eq!(status.vector_count, 5);
}

#[test]
fn test_search_returns_correct_result() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();

    let file = fake_markdown_file("target.md", "hash");
    let chunks = vec![Chunk {
        id: "target.md#0".to_string(),
        source_path: PathBuf::from("target.md"),
        heading_hierarchy: vec!["Title".to_string()],
        content: "Target content".to_string(),
        start_line: 1,
        end_line: 5,
        chunk_index: 0,
        is_sub_split: false,
    }];

    // Use a known vector
    let known_vec = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    index.upsert(&file, &chunks, &[known_vec.clone()]).unwrap();

    // Search with the same vector — should find it
    let results = index.search(&known_vec, 1).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "target.md#0");
}

#[test]
fn test_corrupted_magic_bytes() {
    let (_dir, path) = create_index_dir();

    // Write a file with wrong magic bytes
    let mut data = vec![0u8; 128];
    data[..6].copy_from_slice(b"WRONG\x00");
    fs::write(&path, &data).unwrap();

    let result = Index::open(&path);
    assert!(
        matches!(result, Err(Error::IndexCorrupted(_))),
        "expected IndexCorrupted for corrupted magic"
    );
}

#[test]
fn test_version_mismatch() {
    let (_dir, path) = create_index_dir();

    // Write a file with correct magic but wrong version
    let mut data = vec![0u8; 128];
    data[..6].copy_from_slice(b"MDVDB\x00");
    data[6..10].copy_from_slice(&999u32.to_le_bytes());
    fs::write(&path, &data).unwrap();

    let result = Index::open(&path);
    assert!(
        matches!(result, Err(Error::IndexCorrupted(_))),
        "expected IndexCorrupted for version mismatch"
    );
}

#[test]
fn test_config_compatibility_match() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();

    let compat_config = test_embedding_config();
    let result = index.check_config_compatibility(&compat_config);
    assert!(result.is_ok());
}

#[test]
fn test_config_compatibility_mismatch() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();

    let bad_config = EmbeddingConfig {
        provider: "OpenAI".to_string(),
        model: "test-model".to_string(),
        dimensions: 9999,
    };
    let result = index.check_config_compatibility(&bad_config);
    assert!(
        matches!(result, Err(Error::IndexCorrupted(_))),
        "expected dimension mismatch error"
    );
}

#[test]
fn test_header_magic_bytes() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    index.save().unwrap();

    let raw = fs::read(&path).unwrap();
    assert!(raw.len() >= 64, "file should be at least 64 bytes");
    assert_eq!(&raw[..6], b"MDVDB\x00", "magic bytes mismatch");
}

#[test]
fn test_concurrent_readers() {
    let (_dir, path) = create_index_dir();
    let index = Arc::new(Index::create(&path, &test_config()).unwrap());

    let file = fake_markdown_file("concurrent.md", "hash");
    index
        .upsert(
            &file,
            &fake_chunks("concurrent.md", 2),
            &fake_embeddings(2, 8),
        )
        .unwrap();

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let idx = Arc::clone(&index);
            thread::spawn(move || {
                let _status = idx.status();
                let _file = idx.get_file("concurrent.md");
                let _hashes = idx.get_file_hashes();
            })
        })
        .collect();

    for h in handles {
        h.join().expect("reader thread panicked");
    }
}

#[test]
fn test_concurrent_read_write() {
    let (_dir, path) = create_index_dir();
    let index = Arc::new(Index::create(&path, &test_config()).unwrap());

    // Pre-populate
    let file = fake_markdown_file("rw.md", "hash");
    index
        .upsert(&file, &fake_chunks("rw.md", 1), &fake_embeddings(1, 8))
        .unwrap();

    let writer = {
        let idx = Arc::clone(&index);
        thread::spawn(move || {
            for i in 0..10 {
                let f = fake_markdown_file(&format!("w{i}.md"), &format!("h{i}"));
                let chunks = fake_chunks(&format!("w{i}.md"), 1);
                let emb = fake_embeddings(1, 8);
                idx.upsert(&f, &chunks, &emb).unwrap();
            }
        })
    };

    let readers: Vec<_> = (0..4)
        .map(|_| {
            let idx = Arc::clone(&index);
            thread::spawn(move || {
                for _ in 0..20 {
                    let _s = idx.status();
                    let _f = idx.get_file("rw.md");
                }
            })
        })
        .collect();

    writer.join().expect("writer panicked");
    for r in readers {
        r.join().expect("reader panicked");
    }
}

#[test]
fn test_atomic_save() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    index
        .upsert(
            &fake_markdown_file("atomic.md", "h"),
            &fake_chunks("atomic.md", 1),
            &fake_embeddings(1, 8),
        )
        .unwrap();

    index.save().unwrap();

    // After save the .tmp file should not exist (renamed to final)
    let tmp_path = path.with_extension("tmp");
    assert!(!tmp_path.exists(), ".tmp file should not remain after save");
    assert!(path.exists(), "final index file should exist");
}

#[test]
fn test_portable_paths() {
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();

    // Use relative paths for files
    let file = fake_markdown_file("docs/notes/readme.md", "hash");
    let chunks = fake_chunks("docs/notes/readme.md", 2);
    let emb = fake_embeddings(2, 8);
    index.upsert(&file, &chunks, &emb).unwrap();
    index.save().unwrap();

    // Reopen and verify all stored paths are relative
    let reopened = Index::open(&path).unwrap();
    let stored = reopened.get_file("docs/notes/readme.md");
    assert!(stored.is_some(), "should find file by relative path");

    let sf = stored.unwrap();
    assert!(
        !sf.relative_path.starts_with('/'),
        "stored path should be relative, got: {}",
        sf.relative_path
    );

    let hashes = reopened.get_file_hashes();
    for (p, _) in &hashes {
        assert!(
            !p.starts_with('/'),
            "hash key path should be relative, got: {p}"
        );
    }
}

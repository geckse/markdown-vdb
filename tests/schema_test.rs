use std::path::PathBuf;

use mdvdb::index::{EmbeddingConfig, Index};
use mdvdb::parser::MarkdownFile;
use mdvdb::schema::Schema;
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

fn create_index_dir() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.idx");
    (dir, path)
}

fn make_file(path: &str, frontmatter: serde_json::Value) -> MarkdownFile {
    MarkdownFile {
        path: PathBuf::from(path),
        frontmatter: Some(frontmatter),
        headings: vec![],
        body: "Test body".to_string(),
        content_hash: format!("hash_{path}"),
        file_size: 100,
        links: Vec::new(),
        modified_at: 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn schema_roundtrip_in_index() {
    // 1. Create test MarkdownFiles with various frontmatter types
    let files = vec![
        make_file(
            "a.md",
            serde_json::json!({
                "title": "First Post",
                "tags": ["rust", "testing"],
                "draft": true,
                "priority": 1,
                "created": "2025-01-15"
            }),
        ),
        make_file(
            "b.md",
            serde_json::json!({
                "title": "Second Post",
                "tags": ["python"],
                "draft": false,
                "priority": 2,
                "author": "Alice"
            }),
        ),
        make_file(
            "c.md",
            serde_json::json!({
                "title": "Third Post",
                "priority": 3,
                "created": "2025-03-10"
            }),
        ),
    ];

    // 2. Infer schema
    let schema = Schema::infer(&files);

    // Verify inferred schema has expected fields
    assert!(schema.fields.len() >= 5, "should have at least 5 fields");
    assert!(schema.get_field("title").is_some());
    assert!(schema.get_field("tags").is_some());
    assert!(schema.get_field("draft").is_some());
    assert!(schema.get_field("priority").is_some());

    // Check occurrence counts
    let title_field = schema.get_field("title").unwrap();
    assert_eq!(title_field.occurrence_count, 3);

    let draft_field = schema.get_field("draft").unwrap();
    assert_eq!(draft_field.occurrence_count, 2);

    // 3. Create index and set schema
    let (_dir, path) = create_index_dir();
    let index = Index::create(&path, &test_config()).unwrap();
    index.set_schema(Some(schema.clone()));

    // Verify schema is set in memory
    let retrieved = index.get_schema().expect("schema should be set");
    assert_eq!(retrieved.fields.len(), schema.fields.len());

    // 4. Save the index
    index.save().unwrap();

    // 5. Reopen the index
    let reopened = Index::open(&path).unwrap();

    // 6. Verify schema is recovered unchanged
    let recovered = reopened.get_schema().expect("schema should persist after reopen");
    assert_eq!(
        recovered.fields.len(),
        schema.fields.len(),
        "field count should match"
    );
    assert_eq!(recovered.last_updated, schema.last_updated);

    // Check each field matches
    for original_field in &schema.fields {
        let recovered_field = recovered
            .get_field(&original_field.name)
            .unwrap_or_else(|| panic!("field '{}' should exist after reopen", original_field.name));

        assert_eq!(
            format!("{:?}", recovered_field.field_type),
            format!("{:?}", original_field.field_type),
            "field type mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.occurrence_count, original_field.occurrence_count,
            "occurrence count mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.sample_values, original_field.sample_values,
            "sample values mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.description, original_field.description,
            "description mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.required, original_field.required,
            "required mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.allowed_values, original_field.allowed_values,
            "allowed_values mismatch for '{}'",
            original_field.name
        );
    }
}

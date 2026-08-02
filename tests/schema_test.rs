use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::index::{EmbeddingConfig, Index};
use mdvdb::parser::MarkdownFile;
use mdvdb::schema::{OverlayField, Schema};
use mdvdb::{IngestOptions, MarkdownVdb, SearchMode};
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
        frontmatter_links: Vec::new(),
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
                "created": "2025-01-15",
                "attachments": ["[[assets/mockup.png]]"],
                "payload": {"message": "hello", "retries": 2}
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

    // 2. Infer schema and add overlay-only computed fields. This also verifies
    // the archived index layout for Lookup/Rollup-specific metadata.
    let schema = Schema::merge(
        Schema::infer(&files),
        Some(HashMap::from([
            (
                "client_domain".to_string(),
                OverlayField {
                    field_type: Some("lookup".to_string()),
                    relation_field: Some("client".to_string()),
                    target_field: Some("domain".to_string()),
                    ..OverlayField::default()
                },
            ),
            (
                "invoice_total".to_string(),
                OverlayField {
                    field_type: Some("rollup".to_string()),
                    relation_field: Some("client".to_string()),
                    target_field: Some("total".to_string()),
                    relation_direction: Some("incoming".to_string()),
                    relation_scope: Some("invoices/".to_string()),
                    formula: Some("values.reduce((sum, value) => sum + value, 0)".to_string()),
                    result_type: Some("number".to_string()),
                    ..OverlayField::default()
                },
            ),
        ])),
    );

    // Verify inferred schema has expected fields
    assert!(schema.fields.len() >= 6, "should have at least 6 fields");
    assert!(schema.get_field("title").is_some());
    assert!(schema.get_field("tags").is_some());
    assert!(schema.get_field("draft").is_some());
    assert!(schema.get_field("priority").is_some());
    assert_eq!(
        schema.get_field("attachments").unwrap().field_type,
        mdvdb::FieldType::File
    );
    assert_eq!(
        schema.get_field("payload").unwrap().field_type,
        mdvdb::FieldType::Json
    );
    assert_eq!(
        schema
            .get_field("client_domain")
            .unwrap()
            .relation_direction,
        Some(mdvdb::RelationDirection::Outgoing)
    );
    assert_eq!(
        schema
            .get_field("invoice_total")
            .unwrap()
            .relation_scope
            .as_deref(),
        Some("invoices")
    );

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
    let recovered = reopened
        .get_schema()
        .expect("schema should persist after reopen");
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
        assert_eq!(
            recovered_field.relation_target, original_field.relation_target,
            "relation_target mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.formula, original_field.formula,
            "formula mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.result_type, original_field.result_type,
            "result_type mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.relation_field, original_field.relation_field,
            "relation_field mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.target_field, original_field.target_field,
            "target_field mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.relation_direction, original_field.relation_direction,
            "relation_direction mismatch for '{}'",
            original_field.name
        );
        assert_eq!(
            recovered_field.relation_scope, original_field.relation_scope,
            "relation_scope mismatch for '{}'",
            original_field.name
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers for integration tests
// ---------------------------------------------------------------------------

fn mock_config() -> Config {
    Config {
        embedding_provider: EmbeddingProviderType::Mock,
        embedding_model: "mock-model".into(),
        embedding_dimensions: 8,
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
        clustering_algorithm: mdvdb::config::ClusteringAlgorithm::Leiden,
        clustering_knn: 15,
        clustering_resolution: 1.0,
        clustering_min_cluster_size: 2,
        topics_min_similarity: 0.30,
        clustering_rebalance_threshold: 50,
        clustering_granularity: 1.0,
        search_default_limit: 10,
        search_min_score: 0.0,
        search_default_mode: SearchMode::Hybrid,
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

fn setup_scoped_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join("config.yaml"),
        "embedding:\n  provider: mock\n  dimensions: 8\n",
    )
    .unwrap();

    // Blog files with blog-specific frontmatter
    fs::create_dir_all(root.join("blog")).unwrap();
    fs::write(
        root.join("blog/post1.md"),
        "---\ntitle: First Post\nstatus: draft\ntags:\n  - rust\n---\n\n# First Post\n\nBlog content here.\n",
    )
    .unwrap();
    fs::write(
        root.join("blog/post2.md"),
        "---\ntitle: Second Post\nstatus: published\ntags:\n  - python\n---\n\n# Second Post\n\nMore blog content.\n",
    )
    .unwrap();

    // Docs files with different frontmatter
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(
        root.join("docs/guide.md"),
        "---\ntitle: User Guide\nversion: 1.0\n---\n\n# Guide\n\nDocumentation content.\n",
    )
    .unwrap();

    dir
}

// ---------------------------------------------------------------------------
// Integration tests for path-scoped schema
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_schema_scoped_api() {
    let dir = setup_scoped_dir();
    let root = dir.path();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();

    let scoped = vdb.schema_scoped("blog").unwrap();
    assert_eq!(scoped.scope, "blog");

    // Blog files have title, status, tags — but not version
    assert!(
        scoped.schema.get_field("title").is_some(),
        "blog scope should have title"
    );
    assert!(
        scoped.schema.get_field("status").is_some(),
        "blog scope should have status"
    );
    assert!(
        scoped.schema.get_field("tags").is_some(),
        "blog scope should have tags"
    );
    assert!(
        scoped.schema.get_field("version").is_none(),
        "blog scope should NOT have version (that's in docs)"
    );

    // Verify occurrence counts match blog file count
    let status_field = scoped.schema.get_field("status").unwrap();
    assert_eq!(status_field.occurrence_count, 2);
}

#[tokio::test]
async fn test_schema_scoped_persisted_after_ingest() {
    let dir = setup_scoped_dir();
    let root = dir.path();

    // Ingest to populate the index
    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();

    // Drop and reopen — scoped schema should be available from index
    drop(vdb);
    let vdb2 = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();

    let scoped = vdb2.schema_scoped("blog").unwrap();
    assert_eq!(scoped.scope, "blog");
    assert!(
        scoped.schema.get_field("status").is_some(),
        "scoped schema should be available after reopen"
    );
    assert!(
        scoped.schema.get_field("version").is_none(),
        "blog scope should not include docs-only fields"
    );
}

#[tokio::test]
async fn live_file_overlay_overrides_a_persisted_relation_without_reingest() {
    let dir = setup_scoped_dir();
    let root = dir.path();
    fs::write(
        root.join("blog/assets.md"),
        "---\nattachments:\n  - \"[[assets/mockup.png]]\"\n  - \"[[documents/spec.pdf]]\"\n---\n\n# Assets\n",
    )
    .unwrap();
    fs::write(
        root.join(".markdownvdb.schema.yml"),
        "scopes:\n  blog:\n    fields:\n      attachments:\n        field_type: relation\n",
    )
    .unwrap();

    let vdb = MarkdownVdb::open_with_config(root.to_path_buf(), mock_config()).unwrap();
    vdb.ingest(IngestOptions::default()).await.unwrap();
    assert_eq!(
        vdb.schema_scoped("blog")
            .unwrap()
            .schema
            .get_field("attachments")
            .unwrap()
            .field_type,
        mdvdb::FieldType::Relation
    );

    // The schema file is live configuration: changing its explicit type must
    // be visible without another ingest.
    fs::write(
        root.join(".markdownvdb.schema.yml"),
        "scopes:\n  blog:\n    fields:\n      attachments:\n        field_type: file\n",
    )
    .unwrap();
    assert_eq!(
        vdb.schema_scoped("blog")
            .unwrap()
            .schema
            .get_field("attachments")
            .unwrap()
            .field_type,
        mdvdb::FieldType::File
    );

    // Removing the overlay also recovers old persisted Relation schemas from
    // their non-Markdown link samples.
    fs::remove_file(root.join(".markdownvdb.schema.yml")).unwrap();
    assert_eq!(
        vdb.schema_scoped("blog")
            .unwrap()
            .schema
            .get_field("attachments")
            .unwrap()
            .field_type,
        mdvdb::FieldType::File
    );
}

#[test]
fn test_schema_cli_with_path_flag() {
    let dir = setup_scoped_dir();
    let root = dir.path();

    // First ingest via CLI
    let output = Command::new(env!("CARGO_BIN_EXE_mdvdb"))
        .arg("ingest")
        .current_dir(root)
        .output()
        .expect("failed to run ingest");
    assert!(
        output.status.success(),
        "ingest should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Run schema --path blog/ --json
    let output = Command::new(env!("CARGO_BIN_EXE_mdvdb"))
        .args(["schema", "--path", "blog/", "--json"])
        .current_dir(root)
        .output()
        .expect("failed to run schema");
    assert!(
        output.status.success(),
        "schema --path should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("should be valid JSON: {e}\nstdout: {stdout}"));

    // Verify structure
    assert!(json.get("scope").is_some(), "should have scope field");
    assert!(json.get("schema").is_some(), "should have schema field");
    assert_eq!(json["scope"].as_str().unwrap(), "blog/");

    // Verify schema contains blog fields
    let fields = json["schema"]["fields"]
        .as_array()
        .expect("fields should be array");
    let field_names: Vec<&str> = fields.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(
        field_names.contains(&"status"),
        "should contain status field"
    );
    assert!(field_names.contains(&"tags"), "should contain tags field");
    assert!(
        !field_names.contains(&"version"),
        "should not contain version field"
    );
}

#[test]
fn infer_scoped_normalizes_backslash_file_paths() {
    // Files whose PathBufs carry Windows-style separators must still match
    // the slash-separated scope prefix.
    let files = vec![
        make_file(
            r"docs\a.md",
            serde_json::json!({"title": "A", "draft": true}),
        ),
        make_file(r"docs\b.md", serde_json::json!({"title": "B"})),
        make_file("other/c.md", serde_json::json!({"unrelated": 1})),
    ];

    let schema = Schema::infer_scoped(&files, "docs");
    let names: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"title"),
        "scoped schema should see docs files: {names:?}"
    );
    assert!(names.contains(&"draft"));
    assert!(
        !names.contains(&"unrelated"),
        "files outside the scope must not contribute fields"
    );

    // Scope discovery also sees the normalized top-level directory.
    let scopes = Schema::discover_scopes(&files);
    assert!(scopes.contains(&"docs".to_string()), "scopes: {scopes:?}");
}

#[test]
fn lookup_rollup_overlay_public_schema_contract() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(".markdownvdb.schema.yml"),
        r#"scopes:
  contacts:
    fields:
      client_domain:
        field_type: lookup
        relation_field: client
        target_field: domain
  clients:
    fields:
      invoice_total:
        field_type: rollup
        relation_field: client
        target_field: total
        relation_direction: incoming
        relation_scope: invoices/
        formula: values.reduce((sum, value) => sum + value, 0)
        result_type: number
"#,
    )
    .unwrap();

    let overlay = Schema::load_overlay(dir.path()).unwrap().unwrap();
    let contacts = Schema::merge(
        Schema::infer(&[]),
        Some(Schema::resolve_overlay_for_path(
            &overlay,
            Some("contacts/c1.md"),
        )),
    );
    let lookup = contacts.get_field("client_domain").unwrap();
    assert_eq!(lookup.field_type, mdvdb::FieldType::Lookup);
    assert_eq!(lookup.relation_field.as_deref(), Some("client"));
    assert_eq!(lookup.target_field.as_deref(), Some("domain"));
    assert_eq!(
        lookup.relation_direction,
        Some(mdvdb::schema::RelationDirection::Outgoing)
    );

    let clients = Schema::merge(
        Schema::infer(&[]),
        Some(Schema::resolve_overlay_for_path(
            &overlay,
            Some("clients/acme.md"),
        )),
    );
    let rollup = clients.get_field("invoice_total").unwrap();
    assert_eq!(rollup.field_type, mdvdb::FieldType::Rollup);
    assert_eq!(
        rollup.relation_direction,
        Some(mdvdb::schema::RelationDirection::Incoming)
    );
    assert_eq!(rollup.relation_scope.as_deref(), Some("invoices"));

    let json = serde_json::to_value(rollup).unwrap();
    assert_eq!(json["field_type"], "Rollup");
    assert_eq!(json["relation_direction"], "Incoming");
    assert_eq!(json["relation_scope"], "invoices");
    assert_eq!(json["result_type"], "Number");
}

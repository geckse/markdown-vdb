//! Integration tests for phase 31 — frontmatter relations (wiki-link foreign keys).
//!
//! Fixture vault:
//!
//! ```text
//! clients/acme.md      title=Acme Corp, industry=tech
//! clients/globex.md    title=Globex
//! invoices/i1.md       client="[[clients/acme]]", contact="[[clients/globex#Contacts|Globex Contact]]",
//!                      attachment="[[sub/note]]", amount=100  (+ body link ../clients/acme)
//! invoices/i2.md       clients=["[[clients/acme|A]]", "not-a-link", "[[clients/globex]]", "[[clients/acme|A]]"],
//!                      spec="[Spec](clients/acme.md)", note="clients/globex.md"
//! invoices/i3.md       client="[[acme]]"                  (bare → overlay target folder)
//! invoices/sub/note.md (attachment target, source-dir fallback case)
//! notes/dangling.md    client="[[clients/ghost]]"         (dangling)
//! notes/selfref.md     parent="[[notes/selfref]]"         (self-reference)
//! notes/local.md       ref="[[dangling]]"                 (bare, no target → source-dir-relative)
//! notes/unquoted.md    client: [[clients/acme]]           (unquoted YAML footgun)
//! ```
//!
//! Overlay: scope `invoices` declares `client: {field_type: relation, target: clients}`,
//! plus a global `missing_rel: {target: ghostfolder}` for the doctor hygiene warning.

use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::search::SearchMode;
use mdvdb::watcher::{FileEvent, Watcher};
use mdvdb::{
    CollectionQuery, FieldType, IngestOptions, MarkdownVdb, MetadataFilter, SearchQuery,
    VectorQuantization,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
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
        vector_quantization: VectorQuantization::F16,
        index_compression: true,
        edge_embeddings: true,
        edge_boost_weight: 0.15,
        edge_cluster_rebalance: 50,
        custom_cluster_defs: Vec::new(),
    }
}

fn write_relations_vault(root: &std::path::Path) {
    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join("config.yaml"),
        "embedding:\n  provider: mock\n  dimensions: 8\n",
    )
    .unwrap();

    fs::write(
        root.join(".markdownvdb.schema.yml"),
        r#"fields:
  missing_rel:
    target: ghostfolder
scopes:
  invoices:
    fields:
      client:
        field_type: relation
        target: clients
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("clients")).unwrap();
    fs::write(
        root.join("clients/acme.md"),
        "---\ntitle: Acme Corp\nindustry: tech\n---\n\n# Acme\n\nAcme client profile.\n",
    )
    .unwrap();
    fs::write(
        root.join("clients/globex.md"),
        "---\ntitle: Globex\n---\n\n# Globex\n\n## Contacts\n\nGlobex client profile.\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("invoices/sub")).unwrap();
    fs::write(
        root.join("invoices/i1.md"),
        "---\nclient: \"[[clients/acme]]\"\ncontact: \"[[clients/globex#Contacts|Globex Contact]]\"\nattachment: \"[[sub/note]]\"\namount: 100\n---\n\n# Invoice i1\n\nInvoice for Acme. See [[../clients/acme|Acme]].\n",
    )
    .unwrap();
    fs::write(
        root.join("invoices/i2.md"),
        "---\ntitle: Invoice i2\nclients:\n  - \"[[clients/acme|A]]\"\n  - not-a-link\n  - \"[[clients/globex]]\"\n  - \"[[clients/acme|A]]\"\nspec: \"[Spec](clients/acme.md)\"\nnote: clients/globex.md\n---\n\n# Invoice i2\n\nMulti-client invoice.\n",
    )
    .unwrap();
    fs::write(
        root.join("invoices/i3.md"),
        "---\nclient: \"[[acme]]\"\n---\n\n# Invoice i3\n\nBare name resolved via target folder.\n",
    )
    .unwrap();
    fs::write(
        root.join("invoices/sub/note.md"),
        "# Note\n\nInvoice attachment note.\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("notes")).unwrap();
    fs::write(
        root.join("notes/dangling.md"),
        "---\nclient: \"[[clients/ghost]]\"\n---\n\n# Dangling\n\nReferences a missing client.\n",
    )
    .unwrap();
    fs::write(
        root.join("notes/selfref.md"),
        "---\nparent: \"[[notes/selfref]]\"\n---\n\n# Selfref\n\nSelf reference.\n",
    )
    .unwrap();
    fs::write(
        root.join("notes/local.md"),
        "---\nref: \"[[dangling]]\"\n---\n\n# Local\n\nSource-dir-relative bare reference.\n",
    )
    .unwrap();
    fs::write(
        root.join("notes/unquoted.md"),
        "---\nclient: [[clients/acme]]\n---\n\n# Unquoted\n\nYAML footgun case.\n",
    )
    .unwrap();
}

fn setup_relations_vault() -> TempDir {
    let dir = TempDir::new().unwrap();
    write_relations_vault(dir.path());
    let vdb = open(&dir);
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(vdb.ingest(IngestOptions::default()))
        .unwrap();
    dir
}

/// Async variant for `#[tokio::test]` bodies (no nested runtime).
async fn setup_relations_vault_async() -> TempDir {
    let dir = TempDir::new().unwrap();
    write_relations_vault(dir.path());
    let vdb = open(&dir);
    vdb.ingest(IngestOptions::default()).await.unwrap();
    dir
}

fn open(dir: &TempDir) -> MarkdownVdb {
    MarkdownVdb::open_with_config(dir.path().to_path_buf(), mock_config()).unwrap()
}

// ---------------------------------------------------------------------------
// get --populate
// ---------------------------------------------------------------------------

#[test]
fn test_get_without_populate_omits_keys() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let doc = vdb.get_document("invoices/i1.md").unwrap();
    assert!(doc.relations.is_none());
    assert!(doc.referenced_by.is_none());

    let json = serde_json::to_value(&doc).unwrap();
    let obj = json.as_object().unwrap();
    assert!(!obj.contains_key("relations"), "key must be absent, not null");
    assert!(!obj.contains_key("referenced_by"), "key must be absent, not null");
}

#[test]
fn test_get_populated_relations_and_contract_shape() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let doc = vdb.get_document_populated("invoices/i1.md").unwrap();
    let relations = doc.relations.as_ref().unwrap();

    // Map keys are the relation-bearing fields, alphabetical (BTreeMap).
    let keys: Vec<&str> = relations.keys().map(|k| k.as_str()).collect();
    assert_eq!(keys, vec!["attachment", "client", "contact"]);

    // client: root-relative resolution.
    let client = &relations["client"];
    assert_eq!(client.len(), 1);
    assert_eq!(client[0].raw, "[[clients/acme]]");
    assert_eq!(client[0].path.as_deref(), Some("clients/acme.md"));
    assert!(client[0].exists);
    assert_eq!(client[0].title.as_deref(), Some("Acme Corp"));
    let fm = client[0].frontmatter.as_ref().unwrap();
    assert_eq!(fm["industry"], "tech");
    assert!(
        fm.get("relations").is_none(),
        "populated target frontmatter is never nested"
    );

    // contact: fragment stripped, alias is display-only.
    let contact = &relations["contact"];
    assert_eq!(contact[0].path.as_deref(), Some("clients/globex.md"));
    assert!(contact[0].exists);
    assert_eq!(contact[0].title.as_deref(), Some("Globex"));

    // attachment: root-relative miss falls back to source-dir-relative.
    let attachment = &relations["attachment"];
    assert_eq!(attachment[0].path.as_deref(), Some("invoices/sub/note.md"));
    assert!(attachment[0].exists);
    // Target has no frontmatter → title from filename stem, frontmatter null.
    assert_eq!(attachment[0].title.as_deref(), Some("note"));
    assert!(attachment[0].frontmatter.is_none());

    // Non-link fields (amount) produce no key.
    assert!(!relations.contains_key("amount"));

    // The JSON shape: frontmatter is an ALWAYS-present key on every value.
    let json = serde_json::to_value(&doc).unwrap();
    for (_field, values) in json["relations"].as_object().unwrap() {
        for value in values.as_array().unwrap() {
            let obj = value.as_object().unwrap();
            for key in ["raw", "path", "exists", "title", "frontmatter"] {
                assert!(obj.contains_key(key), "RelationValue missing key {key}");
            }
        }
    }
    assert!(json["relations"]["attachment"][0]["frontmatter"].is_null());
}

#[test]
fn test_get_populated_dangling_relation() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let doc = vdb.get_document_populated("notes/dangling.md").unwrap();
    let relations = doc.relations.as_ref().unwrap();
    let client = &relations["client"];
    assert_eq!(client[0].path.as_deref(), Some("clients/ghost.md"));
    assert!(!client[0].exists);
    assert!(client[0].title.is_none());
    assert!(client[0].frontmatter.is_none());
}

#[test]
fn test_get_populated_array_order_and_duplicates() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let doc = vdb.get_document_populated("invoices/i2.md").unwrap();
    let relations = doc.relations.as_ref().unwrap();

    // Source order preserved, duplicate preserved, "not-a-link" skipped.
    let clients = &relations["clients"];
    let paths: Vec<&str> = clients.iter().filter_map(|v| v.path.as_deref()).collect();
    assert_eq!(
        paths,
        vec!["clients/acme.md", "clients/globex.md", "clients/acme.md"]
    );

    // Markdown-link and bare-path syntaxes are relations too.
    assert_eq!(relations["spec"][0].path.as_deref(), Some("clients/acme.md"));
    assert_eq!(relations["note"][0].path.as_deref(), Some("clients/globex.md"));
}

#[test]
fn test_get_populated_bare_name_resolves_via_overlay_target() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let doc = vdb.get_document_populated("invoices/i3.md").unwrap();
    let relations = doc.relations.as_ref().unwrap();
    let client = &relations["client"];
    assert_eq!(client[0].raw, "[[acme]]");
    assert_eq!(client[0].path.as_deref(), Some("clients/acme.md"));
    assert!(client[0].exists);
    assert_eq!(client[0].title.as_deref(), Some("Acme Corp"));
}

#[test]
fn test_get_populated_bare_name_without_target_is_source_dir_relative() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let doc = vdb.get_document_populated("notes/local.md").unwrap();
    let relations = doc.relations.as_ref().unwrap();
    assert_eq!(
        relations["ref"][0].path.as_deref(),
        Some("notes/dangling.md")
    );
    assert!(relations["ref"][0].exists);
}

#[test]
fn test_get_populated_self_reference_skipped() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let doc = vdb.get_document_populated("notes/selfref.md").unwrap();
    let relations = doc.relations.as_ref().unwrap();
    assert!(
        !relations.contains_key("parent"),
        "self-FK must produce no relations key, got {relations:?}"
    );

    // And no graph edge either.
    let links = vdb.links("notes/selfref.md").unwrap();
    assert!(links.outgoing.is_empty());
}

#[test]
fn test_get_populated_referenced_by() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let doc = vdb.get_document_populated("clients/acme.md").unwrap();

    // Doc without link-shaped values → empty map (not absent).
    assert!(doc.relations.as_ref().unwrap().is_empty());

    let referenced_by = doc.referenced_by.as_ref().unwrap();
    let entries: Vec<(String, String)> = referenced_by
        .iter()
        .map(|r| (r.source.clone(), r.field.clone()))
        .collect();
    // Sorted by (source, field). notes/unquoted.md must NOT appear (nested
    // YAML array, not a relation). The body link from i1 must not appear either
    // (referenced_by is relation backlinks only).
    assert_eq!(
        entries,
        vec![
            ("invoices/i1.md".to_string(), "client".to_string()),
            ("invoices/i2.md".to_string(), "clients".to_string()),
            ("invoices/i2.md".to_string(), "spec".to_string()),
            ("invoices/i3.md".to_string(), "client".to_string()),
        ]
    );
    // Titles derive from the SOURCE documents.
    assert_eq!(referenced_by[1].title, "Invoice i2");
    assert_eq!(referenced_by[0].title, "i1"); // no frontmatter title → stem
}

// ---------------------------------------------------------------------------
// Link graph surfaces
// ---------------------------------------------------------------------------

#[test]
fn test_links_carry_field_and_sentinel() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let result = vdb.links("invoices/i1.md").unwrap();

    // Body link + relation to the same target = two distinct entries.
    let to_acme: Vec<_> = result
        .outgoing
        .iter()
        .filter(|l| l.entry.target == "clients/acme.md")
        .collect();
    assert_eq!(to_acme.len(), 2, "body + relation edges must coexist");
    let body = to_acme.iter().find(|l| l.entry.field.is_none()).unwrap();
    let relation = to_acme.iter().find(|l| l.entry.field.is_some()).unwrap();
    assert!(body.entry.line_number > 0);
    assert_eq!(relation.entry.field.as_deref(), Some("client"));
    assert_eq!(relation.entry.line_number, 0, "frontmatter sentinel");

    // JSON: `field` is an always-present key (null for body links).
    let json = serde_json::to_value(&result).unwrap();
    for link in json["outgoing"].as_array().unwrap() {
        assert!(link["entry"].as_object().unwrap().contains_key("field"));
    }
}

#[test]
fn test_backlinks_include_relations() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let backlinks = vdb.backlinks("clients/globex.md").unwrap();
    let sources: Vec<(&str, Option<&str>)> = backlinks
        .iter()
        .map(|l| (l.entry.source.as_str(), l.entry.field.as_deref()))
        .collect();
    assert!(sources.contains(&("invoices/i1.md", Some("contact"))));
    assert!(sources.contains(&("invoices/i2.md", Some("clients"))));
    assert!(sources.contains(&("invoices/i2.md", Some("note"))));
}

#[test]
fn test_frontmatter_only_referenced_doc_is_not_orphan() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let orphans = vdb.orphans().unwrap();
    let orphan_paths: Vec<&str> = orphans.iter().map(|o| o.path.as_str()).collect();
    // globex is referenced only via frontmatter (i1.contact, i2.clients/note).
    assert!(
        !orphan_paths.contains(&"clients/globex.md"),
        "frontmatter-only-referenced doc must not be an orphan, orphans: {orphan_paths:?}"
    );
    // unquoted.md has no valid links in either direction.
    assert!(orphan_paths.contains(&"notes/unquoted.md"));
}

#[test]
fn test_graph_edges_carry_field() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let data = vdb.graph_data(None).unwrap();
    let json = serde_json::to_value(&data).unwrap();
    let edges = json["edges"].as_array().unwrap();
    assert!(!edges.is_empty());
    for edge in edges {
        assert!(
            edge.as_object().unwrap().contains_key("field"),
            "GraphEdge.field must be always-present, got {edge}"
        );
    }
    // At least one relation edge with a concrete field, and the body edge null.
    assert!(edges.iter().any(|e| e["field"] == "client"));
    assert!(edges
        .iter()
        .any(|e| e["source"] == "invoices/i1.md" && e["field"].is_null()));
}

// ---------------------------------------------------------------------------
// Schema and collection columns
// ---------------------------------------------------------------------------

#[test]
fn test_scoped_schema_relation_field() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let scoped = vdb.schema_scoped("invoices").unwrap();
    let client = scoped.schema.get_field("client").unwrap();
    assert_eq!(client.field_type, FieldType::Relation);
    assert_eq!(client.relation_target.as_deref(), Some("clients"));

    // Value-driven inference without any overlay: markdown-link and bare-path
    // fields type as Relation with no target.
    let spec = scoped.schema.get_field("spec").unwrap();
    assert_eq!(spec.field_type, FieldType::Relation);
    assert_eq!(spec.relation_target, None);
    let note = scoped.schema.get_field("note").unwrap();
    assert_eq!(note.field_type, FieldType::Relation);
    let contact = scoped.schema.get_field("contact").unwrap();
    assert_eq!(contact.field_type, FieldType::Relation);

    // A list mixing link and non-link elements stays List (no Relation typing);
    // populate still resolves its link-shaped elements (value-driven).
    let clients = scoped.schema.get_field("clients").unwrap();
    assert_eq!(clients.field_type, FieldType::List);
}

#[test]
fn test_collection_columns_relation_target() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let resp = vdb
        .collection(CollectionQuery {
            path: "invoices".into(),
            recursive: false,
            ..Default::default()
        })
        .unwrap();

    let client_col = resp.columns.iter().find(|c| c.name == "client").unwrap();
    assert_eq!(client_col.field_type, FieldType::Relation);
    assert_eq!(client_col.relation_target.as_deref(), Some("clients"));

    // relation_target is an always-present JSON key (null when unscoped).
    let json = serde_json::to_value(&resp.columns).unwrap();
    for col in json.as_array().unwrap() {
        assert!(col.as_object().unwrap().contains_key("relation_target"));
    }
}

// ---------------------------------------------------------------------------
// collection --populate
// ---------------------------------------------------------------------------

#[test]
fn test_collection_populate_page_rows_only() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let resp = vdb
        .collection(CollectionQuery {
            path: "invoices".into(),
            recursive: false,
            limit: Some(2),
            populate: true,
            ..Default::default()
        })
        .unwrap();

    assert_eq!(resp.total_rows, 3, "populate must not affect total_rows");
    assert_eq!(resp.rows.len(), 2);
    for row in &resp.rows {
        let relations = row.relations.as_ref().expect("page rows are populated");
        assert!(!relations.is_empty());
        // frontmatter stays the RAW object.
        let raw_client = row.frontmatter.get("client").or_else(|| row.frontmatter.get("clients"));
        assert!(raw_client.is_some());
        assert!(row.frontmatter.get("relations").is_none());
    }

    // i1 (sorted by path → first page row): resolved client relation present.
    let i1 = &resp.rows[0];
    assert_eq!(i1.path, "invoices/i1.md");
    assert_eq!(i1.frontmatter["client"], "[[clients/acme]]", "raw untouched");
    let client = &i1.relations.as_ref().unwrap()["client"];
    assert_eq!(client[0].path.as_deref(), Some("clients/acme.md"));
    assert_eq!(client[0].title.as_deref(), Some("Acme Corp"));
}

#[test]
fn test_collection_without_populate_omits_relations() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let resp = vdb
        .collection(CollectionQuery {
            path: "invoices".into(),
            ..Default::default()
        })
        .unwrap();
    let json = serde_json::to_value(&resp).unwrap();
    for row in json["rows"].as_array().unwrap() {
        assert!(!row.as_object().unwrap().contains_key("relations"));
    }
}

// ---------------------------------------------------------------------------
// search --populate + relation-aware filters
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_populate_fills_result_relations() {
    let dir = setup_relations_vault_async().await;
    let vdb = open(&dir);

    let query = SearchQuery::new("Invoice for Acme")
        .with_mode(SearchMode::Lexical)
        .with_populate(true);
    let response = vdb.search(query).await.unwrap();
    assert!(!response.results.is_empty());

    let i1 = response
        .results
        .iter()
        .find(|r| r.file.path == "invoices/i1.md")
        .expect("i1 should match the lexical query");
    let relations = i1.file.relations.as_ref().unwrap();
    assert_eq!(
        relations["client"][0].path.as_deref(),
        Some("clients/acme.md")
    );
}

#[tokio::test]
async fn test_search_without_populate_omits_relations() {
    let dir = setup_relations_vault_async().await;
    let vdb = open(&dir);

    let query = SearchQuery::new("Invoice for Acme").with_mode(SearchMode::Lexical);
    let response = vdb.search(query).await.unwrap();
    assert!(!response.results.is_empty());
    let json = serde_json::to_value(&response.results).unwrap();
    for result in json.as_array().unwrap() {
        assert!(!result["file"].as_object().unwrap().contains_key("relations"));
    }
}

#[tokio::test]
async fn test_search_filter_matches_relation_syntax() {
    let dir = setup_relations_vault_async().await;
    let vdb = open(&dir);

    for filter_value in ["clients/acme", "clients/acme.md", "[[clients/acme]]"] {
        let query = SearchQuery::new("Invoice")
            .with_mode(SearchMode::Lexical)
            .with_filter(MetadataFilter::Equals {
                field: "client".into(),
                value: serde_json::json!(filter_value),
            });
        let response = vdb.search(query).await.unwrap();
        let paths: Vec<&str> = response.results.iter().map(|r| r.file.path.as_str()).collect();
        assert!(
            paths.contains(&"invoices/i1.md"),
            "filter {filter_value:?} should match i1, got {paths:?}"
        );
        assert!(!paths.contains(&"invoices/i3.md"), "[[acme]] must not match {filter_value:?} (syntactic only)");
    }
}

#[test]
fn test_collection_filter_matches_relation_syntax() {
    let dir = setup_relations_vault();
    let vdb = open(&dir);

    let resp = vdb
        .collection(CollectionQuery {
            path: "invoices".into(),
            filters: vec![MetadataFilter::Equals {
                field: "clients".into(),
                value: serde_json::json!("clients/globex"),
            }],
            ..Default::default()
        })
        .unwrap();
    let paths: Vec<&str> = resp.rows.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, vec!["invoices/i2.md"]);
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_doctor_relations_check_warns() {
    let dir = setup_relations_vault_async().await;
    let vdb = open(&dir);

    let result = vdb.doctor().await.unwrap();
    let check = result
        .checks
        .iter()
        .find(|c| c.name == "Relations")
        .expect("Relations check present");
    assert_eq!(check.status, mdvdb::CheckStatus::Warn, "{}", check.detail);
    assert!(
        check.detail.contains("notes/dangling.md#client → clients/ghost.md"),
        "dangling example missing: {}",
        check.detail
    );
    assert!(check.detail.contains("ghostfolder"), "{}", check.detail);
    assert!(
        check.detail.contains("notes/unquoted.md#client"),
        "unquoted footgun missing: {}",
        check.detail
    );
}

#[tokio::test]
async fn test_doctor_relations_check_passes_on_clean_vault() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join("config.yaml"),
        "embedding:\n  provider: mock\n  dimensions: 8\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("clients")).unwrap();
    fs::write(root.join("clients/acme.md"), "---\ntitle: Acme\n---\nAcme.\n").unwrap();
    fs::write(
        root.join("invoice.md"),
        "---\nclient: \"[[clients/acme]]\"\n---\nInvoice.\n",
    )
    .unwrap();

    let vdb = open(&dir);
    vdb.ingest(IngestOptions::default()).await.unwrap();

    let result = vdb.doctor().await.unwrap();
    let check = result.checks.iter().find(|c| c.name == "Relations").unwrap();
    assert_eq!(check.status, mdvdb::CheckStatus::Pass, "{}", check.detail);
    assert!(check.detail.contains("1 relation link(s)"), "{}", check.detail);
}

// ---------------------------------------------------------------------------
// watcher incremental updates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_watcher_updates_relation_edges_incrementally() {
    let dir = setup_relations_vault_async().await;
    let vdb = open(&dir);

    let watcher = Watcher::new(
        vdb.config().clone(),
        vdb.root(),
        vdb.index_arc(),
        vdb.fts_index_arc(),
        vdb.provider_arc().unwrap(),
        None,
    );

    // Retarget notes/local.md's relation from dangling → selfref.
    fs::write(
        dir.path().join("notes/local.md"),
        "---\nref: \"[[selfref]]\"\n---\n\n# Local\n\nRetargeted reference.\n",
    )
    .unwrap();
    watcher
        .handle_event(&FileEvent::Modified(PathBuf::from("notes/local.md")))
        .await
        .unwrap();

    let links = vdb.links("notes/local.md").unwrap();
    assert_eq!(links.outgoing.len(), 1);
    assert_eq!(links.outgoing[0].entry.target, "notes/selfref.md");
    assert_eq!(links.outgoing[0].entry.field.as_deref(), Some("ref"));

    // Remove the last link entirely → graph entry must disappear.
    fs::write(
        dir.path().join("notes/local.md"),
        "# Local\n\nNo more references at all.\n",
    )
    .unwrap();
    watcher
        .handle_event(&FileEvent::Modified(PathBuf::from("notes/local.md")))
        .await
        .unwrap();

    let links = vdb.links("notes/local.md").unwrap();
    assert!(
        links.outgoing.is_empty(),
        "removing the last frontmatter link must clear the graph entry"
    );
}

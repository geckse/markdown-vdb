use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType};
use mdvdb::{
    CollectionQuery, FileState, IngestOptions, MarkdownVdb, MetadataFilter, SearchMode, SortOrder,
    TitleSource, VectorQuantization,
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

/// Write the standard fixture vault (without ingesting). Layout:
///
/// ```text
/// blog/launch.md      title=Launch Announcement, status=published, date=2024-06-01, tags=[news]
/// blog/intro.md       title=Intro Post,          status=draft,     date=2024-01-15
/// blog/no-title.md    (no title),                status=published, date=2024-03-10
/// blog/no-date.md     title=No Date,             status=draft      (no date)
/// blog/plain.md       (no frontmatter at all)
/// blog/2024/recap.md  title=Year Recap,          status=published, date=2024-12-31  (nested)
/// docs/guide.md       title=Guide, version=1.0   (different scope)
/// ```
fn write_standard_vault(root: &std::path::Path) {
    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join("config.yaml"),
        "embedding:\n  provider: mock\n  dimensions: 8\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("blog")).unwrap();
    fs::write(
        root.join("blog/launch.md"),
        "---\ntitle: Launch Announcement\nstatus: published\ndate: 2024-06-01\ntags:\n  - news\n---\n\n# Launch\n\nLaunch content.\n",
    )
    .unwrap();
    fs::write(
        root.join("blog/intro.md"),
        "---\ntitle: Intro Post\nstatus: draft\ndate: 2024-01-15\n---\n\n# Intro\n\nIntro content.\n",
    )
    .unwrap();
    fs::write(
        root.join("blog/no-title.md"),
        "---\nstatus: published\ndate: 2024-03-10\n---\n\n# Untitled body\n\nNo title field.\n",
    )
    .unwrap();
    fs::write(
        root.join("blog/no-date.md"),
        "---\ntitle: No Date\nstatus: draft\n---\n\n# No Date\n\nMissing date field.\n",
    )
    .unwrap();
    fs::write(root.join("blog/plain.md"), "# Plain\n\nNo frontmatter whatsoever.\n").unwrap();

    fs::create_dir_all(root.join("blog/2024")).unwrap();
    fs::write(
        root.join("blog/2024/recap.md"),
        "---\ntitle: Year Recap\nstatus: published\ndate: 2024-12-31\n---\n\n# Recap\n\nNested content.\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(
        root.join("docs/guide.md"),
        "---\ntitle: Guide\nversion: 1.0\n---\n\n# Guide\n\nDocs content.\n",
    )
    .unwrap();
}

/// Write the standard fixture and run a full ingest, returning the temp dir.
fn setup_collection_vault() -> TempDir {
    let dir = TempDir::new().unwrap();
    write_standard_vault(dir.path());
    let vdb = MarkdownVdb::open_with_config(dir.path().to_path_buf(), mock_config()).unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(vdb.ingest(IngestOptions::default()))
        .unwrap();
    dir
}

fn open(dir: &TempDir) -> MarkdownVdb {
    MarkdownVdb::open_with_config(dir.path().to_path_buf(), mock_config()).unwrap()
}

fn paths(resp: &mdvdb::CollectionResponse) -> Vec<&str> {
    resp.rows.iter().map(|r| r.path.as_str()).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_collection_direct_children_only() {
    let dir = setup_collection_vault();
    let vdb = open(&dir);

    let resp = vdb
        .collection(CollectionQuery {
            path: "blog".into(),
            recursive: false,
            ..Default::default()
        })
        .unwrap();

    assert_eq!(resp.scope, "blog/");
    assert!(!resp.recursive);
    let p = paths(&resp);
    // 5 direct children, NOT the nested recap, NOT docs.
    assert_eq!(resp.rows.len(), 5, "direct children only, got {p:?}");
    assert!(p.contains(&"blog/launch.md"));
    assert!(p.contains(&"blog/plain.md"));
    assert!(!p.contains(&"blog/2024/recap.md"), "should exclude nested files");
    assert!(!p.iter().any(|x| x.starts_with("docs/")), "should exclude other scopes");
    assert_eq!(resp.total_rows, 5);
}

#[test]
fn test_collection_recursive_includes_nested() {
    let dir = setup_collection_vault();
    let vdb = open(&dir);

    let resp = vdb
        .collection(CollectionQuery {
            path: "blog".into(),
            recursive: true,
            ..Default::default()
        })
        .unwrap();

    assert!(resp.recursive);
    let p = paths(&resp);
    assert!(p.contains(&"blog/2024/recap.md"), "recursive should include nested, got {p:?}");
    assert_eq!(resp.rows.len(), 6);
    assert_eq!(resp.total_rows, 6);
}

#[test]
fn test_collection_columns_union() {
    // Full ingest, then single-file re-ingest after adding a new frontmatter key.
    // Single-file ingest does NOT recompute persisted scoped schemas (documented
    // behavior), so the new key must surface as an `in_schema:false` column.
    let dir = setup_collection_vault();
    let vdb = open(&dir);

    let rt = tokio::runtime::Runtime::new().unwrap();

    // Add a brand-new key not present anywhere at full-ingest time.
    fs::write(
        dir.path().join("blog/launch.md"),
        "---\ntitle: Launch Announcement\nstatus: published\ndate: 2024-06-01\nfeatured: true\n---\n\n# Launch\n\nNow featured.\n",
    )
    .unwrap();
    rt.block_on(vdb.ingest(IngestOptions {
        full: true,
        file: Some(PathBuf::from("blog/launch.md")),
        ..Default::default()
    }))
    .unwrap();

    let resp = vdb
        .collection(CollectionQuery {
            path: "blog".into(),
            recursive: true,
            ..Default::default()
        })
        .unwrap();

    // Scoped-schema fields are in_schema:true.
    let status_col = resp.columns.iter().find(|c| c.name == "status").unwrap();
    assert!(status_col.in_schema, "status comes from the scoped schema");

    // The newly-added key is present in a row but absent from the (stale) schema.
    let featured = resp
        .columns
        .iter()
        .find(|c| c.name == "featured")
        .expect("featured should appear as a column");
    assert!(!featured.in_schema, "featured should be in_schema:false");
    assert_eq!(featured.occurrence_count, 0);
    assert!(featured.sample_values.is_empty());

    // Column ordering: all in_schema columns precede all unscoped ones.
    let last_schema = resp.columns.iter().rposition(|c| c.in_schema).unwrap();
    let first_unscoped = resp.columns.iter().position(|c| !c.in_schema).unwrap();
    assert!(last_schema < first_unscoped, "schema columns come first");
}

#[test]
fn test_collection_title_derivation() {
    let dir = setup_collection_vault();
    let vdb = open(&dir);

    let resp = vdb
        .collection(CollectionQuery {
            path: "blog".into(),
            recursive: false,
            ..Default::default()
        })
        .unwrap();

    let launch = resp.rows.iter().find(|r| r.path == "blog/launch.md").unwrap();
    assert_eq!(launch.title, "Launch Announcement");
    assert_eq!(launch.title_source, TitleSource::Frontmatter);

    let no_title = resp.rows.iter().find(|r| r.path == "blog/no-title.md").unwrap();
    assert_eq!(no_title.title, "no-title");
    assert_eq!(no_title.title_source, TitleSource::Filename);

    // A file with no frontmatter at all also falls back to the stem.
    let plain = resp.rows.iter().find(|r| r.path == "blog/plain.md").unwrap();
    assert_eq!(plain.title, "plain");
    assert_eq!(plain.title_source, TitleSource::Filename);
    assert!(resp.rows.iter().all(|r| !r.title.is_empty()), "title never empty");
}

#[test]
fn test_collection_sort_asc_desc_nulls_last() {
    let dir = setup_collection_vault();
    let vdb = open(&dir);

    // Ascending by date: earliest first, missing-date rows LAST.
    let asc = vdb
        .collection(CollectionQuery {
            path: "blog".into(),
            recursive: false,
            sort_by: Some("date".into()),
            order: SortOrder::Asc,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(asc.rows[0].path, "blog/intro.md", "earliest date first (asc)");
    let last_asc = asc.rows.last().unwrap();
    assert!(
        last_asc.frontmatter.get("date").is_none_or(|v| v.is_null()),
        "missing-date row sorts last (asc)"
    );

    // Descending by date: latest first, missing-date rows STILL last.
    let desc = vdb
        .collection(CollectionQuery {
            path: "blog".into(),
            recursive: false,
            sort_by: Some("date".into()),
            order: SortOrder::Desc,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(desc.rows[0].path, "blog/launch.md", "latest date first (desc)");
    let last_desc = desc.rows.last().unwrap();
    assert!(
        last_desc.frontmatter.get("date").is_none_or(|v| v.is_null()),
        "missing-date row sorts last (desc), not first"
    );
}

#[test]
fn test_collection_filter_reuses_metadatafilter() {
    let dir = setup_collection_vault();

    // An unindexed file on disk → state New with {} frontmatter → dropped by any filter.
    fs::write(
        dir.path().join("blog/freshly-added.md"),
        "---\nstatus: published\n---\n\n# Fresh\n",
    )
    .unwrap();

    let vdb = open(&dir);

    let resp = vdb
        .collection(CollectionQuery {
            path: "blog".into(),
            recursive: false,
            filters: vec![MetadataFilter::Equals {
                field: "status".into(),
                value: serde_json::json!("published"),
            }],
            ..Default::default()
        })
        .unwrap();

    let p = paths(&resp);
    // launch + no-title are published; intro/no-date are draft; plain has none;
    // freshly-added is a New row with {} frontmatter → all excluded.
    assert_eq!(resp.total_rows, 2, "post-filter count, got {p:?}");
    assert!(p.contains(&"blog/launch.md"));
    assert!(p.contains(&"blog/no-title.md"));
    assert!(!p.contains(&"blog/freshly-added.md"), "New {{}} row dropped under any filter");

    // Second filter ANDs with the first.
    let resp2 = vdb
        .collection(CollectionQuery {
            path: "blog".into(),
            recursive: false,
            filters: vec![
                MetadataFilter::Equals { field: "status".into(), value: serde_json::json!("published") },
                MetadataFilter::Equals { field: "date".into(), value: serde_json::json!("2024-03-10") },
            ],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(resp2.total_rows, 1);
    assert_eq!(resp2.rows[0].path, "blog/no-title.md");
}

#[test]
fn test_collection_pagination_total_rows() {
    let dir = setup_collection_vault();
    let vdb = open(&dir);

    let full = vdb
        .collection(CollectionQuery { path: "blog".into(), recursive: true, ..Default::default() })
        .unwrap();
    assert_eq!(full.total_rows, 6);

    let page = vdb
        .collection(CollectionQuery {
            path: "blog".into(),
            recursive: true,
            limit: Some(2),
            offset: 1,
            ..Default::default()
        })
        .unwrap();

    // total_rows is independent of the page size.
    assert_eq!(page.total_rows, 6);
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.limit, Some(2));
    assert_eq!(page.offset, 1);

    // Default path-ascending order: page is the 2nd and 3rd rows.
    assert_eq!(page.rows[0].path, full.rows[1].path);
    assert_eq!(page.rows[1].path, full.rows[2].path);
}

#[test]
fn test_collection_frontmatter_always_object() {
    let dir = setup_collection_vault();
    let vdb = open(&dir);

    let resp = vdb
        .collection(CollectionQuery { path: "blog".into(), recursive: false, ..Default::default() })
        .unwrap();

    let plain = resp.rows.iter().find(|r| r.path == "blog/plain.md").unwrap();
    assert!(plain.frontmatter.is_object(), "frontmatter is always an object");
    assert!(
        plain.frontmatter.as_object().unwrap().is_empty(),
        "no-frontmatter file yields {{}}, not null"
    );
    // Every row's frontmatter is an object, never null.
    assert!(resp.rows.iter().all(|r| r.frontmatter.is_object()));
}

#[test]
fn test_collection_new_and_deleted_states() {
    let dir = setup_collection_vault();

    // Add an unindexed file on disk → New.
    fs::write(
        dir.path().join("blog/brand-new.md"),
        "---\ntitle: Brand New\n---\n\n# Brand New\n",
    )
    .unwrap();
    // Delete an indexed file → Deleted.
    fs::remove_file(dir.path().join("blog/intro.md")).unwrap();

    let vdb = open(&dir);
    let resp = vdb
        .collection(CollectionQuery { path: "blog".into(), recursive: false, ..Default::default() })
        .unwrap();

    let new_row = resp.rows.iter().find(|r| r.path == "blog/brand-new.md").unwrap();
    assert_eq!(new_row.state, FileState::New);
    assert!(new_row.content_hash.is_none(), "new rows have null content_hash");
    assert!(new_row.indexed_at.is_none(), "new rows have null indexed_at");
    assert!(new_row.frontmatter.is_object());
    assert!(
        new_row.frontmatter.as_object().unwrap().is_empty(),
        "new files are not parsed in v1 → empty frontmatter"
    );
    assert_eq!(new_row.title, "brand-new", "new row title from filename stem");

    let deleted_row = resp.rows.iter().find(|r| r.path == "blog/intro.md").unwrap();
    assert_eq!(deleted_row.state, FileState::Deleted);
    assert!(deleted_row.content_hash.is_some(), "deleted rows keep stored content_hash");
    assert!(deleted_row.indexed_at.is_some(), "deleted rows keep stored indexed_at");
    // Stored frontmatter survives deletion.
    assert_eq!(
        deleted_row.frontmatter.get("title").and_then(|v| v.as_str()),
        Some("Intro Post")
    );
}

#[test]
fn test_collection_whole_vault_scope() {
    let dir = setup_collection_vault();
    let vdb = open(&dir);

    // "." => whole vault. Non-recursive → only root-level files (there are none
    // at the root here), so recursive is needed to see everything.
    let resp = vdb
        .collection(CollectionQuery { path: ".".into(), recursive: true, ..Default::default() })
        .unwrap();
    assert_eq!(resp.scope, ".");
    let p = paths(&resp);
    assert!(p.contains(&"blog/launch.md"));
    assert!(p.contains(&"docs/guide.md"));
    assert!(p.contains(&"blog/2024/recap.md"));
    // Columns come from the global schema; "version" (docs-only) is present.
    assert!(resp.columns.iter().any(|c| c.name == "version"));
    assert!(resp.columns.iter().any(|c| c.name == "title"));
}

#[test]
fn test_collection_read_only_no_markdown_writes() {
    let dir = setup_collection_vault();
    let vdb = open(&dir);

    let before = fs::read_to_string(dir.path().join("blog/launch.md")).unwrap();
    let _ = vdb
        .collection(CollectionQuery {
            path: "blog".into(),
            recursive: true,
            sort_by: Some("date".into()),
            filters: vec![MetadataFilter::Exists { field: "status".into() }],
            ..Default::default()
        })
        .unwrap();
    let after = fs::read_to_string(dir.path().join("blog/launch.md")).unwrap();
    assert_eq!(before, after, "collection must never modify markdown files");
}

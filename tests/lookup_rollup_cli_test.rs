use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use mdvdb::index::state::Index;
use mdvdb::index::types::EmbeddingConfig;
use mdvdb::{ComputedFieldDiagnostic, ComputedFieldEntry};
use serde_json::Value;
use tempfile::TempDir;

fn mdvdb_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mdvdb"))
}

fn run(root: &Path, args: &[&str]) -> Output {
    mdvdb_bin()
        .args(args)
        .env("MDVDB_NO_UPDATE_CHECK", "1")
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("failed to run `mdvdb {}`: {error}", args.join(" ")))
}

fn run_json(root: &Path, args: &[&str]) -> Value {
    let output = run(root, args);
    assert!(
        output.status.success(),
        "`mdvdb {}` failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "`mdvdb {}` returned invalid JSON: {error}\nstdout:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
        )
    })
}

fn write_config(root: &Path) {
    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb/config.yaml"),
        "embedding:\n  provider: mock\n  dimensions: 8\n",
    )
    .unwrap();
}

fn setup_relation_vault() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_config(root);
    fs::write(
        root.join(".markdownvdb.schema.yml"),
        r#"scopes:
  contacts:
    fields:
      client:
        field_type: relation
        target: clients
      client_domain:
        field_type: lookup
        relation_field: client
        target_field: domain
  clients:
    fields:
      invoice_total:
        field_type: rollup
        relation_direction: incoming
        relation_scope: invoices
        relation_field: client
        target_field: total
        formula: values.reduce((sum, value) => sum + value, 0)
        result_type: number
  invoices:
    fields:
      client:
        field_type: relation
        target: clients
      total:
        field_type: formula
        formula: subtotal + tax
        result_type: number
"#,
    )
    .unwrap();

    for scope in ["contacts", "clients", "invoices"] {
        fs::create_dir_all(root.join(scope)).unwrap();
    }
    fs::write(
        root.join("clients/acme.md"),
        "---\ndomain: acme.example\n---\n\n# Acme\n",
    )
    .unwrap();
    fs::write(
        root.join("contacts/alice.md"),
        "---\nclient: \"[[clients/acme]]\"\n---\n\n# Alice\n",
    )
    .unwrap();
    fs::write(
        root.join("invoices/one.md"),
        "---\nclient: \"[[clients/acme]]\"\nsubtotal: 10.1\ntax: 0.2\n---\n\n# One\n",
    )
    .unwrap();
    fs::write(
        root.join("invoices/two.md"),
        "---\nclient: \"[[clients/acme]]\"\nsubtotal: 20.2\ntax: 0.3\n---\n\n# Two\n",
    )
    .unwrap();
    dir
}

#[test]
fn metadata_only_manual_run_rebuilds_complete_scoped_target_schemas() {
    let dir = setup_relation_vault();
    let root = dir.path();

    // Model the command after a prior self-heal created a compatible empty
    // generation. No ingest and no watcher: the next manual module run must
    // bootstrap source metadata and the ordinary scoped schema without
    // embedding anything.
    Index::create(
        &root.join(".markdownvdb/index"),
        &EmbeddingConfig {
            provider: "Mock".to_string(),
            model: "text-embedding-3-small".to_string(),
            dimensions: 8,
        },
    )
    .unwrap();
    let report = run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    assert!(report["module_reports"]
        .as_array()
        .unwrap()
        .iter()
        .all(|module| module["diagnostics"].as_array().is_some_and(Vec::is_empty)));

    let clients = run_json(root, &["schema", "--path", "clients", "--json"]);
    let client_fields = clients["schema"]["fields"].as_array().unwrap();
    let domain = client_fields
        .iter()
        .find(|field| field["name"] == "domain")
        .expect("ordinary Lookup target belongs to the persisted clients schema");
    assert_eq!(domain["field_type"], "String");
    assert_eq!(domain["occurrence_count"], 1);
    assert!(client_fields
        .iter()
        .any(|field| field["name"] == "invoice_total" && field["field_type"] == "Rollup"));

    let invoices = run_json(root, &["schema", "--path", "invoices", "--json"]);
    let invoice_fields = invoices["schema"]["fields"].as_array().unwrap();
    for ordinary in ["subtotal", "tax"] {
        assert!(invoice_fields
            .iter()
            .any(|field| field["name"] == ordinary && field["field_type"] == "Number"));
    }
    assert!(invoice_fields
        .iter()
        .any(|field| field["name"] == "total" && field["field_type"] == "Formula"));

    let collection = run_json(root, &["collection", "clients", "--json"]);
    assert!(collection["columns"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| {
            field["name"] == "domain" && field["in_schema"] == serde_json::json!(true)
        }));

    // Watcher-off edits to an ordinary target field must refresh the same
    // cached scope on the next manual run, not remain a present-only column.
    let client_path = root.join("clients/acme.md");
    let client = fs::read_to_string(&client_path).unwrap();
    fs::write(
        &client_path,
        client.replace(
            "domain: acme.example",
            "domain: acme.example\nindustry: manufacturing",
        ),
    )
    .unwrap();
    run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    let clients = run_json(root, &["schema", "--path", "clients", "--json"]);
    assert!(clients["schema"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["name"] == "industry" && field["field_type"] == "String"));

    let index = Index::open(&root.join(".markdownvdb/index")).unwrap();
    assert!(index
        .get_all_files()
        .values()
        .all(|file| file.chunk_ids.is_empty() && file.embedding_body_hash.is_empty()));
}

#[test]
fn retargeting_a_lookup_with_a_spaced_output_key_never_duplicates_or_hides_frontmatter() {
    let dir = setup_relation_vault();
    let root = dir.path();
    let overlay_path = root.join(".markdownvdb.schema.yml");
    let overlay = fs::read_to_string(&overlay_path).unwrap();
    fs::write(
        &overlay_path,
        overlay
            .replace("client_domain:", "Client Name:")
            .replace("target_field: domain", "target_field: title"),
    )
    .unwrap();
    let client_path = root.join("clients/acme.md");
    fs::write(
        &client_path,
        "---\ntitle: Acme Corp\ndomain: acme.example\nindustry: manufacturing\n---\n\n# Acme\n",
    )
    .unwrap();
    Index::create(
        &root.join(".markdownvdb/index"),
        &EmbeddingConfig {
            provider: "Mock".to_string(),
            model: "text-embedding-3-small".to_string(),
            dimensions: 8,
        },
    )
    .unwrap();

    let first = run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    assert!(first["diagnostics"].as_array().is_some_and(Vec::is_empty));
    let contact_path = root.join("contacts/alice.md");
    let first_source = fs::read_to_string(&contact_path).unwrap();
    assert_eq!(first_source.matches("Client Name").count(), 1);
    assert!(first_source.contains("\"Client Name\": \"Acme Corp\""));
    assert!(first_source.contains("client: \"[[clients/acme]]\""));
    assert!(first_source.ends_with("\n# Alice\n"));

    let overlay = fs::read_to_string(&overlay_path).unwrap();
    fs::write(
        &overlay_path,
        overlay.replace("target_field: title", "target_field: industry"),
    )
    .unwrap();
    let second = run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    assert!(second["diagnostics"].as_array().is_some_and(Vec::is_empty));
    let retargeted = fs::read_to_string(&contact_path).unwrap();
    assert_eq!(retargeted.matches("Client Name").count(), 1, "{retargeted}");
    assert!(retargeted.contains("\"Client Name\": \"manufacturing\""));
    assert!(!retargeted.contains("Acme Corp"));
    assert!(retargeted.contains("client: \"[[clients/acme]]\""));
    assert!(retargeted.ends_with("\n# Alice\n"));
    let parsed = mdvdb::parser::parse_markdown_file(root, Path::new("contacts/alice.md")).unwrap();
    let frontmatter = parsed
        .frontmatter
        .expect("retargeted frontmatter stays valid");
    assert_eq!(frontmatter["Client Name"], "manufacturing");
    assert_eq!(frontmatter["client"], "[[clients/acme]]");

    let third = run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    assert!(third["diagnostics"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(fs::read_to_string(contact_path).unwrap(), retargeted);
}

#[test]
fn renaming_a_lookup_definition_cleans_only_the_old_owned_key() {
    let dir = setup_relation_vault();
    let root = dir.path();
    run_json(root, &["ingest", "--json"]);

    let contact_path = root.join("contacts/alice.md");
    let initial = fs::read_to_string(&contact_path).unwrap();
    assert!(
        initial.contains("client_domain: \"acme.example\""),
        "{initial}"
    );

    let overlay_path = root.join(".markdownvdb.schema.yml");
    let overlay = fs::read_to_string(&overlay_path).unwrap();
    let renamed_overlay = overlay.replacen("      client_domain:\n", "      client_industry:\n", 1);
    assert_ne!(
        renamed_overlay, overlay,
        "fixture must rename the Lookup definition"
    );
    fs::write(&overlay_path, renamed_overlay).unwrap();

    let report = run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    assert!(report["diagnostics"].as_array().is_some_and(Vec::is_empty));

    let renamed = fs::read_to_string(&contact_path).unwrap();
    assert!(!renamed.contains("client_domain"), "{renamed}");
    assert_eq!(renamed.matches("client_industry").count(), 1, "{renamed}");
    assert!(
        renamed.contains("client_industry: \"acme.example\""),
        "{renamed}"
    );
    assert!(
        renamed.contains("client: \"[[clients/acme]]\""),
        "{renamed}"
    );
    assert!(renamed.ends_with("\n# Alice\n"), "{renamed}");

    let visible = run_json(root, &["get", "contacts/alice.md", "--json"]);
    assert!(visible["frontmatter"].get("client_domain").is_none());
    assert_eq!(visible["frontmatter"]["client_industry"], "acme.example");
    assert!(visible["computed_fields"].get("client_domain").is_none());
    assert_eq!(
        visible["computed_fields"]["client_industry"],
        "acme.example"
    );

    run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    assert_eq!(fs::read_to_string(contact_path).unwrap(), renamed);
}

#[test]
fn batch_write_failure_is_path_local_and_never_rewrites_the_unsafe_record() {
    let dir = setup_relation_vault();
    let root = dir.path();
    let unsafe_source = concat!(
        "---\n",
        "title: Bob\n",
        "Other Name: preserve these exact bytes\n",
        "client: \"[[clients/acme]]\"\n",
        "---\n",
        "\n",
        "# Unsafe first owner\n",
    );
    // This sorts before the pre-existing `alice.md`. The runner must keep
    // advancing after the first owner fails instead of aborting the batch or
    // rolling back a later independently safe owner.
    fs::write(root.join("contacts/00-unsafe.md"), unsafe_source).unwrap();
    Index::create(
        &root.join(".markdownvdb/index"),
        &EmbeddingConfig {
            provider: "Mock".to_string(),
            model: "text-embedding-3-small".to_string(),
            dimensions: 8,
        },
    )
    .unwrap();

    let report = run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic["path"] == "contacts/00-unsafe.md"
            && diagnostic["field"] == "client_domain"
            && diagnostic["code"] == "writeback_failed"
    }));
    assert_eq!(
        fs::read_to_string(root.join("contacts/00-unsafe.md")).unwrap(),
        unsafe_source
    );

    let alice = fs::read_to_string(root.join("contacts/alice.md")).unwrap();
    assert!(alice.contains("client_domain: \"acme.example\""));
    let parsed = mdvdb::parser::parse_markdown_file(root, Path::new("contacts/alice.md")).unwrap();
    assert_eq!(parsed.frontmatter.unwrap()["client_domain"], "acme.example");
}

#[test]
fn one_owner_with_multiple_computed_fields_is_all_or_nothing_on_unsafe_yaml() {
    let dir = setup_relation_vault();
    let root = dir.path();
    let overlay_path = root.join(".markdownvdb.schema.yml");
    let overlay = fs::read_to_string(&overlay_path).unwrap();
    fs::write(
        &overlay_path,
        overlay.replace(
            "      client_domain:\n        field_type: lookup\n        relation_field: client\n        target_field: domain\n",
            concat!(
                "      client_domain:\n",
                "        field_type: lookup\n",
                "        relation_field: client\n",
                "        target_field: domain\n",
                "      client_domain_copy:\n",
                "        field_type: lookup\n",
                "        relation_field: client\n",
                "        target_field: domain\n",
            ),
        ),
    )
    .unwrap();

    // `Other Name` is valid YAML, but it exercises the lossless writer's
    // fail-closed path for a CST construct it cannot safely round-trip. Both
    // computed fields belong to one owner patch, so neither may leak through.
    let unsafe_source = concat!(
        "---\n",
        "title: Unsafe owner\n",
        "Other Name: preserve these exact bytes\n",
        "client: \"[[clients/acme]]\"\n",
        "---\n",
        "\n",
        "# Unsafe owner\n",
    );
    let unsafe_path = root.join("contacts/00-unsafe.md");
    fs::write(&unsafe_path, unsafe_source).unwrap();
    Index::create(
        &root.join(".markdownvdb/index"),
        &EmbeddingConfig {
            provider: "Mock".to_string(),
            model: "text-embedding-3-small".to_string(),
            dimensions: 8,
        },
    )
    .unwrap();

    let report = run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    let failed_fields: std::collections::BTreeSet<_> = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|diagnostic| {
            diagnostic["path"] == "contacts/00-unsafe.md"
                && diagnostic["code"] == "writeback_failed"
        })
        .filter_map(|diagnostic| diagnostic["field"].as_str())
        .collect();
    assert_eq!(
        failed_fields,
        std::collections::BTreeSet::from(["client_domain", "client_domain_copy"])
    );
    assert_eq!(fs::read_to_string(&unsafe_path).unwrap(), unsafe_source);

    let parsed = mdvdb::parser::parse_markdown_file(root, Path::new("contacts/00-unsafe.md"))
        .unwrap()
        .frontmatter
        .expect("the rejected owner must retain valid frontmatter");
    assert_eq!(parsed["title"], "Unsafe owner");
    assert_eq!(parsed["Other Name"], "preserve these exact bytes");
    assert_eq!(parsed["client"], "[[clients/acme]]");
    assert!(parsed.get("client_domain").is_none());
    assert!(parsed.get("client_domain_copy").is_none());

    // A separate owner in the same batch remains independently committable.
    let safe = fs::read_to_string(root.join("contacts/alice.md")).unwrap();
    assert!(safe.contains("client_domain: \"acme.example\""), "{safe}");
    assert!(
        safe.contains("client_domain_copy: \"acme.example\""),
        "{safe}"
    );

    let index = Index::open(&root.join(".markdownvdb/index")).unwrap();
    let fields = index.get_computed_fields("contacts/00-unsafe.md").unwrap();
    for field in ["client_domain", "client_domain_copy"] {
        let entry = &fields[field];
        assert!(entry.value_json.is_none());
        assert!(entry.materialized_value_json.is_none());
        assert_eq!(entry.diagnostic.as_ref().unwrap().code, "writeback_failed");
    }
}

#[test]
fn ordinary_collision_survives_repeated_failing_runs_and_index_reopen() {
    let dir = setup_relation_vault();
    let root = dir.path();
    let overlay_path = root.join(".markdownvdb.schema.yml");
    let overlay = fs::read_to_string(&overlay_path).unwrap();
    fs::write(
        &overlay_path,
        overlay.replace("target_field: domain", "target_field: missing_target_field"),
    )
    .unwrap();

    let source = concat!(
        "---\n",
        "client: \"[[clients/acme]]\"\n",
        "client_domain: user-owned.example\n",
        "note: never delete this ordinary value\n",
        "---\n",
        "\n",
        "# Alice\n",
    );
    let owner_path = root.join("contacts/alice.md");
    fs::write(&owner_path, source).unwrap();
    Index::create(
        &root.join(".markdownvdb/index"),
        &EmbeddingConfig {
            provider: "Mock".to_string(),
            model: "text-embedding-3-small".to_string(),
            dimensions: 8,
        },
    )
    .unwrap();

    for attempt in 1..=2 {
        let report = run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
        assert!(report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| {
                diagnostic["path"] == "contacts/alice.md"
                    && diagnostic["field"] == "client_domain"
                    && diagnostic["code"] == "writeback_failed"
            }));
        assert_eq!(
            fs::read_to_string(&owner_path).unwrap(),
            source,
            "manual run {attempt} rewrote an unowned collision"
        );

        // Opening the durable generation between CLI processes must not turn
        // mere provenance or key presence into ownership on the next run.
        let index = Index::open(&root.join(".markdownvdb/index")).unwrap();
        let file = index.get_file("contacts/alice.md").unwrap();
        let entry = &file.computed_fields["client_domain"];
        assert!(entry.value_json.is_none());
        assert!(entry.materialized_value_json.is_none());
        assert_eq!(entry.diagnostic.as_ref().unwrap().code, "writeback_failed");
        assert!(!file.materialized_field_matches("client_domain", entry));
        drop(index);

        let visible = run_json(root, &["get", "contacts/alice.md", "--json"]);
        assert_eq!(
            visible["frontmatter"]["client_domain"], "user-owned.example",
            "an unowned collision must not be suppressed as a stale computed value"
        );
        assert_eq!(
            visible["frontmatter"]["note"],
            "never delete this ordinary value"
        );
    }
}

#[test]
fn lookup_and_incoming_rollup_materialize_and_follow_target_changes() {
    let dir = setup_relation_vault();
    let root = dir.path();

    let ingest = run_json(root, &["ingest", "--json"]);
    let modules = ingest["module_reports"].as_array().unwrap();
    assert_eq!(modules[0]["module"], "formula");
    assert_eq!(modules[1]["module"], "lookup_rollup");

    let contact = run_json(root, &["get", "contacts/alice.md", "--json"]);
    assert_eq!(contact["frontmatter"]["client_domain"], "acme.example");
    assert_eq!(contact["computed_fields"]["client_domain"], "acme.example");

    let client = run_json(root, &["get", "clients/acme.md", "--json"]);
    assert_eq!(
        client["frontmatter"]["invoice_total"],
        serde_json::json!(30.8)
    );
    assert_eq!(
        client["computed_fields"]["invoice_total"],
        serde_json::json!(30.8)
    );

    fs::write(
        root.join("clients/acme.md"),
        fs::read_to_string(root.join("clients/acme.md"))
            .unwrap()
            .replace("domain: acme.example", "domain: new.example"),
    )
    .unwrap();
    fs::write(
        root.join("invoices/two.md"),
        fs::read_to_string(root.join("invoices/two.md"))
            .unwrap()
            .replace("subtotal: 20.2", "subtotal: 30.2"),
    )
    .unwrap();
    run_json(root, &["ingest", "--json"]);

    let contact = run_json(root, &["get", "contacts/alice.md", "--json"]);
    assert_eq!(contact["frontmatter"]["client_domain"], "new.example");
    let client = run_json(root, &["get", "clients/acme.md", "--json"]);
    assert_eq!(
        client["frontmatter"]["invoice_total"],
        serde_json::json!(40.8)
    );

    fs::remove_file(root.join("invoices/two.md")).unwrap();
    run_json(root, &["ingest", "--json"]);
    let client = run_json(root, &["get", "clients/acme.md", "--json"]);
    assert_eq!(
        client["frontmatter"]["invoice_total"],
        serde_json::json!(10.3)
    );

    fs::write(
        root.join("clients/acme.md"),
        fs::read_to_string(root.join("clients/acme.md"))
            .unwrap()
            .replace("domain: new.example\n", ""),
    )
    .unwrap();
    run_json(root, &["ingest", "--json"]);
    let contact = run_json(root, &["get", "contacts/alice.md", "--json"]);
    assert!(contact["frontmatter"].get("client_domain").is_none());
    assert_eq!(
        contact["computed_field_errors"]["client_domain"]["code"],
        "target_field_missing"
    );

    fs::write(
        root.join("clients/acme.md"),
        "---\ndomain: restored.example\ninvoice_total: 10.3\n---\n\n# Acme\n",
    )
    .unwrap();
    run_json(root, &["ingest", "--json"]);
    let contact = run_json(root, &["get", "contacts/alice.md", "--json"]);
    assert_eq!(contact["frontmatter"]["client_domain"], "restored.example");
}

#[test]
fn lookup_preserves_array_order_duplicates_and_nested_values() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_config(root);
    fs::write(
        root.join(".markdownvdb.schema.yml"),
        r#"scopes:
  contacts:
    fields:
      clients:
        field_type: relation
        target: clients
      client_metadata:
        field_type: lookup
        relation_field: clients
        target_field: metadata
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("contacts")).unwrap();
    fs::create_dir_all(root.join("clients")).unwrap();
    fs::write(
        root.join("clients/a.md"),
        "---\nmetadata:\n  regions: [eu, us]\n  active: true\n---\n",
    )
    .unwrap();
    fs::write(
        root.join("clients/b.md"),
        "---\nmetadata:\n  regions: [apac]\n  active: false\n---\n",
    )
    .unwrap();
    fs::write(
        root.join("contacts/team.md"),
        "---\nclients:\n  - \"[[clients/b]]\"\n  - \"[[clients/a]]\"\n  - \"[[clients/b]]\"\n---\n",
    )
    .unwrap();

    run_json(root, &["ingest", "--json"]);
    let contact = run_json(root, &["get", "contacts/team.md", "--json"]);
    let values = contact["frontmatter"]["client_metadata"]
        .as_array()
        .unwrap();
    assert_eq!(values.len(), 3);
    assert_eq!(values[0]["regions"], serde_json::json!(["apac"]));
    assert_eq!(values[1]["regions"], serde_json::json!(["eu", "us"]));
    assert_eq!(values[2], values[0]);
}

#[test]
fn lookup_rollup_cli_is_discoverable_validatable_and_manually_runnable() {
    let dir = setup_relation_vault();
    let root = dir.path();
    run_json(root, &["ingest", "--json"]);

    let modules = run_json(root, &["modules", "list", "--json"]);
    let descriptor = modules
        .as_array()
        .unwrap()
        .iter()
        .find(|module| module["id"] == "lookup_rollup")
        .expect("lookup_rollup should be compiled in");
    assert_eq!(descriptor["name"], "Lookup & Rollup");
    assert_eq!(descriptor["version"], 1);

    let validation = run_json(
        root,
        &[
            "modules",
            "validate",
            "lookup_rollup",
            "--formula",
            "values.reduce((sum, value) => sum + value, 0)",
            "--result-type",
            "number",
            "--json",
        ],
    );
    assert_eq!(validation["valid"], true);

    // Even a scoped request must fail closed collection-wide: the malformed
    // overlay invalidates every definition, including the Client rollup outside
    // the requested Contacts output lens.
    let report = run_json(
        root,
        &[
            "modules",
            "run",
            "lookup_rollup",
            "--path",
            "contacts",
            "--json",
        ],
    );
    assert_eq!(report["module"], "lookup_rollup");
    assert_eq!(report["event"], "manual_run");
    let ordered = report["module_reports"].as_array().unwrap();
    assert_eq!(ordered[0]["module"], "formula");
    assert_eq!(ordered[1]["module"], "lookup_rollup");
}

#[test]
fn manual_runs_follow_exact_cross_scope_prerequisite_and_downstream_closures() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_config(root);
    fs::write(
        root.join(".markdownvdb.schema.yml"),
        r#"scopes:
  reports:
    fields:
      invoice:
        field_type: relation
        target: invoices
      invoice_total:
        field_type: lookup
        relation_field: invoice
        target_field: total
  invoices:
    fields:
      total:
        field_type: formula
        formula: subtotal + tax
        result_type: number
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("reports")).unwrap();
    fs::create_dir_all(root.join("invoices")).unwrap();
    fs::write(
        root.join("reports/summary.md"),
        "---\ninvoice: \"[[invoices/used]]\"\n---\n",
    )
    .unwrap();
    fs::write(
        root.join("invoices/used.md"),
        "---\nsubtotal: 1\ntax: 1\n---\n",
    )
    .unwrap();
    fs::write(
        root.join("invoices/unused.md"),
        "---\nsubtotal: 10\ntax: 10\n---\n",
    )
    .unwrap();
    run_json(root, &["ingest", "--json"]);

    let used = fs::read_to_string(root.join("invoices/used.md")).unwrap();
    let unused = fs::read_to_string(root.join("invoices/unused.md")).unwrap();
    fs::write(
        root.join("invoices/used.md"),
        used.replace("subtotal: 1", "subtotal: 3"),
    )
    .unwrap();
    fs::write(
        root.join("invoices/unused.md"),
        unused.replace("subtotal: 10", "subtotal: 30"),
    )
    .unwrap();

    let report = run_json(
        root,
        &[
            "modules",
            "run",
            "lookup_rollup",
            "--path",
            "reports",
            "--json",
        ],
    );
    let ordered = report["module_reports"].as_array().unwrap();
    assert_eq!(ordered[0]["module"], "formula");
    assert_eq!(ordered[1]["module"], "lookup_rollup");
    assert!(fs::read_to_string(root.join("invoices/used.md"))
        .unwrap()
        .contains("total: 4"));
    assert!(fs::read_to_string(root.join("invoices/unused.md"))
        .unwrap()
        .contains("total: 20"));
    let summary = run_json(root, &["get", "reports/summary.md", "--json"]);
    assert_eq!(summary["frontmatter"]["invoice_total"], 4);

    let used = fs::read_to_string(root.join("invoices/used.md")).unwrap();
    fs::write(
        root.join("invoices/used.md"),
        used.replace("subtotal: 3", "subtotal: 5"),
    )
    .unwrap();
    let report = run_json(
        root,
        &[
            "modules",
            "run",
            "formula",
            "--path",
            "invoices/used.md",
            "--json",
        ],
    );
    let ordered = report["module_reports"].as_array().unwrap();
    assert_eq!(ordered[0]["module"], "formula");
    assert_eq!(ordered[1]["module"], "lookup_rollup");
    let summary = run_json(root, &["get", "reports/summary.md", "--json"]);
    assert_eq!(summary["frontmatter"]["invoice_total"], 6);
}

#[test]
fn scoped_manual_run_reconciles_new_and_deleted_dependencies_without_embedding() {
    let dir = setup_relation_vault();
    let root = dir.path();
    run_json(root, &["ingest", "--json"]);

    // The watcher is intentionally absent: mutate collection membership on
    // disk, then request only the Client output scope. The new Invoice is both
    // a cross-scope Formula prerequisite and an incoming Rollup member.
    fs::remove_file(root.join("invoices/two.md")).unwrap();
    fs::write(
        root.join("invoices/three.md"),
        "---\nclient: \"[[clients/acme]]\"\nsubtotal: 5\ntax: 0.5\n---\n\n# Three\n",
    )
    .unwrap();

    let report = run_json(
        root,
        &[
            "modules",
            "run",
            "lookup_rollup",
            "--path",
            "clients",
            "--json",
        ],
    );
    let ordered = report["module_reports"].as_array().unwrap();
    assert_eq!(ordered[0]["module"], "formula");
    assert_eq!(ordered[1]["module"], "lookup_rollup");
    assert!(ordered
        .iter()
        .all(|module| { module["diagnostics"].as_array().is_some_and(Vec::is_empty) }));
    assert!(!root.join(".markdownvdb/fts-reconcile-required").exists());

    let client = run_json(root, &["get", "clients/acme.md", "--json"]);
    assert_eq!(
        client["frontmatter"]["invoice_total"],
        serde_json::json!(15.8)
    );
    let invoice = run_json(root, &["get", "invoices/three.md", "--json"]);
    assert_eq!(invoice["frontmatter"]["total"], serde_json::json!(5.5));

    // Manual dependency catch-up is metadata-only. Deleted membership is gone,
    // while the new source has no vectors and carries a sentinel that forces a
    // later ingest to perform the deferred embedding.
    {
        let index = Index::open(&root.join(".markdownvdb/index")).unwrap();
        assert!(index.get_file("invoices/two.md").is_none());
        let provisional = index.get_file("invoices/three.md").unwrap();
        assert!(provisional.chunk_ids.is_empty());
        assert!(provisional.embedding_body_hash.is_empty());
    }

    run_json(root, &["ingest", "--json"]);
    let index = Index::open(&root.join(".markdownvdb/index")).unwrap();
    let embedded = index.get_file("invoices/three.md").unwrap();
    assert!(!embedded.chunk_ids.is_empty());
    assert!(!embedded.embedding_body_hash.is_empty());
}

#[test]
fn module_transaction_materializes_an_overlay_edit_before_releasing_its_lock() {
    let dir = setup_relation_vault();
    let root = dir.path();
    run_json(root, &["ingest", "--json"]);

    let mut child = mdvdb_bin()
        .args([
            "modules",
            "--json",
            "--root",
            root.to_str().unwrap(),
            "transaction",
            "lookup_rollup",
        ])
        .env("MDVDB_NO_UPDATE_CHECK", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut ready = String::new();
    stdout.read_line(&mut ready).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&ready).unwrap()["status"],
        "locked"
    );

    let client_path = root.join("clients/acme.md");
    let client = fs::read_to_string(&client_path).unwrap();
    fs::write(
        &client_path,
        client.replace("domain: acme.example", "domain: transaction.example"),
    )
    .unwrap();
    child.stdin.take().unwrap().write_all(b"run\n").unwrap();

    let mut payload = String::new();
    stdout.read_to_string(&mut payload).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    let report: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(report["module"], "lookup_rollup");
    let contact = run_json(root, &["get", "contacts/alice.md", "--json"]);
    assert_eq!(
        contact["frontmatter"]["client_domain"],
        "transaction.example"
    );
}

#[test]
fn lookup_rollup_validation_rejects_non_values_inputs() {
    let dir = setup_relation_vault();
    let validation = run_json(
        dir.path(),
        &[
            "modules",
            "validate",
            "lookup_rollup",
            "--formula",
            "price + 1",
            "--result-type",
            "number",
            "--json",
        ],
    );

    assert_eq!(validation["valid"], false);
    assert_eq!(validation["diagnostics"][0]["module"], "lookup_rollup");
    assert_eq!(validation["diagnostics"][0]["code"], "unknown_identifier");
}

#[test]
fn read_only_schema_hides_unreconciled_lookup_rollup_overlay_edits() {
    let dir = setup_relation_vault();
    let root = dir.path();
    run_json(root, &["ingest", "--json"]);

    let overlay_path = root.join(".markdownvdb.schema.yml");
    let overlay = fs::read_to_string(&overlay_path).unwrap();
    fs::write(
        &overlay_path,
        overlay.replacen(
            "        target_field: domain\n",
            "        target_field: domain\n      pending_domain:\n        field_type: lookup\n        relation_field: client\n        target_field: domain\n",
            1,
        ),
    )
    .unwrap();

    let before = run_json(root, &["schema", "--path", "contacts", "--json"]);
    let before_fields = before["schema"]["fields"].as_array().unwrap();
    assert!(before_fields
        .iter()
        .all(|field| field["name"] != "pending_domain"));

    run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    let after = run_json(root, &["schema", "--path", "contacts", "--json"]);
    let after_fields = after["schema"]["fields"].as_array().unwrap();
    assert!(after_fields
        .iter()
        .any(|field| field["name"] == "pending_domain"));
}

#[test]
fn manual_run_with_invalid_overlay_fails_closed_instead_of_leaving_stale_values() {
    let dir = setup_relation_vault();
    let root = dir.path();
    run_json(root, &["ingest", "--json"]);

    fs::write(
        root.join(".markdownvdb.schema.yml"),
        "fields: [this is not a mapping]\n",
    )
    .unwrap();

    let report = run_json(root, &["modules", "run", "lookup_rollup", "--json"]);
    let module_reports = report["module_reports"].as_array().unwrap();
    assert_eq!(module_reports[0]["module"], "formula");
    assert_eq!(module_reports[1]["module"], "lookup_rollup");
    assert!(module_reports.iter().all(|module| {
        module["diagnostics"].as_array().is_some_and(|diagnostics| {
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic["code"] == "invalid_schema")
        })
    }));

    let contact = run_json(root, &["get", "contacts/alice.md", "--json"]);
    assert!(contact["frontmatter"].get("client_domain").is_none());
    assert_eq!(
        contact["computed_field_errors"]["client_domain"]["code"],
        "invalid_schema"
    );

    let client = run_json(root, &["get", "clients/acme.md", "--json"]);
    assert!(client["frontmatter"].get("invoice_total").is_none());
    assert_eq!(
        client["computed_field_errors"]["invoice_total"]["code"],
        "invalid_schema"
    );
}

#[test]
fn stale_diagnostic_is_suppressed_while_another_owner_remains_successful() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_config(root);
    fs::create_dir_all(root.join("contacts")).unwrap();
    fs::write(
        root.join("contacts/stale.md"),
        "---\nclient_domain: stale.example\n---\nStale\n",
    )
    .unwrap();
    fs::write(
        root.join("contacts/fresh.md"),
        "---\nclient_domain: fresh.example\n---\nFresh\n",
    )
    .unwrap();
    run_json(root, &["ingest", "--json"]);

    // Model the safety fallback used when a concurrent edit prevents physical
    // cleanup: the stale YAML pair remains on disk, while persisted ownership
    // carries a diagnostic and no successful value.
    let index = Index::open(&root.join(".markdownvdb/index")).unwrap();
    index
        .replace_computed_fields(
            "contacts/stale.md",
            HashMap::from([(
                "client_domain".to_string(),
                ComputedFieldEntry {
                    module: "lookup_rollup".to_string(),
                    definition_fingerprint: "lookup-v1".to_string(),
                    input_fingerprint: Some("inputs-v1".to_string()),
                    dependency_snapshot: Default::default(),
                    value_json: None,
                    // The failed cleanup retained the exact module-authored
                    // value, so this proof still authorizes query suppression.
                    materialized_value_json: Some("\"stale.example\"".to_string()),
                    diagnostic: Some(ComputedFieldDiagnostic {
                        module: "lookup_rollup".to_string(),
                        field: "client_domain".to_string(),
                        code: "dependency_changed".to_string(),
                        message: "target changed during write-back".to_string(),
                        span_start: None,
                        span_end: None,
                    }),
                },
            )]),
        )
        .unwrap();
    index
        .replace_computed_fields(
            "contacts/fresh.md",
            HashMap::from([(
                "client_domain".to_string(),
                ComputedFieldEntry {
                    module: "lookup_rollup".to_string(),
                    definition_fingerprint: "lookup-v1".to_string(),
                    input_fingerprint: Some("inputs-v2".to_string()),
                    dependency_snapshot: Default::default(),
                    value_json: Some("\"fresh.example\"".to_string()),
                    materialized_value_json: Some("\"fresh.example\"".to_string()),
                    diagnostic: None,
                },
            )]),
        )
        .unwrap();
    index.save().unwrap();
    drop(index);

    let stale = run_json(root, &["get", "contacts/stale.md", "--json"]);
    assert!(stale["frontmatter"].get("client_domain").is_none());
    assert!(stale["computed_fields"].get("client_domain").is_none());
    assert_eq!(
        stale["computed_field_errors"]["client_domain"]["code"],
        "dependency_changed"
    );

    let fresh = run_json(root, &["get", "contacts/fresh.md", "--json"]);
    assert_eq!(fresh["frontmatter"]["client_domain"], "fresh.example");
    assert_eq!(fresh["computed_fields"]["client_domain"], "fresh.example");
    assert!(fresh["computed_field_errors"]
        .as_object()
        .is_none_or(serde_json::Map::is_empty));

    let filtered = run_json(
        root,
        &[
            "collection",
            "contacts",
            "--filter",
            "client_domain=stale.example",
            "--json",
        ],
    );
    assert_eq!(filtered["total_rows"], 0);

    let sorted = run_json(
        root,
        &[
            "collection",
            "contacts",
            "--sort",
            "client_domain",
            "--json",
        ],
    );
    let rows = sorted["rows"].as_array().unwrap();
    assert_eq!(rows[0]["path"], "contacts/fresh.md");
    assert_eq!(rows[0]["frontmatter"]["client_domain"], "fresh.example");
    assert_eq!(rows[1]["path"], "contacts/stale.md");
    assert!(rows[1]["frontmatter"].get("client_domain").is_none());
}

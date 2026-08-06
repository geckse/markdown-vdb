use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::str::FromStr;

use rust_decimal::Decimal;
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

fn assert_decimal(value: &Value, expected: &str) {
    let actual = Decimal::from_str(&value.to_string())
        .unwrap_or_else(|error| panic!("{value} is not an exact JSON decimal: {error}"));
    let expected = Decimal::from_str(expected).unwrap();
    assert_eq!(actual, expected);
}

fn write_config(root: &Path) {
    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb/config.yaml"),
        "embedding:\n  provider: mock\n  dimensions: 8\n",
    )
    .unwrap();
}

fn write_initial_schema(root: &Path) {
    fs::write(
        root.join(".markdownvdb.schema.yml"),
        r#"scopes:
  invoices:
    fields:
      exact_sum:
        field_type: formula
        formula: price + fee
        result_type: number
      doubled:
        field_type: formula
        formula: exact_sum * quantity
        result_type: number
      unit_total:
        field_type: formula
        formula: 'fields["Unit Price"] * quantity'
        result_type: number
      broken:
        field_type: formula
        formula: price / divisor
        result_type: number
"#,
    )
    .unwrap();
}

fn write_replacement_schema(root: &Path, increment: u32) {
    fs::write(
        root.join(".markdownvdb.schema.yml"),
        format!(
            r#"scopes:
  invoices:
    fields:
      exact_sum:
        field_type: formula
        formula: price + fee + {increment}
        result_type: number
      doubled:
        field_type: formula
        formula: exact_sum * quantity
        result_type: number
"#
        ),
    )
    .unwrap();
}

fn setup_formula_vault() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_config(root);
    write_initial_schema(root);
    fs::create_dir_all(root.join("invoices")).unwrap();
    fs::write(
        root.join("invoices/a.md"),
        r#"---
price: 0.1
fee: 0.2
quantity: 2
divisor: 0
"Unit Price": 0.15
collision: 999
---

# Invoice A

Invoice formula test record alpha.
"#,
    )
    .unwrap();
    fs::write(
        root.join("invoices/b.md"),
        r#"---
price: 0.2
fee: 0.2
quantity: 3
divisor: 1
"Unit Price": 0.05
---

# Invoice B

Invoice formula test record beta.
"#,
    )
    .unwrap();
    dir
}

fn ingest(root: &Path) -> Value {
    run_json(root, &["ingest", "--json"])
}

fn document(root: &Path, path: &str) -> Value {
    run_json(root, &["get", path, "--json"])
}

#[test]
fn ingest_materializes_formula_results_and_queries_use_source_frontmatter() {
    let dir = setup_formula_vault();
    let root = dir.path();
    let a_path = root.join("invoices/a.md");
    let b_path = root.join("invoices/b.md");
    let a_before = fs::read(&a_path).unwrap();
    let b_before = fs::read(&b_path).unwrap();

    let ingest = ingest(root);
    assert_eq!(ingest["files_indexed"], 2);
    assert_eq!(ingest["files_failed"], 0);
    let report = &ingest["module_reports"][0];
    assert_eq!(report["module"], "formula");
    assert_eq!(report["event"], "files_changed");
    assert_eq!(report["files_evaluated"], 2);

    assert_ne!(fs::read(&a_path).unwrap(), a_before);
    assert_ne!(fs::read(&b_path).unwrap(), b_before);

    // A separate read-only CLI process proves that computed state was persisted.
    let a = document(root, "invoices/a.md");
    assert!(a["computed_fields"].is_object());
    assert!(a["computed_field_errors"].is_object());
    assert_decimal(&a["computed_fields"]["exact_sum"], "0.3");
    assert_decimal(&a["computed_fields"]["doubled"], "0.6");
    assert_decimal(&a["computed_fields"]["unit_total"], "0.3");
    assert!(a["computed_fields"].get("broken").is_none());
    assert_decimal(&a["frontmatter"]["exact_sum"], "0.3");
    assert_decimal(&a["frontmatter"]["doubled"], "0.6");
    assert_decimal(&a["frontmatter"]["unit_total"], "0.3");
    assert_decimal(&a["frontmatter"]["collision"], "999");
    assert!(a["frontmatter"].get("broken").is_none());
    assert_eq!(
        a["computed_field_errors"]["broken"]["code"],
        "division_by_zero"
    );

    let b = document(root, "invoices/b.md");
    assert_decimal(&b["computed_fields"]["exact_sum"], "0.4");
    assert_decimal(&b["computed_fields"]["doubled"], "1.2");
    assert_decimal(&b["computed_fields"]["unit_total"], "0.15");
    assert_decimal(&b["computed_fields"]["broken"], "0.2");
    assert_eq!(b["computed_field_errors"], serde_json::json!({}));

    let collection = run_json(
        root,
        &[
            "collection",
            "invoices",
            "--sort",
            "doubled",
            "--order",
            "asc",
            "--json",
        ],
    );
    let rows = collection["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["path"], "invoices/a.md");
    assert_eq!(rows[1]["path"], "invoices/b.md");
    assert!(rows
        .iter()
        .all(|row| row["computed_fields"].is_object() && row["computed_field_errors"].is_object()));
    let exact_sum_column = collection["columns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|column| column["name"] == "exact_sum")
        .expect("formula column should be part of the collection schema");
    assert_eq!(exact_sum_column["field_type"], "Formula");
    assert_eq!(exact_sum_column["formula"], "price + fee");
    assert_eq!(exact_sum_column["result_type"], "Number");
    assert_eq!(exact_sum_column["occurrence_count"], 2);

    let filtered = run_json(
        root,
        &[
            "collection",
            "invoices",
            "--filter",
            "exact_sum=0.3",
            "--json",
        ],
    );
    assert_eq!(filtered["total_rows"], 1);
    assert_eq!(filtered["rows"][0]["path"], "invoices/a.md");
    assert_decimal(&filtered["rows"][0]["frontmatter"]["exact_sum"], "0.3");
    assert_decimal(&filtered["rows"][0]["computed_fields"]["exact_sum"], "0.3");

    // An ordinary field remains ordinary when no computed definition owns it.
    let collision_filtered = run_json(
        root,
        &[
            "collection",
            "invoices",
            "--filter",
            "collision=999",
            "--json",
        ],
    );
    assert_eq!(collision_filtered["total_rows"], 1);
    assert_eq!(collision_filtered["rows"][0]["path"], "invoices/a.md");
    assert_decimal(
        &collision_filtered["rows"][0]["frontmatter"]["collision"],
        "999",
    );
    assert!(collision_filtered["rows"][0]["computed_fields"]
        .get("collision")
        .is_none());

    let search = run_json(
        root,
        &[
            "search",
            "invoice",
            "--mode",
            "lexical",
            "--filter",
            "exact_sum=0.3",
            "--json",
        ],
    );
    let results = search["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "computed metadata should be usable by search filters"
    );
    for result in results {
        assert_eq!(result["file"]["path"], "invoices/a.md");
        assert!(result["file"]["computed_fields"].is_object());
        assert!(result["file"]["computed_field_errors"].is_object());
        assert_decimal(&result["file"]["computed_fields"]["exact_sum"], "0.3");
        assert_decimal(&result["file"]["frontmatter"]["exact_sum"], "0.3");
    }
}

#[test]
fn formula_definition_adopts_an_existing_ordinary_key() {
    // Adopt-by-declaration: the overlay is the user's own statement that
    // `collision` is computed for this scope, so a pre-existing same-named
    // value (a stale materialization after an index rebuild, or a manual
    // value the user has since declared computed) is overwritten and owned.
    // This is what lets a rebuilt index self-heal instead of refusing
    // writebacks forever.
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_config(root);
    fs::write(
        root.join(".markdownvdb.schema.yml"),
        r#"scopes:
  invoices:
    fields:
      collision:
        field_type: formula
        formula: price * 10
        result_type: number
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("invoices")).unwrap();
    let path = root.join("invoices/a.md");
    let original = r#"---
price: 1
collision: 999
---
Body
"#;
    fs::write(&path, original).unwrap();

    let first = ingest(root);
    assert_eq!(first["files_failed"], 0);
    let diagnostics = first["module_reports"][0]["diagnostics"]
        .as_array()
        .unwrap();
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic["field"] == "collision"),
        "adoption must not report a writeback failure: {diagnostics:?}"
    );
    let rewritten = fs::read_to_string(&path).unwrap();
    assert!(
        rewritten.contains("collision: 10"),
        "the declared formula value must replace the stale key, got:\n{rewritten}"
    );
    assert!(
        rewritten.contains("price: 1") && rewritten.ends_with("Body\n"),
        "unrelated YAML and the body must be preserved"
    );

    let first_document = document(root, "invoices/a.md");
    assert_decimal(&first_document["frontmatter"]["collision"], "10");
    assert_decimal(&first_document["computed_fields"]["collision"], "10");
    assert!(first_document["computed_field_errors"]
        .get("collision")
        .is_none());

    // Converged state: another ingest must not rewrite the file again.
    let after_first = fs::read(&path).unwrap();
    let _second = ingest(root);
    assert_eq!(fs::read(&path).unwrap(), after_first);
    let reopened = document(root, "invoices/a.md");
    assert_decimal(&reopened["frontmatter"]["collision"], "10");
    assert_decimal(&reopened["computed_fields"]["collision"], "10");
}

#[test]
fn ingest_catches_up_schema_only_formula_changes_without_reembedding() {
    let dir = setup_formula_vault();
    let root = dir.path();
    let a_path = root.join("invoices/a.md");
    let b_path = root.join("invoices/b.md");
    ingest(root);
    let a_before = fs::read(&a_path).unwrap();
    let b_before = fs::read(&b_path).unwrap();

    write_replacement_schema(root, 1);
    let catch_up = ingest(root);
    assert_eq!(catch_up["files_indexed"], 0);
    assert_eq!(catch_up["files_skipped"], 2);
    assert_eq!(catch_up["api_calls"], 0);
    assert_eq!(catch_up["module_reports"][0]["module"], "formula");
    assert_eq!(catch_up["module_reports"][0]["event"], "files_changed");
    assert_eq!(catch_up["module_reports"][0]["files_evaluated"], 2);

    let a = document(root, "invoices/a.md");
    assert_decimal(&a["computed_fields"]["exact_sum"], "1.3");
    assert_decimal(&a["computed_fields"]["doubled"], "2.6");
    assert!(a["computed_fields"].get("unit_total").is_none());
    assert!(a["computed_field_errors"].get("broken").is_none());

    let a_after = fs::read_to_string(&a_path).unwrap();
    let b_after = fs::read_to_string(&b_path).unwrap();
    assert_ne!(a_after.as_bytes(), a_before);
    assert_ne!(b_after.as_bytes(), b_before);
    assert!(!a_after.contains("unit_total:"));
    assert!(a_after.contains("collision: 999"));
    assert!(!b_after.contains("broken:"));
    let collection = run_json(root, &["collection", "invoices", "--json"]);
    let columns = collection["columns"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|column| column["name"].as_str())
        .collect::<Vec<_>>();
    assert!(!columns.contains(&"unit_total"));
    assert!(!columns.contains(&"broken"));
    assert!(columns.contains(&"collision"));
}

#[test]
fn frontmatter_input_change_recomputes_once_without_reembedding_or_echo_loop() {
    let dir = setup_formula_vault();
    let root = dir.path();
    let a_path = root.join("invoices/a.md");
    ingest(root);

    let current = fs::read_to_string(&a_path).unwrap();
    fs::write(&a_path, current.replacen("price: 0.1", "price: 0.4", 1)).unwrap();

    let changed = ingest(root);
    assert_eq!(changed["files_indexed"], 1);
    assert_eq!(changed["files_skipped"], 1);
    assert_eq!(changed["api_calls"], 0);
    assert_eq!(
        changed["module_reports"][0]["files_evaluated"], 1,
        "the edited row is recomputed exactly once"
    );
    let a = document(root, "invoices/a.md");
    assert_decimal(&a["frontmatter"]["exact_sum"], "0.6");
    assert_decimal(&a["frontmatter"]["doubled"], "1.2");
    assert_decimal(&a["frontmatter"]["collision"], "999");

    // The Formula write-back hash was committed with the same index save. Its
    // filesystem echo is a true no-op on the next catch-up.
    let echo = ingest(root);
    assert_eq!(echo["files_indexed"], 0);
    assert_eq!(echo["files_skipped"], 2);
    assert_eq!(echo["api_calls"], 0);
    assert_eq!(echo["module_reports"][0]["files_evaluated"], 0);
}

#[test]
fn manual_formula_run_reads_current_markdown_without_hiding_a_changed_body() {
    let dir = setup_formula_vault();
    let root = dir.path();
    let a_path = root.join("invoices/a.md");
    ingest(root);

    let current = fs::read_to_string(&a_path).unwrap();
    let changed = current.replacen("price: 0.1", "price: 0.5", 1).replace(
        "Invoice formula test record alpha.",
        "A genuinely changed body.",
    );
    fs::write(&a_path, changed).unwrap();

    let report = run_json(
        root,
        &["modules", "run", "formula", "--path", "invoices", "--json"],
    );
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["field"], "broken");
    assert_eq!(diagnostics[0]["code"], "division_by_zero");
    let a = document(root, "invoices/a.md");
    assert_decimal(&a["frontmatter"]["exact_sum"], "0.7");

    let catch_up = ingest(root);
    assert_eq!(catch_up["files_indexed"], 1);
    assert_eq!(catch_up["api_calls"], 1);
}

#[test]
fn modules_cli_lists_validates_runs_and_reports_cached_status() {
    let dir = setup_formula_vault();
    let root = dir.path();
    let a_path = root.join("invoices/a.md");
    ingest(root);
    let a_before = fs::read(&a_path).unwrap();

    let modules = run_json(root, &["modules", "list", "--json"]);
    let formula = modules
        .as_array()
        .unwrap()
        .iter()
        .find(|module| module["id"] == "formula")
        .expect("formula should be a compiled-in module");
    assert_eq!(formula["name"], "Formula");
    assert_eq!(formula["always_on"], true);
    assert_eq!(
        formula["hooks"],
        serde_json::json!([
            "full_ingest",
            "files_changed",
            "schema_changed",
            "manual_run"
        ])
    );

    let valid = run_json(
        root,
        &[
            "modules",
            "validate",
            "formula",
            "--formula",
            "Math.round((price + fee) * 100) / 100",
            "--result-type",
            "number",
            "--json",
        ],
    );
    assert_eq!(valid["valid"], true);
    assert_eq!(valid["diagnostics"], serde_json::json!([]));

    let invalid = run_json(
        root,
        &[
            "modules",
            "validate",
            "formula",
            "--formula",
            "price = 3",
            "--result-type",
            "number",
            "--json",
        ],
    );
    assert_eq!(invalid["valid"], false);
    let invalid_diagnostics = invalid["diagnostics"].as_array().unwrap();
    assert!(!invalid_diagnostics.is_empty());
    assert_eq!(invalid_diagnostics[0]["module"], "formula");
    assert_eq!(invalid_diagnostics[0]["field"], "__validation__");
    assert!(invalid_diagnostics[0]["code"].is_string());
    assert!(invalid_diagnostics[0]["message"].is_string());

    let initial_status = run_json(
        root,
        &[
            "modules", "status", "formula", "--path", "invoices", "--json",
        ],
    );
    let status_codes: Vec<&str> = initial_status
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap())
        .collect();
    assert!(status_codes.contains(&"division_by_zero"));

    // Status is read-only: changing definitions alone must not replace cached
    // diagnostics until an ingest, watcher hook, or explicit module run occurs.
    write_replacement_schema(root, 2);
    let still_cached = run_json(
        root,
        &[
            "modules", "status", "formula", "--path", "invoices", "--json",
        ],
    );
    assert_eq!(still_cached, initial_status);

    let report = run_json(
        root,
        &["modules", "run", "formula", "--path", "invoices", "--json"],
    );
    assert_eq!(report["module"], "formula");
    assert_eq!(report["event"], "manual_run");
    assert_eq!(report["files_evaluated"], 2);
    assert_eq!(report["fields_updated"], 4);
    assert_eq!(report["diagnostics"], serde_json::json!([]));

    let refreshed_status = run_json(
        root,
        &[
            "modules", "status", "formula", "--path", "invoices", "--json",
        ],
    );
    assert_eq!(refreshed_status, serde_json::json!([]));

    let a = document(root, "invoices/a.md");
    assert_decimal(&a["computed_fields"]["exact_sum"], "2.3");
    assert_decimal(&a["computed_fields"]["doubled"], "4.6");
    assert_eq!(a["computed_field_errors"], serde_json::json!({}));
    assert_ne!(fs::read(&a_path).unwrap(), a_before);
}

#[test]
fn rollup_self_heals_stale_materialized_values_after_index_rebuild() {
    // The exact trap from the checked-in test vault: markdown files carry
    // previously materialized rollup values, but the index (and with it all
    // ownership provenance) is brand new. Adopt-by-declaration must correct
    // the stale values on the first run and keep propagating afterwards.
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_config(root);
    fs::write(
        root.join(".markdownvdb.schema.yml"),
        r#"scopes:
  invoices:
    fields:
      client:
        field_type: relation
        target: clients
  clients:
    fields:
      invoice_total:
        field_type: rollup
        relation_direction: incoming
        relation_scope: invoices
        relation_field: client
        target_field: amount
        formula: values.reduce((sum, value) => sum + value, 0)
        result_type: number
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("clients")).unwrap();
    fs::create_dir_all(root.join("invoices")).unwrap();
    // Stale materialization baked into the file: true total is 8800 + 1500.
    fs::write(
        root.join("clients/acme.md"),
        "---\ntitle: Acme\ninvoice_total: 6300\n---\nAcme body\n",
    )
    .unwrap();
    fs::write(
        root.join("invoices/one.md"),
        "---\nclient: \"[[clients/acme]]\"\namount: 8800\n---\nInvoice one\n",
    )
    .unwrap();
    fs::write(
        root.join("invoices/two.md"),
        "---\nclient: \"[[clients/acme]]\"\namount: 1500\n---\nInvoice two\n",
    )
    .unwrap();

    ingest(root);
    let healed = document(root, "clients/acme.md");
    assert_eq!(
        healed["frontmatter"]["invoice_total"], 10300,
        "a fresh index must adopt and correct the stale materialized value"
    );
    assert!(healed["computed_field_errors"]
        .get("invoice_total")
        .is_none());

    // Ownership is recorded, so a later source edit propagates normally.
    fs::write(
        root.join("invoices/one.md"),
        "---\nclient: \"[[clients/acme]]\"\namount: 6800\n---\nInvoice one\n",
    )
    .unwrap();
    ingest(root);
    let updated = document(root, "clients/acme.md");
    assert_eq!(updated["frontmatter"]["invoice_total"], 8300);
}

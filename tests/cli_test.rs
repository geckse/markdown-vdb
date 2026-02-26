use std::fs;
use std::process::Command;

use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the path to the compiled binary via `cargo build` output directory.
/// We rely on `env!("CARGO_BIN_EXE_mdvdb")` at test-time, but since that
/// requires assert_cmd's `cargo_bin`, we use the CARGO_BIN_EXE env var that
/// cargo sets for integration tests when [[bin]] is defined.
fn mdvdb_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mdvdb"))
}

/// Create a temp directory with a mock-provider config and some markdown files,
/// then run `ingest` so the index is populated and ready for queries.
fn setup_and_ingest() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(
        root.join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    fs::write(
        root.join("hello.md"),
        "---\ntitle: Hello World\nstatus: published\n---\n\n# Hello\n\nThis is a test document about greetings.\n",
    )
    .unwrap();

    fs::write(
        root.join("rust.md"),
        "---\ntitle: Rust Guide\nstatus: draft\n---\n\n# Rust\n\nRust is a systems programming language.\n",
    )
    .unwrap();

    let output = mdvdb_bin()
        .arg("ingest")
        .current_dir(root)
        .output()
        .expect("failed to run ingest");
    assert!(
        output.status.success(),
        "ingest should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    dir
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_status_json_without_index_exits_with_error() {
    let dir = TempDir::new().unwrap();
    let output = mdvdb_bin()
        .args(["status", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to execute mdvdb");

    assert!(
        !output.status.success(),
        "status --json without index should fail, got exit code {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.is_empty(),
        "expected error message on stderr"
    );
}

#[test]
fn test_init_creates_config_file() {
    let dir = TempDir::new().unwrap();
    let output = mdvdb_bin()
        .arg("init")
        .current_dir(dir.path())
        .output()
        .expect("failed to execute mdvdb");

    assert!(
        output.status.success(),
        "init should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dir.path().join(".markdownvdb").exists(),
        ".markdownvdb file should be created"
    );
}

#[test]
fn test_init_when_config_exists_fails() {
    let dir = TempDir::new().unwrap();
    // First init — should succeed.
    let first = mdvdb_bin()
        .arg("init")
        .current_dir(dir.path())
        .output()
        .expect("failed to execute mdvdb");
    assert!(first.status.success(), "first init should succeed");

    // Second init — should fail.
    let second = mdvdb_bin()
        .arg("init")
        .current_dir(dir.path())
        .output()
        .expect("failed to execute mdvdb");

    assert!(
        !second.status.success(),
        "init when .markdownvdb exists should fail"
    );
}

#[test]
fn test_completions_bash_outputs_script() {
    let output = mdvdb_bin()
        .args(["completions", "bash"])
        .output()
        .expect("failed to execute mdvdb");

    assert!(
        output.status.success(),
        "completions bash should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("complete") || stdout.contains("_mdvdb"),
        "should output a bash completion script, got: {}",
        &stdout[..stdout.len().min(200)]
    );
}

#[test]
fn test_help_shows_all_subcommands() {
    let output = mdvdb_bin()
        .arg("--help")
        .output()
        .expect("failed to execute mdvdb");

    assert!(output.status.success(), "—help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    for cmd in ["search", "ingest", "status", "init"] {
        assert!(
            stdout.contains(cmd),
            "--help output should mention '{cmd}'"
        );
    }
}

#[test]
fn test_search_help_shows_flags() {
    let output = mdvdb_bin()
        .args(["search", "--help"])
        .output()
        .expect("failed to execute mdvdb");

    assert!(output.status.success(), "search --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    for flag in ["--limit", "--min-score", "--filter", "--json"] {
        assert!(
            stdout.contains(flag),
            "search --help should mention '{flag}'"
        );
    }
}

#[test]
fn test_ingest_json_output() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(
        root.join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();
    fs::write(root.join("doc.md"), "# Doc\n\nSome content.\n").unwrap();

    let output = mdvdb_bin()
        .args(["ingest", "--json"])
        .current_dir(root)
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success(), "ingest --json should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["files_indexed"].as_u64().unwrap() > 0);
    assert_eq!(json["files_failed"].as_u64().unwrap(), 0);
}

#[test]
fn test_search_json_output_format() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["search", "rust programming", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success(), "search --json should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    // Wrapped format: { results, query, total_results }
    assert!(json["results"].is_array(), "should have 'results' array");
    assert_eq!(json["query"].as_str().unwrap(), "rust programming");
    assert!(json["total_results"].is_number(), "should have 'total_results'");
}

#[test]
fn test_status_json_output_after_ingest() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["status", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success(), "status --json should succeed after ingest");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    assert!(json["document_count"].as_u64().unwrap() > 0);
    assert!(json["chunk_count"].as_u64().unwrap() > 0);
    assert!(json["vector_count"].as_u64().unwrap() > 0);
}

#[test]
fn test_schema_json_output() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["schema", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success(), "schema --json should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    assert!(json["fields"].is_array(), "schema should have 'fields' array");
    let fields = json["fields"].as_array().unwrap();
    assert!(!fields.is_empty(), "should have inferred schema fields");

    let names: Vec<&str> = fields.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"title"), "schema should contain 'title', got: {names:?}");
    assert!(names.contains(&"status"), "schema should contain 'status', got: {names:?}");
}

#[test]
fn test_get_json_output() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["get", "hello.md", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success(), "get --json should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    assert_eq!(json["path"].as_str().unwrap(), "hello.md");
    assert!(json["chunk_count"].as_u64().unwrap() > 0);
    assert!(json["file_size"].as_u64().unwrap() > 0);
    assert!(!json["content_hash"].as_str().unwrap().is_empty());
    // frontmatter should be present
    assert!(json["frontmatter"].is_object(), "get --json should include frontmatter");
    assert_eq!(json["frontmatter"]["title"].as_str().unwrap(), "Hello World");
}

#[test]
fn test_get_nonexistent_file_exits_with_error() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["get", "nonexistent.md"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        !output.status.success(),
        "get for nonexistent file should fail"
    );
}

#[test]
fn test_search_limit_flag() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["search", "document", "--limit", "1", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success(), "search --limit should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    let results = json["results"].as_array().unwrap();
    assert!(results.len() <= 1, "should return at most 1 result with --limit 1");
}

#[test]
fn test_search_filter_flag() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["search", "document", "--filter", "status=published", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "search --filter should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["results"].is_array(), "filtered search should return results array");
}

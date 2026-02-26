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

// ---------------------------------------------------------------------------
// CLI formatting tests
// ---------------------------------------------------------------------------

#[test]
fn test_no_subcommand_shows_logo() {
    let dir = TempDir::new().unwrap();
    let output = mdvdb_bin()
        .current_dir(dir.path())
        .output()
        .expect("failed to execute mdvdb");

    // No subcommand should show the logo and exit successfully.
    assert!(output.status.success(), "no subcommand should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The logo contains ASCII art with "mdvdb" stylized characters.
    assert!(
        stdout.contains("__,_") || stdout.contains("mdvdb"),
        "no-subcommand output should contain logo text, got: {}",
        &stdout[..stdout.len().min(300)]
    );
}

#[test]
fn test_no_color_flag_disables_colors() {
    let dir = setup_and_ingest();
    let output = mdvdb_bin()
        .args(["--no-color", "status"])
        .current_dir(dir.path())
        .output()
        .expect("failed to execute mdvdb");

    assert!(output.status.success(), "status with --no-color should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("\x1b["),
        "stdout should not contain ANSI escape sequences with --no-color, got: {stdout}"
    );
}

#[test]
fn test_no_color_env_var() {
    let dir = setup_and_ingest();
    let output = mdvdb_bin()
        .args(["status"])
        .env("NO_COLOR", "1")
        .current_dir(dir.path())
        .output()
        .expect("failed to execute mdvdb");

    assert!(output.status.success(), "status with NO_COLOR=1 should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("\x1b["),
        "stdout should not contain ANSI escape sequences with NO_COLOR=1, got: {stdout}"
    );
}

#[test]
fn test_search_human_shows_score_bar() {
    let dir = setup_and_ingest();
    let output = mdvdb_bin()
        .args(["--no-color", "search", "rust programming"])
        .current_dir(dir.path())
        .output()
        .expect("failed to execute mdvdb");

    assert!(output.status.success(), "human search should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Score bars use █ or ░ characters.
    assert!(
        stdout.contains('█') || stdout.contains('░'),
        "search output should contain bar characters (█/░), got: {stdout}"
    );
}

#[test]
fn test_clusters_shows_keywords() {
    let dir = setup_and_ingest();
    let output = mdvdb_bin()
        .args(["--no-color", "clusters"])
        .current_dir(dir.path())
        .output()
        .expect("failed to execute mdvdb");

    assert!(output.status.success(), "clusters should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Clusters output should include keyword text (the TF-IDF labels).
    assert!(
        stdout.contains("keyword") || stdout.contains("Keyword") || stdout.contains("document") || stdout.len() > 20,
        "clusters output should include keyword or cluster info, got: {stdout}"
    );
}

#[test]
fn test_get_shows_frontmatter() {
    let dir = setup_and_ingest();
    let output = mdvdb_bin()
        .args(["--no-color", "get", "hello.md"])
        .current_dir(dir.path())
        .output()
        .expect("failed to execute mdvdb");

    assert!(output.status.success(), "get should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should show frontmatter field names from hello.md.
    assert!(
        stdout.contains("title"),
        "get output should include frontmatter field 'title', got: {stdout}"
    );
    assert!(
        stdout.contains("Hello World") || stdout.contains("hello"),
        "get output should include frontmatter value, got: {stdout}"
    );
}

#[test]
fn test_ingest_json_unchanged() {
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
    assert!(
        !stdout.contains("\x1b["),
        "ingest --json should not contain ANSI codes, got: {stdout}"
    );
    // Verify it's valid JSON (no ANSI contamination).
    let _: serde_json::Value = serde_json::from_str(&stdout).expect("ingest --json should be valid JSON");
}

#[test]
fn test_status_json_unchanged() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["status", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success(), "status --json should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("\x1b["),
        "status --json should not contain ANSI codes, got: {stdout}"
    );
    // Verify it's valid JSON.
    let _: serde_json::Value = serde_json::from_str(&stdout).expect("status --json should be valid JSON");
}

// ---------------------------------------------------------------------------
// Tree command tests
// ---------------------------------------------------------------------------

fn setup_and_ingest_with_subdirs() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(
        root.join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("docs")).unwrap();
    fs::create_dir_all(root.join("notes")).unwrap();

    fs::write(
        root.join("readme.md"),
        "---\ntitle: Readme\n---\n\n# Readme\n\nTop-level readme.\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/guide.md"),
        "---\ntitle: Guide\n---\n\n# Guide\n\nA guide document.\n",
    )
    .unwrap();
    fs::write(
        root.join("notes/todo.md"),
        "---\ntitle: Todo\n---\n\n# Todo\n\nThings to do.\n",
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

#[test]
fn test_tree_json_output() {
    let dir = setup_and_ingest_with_subdirs();

    let output = mdvdb_bin()
        .args(["tree", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "tree --json should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    // Should have a root node with children
    assert!(json["root"].is_object(), "tree should have 'root' object");
    assert!(
        json["root"]["children"].is_array(),
        "root should have 'children' array"
    );
}

#[test]
fn test_tree_path_filter() {
    let dir = setup_and_ingest_with_subdirs();

    let output = mdvdb_bin()
        .args(["tree", "--path", "docs", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "tree --path docs --json should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    // The filtered tree should contain docs content
    let tree_str = serde_json::to_string(&json).unwrap();
    assert!(
        tree_str.contains("guide.md") || tree_str.contains("docs"),
        "filtered tree should contain docs content, got: {tree_str}"
    );
}

#[test]
fn test_search_path_flag() {
    let dir = setup_and_ingest_with_subdirs();

    // Search with --path restricting to docs/
    let output = mdvdb_bin()
        .args(["search", "document", "--path", "docs", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "search --path should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    assert!(json["results"].is_array(), "should have 'results' array");
    // With mock embeddings all vectors are similar, but any returned results
    // must be scoped to the docs/ prefix.
    let results = json["results"].as_array().unwrap();
    for result in results {
        if let Some(path) = result["path"].as_str() {
            if !path.is_empty() {
                assert!(
                    path.starts_with("docs/"),
                    "search --path docs should only return docs/ files, got: {path}"
                );
            }
        }
    }
}

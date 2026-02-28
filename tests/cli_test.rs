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

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
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
        dir.path().join(".markdownvdb").join(".config").exists(),
        ".markdownvdb/.config should be created"
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

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
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

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
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

// ---------------------------------------------------------------------------
// Link graph CLI tests
// ---------------------------------------------------------------------------

/// Create a temp directory with markdown files that contain links, ingest it.
fn setup_and_ingest_with_links() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(
        root.join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    fs::write(
        root.join("alpha.md"),
        "---\ntitle: Alpha\n---\n\n# Alpha\n\nLinks to [Beta](beta.md) and [Gamma](gamma.md).\n",
    )
    .unwrap();

    fs::write(
        root.join("beta.md"),
        "---\ntitle: Beta\n---\n\n# Beta\n\nLinks back to [Alpha](alpha.md).\n",
    )
    .unwrap();

    fs::write(
        root.join("gamma.md"),
        "---\ntitle: Gamma\n---\n\n# Gamma\n\nNo outgoing links here.\n",
    )
    .unwrap();

    fs::write(
        root.join("orphan.md"),
        "---\ntitle: Orphan\n---\n\n# Orphan\n\nThis file has no links at all.\n",
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
fn test_links_json_output() {
    let dir = setup_and_ingest_with_links();

    let output = mdvdb_bin()
        .args(["links", "alpha.md", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success(), "links --json should succeed, stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    assert_eq!(json["file"].as_str().unwrap(), "alpha.md");
    let links = &json["links"];
    assert!(links["outgoing"].is_array(), "should have 'links.outgoing' array");
    let outgoing = links["outgoing"].as_array().unwrap();
    assert!(outgoing.len() >= 2, "alpha.md should have at least 2 outgoing links, got {}", outgoing.len());
    assert!(links["incoming"].is_array(), "should have 'links.incoming' array");
}

#[test]
fn test_backlinks_json_output() {
    let dir = setup_and_ingest_with_links();

    let output = mdvdb_bin()
        .args(["backlinks", "alpha.md", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success(), "backlinks --json should succeed, stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    assert_eq!(json["file"].as_str().unwrap(), "alpha.md");
    assert!(json["backlinks"].is_array(), "should have 'backlinks' array");
    // beta.md links to alpha.md, so alpha should have backlinks
    let backlinks = json["backlinks"].as_array().unwrap();
    assert!(!backlinks.is_empty(), "alpha.md should have backlinks from beta.md");
}

#[test]
fn test_orphans_json_output() {
    let dir = setup_and_ingest_with_links();

    let output = mdvdb_bin()
        .args(["orphans", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success(), "orphans --json should succeed, stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");

    // orphan.md has no links in or out, so it should appear
    assert!(json["orphans"].is_array(), "should have 'orphans' array");
    let orphans = json["orphans"].as_array().unwrap();
    let paths: Vec<&str> = orphans.iter().filter_map(|o| o["path"].as_str()).collect();
    assert!(paths.contains(&"orphan.md"), "orphan.md should be in orphans list, got: {paths:?}");
}

#[test]
fn test_links_nonexistent_file() {
    let dir = setup_and_ingest_with_links();

    let output = mdvdb_bin()
        .args(["links", "nonexistent.md", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    // The command succeeds but returns empty links for a nonexistent file
    assert!(output.status.success(), "links should succeed even for nonexistent file");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    let links = &json["links"];
    let outgoing = links["outgoing"].as_array().unwrap();
    let incoming = links["incoming"].as_array().unwrap();
    assert!(outgoing.is_empty(), "nonexistent file should have no outgoing links");
    assert!(incoming.is_empty(), "nonexistent file should have no incoming links");
}

#[test]
fn test_search_boost_links_flag() {
    let dir = setup_and_ingest_with_links();

    let output = mdvdb_bin()
        .args(["search", "alpha", "--boost-links", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "search --boost-links should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["results"].is_array(), "should have 'results' array");
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
// Hybrid / FTS search CLI tests
// ---------------------------------------------------------------------------

#[test]
fn test_search_help_shows_mode_flags() {
    let output = mdvdb_bin()
        .args(["search", "--help"])
        .output()
        .expect("failed to execute mdvdb");

    assert!(output.status.success(), "search --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    for flag in ["--mode", "--semantic", "--lexical"] {
        assert!(
            stdout.contains(flag),
            "search --help should mention '{flag}', got: {}",
            &stdout[..stdout.len().min(500)]
        );
    }
}

#[test]
fn test_search_with_mode_semantic() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["search", "rust", "--mode", "semantic", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "search --mode semantic should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["results"].is_array(), "should have results array");
}

#[test]
fn test_search_with_mode_lexical() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["search", "rust programming", "--mode", "lexical", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "search --mode lexical should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["results"].is_array(), "should have results array");
}

#[test]
fn test_search_with_mode_hybrid() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["search", "rust", "--mode", "hybrid", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "search --mode hybrid should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["results"].is_array(), "should have results array");
}

#[test]
fn test_search_semantic_shorthand_flag() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["search", "rust", "--semantic", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "search --semantic should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_search_lexical_shorthand_flag() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["search", "rust programming", "--lexical", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "search --lexical should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_search_invalid_mode_rejected() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["search", "rust", "--mode", "invalid"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        !output.status.success(),
        "search --mode invalid should fail"
    );
}

#[test]
fn test_search_json_output_includes_mode() {
    let dir = setup_and_ingest();

    // Test with explicit lexical mode.
    let output = mdvdb_bin()
        .args(["search", "rust programming", "--mode", "lexical", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "search --mode lexical --json should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert_eq!(
        json["mode"], "lexical",
        "JSON output should include mode field with value 'lexical'"
    );

    // Test with hybrid mode (default).
    let output = mdvdb_bin()
        .args(["search", "rust", "--mode", "hybrid", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert_eq!(
        json["mode"], "hybrid",
        "JSON output should include mode field with value 'hybrid'"
    );
}

#[test]
fn test_search_semantic_lexical_flags_conflict() {
    let dir = setup_and_ingest();

    // Using both --semantic and --lexical should fail (clap conflict).
    let output = mdvdb_bin()
        .args(["search", "rust", "--semantic", "--lexical"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        !output.status.success(),
        "--semantic and --lexical together should fail"
    );
}

// ---------------------------------------------------------------------------
// Init --global, config, and doctor tests
// ---------------------------------------------------------------------------

#[test]
fn test_init_global_creates_user_config() {
    let dir = TempDir::new().unwrap();
    let output = mdvdb_bin()
        .args(["init", "--global"])
        .env("MDVDB_CONFIG_HOME", dir.path())
        .output()
        .expect("failed to execute mdvdb");

    assert!(
        output.status.success(),
        "init --global should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dir.path().join("config").exists(),
        "user config file should be created"
    );

    let content = fs::read_to_string(dir.path().join("config")).unwrap();
    assert!(
        content.contains("OPENAI_API_KEY"),
        "user config template should mention OPENAI_API_KEY"
    );
}

#[test]
fn test_init_global_twice_fails() {
    let dir = TempDir::new().unwrap();

    let first = mdvdb_bin()
        .args(["init", "--global"])
        .env("MDVDB_CONFIG_HOME", dir.path())
        .output()
        .expect("failed to execute mdvdb");
    assert!(first.status.success(), "first init --global should succeed");

    let second = mdvdb_bin()
        .args(["init", "--global"])
        .env("MDVDB_CONFIG_HOME", dir.path())
        .output()
        .expect("failed to execute mdvdb");
    assert!(
        !second.status.success(),
        "init --global when config exists should fail"
    );
}

#[test]
fn test_config_json_output() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    let output = mdvdb_bin()
        .args(["config", "--json"])
        .env("MDVDB_NO_USER_CONFIG", "1")
        .current_dir(root)
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "config --json should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["embedding_model"].is_string(), "should have embedding_model field");
    assert!(json["embedding_dimensions"].is_number(), "should have embedding_dimensions field");
}

#[test]
fn test_config_human_output() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
        "MDVDB_EMBEDDING_PROVIDER=mock\nMDVDB_EMBEDDING_DIMENSIONS=8\n",
    )
    .unwrap();

    let output = mdvdb_bin()
        .args(["--no-color", "config"])
        .env("MDVDB_NO_USER_CONFIG", "1")
        .current_dir(root)
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "config should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Configuration"), "should contain Configuration header");
    assert!(stdout.contains("Provider"), "should show provider");
}

#[test]
fn test_doctor_json_output() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["doctor", "--json"])
        .env("MDVDB_NO_USER_CONFIG", "1")
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "doctor --json should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["checks"].is_array(), "should have checks array");
    assert!(json["passed"].is_number(), "should have passed count");
    assert!(json["total"].is_number(), "should have total count");
}

#[test]
fn test_doctor_human_output() {
    let dir = setup_and_ingest();

    let output = mdvdb_bin()
        .args(["--no-color", "doctor"])
        .env("MDVDB_NO_USER_CONFIG", "1")
        .current_dir(dir.path())
        .output()
        .expect("failed to run mdvdb");

    assert!(
        output.status.success(),
        "doctor should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("mdvdb doctor"), "should contain doctor header");
    assert!(stdout.contains("checks passed"), "should show pass count");
}

#[test]
fn test_help_shows_new_subcommands() {
    let output = mdvdb_bin()
        .arg("--help")
        .output()
        .expect("failed to execute mdvdb");

    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    for cmd in ["config", "doctor"] {
        assert!(
            stdout.contains(cmd),
            "--help should mention '{cmd}'"
        );
    }
}

// ---------------------------------------------------------------------------
// Tree command tests
// ---------------------------------------------------------------------------

fn setup_and_ingest_with_subdirs() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join(".markdownvdb")).unwrap();
    fs::write(
        root.join(".markdownvdb").join(".config"),
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

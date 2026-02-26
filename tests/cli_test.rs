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

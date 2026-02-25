use std::fs;
use std::path::PathBuf;

use mdvdb::config::Config;
use mdvdb::discovery::FileDiscovery;
use serial_test::serial;
use tempfile::TempDir;

/// All MDVDB env vars that could affect config loading.
const ALL_ENV_VARS: &[&str] = &[
    "MDVDB_EMBEDDING_PROVIDER",
    "MDVDB_EMBEDDING_MODEL",
    "MDVDB_EMBEDDING_DIMENSIONS",
    "MDVDB_EMBEDDING_BATCH_SIZE",
    "OPENAI_API_KEY",
    "OLLAMA_HOST",
    "MDVDB_EMBEDDING_ENDPOINT",
    "MDVDB_SOURCE_DIRS",
    "MDVDB_INDEX_FILE",
    "MDVDB_IGNORE_PATTERNS",
    "MDVDB_WATCH",
    "MDVDB_WATCH_DEBOUNCE_MS",
    "MDVDB_CHUNK_MAX_TOKENS",
    "MDVDB_CHUNK_OVERLAP_TOKENS",
    "MDVDB_CLUSTERING_ENABLED",
    "MDVDB_CLUSTERING_REBALANCE_THRESHOLD",
    "MDVDB_SEARCH_DEFAULT_LIMIT",
    "MDVDB_SEARCH_MIN_SCORE",
];

fn clear_env() {
    for var in ALL_ENV_VARS {
        std::env::remove_var(var);
    }
}

/// Helper: create a file with parent dirs.
fn create_file(base: &std::path::Path, rel: &str, content: &str) {
    let path = base.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, content).unwrap();
}

#[test]
#[serial]
fn discover_only_md_files() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    create_file(tmp.path(), "readme.md", "# Hello");
    create_file(tmp.path(), "notes.txt", "not markdown");
    create_file(tmp.path(), "code.rs", "fn main() {}");
    create_file(tmp.path(), "sub/doc.md", "# Sub doc");
    create_file(tmp.path(), "sub/data.json", "{}");

    let config = Config::load(tmp.path()).unwrap();
    let discovery = FileDiscovery::new(tmp.path(), &config);
    let files = discovery.discover().unwrap();

    assert_eq!(files.len(), 2);
    assert!(files.contains(&PathBuf::from("readme.md")));
    assert!(files.contains(&PathBuf::from("sub/doc.md")));
}

#[test]
#[serial]
fn discover_builtin_ignores() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    create_file(tmp.path(), "visible.md", "# Visible");
    create_file(tmp.path(), ".git/HEAD.md", "ref");
    create_file(tmp.path(), "node_modules/pkg/readme.md", "# Pkg");
    create_file(tmp.path(), "target/doc/index.md", "# Target");
    create_file(tmp.path(), ".vscode/notes.md", "# VSCode");
    create_file(tmp.path(), "__pycache__/cached.md", "# Cache");

    let config = Config::load(tmp.path()).unwrap();
    let discovery = FileDiscovery::new(tmp.path(), &config);
    let files = discovery.discover().unwrap();

    assert_eq!(files, vec![PathBuf::from("visible.md")]);
}

#[test]
#[serial]
fn discover_user_ignores() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    create_file(tmp.path(), "keep.md", "# Keep");
    create_file(tmp.path(), "drafts/wip.md", "# WIP");
    create_file(tmp.path(), "archive/old.md", "# Old");

    std::env::set_var("MDVDB_IGNORE_PATTERNS", "drafts/,archive/");

    let config = Config::load(tmp.path()).unwrap();
    let discovery = FileDiscovery::new(tmp.path(), &config);
    let files = discovery.discover().unwrap();

    assert_eq!(files, vec![PathBuf::from("keep.md")]);
    clear_env();
}

#[test]
#[serial]
fn discover_gitignore() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    // The ignore crate needs a .git dir to activate .gitignore processing.
    fs::create_dir(tmp.path().join(".git")).unwrap();
    create_file(tmp.path(), ".gitignore", "ignored/\n");
    create_file(tmp.path(), "visible.md", "# Visible");
    create_file(tmp.path(), "ignored/secret.md", "# Secret");

    let config = Config::load(tmp.path()).unwrap();
    let discovery = FileDiscovery::new(tmp.path(), &config);
    let files = discovery.discover().unwrap();

    assert_eq!(files, vec![PathBuf::from("visible.md")]);
}

#[test]
#[serial]
fn discover_multiple_source_dirs() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    create_file(tmp.path(), "docs/guide.md", "# Guide");
    create_file(tmp.path(), "notes/daily.md", "# Daily");
    create_file(tmp.path(), "other/skip.md", "# Skip");

    std::env::set_var("MDVDB_SOURCE_DIRS", "docs,notes");

    let config = Config::load(tmp.path()).unwrap();
    let discovery = FileDiscovery::new(tmp.path(), &config);
    let files = discovery.discover().unwrap();

    assert_eq!(files.len(), 2);
    assert!(files.contains(&PathBuf::from("docs/guide.md")));
    assert!(files.contains(&PathBuf::from("notes/daily.md")));
    clear_env();
}

#[test]
#[serial]
fn discover_relative_paths() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    create_file(tmp.path(), "a.md", "# A");
    create_file(tmp.path(), "sub/b.md", "# B");

    let config = Config::load(tmp.path()).unwrap();
    let discovery = FileDiscovery::new(tmp.path(), &config);
    let files = discovery.discover().unwrap();

    for path in &files {
        assert!(path.is_relative(), "Path should be relative: {path:?}");
        assert!(
            !path.to_string_lossy().starts_with('/'),
            "Path should not start with /: {path:?}"
        );
    }
}

#[test]
#[serial]
fn discover_empty_dir() {
    clear_env();
    let tmp = TempDir::new().unwrap();

    let config = Config::load(tmp.path()).unwrap();
    let discovery = FileDiscovery::new(tmp.path(), &config);
    let files = discovery.discover().unwrap();

    assert!(files.is_empty());
}

#[test]
#[serial]
fn discover_sorted_output() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    create_file(tmp.path(), "zebra.md", "# Z");
    create_file(tmp.path(), "alpha.md", "# A");
    create_file(tmp.path(), "middle.md", "# M");
    create_file(tmp.path(), "sub/beta.md", "# B");

    let config = Config::load(tmp.path()).unwrap();
    let discovery = FileDiscovery::new(tmp.path(), &config);
    let files = discovery.discover().unwrap();

    let mut sorted = files.clone();
    sorted.sort();
    assert_eq!(files, sorted, "Output should be sorted");
}

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;

use crate::config::Config;
use crate::discovery::FileDiscovery;
use crate::error::Error;
use crate::index::Index;
use crate::parser::compute_content_hash;

/// Sync state of a file relative to the index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileState {
    Indexed,
    Modified,
    New,
    Deleted,
}

/// A node in the file tree (either a directory or a file).
#[derive(Debug, Clone, Serialize)]
pub struct FileTreeNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub state: Option<FileState>,
    pub children: Vec<FileTreeNode>,
}

/// Complete file tree with summary counts.
#[derive(Debug, Clone, Serialize)]
pub struct FileTree {
    pub root: FileTreeNode,
    pub total_files: usize,
    pub indexed_count: usize,
    pub modified_count: usize,
    pub new_count: usize,
    pub deleted_count: usize,
}

/// Build a file tree by comparing discovered files on disk against the index.
///
/// Classifies each file as Indexed (hash match), Modified (hash mismatch),
/// New (on disk but not in index), or Deleted (in index but not on disk).
pub fn build_file_tree(root: &Path, config: &Config, index: &Index) -> Result<FileTree, Error> {
    let discovery = FileDiscovery::new(root, config);
    let disk_files = discovery.discover()?;
    let indexed_hashes: HashMap<String, String> = index.get_file_hashes();

    let disk_paths: HashSet<String> = disk_files
        .iter()
        .filter_map(|p| p.to_str().map(|s| s.to_string()))
        .collect();

    let mut entries: Vec<(String, FileState)> = Vec::new();
    let mut indexed_count = 0usize;
    let mut modified_count = 0usize;
    let mut new_count = 0usize;
    let mut deleted_count = 0usize;

    // Classify disk files
    for rel_path in &disk_paths {
        if let Some(expected_hash) = indexed_hashes.get(rel_path) {
            // File exists in index — compare hashes
            let full_path = root.join(rel_path);
            let content = std::fs::read_to_string(&full_path).map_err(|e| {
                Error::Io(std::io::Error::new(e.kind(), format!("{}: {}", rel_path, e)))
            })?;
            let disk_hash = compute_content_hash(&content);
            if disk_hash == *expected_hash {
                entries.push((rel_path.clone(), FileState::Indexed));
                indexed_count += 1;
            } else {
                entries.push((rel_path.clone(), FileState::Modified));
                modified_count += 1;
            }
        } else {
            entries.push((rel_path.clone(), FileState::New));
            new_count += 1;
        }
    }

    // Find deleted files (in index but not on disk)
    for indexed_path in indexed_hashes.keys() {
        if !disk_paths.contains(indexed_path) {
            entries.push((indexed_path.clone(), FileState::Deleted));
            deleted_count += 1;
        }
    }

    let total_files = entries.len();
    let root_node = build_tree_from_entries(&entries);

    Ok(FileTree {
        root: root_node,
        total_files,
        indexed_count,
        modified_count,
        new_count,
        deleted_count,
    })
}

/// Build a hierarchical tree from a flat list of (path, state) entries.
///
/// Creates intermediate directory nodes as needed. Children are sorted
/// with directories first (alphabetical), then files (alphabetical).
pub fn build_tree_from_entries(entries: &[(String, FileState)]) -> FileTreeNode {
    let mut root = FileTreeNode {
        name: ".".to_string(),
        path: ".".to_string(),
        is_dir: true,
        state: None,
        children: Vec::new(),
    };

    for (path, state) in entries {
        let parts: Vec<&str> = path.split('/').collect();
        let mut current = &mut root;

        for (i, part) in parts.iter().enumerate() {
            let is_last = i == parts.len() - 1;

            if is_last {
                // Insert leaf file node
                current.children.push(FileTreeNode {
                    name: part.to_string(),
                    path: path.clone(),
                    is_dir: false,
                    state: Some(state.clone()),
                    children: Vec::new(),
                });
            } else {
                // Find or create intermediate directory
                let dir_path = parts[..=i].join("/");
                let pos = current
                    .children
                    .iter()
                    .position(|c| c.is_dir && c.name == *part);

                if let Some(pos) = pos {
                    current = &mut current.children[pos];
                } else {
                    current.children.push(FileTreeNode {
                        name: part.to_string(),
                        path: dir_path,
                        is_dir: true,
                        state: None,
                        children: Vec::new(),
                    });
                    let last = current.children.len() - 1;
                    current = &mut current.children[last];
                }
            }
        }
    }

    sort_children(&mut root);
    root
}

/// Recursively sort children: directories first (alphabetical), then files (alphabetical).
fn sort_children(node: &mut FileTreeNode) {
    node.children.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        }
    });
    for child in &mut node.children {
        if child.is_dir {
            sort_children(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::path::PathBuf;
    use crate::config::EmbeddingProviderType;
    use crate::index::Index;
    use crate::index::types::EmbeddingConfig;

    const DIMS: usize = 8;

    fn test_embedding_config() -> EmbeddingConfig {
        EmbeddingConfig {
            provider: "mock".to_string(),
            model: "test".to_string(),
            dimensions: DIMS,
        }
    }

    fn mock_config() -> Config {
        Config {
            embedding_provider: EmbeddingProviderType::Mock,
            embedding_model: "mock-model".into(),
            embedding_dimensions: DIMS,
            embedding_batch_size: 100,
            openai_api_key: None,
            ollama_host: "http://localhost:11434".into(),
            embedding_endpoint: None,
            source_dirs: vec![PathBuf::from(".")],
            index_file: PathBuf::from(".markdownvdb.index"),
            ignore_patterns: vec![],
            watch_enabled: false,
            watch_debounce_ms: 300,
            chunk_max_tokens: 512,
            chunk_overlap_tokens: 50,
            clustering_enabled: false,
            clustering_rebalance_threshold: 50,
            search_default_limit: 10,
            search_min_score: 0.0,
        }
    }

    #[test]
    fn test_build_file_tree_summary_counts() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Create markdown files on disk
        std::fs::write(root.join("a.md"), "# Alpha\nContent A").unwrap();
        std::fs::write(root.join("b.md"), "# Beta\nContent B").unwrap();
        std::fs::write(root.join("c.md"), "# Gamma\nContent C").unwrap();

        let config = mock_config();
        let idx_path = root.join(".markdownvdb.index");
        let index = Index::create(&idx_path, &test_embedding_config()).unwrap();

        // "a.md" with matching hash → Indexed
        let content_a = std::fs::read_to_string(root.join("a.md")).unwrap();
        let hash_a = compute_content_hash(&content_a);
        index.insert_file_hash_for_test("a.md", &hash_a);

        // "d.md" not on disk → Deleted
        index.insert_file_hash_for_test("d.md", "oldhash");

        // b.md and c.md not in index → New
        let tree = build_file_tree(root, &config, &index).unwrap();

        assert_eq!(tree.total_files, 4);
        assert_eq!(tree.indexed_count, 1);
        assert_eq!(tree.new_count, 2);
        assert_eq!(tree.deleted_count, 1);
        assert_eq!(tree.modified_count, 0);
    }

    #[test]
    fn test_build_file_tree_modified_detection() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        std::fs::write(root.join("doc.md"), "# Updated\nNew content").unwrap();

        let config = mock_config();
        let idx_path = root.join(".markdownvdb.index");
        let index = Index::create(&idx_path, &test_embedding_config()).unwrap();

        // Index with stale hash → Modified
        index.insert_file_hash_for_test("doc.md", "stale_hash_value");

        let tree = build_file_tree(root, &config, &index).unwrap();

        assert_eq!(tree.total_files, 1);
        assert_eq!(tree.modified_count, 1);
        assert_eq!(tree.indexed_count, 0);
    }

    #[test]
    fn test_build_tree_empty() {
        let entries: Vec<(String, FileState)> = vec![];
        let root = build_tree_from_entries(&entries);
        assert_eq!(root.name, ".");
        assert!(root.is_dir);
        assert!(root.children.is_empty());
        assert!(root.state.is_none());
    }

    #[test]
    fn test_build_tree_single_file() {
        let entries = vec![("readme.md".to_string(), FileState::Indexed)];
        let root = build_tree_from_entries(&entries);
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].name, "readme.md");
        assert!(!root.children[0].is_dir);
        assert_eq!(root.children[0].state, Some(FileState::Indexed));
        assert_eq!(root.children[0].path, "readme.md");
    }

    #[test]
    fn test_build_tree_nested() {
        let entries = vec![
            ("docs/api/auth.md".to_string(), FileState::Indexed),
            ("docs/guide.md".to_string(), FileState::New),
        ];
        let root = build_tree_from_entries(&entries);

        // root -> docs (dir)
        assert_eq!(root.children.len(), 1);
        let docs = &root.children[0];
        assert_eq!(docs.name, "docs");
        assert!(docs.is_dir);
        assert_eq!(docs.path, "docs");

        // docs -> api (dir), guide.md (file) — dirs first
        assert_eq!(docs.children.len(), 2);
        let api = &docs.children[0];
        assert!(api.is_dir);
        assert_eq!(api.name, "api");

        let guide = &docs.children[1];
        assert!(!guide.is_dir);
        assert_eq!(guide.name, "guide.md");
        assert_eq!(guide.state, Some(FileState::New));

        // api -> auth.md
        assert_eq!(api.children.len(), 1);
        assert_eq!(api.children[0].name, "auth.md");
        assert_eq!(api.children[0].state, Some(FileState::Indexed));
    }

    #[test]
    fn test_build_tree_sorting() {
        let entries = vec![
            ("zebra.md".to_string(), FileState::Indexed),
            ("alpha.md".to_string(), FileState::Indexed),
            ("docs/b.md".to_string(), FileState::New),
            ("notes/a.md".to_string(), FileState::Modified),
            ("beta.md".to_string(), FileState::Deleted),
        ];
        let root = build_tree_from_entries(&entries);

        // Dirs first (docs, notes), then files (alpha, beta, zebra)
        assert_eq!(root.children.len(), 5);
        assert!(root.children[0].is_dir);
        assert_eq!(root.children[0].name, "docs");
        assert!(root.children[1].is_dir);
        assert_eq!(root.children[1].name, "notes");
        assert!(!root.children[2].is_dir);
        assert_eq!(root.children[2].name, "alpha.md");
        assert_eq!(root.children[3].name, "beta.md");
        assert_eq!(root.children[4].name, "zebra.md");
    }

    #[test]
    fn test_file_state_serialization() {
        let json = serde_json::to_string(&FileState::Indexed).unwrap();
        assert_eq!(json, "\"indexed\"");

        let json = serde_json::to_string(&FileState::Modified).unwrap();
        assert_eq!(json, "\"modified\"");

        let json = serde_json::to_string(&FileState::New).unwrap();
        assert_eq!(json, "\"new\"");

        let json = serde_json::to_string(&FileState::Deleted).unwrap();
        assert_eq!(json, "\"deleted\"");
    }
}

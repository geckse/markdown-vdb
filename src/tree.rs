use serde::Serialize;

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

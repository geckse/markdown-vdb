use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tracing::debug;

use crate::parser::MarkdownFile;

/// A single link extracted from a markdown file.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct LinkEntry {
    /// Source file (relative path).
    pub source: String,
    /// Target file (resolved relative path).
    pub target: String,
    /// Display text of the link.
    pub text: String,
    /// Line number in source file (1-based).
    pub line_number: usize,
    /// Whether this was a [[wikilink]].
    pub is_wikilink: bool,
}

/// The complete link graph stored in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct LinkGraph {
    /// Forward links: source path → list of link entries.
    pub forward: HashMap<String, Vec<LinkEntry>>,
    /// Unix timestamp of last update.
    pub last_updated: u64,
}

/// A resolved link with validity status.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedLink {
    /// The link entry.
    pub entry: LinkEntry,
    /// Whether the target exists.
    pub state: LinkState,
}

/// Whether a link target is valid or broken.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum LinkState {
    /// Target file exists in the file set.
    Valid,
    /// Target file not found.
    Broken,
}

/// Result of querying links for a specific file.
#[derive(Debug, Clone, Serialize)]
pub struct LinkQueryResult {
    /// The queried file path.
    pub file: String,
    /// Outgoing links from this file.
    pub outgoing: Vec<ResolvedLink>,
    /// Incoming links (backlinks) to this file.
    pub incoming: Vec<LinkEntry>,
}

/// A file with no incoming or outgoing links.
#[derive(Debug, Clone, Serialize)]
pub struct OrphanFile {
    /// Relative path to the orphan file.
    pub path: String,
}

/// Resolve a raw link target relative to the source file's directory.
///
/// Normalizes path components (`.`, `..`, separators) and ensures `.md` extension.
pub fn resolve_link(source: &str, target: &str) -> String {
    let target = target.trim();

    // Strip any fragment (#section)
    let target = target.split('#').next().unwrap_or(target);
    if target.is_empty() {
        return String::new();
    }

    // Normalize separators
    let target = target.replace('\\', "/");

    // Determine the source directory
    let source_dir = Path::new(source).parent().unwrap_or(Path::new(""));

    // Join with source directory
    let joined = source_dir.join(&target);

    // Normalize path components
    let normalized = normalize_path(&joined);

    // Ensure .md extension
    let result = normalized.to_string_lossy().replace('\\', "/");
    if result.ends_with(".md") {
        result
    } else {
        format!("{}.md", result)
    }
}

/// Normalize a path by resolving `.` and `..` components without filesystem access.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {} // skip .
            Component::ParentDir => {
                components.pop(); // go up
            }
            other => components.push(other),
        }
    }
    components.iter().collect()
}

/// Build a link graph from parsed markdown files.
///
/// Deduplicates same-target links within a file and excludes self-links.
pub fn build_link_graph(files: &[MarkdownFile]) -> LinkGraph {
    let mut forward: HashMap<String, Vec<LinkEntry>> = HashMap::new();

    for file in files {
        let source = file.path.to_string_lossy().replace('\\', "/");
        let mut seen_targets: HashSet<String> = HashSet::new();
        let mut entries = Vec::new();

        for raw_link in &file.links {
            let resolved = resolve_link(&source, &raw_link.target);
            if resolved.is_empty() {
                continue;
            }

            // Skip self-links
            if resolved == source {
                continue;
            }

            // Deduplicate by target
            if !seen_targets.insert(resolved.clone()) {
                continue;
            }

            entries.push(LinkEntry {
                source: source.clone(),
                target: resolved,
                text: raw_link.text.clone(),
                line_number: raw_link.line_number,
                is_wikilink: raw_link.is_wikilink,
            });
        }

        if !entries.is_empty() {
            forward.insert(source, entries);
        }
    }

    let last_updated = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    debug!(
        files = forward.len(),
        "built link graph"
    );

    LinkGraph {
        forward,
        last_updated,
    }
}

/// Compute backlinks (inverted index) from the forward link graph.
///
/// Returns a map from target path → list of LinkEntry pointing to it.
pub fn compute_backlinks(graph: &LinkGraph) -> HashMap<String, Vec<LinkEntry>> {
    let mut backlinks: HashMap<String, Vec<LinkEntry>> = HashMap::new();

    for entries in graph.forward.values() {
        for entry in entries {
            backlinks
                .entry(entry.target.clone())
                .or_default()
                .push(entry.clone());
        }
    }

    backlinks
}

/// Query links for a specific file, classifying outgoing links as valid or broken.
///
/// `known_files` is the set of all known file paths (relative).
pub fn query_links(
    file: &str,
    graph: &LinkGraph,
    backlinks: &HashMap<String, Vec<LinkEntry>>,
    known_files: &HashSet<String>,
) -> LinkQueryResult {
    let outgoing = graph
        .forward
        .get(file)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| {
            let state = if known_files.contains(&entry.target) {
                LinkState::Valid
            } else {
                LinkState::Broken
            };
            ResolvedLink { entry, state }
        })
        .collect();

    let incoming = backlinks.get(file).cloned().unwrap_or_default();

    LinkQueryResult {
        file: file.to_string(),
        outgoing,
        incoming,
    }
}

/// Find orphan files — files with no incoming or outgoing links.
pub fn find_orphans(
    graph: &LinkGraph,
    all_files: &HashSet<String>,
) -> Vec<OrphanFile> {
    let backlinks = compute_backlinks(graph);

    let mut orphans: Vec<OrphanFile> = all_files
        .iter()
        .filter(|file| {
            let has_outgoing = graph.forward.contains_key(file.as_str());
            let has_incoming = backlinks.contains_key(file.as_str());
            !has_outgoing && !has_incoming
        })
        .map(|path| OrphanFile {
            path: path.clone(),
        })
        .collect();

    orphans.sort_by(|a, b| a.path.cmp(&b.path));
    orphans
}

/// Update link entries for a single file (incremental update).
///
/// Replaces the forward links for the given file in the graph.
pub fn update_file_links(graph: &mut LinkGraph, file: &MarkdownFile) {
    let source = file.path.to_string_lossy().replace('\\', "/");

    // Remove old entries
    graph.forward.remove(&source);

    // Build new entries
    let mut seen_targets: HashSet<String> = HashSet::new();
    let mut entries = Vec::new();

    for raw_link in &file.links {
        let resolved = resolve_link(&source, &raw_link.target);
        if resolved.is_empty() {
            continue;
        }
        if resolved == source {
            continue;
        }
        if !seen_targets.insert(resolved.clone()) {
            continue;
        }
        entries.push(LinkEntry {
            source: source.clone(),
            target: resolved,
            text: raw_link.text.clone(),
            line_number: raw_link.line_number,
            is_wikilink: raw_link.is_wikilink,
        });
    }

    if !entries.is_empty() {
        graph.forward.insert(source, entries);
    }

    graph.last_updated = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
}

/// Remove all link entries for a file (when file is deleted).
pub fn remove_file_links(graph: &mut LinkGraph, file_path: &str) {
    graph.forward.remove(file_path);

    graph.last_updated = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::RawLink;
    use std::path::PathBuf;

    fn make_file(path: &str, links: Vec<RawLink>) -> MarkdownFile {
        MarkdownFile {
            path: PathBuf::from(path),
            frontmatter: None,
            headings: Vec::new(),
            body: String::new(),
            content_hash: String::new(),
            file_size: 0,
            links,
        }
    }

    fn make_link(target: &str, text: &str, line: usize, wikilink: bool) -> RawLink {
        RawLink {
            target: target.to_string(),
            text: text.to_string(),
            line_number: line,
            is_wikilink: wikilink,
        }
    }

    // --- resolve_link tests ---

    #[test]
    fn resolve_link_simple_relative() {
        assert_eq!(resolve_link("docs/readme.md", "other"), "docs/other.md");
    }

    #[test]
    fn resolve_link_with_md_extension() {
        assert_eq!(resolve_link("docs/readme.md", "other.md"), "docs/other.md");
    }

    #[test]
    fn resolve_link_with_fragment() {
        assert_eq!(
            resolve_link("docs/readme.md", "other.md#section"),
            "docs/other.md"
        );
    }

    #[test]
    fn resolve_link_parent_dir() {
        assert_eq!(resolve_link("docs/sub/readme.md", "../other"), "docs/other.md");
    }

    #[test]
    fn resolve_link_current_dir() {
        assert_eq!(resolve_link("docs/readme.md", "./other"), "docs/other.md");
    }

    #[test]
    fn resolve_link_root_level() {
        assert_eq!(resolve_link("readme.md", "other"), "other.md");
    }

    #[test]
    fn resolve_link_empty_fragment_only() {
        assert_eq!(resolve_link("readme.md", "#section"), "");
    }

    #[test]
    fn resolve_link_backslash_normalization() {
        assert_eq!(resolve_link("docs/readme.md", "sub\\other"), "docs/sub/other.md");
    }

    // --- build_link_graph tests ---

    #[test]
    fn build_graph_basic() {
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("b.md", vec![make_link("a", "A", 1, true)]),
        ];
        let graph = build_link_graph(&files);

        assert_eq!(graph.forward.len(), 2);
        assert_eq!(graph.forward["a.md"].len(), 1);
        assert_eq!(graph.forward["a.md"][0].target, "b.md");
        assert_eq!(graph.forward["b.md"][0].target, "a.md");
    }

    #[test]
    fn build_graph_deduplicates() {
        let files = vec![make_file(
            "a.md",
            vec![
                make_link("b", "B1", 1, false),
                make_link("b", "B2", 5, false),
            ],
        )];
        let graph = build_link_graph(&files);
        assert_eq!(graph.forward["a.md"].len(), 1);
    }

    #[test]
    fn build_graph_excludes_self_links() {
        let files = vec![make_file("a.md", vec![make_link("a", "self", 1, false)])];
        let graph = build_link_graph(&files);
        assert!(graph.forward.is_empty());
    }

    #[test]
    fn build_graph_empty() {
        let graph = build_link_graph(&[]);
        assert!(graph.forward.is_empty());
    }

    // --- compute_backlinks tests ---

    #[test]
    fn compute_backlinks_basic() {
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("c.md", vec![make_link("b", "B", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let backlinks = compute_backlinks(&graph);

        assert_eq!(backlinks["b.md"].len(), 2);
        let sources: HashSet<_> = backlinks["b.md"].iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains("a.md"));
        assert!(sources.contains("c.md"));
    }

    // --- query_links tests ---

    #[test]
    fn query_links_classifies_valid_and_broken() {
        let files = vec![make_file(
            "a.md",
            vec![
                make_link("b", "B", 1, false),
                make_link("missing", "M", 2, false),
            ],
        )];
        let graph = build_link_graph(&files);
        let backlinks = compute_backlinks(&graph);
        let known: HashSet<String> = ["a.md", "b.md"].iter().map(|s| s.to_string()).collect();

        let result = query_links("a.md", &graph, &backlinks, &known);

        assert_eq!(result.outgoing.len(), 2);
        let valid: Vec<_> = result.outgoing.iter().filter(|r| r.state == LinkState::Valid).collect();
        let broken: Vec<_> = result.outgoing.iter().filter(|r| r.state == LinkState::Broken).collect();
        assert_eq!(valid.len(), 1);
        assert_eq!(valid[0].entry.target, "b.md");
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].entry.target, "missing.md");
    }

    #[test]
    fn query_links_includes_backlinks() {
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("c.md", vec![make_link("b", "B", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let backlinks = compute_backlinks(&graph);
        let known: HashSet<String> = ["a.md", "b.md", "c.md"].iter().map(|s| s.to_string()).collect();

        let result = query_links("b.md", &graph, &backlinks, &known);
        assert_eq!(result.incoming.len(), 2);
    }

    // --- find_orphans tests ---

    #[test]
    fn find_orphans_identifies_disconnected_files() {
        let files = vec![make_file("a.md", vec![make_link("b", "B", 1, false)])];
        let graph = build_link_graph(&files);
        let all: HashSet<String> = ["a.md", "b.md", "orphan.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let orphans = find_orphans(&graph, &all);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].path, "orphan.md");
    }

    #[test]
    fn find_orphans_no_orphans() {
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("b.md", vec![make_link("a", "A", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let all: HashSet<String> = ["a.md", "b.md"].iter().map(|s| s.to_string()).collect();

        let orphans = find_orphans(&graph, &all);
        assert!(orphans.is_empty());
    }

    // --- update_file_links tests ---

    #[test]
    fn update_file_links_replaces_entries() {
        let files = vec![make_file("a.md", vec![make_link("b", "B", 1, false)])];
        let mut graph = build_link_graph(&files);
        assert_eq!(graph.forward["a.md"][0].target, "b.md");

        let updated = make_file("a.md", vec![make_link("c", "C", 1, false)]);
        update_file_links(&mut graph, &updated);

        assert_eq!(graph.forward["a.md"][0].target, "c.md");
    }

    #[test]
    fn update_file_links_removes_when_no_links() {
        let files = vec![make_file("a.md", vec![make_link("b", "B", 1, false)])];
        let mut graph = build_link_graph(&files);

        let updated = make_file("a.md", vec![]);
        update_file_links(&mut graph, &updated);

        assert!(!graph.forward.contains_key("a.md"));
    }

    // --- remove_file_links tests ---

    #[test]
    fn remove_file_links_removes_entries() {
        let files = vec![make_file("a.md", vec![make_link("b", "B", 1, false)])];
        let mut graph = build_link_graph(&files);
        assert!(graph.forward.contains_key("a.md"));

        remove_file_links(&mut graph, "a.md");
        assert!(!graph.forward.contains_key("a.md"));
    }
}

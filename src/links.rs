use std::collections::{HashMap, HashSet, VecDeque};
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

/// A semantic edge representing a link with its surrounding paragraph context.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize, serde::Deserialize)]
#[rkyv(derive(Debug))]
pub struct SemanticEdge {
    /// Unique edge identifier in format `"edge:source.md->target.md@42"`.
    pub edge_id: String,
    /// Source file (relative path).
    pub source: String,
    /// Target file (resolved relative path).
    pub target: String,
    /// Paragraph context surrounding the link.
    pub context_text: String,
    /// Line number of the link in the source file (1-based).
    pub line_number: usize,
    /// Cosine similarity between edge embedding and target document embedding.
    pub strength: Option<f64>,
    /// Auto-discovered relationship type label from edge clustering.
    pub relationship_type: Option<String>,
    /// Cluster ID this edge belongs to (if clustered).
    pub cluster_id: Option<usize>,
}

/// Information about a single edge cluster.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize, serde::Deserialize)]
#[rkyv(derive(Debug))]
pub struct EdgeClusterInfo {
    /// Numeric cluster identifier (0-based).
    pub id: usize,
    /// Human-readable auto-generated label (top TF-IDF keywords).
    pub label: String,
    /// Centroid vector (mean of member edge embeddings).
    pub centroid: Vec<f32>,
    /// Edge IDs belonging to this cluster.
    pub members: Vec<String>,
    /// Top keywords extracted via TF-IDF from edge context paragraphs.
    pub keywords: Vec<String>,
}

/// Edge cluster state persisted in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize, serde::Deserialize)]
#[rkyv(derive(Debug))]
pub struct EdgeClusterState {
    /// All edge clusters.
    pub clusters: Vec<EdgeClusterInfo>,
    /// Number of edges added since last full rebalance.
    pub edges_since_rebalance: usize,
    /// Total edge count at last rebalance.
    pub edges_at_last_rebalance: usize,
}

/// The complete link graph stored in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Serialize)]
#[rkyv(derive(Debug))]
pub struct LinkGraph {
    /// Forward links: source path → list of link entries.
    pub forward: HashMap<String, Vec<LinkEntry>>,
    /// Unix timestamp of last update.
    pub last_updated: u64,
    /// Semantic edges with paragraph context (None for old indices without edge data).
    pub semantic_edges: Option<HashMap<String, SemanticEdge>>,
    /// Edge clustering state (None if not yet clustered or too few edges).
    pub edge_cluster_state: Option<EdgeClusterState>,
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
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
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

/// A node in a tree-structured link neighborhood.
///
/// Represents a linked file with its validity state and recursively
/// discovered children (further links from this file).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct NeighborhoodNode {
    /// Relative path to this file.
    pub path: String,
    /// Whether this file exists in the known file set.
    pub state: LinkState,
    /// Children discovered by following links from this file.
    pub children: Vec<NeighborhoodNode>,
}

/// Result of a deep neighborhood query for a file.
///
/// Contains tree-structured outgoing (forward) and incoming (backlink)
/// neighborhoods to configurable depth.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct NeighborhoodResult {
    /// The queried file path.
    pub file: String,
    /// Tree of outgoing (forward) links from this file.
    pub outgoing: Vec<NeighborhoodNode>,
    /// Tree of incoming (backlinks) to this file.
    pub incoming: Vec<NeighborhoodNode>,
    /// Total count of unique outgoing links (all depths).
    pub outgoing_count: usize,
    /// Total count of unique incoming links (all depths).
    pub incoming_count: usize,
    /// Number of depth levels explored for outgoing links.
    pub outgoing_depth_count: usize,
    /// Number of depth levels explored for incoming links.
    pub incoming_depth_count: usize,
}

/// Generate a unique edge ID in the format `"edge:source.md->target.md@42"`.
///
/// The line number disambiguates multiple links from the same source to the same target.
pub fn edge_id(source: &str, target: &str, line_number: usize) -> String {
    format!("edge:{}->{}@{}", source, target, line_number)
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
        semantic_edges: None,
        edge_cluster_state: None,
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

/// BFS traversal through forward links AND backlinks from seed files.
///
/// Returns a map from discovered file path to its minimum hop distance.
/// Seeds are excluded from output. Cycle-safe via visited set.
/// `max_depth` is clamped to `min(max_depth, 3)`.
pub fn bfs_neighbors(
    graph: &LinkGraph,
    backlinks: &HashMap<String, Vec<LinkEntry>>,
    seeds: &[String],
    max_depth: usize,
) -> HashMap<String, usize> {
    let max_depth = max_depth.min(3);
    if max_depth == 0 || seeds.is_empty() {
        return HashMap::new();
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut result: HashMap<String, usize> = HashMap::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    // Initialize with seeds
    for seed in seeds {
        if visited.insert(seed.clone()) {
            queue.push_back((seed.clone(), 0));
        }
    }

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        let next_depth = depth + 1;

        // Traverse forward links
        if let Some(entries) = graph.forward.get(&current) {
            for entry in entries {
                if visited.insert(entry.target.clone()) {
                    result.insert(entry.target.clone(), next_depth);
                    queue.push_back((entry.target.clone(), next_depth));
                }
            }
        }

        // Traverse backlinks
        if let Some(entries) = backlinks.get(&current) {
            for entry in entries {
                if visited.insert(entry.source.clone()) {
                    result.insert(entry.source.clone(), next_depth);
                    queue.push_back((entry.source.clone(), next_depth));
                }
            }
        }
    }

    result
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

/// Explore the link neighborhood of a file as a tree structure.
///
/// Builds tree-structured outgoing (forward links recursively) and incoming
/// (backlinks recursively) neighborhoods. Cycle detection is per-branch
/// to prevent infinite recursion while still allowing a file to appear in
/// multiple branches. Depth is clamped to 1-3.
pub fn neighborhood(
    graph: &LinkGraph,
    known_files: &HashSet<String>,
    file: &str,
    depth: usize,
) -> NeighborhoodResult {
    let depth = depth.clamp(1, 3);
    let backlinks = compute_backlinks(graph);

    // Build outgoing tree (following forward links)
    let mut outgoing_visited = HashSet::new();
    outgoing_visited.insert(file.to_string());
    let outgoing = build_outgoing_tree(graph, known_files, file, depth, &mut outgoing_visited);
    let outgoing_count = count_nodes(&outgoing);
    let outgoing_depth_count = max_depth(&outgoing);

    // Build incoming tree (following backlinks)
    let mut incoming_visited = HashSet::new();
    incoming_visited.insert(file.to_string());
    let incoming = build_incoming_tree(&backlinks, known_files, file, depth, &mut incoming_visited);
    let incoming_count = count_nodes(&incoming);
    let incoming_depth_count = max_depth(&incoming);

    NeighborhoodResult {
        file: file.to_string(),
        outgoing,
        incoming,
        outgoing_count,
        incoming_count,
        outgoing_depth_count,
        incoming_depth_count,
    }
}

/// Recursively build the outgoing (forward links) tree.
fn build_outgoing_tree(
    graph: &LinkGraph,
    known_files: &HashSet<String>,
    file: &str,
    remaining_depth: usize,
    branch_visited: &mut HashSet<String>,
) -> Vec<NeighborhoodNode> {
    if remaining_depth == 0 {
        return Vec::new();
    }

    let entries = match graph.forward.get(file) {
        Some(entries) => entries,
        None => return Vec::new(),
    };

    let mut nodes = Vec::new();
    for entry in entries {
        // Cycle detection: skip if already on this branch
        if !branch_visited.insert(entry.target.clone()) {
            continue;
        }

        let state = if known_files.contains(&entry.target) {
            LinkState::Valid
        } else {
            LinkState::Broken
        };

        let children = build_outgoing_tree(
            graph,
            known_files,
            &entry.target,
            remaining_depth - 1,
            branch_visited,
        );

        nodes.push(NeighborhoodNode {
            path: entry.target.clone(),
            state,
            children,
        });

        // Remove from branch visited so it can appear in other branches
        branch_visited.remove(&entry.target);
    }

    nodes
}

/// Recursively build the incoming (backlinks) tree.
fn build_incoming_tree(
    backlinks: &HashMap<String, Vec<LinkEntry>>,
    known_files: &HashSet<String>,
    file: &str,
    remaining_depth: usize,
    branch_visited: &mut HashSet<String>,
) -> Vec<NeighborhoodNode> {
    if remaining_depth == 0 {
        return Vec::new();
    }

    let entries = match backlinks.get(file) {
        Some(entries) => entries,
        None => return Vec::new(),
    };

    let mut nodes = Vec::new();
    for entry in entries {
        // Cycle detection: skip if already on this branch
        if !branch_visited.insert(entry.source.clone()) {
            continue;
        }

        let state = if known_files.contains(&entry.source) {
            LinkState::Valid
        } else {
            LinkState::Broken
        };

        let children = build_incoming_tree(
            backlinks,
            known_files,
            &entry.source,
            remaining_depth - 1,
            branch_visited,
        );

        nodes.push(NeighborhoodNode {
            path: entry.source.clone(),
            state,
            children,
        });

        // Remove from branch visited so it can appear in other branches
        branch_visited.remove(&entry.source);
    }

    nodes
}

/// Count total nodes in a neighborhood tree.
fn count_nodes(nodes: &[NeighborhoodNode]) -> usize {
    nodes
        .iter()
        .map(|n| 1 + count_nodes(&n.children))
        .sum()
}

/// Find the maximum depth of a neighborhood tree.
fn max_depth(nodes: &[NeighborhoodNode]) -> usize {
    if nodes.is_empty() {
        return 0;
    }
    nodes
        .iter()
        .map(|n| 1 + max_depth(&n.children))
        .max()
        .unwrap_or(0)
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
            modified_at: 0,
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

    // --- edge_id tests ---

    #[test]
    fn edge_id_format() {
        assert_eq!(
            edge_id("source.md", "target.md", 42),
            "edge:source.md->target.md@42"
        );
    }

    #[test]
    fn edge_id_with_subdirs() {
        assert_eq!(
            edge_id("docs/intro.md", "docs/guide.md", 10),
            "edge:docs/intro.md->docs/guide.md@10"
        );
    }

    // --- SemanticEdge tests ---

    #[test]
    fn semantic_edge_serde_roundtrip() {
        let edge = SemanticEdge {
            edge_id: edge_id("a.md", "b.md", 5),
            source: "a.md".to_string(),
            target: "b.md".to_string(),
            context_text: "See [b](b.md) for details.".to_string(),
            line_number: 5,
            strength: Some(0.85),
            relationship_type: Some("references".to_string()),
            cluster_id: Some(2),
        };
        let json = serde_json::to_string(&edge).unwrap();
        let roundtripped: SemanticEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtripped.edge_id, edge.edge_id);
        assert_eq!(roundtripped.source, edge.source);
        assert_eq!(roundtripped.target, edge.target);
        assert_eq!(roundtripped.context_text, edge.context_text);
        assert_eq!(roundtripped.line_number, edge.line_number);
        assert_eq!(roundtripped.strength, edge.strength);
        assert_eq!(roundtripped.relationship_type, edge.relationship_type);
        assert_eq!(roundtripped.cluster_id, edge.cluster_id);
    }

    #[test]
    fn semantic_edge_optional_fields_none() {
        let edge = SemanticEdge {
            edge_id: edge_id("a.md", "b.md", 1),
            source: "a.md".to_string(),
            target: "b.md".to_string(),
            context_text: "link text".to_string(),
            line_number: 1,
            strength: None,
            relationship_type: None,
            cluster_id: None,
        };
        let json = serde_json::to_string(&edge).unwrap();
        let roundtripped: SemanticEdge = serde_json::from_str(&json).unwrap();
        assert!(roundtripped.strength.is_none());
        assert!(roundtripped.relationship_type.is_none());
        assert!(roundtripped.cluster_id.is_none());
    }

    // --- EdgeClusterState tests ---

    #[test]
    fn edge_cluster_state_serde_roundtrip() {
        let state = EdgeClusterState {
            clusters: vec![EdgeClusterInfo {
                id: 0,
                label: "depends / imports".to_string(),
                centroid: vec![0.1, 0.2, 0.3],
                members: vec![edge_id("a.md", "b.md", 1)],
                keywords: vec!["depends".to_string(), "imports".to_string()],
            }],
            edges_since_rebalance: 5,
            edges_at_last_rebalance: 10,
        };
        let json = serde_json::to_string(&state).unwrap();
        let roundtripped: EdgeClusterState = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtripped.clusters.len(), 1);
        assert_eq!(roundtripped.clusters[0].id, 0);
        assert_eq!(roundtripped.clusters[0].label, "depends / imports");
        assert_eq!(roundtripped.clusters[0].members.len(), 1);
        assert_eq!(roundtripped.edges_since_rebalance, 5);
        assert_eq!(roundtripped.edges_at_last_rebalance, 10);
    }

    // --- LinkGraph backward compat tests ---

    #[test]
    fn link_graph_new_fields_default_none() {
        let graph = build_link_graph(&[]);
        assert!(graph.semantic_edges.is_none());
        assert!(graph.edge_cluster_state.is_none());
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

    // --- bfs_neighbors tests ---

    #[test]
    fn bfs_1_hop() {
        // a -> b -> c
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("b.md", vec![make_link("c", "C", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let backlinks = compute_backlinks(&graph);

        let neighbors = bfs_neighbors(
            &graph,
            &backlinks,
            &["a.md".to_string()],
            1,
        );

        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors["b.md"], 1);
    }

    #[test]
    fn bfs_2_hops() {
        // a -> b -> c -> d
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("b.md", vec![make_link("c", "C", 1, false)]),
            make_file("c.md", vec![make_link("d", "D", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let backlinks = compute_backlinks(&graph);

        let neighbors = bfs_neighbors(
            &graph,
            &backlinks,
            &["a.md".to_string()],
            2,
        );

        assert_eq!(neighbors.len(), 2);
        assert_eq!(neighbors["b.md"], 1);
        assert_eq!(neighbors["c.md"], 2);
        // d.md should NOT be included (3 hops away, but max_depth=2)
        assert!(!neighbors.contains_key("d.md"));
    }

    #[test]
    fn bfs_3_hops() {
        // a -> b -> c -> d -> e
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("b.md", vec![make_link("c", "C", 1, false)]),
            make_file("c.md", vec![make_link("d", "D", 1, false)]),
            make_file("d.md", vec![make_link("e", "E", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let backlinks = compute_backlinks(&graph);

        let neighbors = bfs_neighbors(
            &graph,
            &backlinks,
            &["a.md".to_string()],
            3,
        );

        assert_eq!(neighbors.len(), 3);
        assert_eq!(neighbors["b.md"], 1);
        assert_eq!(neighbors["c.md"], 2);
        assert_eq!(neighbors["d.md"], 3);
        assert!(!neighbors.contains_key("e.md"));
    }

    #[test]
    fn bfs_cycle_safe() {
        // a -> b -> c -> a (cycle), plus d -> e -> a for deeper test
        // We use a longer chain to verify cycles are handled:
        // a -> b -> c -> d -> a (cycle)
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("b.md", vec![make_link("c", "C", 1, false)]),
            make_file("c.md", vec![make_link("d", "D", 1, false)]),
            make_file("d.md", vec![make_link("a", "A", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let backlinks = compute_backlinks(&graph);

        // With max_depth 3, BFS should not loop forever
        let neighbors = bfs_neighbors(
            &graph,
            &backlinks,
            &["a.md".to_string()],
            3,
        );

        // a is seed (excluded). Bidirectional traversal:
        // hop 1: b.md (forward a->b), d.md (backlink d->a)
        // hop 2: c.md (forward b->c) — d already visited
        // hop 3: nothing new (c->d already visited, d->a already visited)
        // a is never re-added since it's in visited set
        assert_eq!(neighbors.len(), 3);
        assert_eq!(neighbors["b.md"], 1);
        assert_eq!(neighbors["d.md"], 1);
        assert_eq!(neighbors["c.md"], 2);
    }

    #[test]
    fn bfs_disconnected() {
        // a -> b, c is isolated
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let backlinks = compute_backlinks(&graph);

        let neighbors = bfs_neighbors(
            &graph,
            &backlinks,
            &["a.md".to_string()],
            3,
        );

        // Only b.md reachable, c.md is disconnected and not reachable
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors["b.md"], 1);
        assert!(!neighbors.contains_key("c.md"));
    }

    #[test]
    fn bfs_bidirectional() {
        // a -> b, c -> a (backlink from c to a)
        // From a: forward gives b at hop 1, backlink gives c at hop 1
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("c.md", vec![make_link("a", "A", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let backlinks = compute_backlinks(&graph);

        let neighbors = bfs_neighbors(
            &graph,
            &backlinks,
            &["a.md".to_string()],
            1,
        );

        // b.md via forward link, c.md via backlink (c links to a, so a has backlink from c)
        assert_eq!(neighbors.len(), 2);
        assert_eq!(neighbors["b.md"], 1);
        assert_eq!(neighbors["c.md"], 1);
    }

    #[test]
    fn bfs_empty_seeds() {
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let backlinks = compute_backlinks(&graph);

        let neighbors = bfs_neighbors(&graph, &backlinks, &[], 2);

        assert!(neighbors.is_empty());
    }

    #[test]
    fn bfs_empty_graph() {
        let graph = build_link_graph(&[]);
        let backlinks = compute_backlinks(&graph);

        let neighbors = bfs_neighbors(
            &graph,
            &backlinks,
            &["a.md".to_string()],
            2,
        );

        assert!(neighbors.is_empty());
    }

    // --- neighborhood tests ---

    #[test]
    fn neighborhood_depth_1() {
        // a -> b, a -> c; d -> a (backlink)
        let files = vec![
            make_file("a.md", vec![
                make_link("b", "B", 1, false),
                make_link("c", "C", 2, false),
            ]),
            make_file("d.md", vec![make_link("a", "A", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let known: HashSet<String> = ["a.md", "b.md", "c.md", "d.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let result = neighborhood(&graph, &known, "a.md", 1);

        assert_eq!(result.file, "a.md");

        // Outgoing: b.md, c.md (depth 1, no children)
        assert_eq!(result.outgoing.len(), 2);
        let out_paths: Vec<&str> = result.outgoing.iter().map(|n| n.path.as_str()).collect();
        assert!(out_paths.contains(&"b.md"));
        assert!(out_paths.contains(&"c.md"));
        for node in &result.outgoing {
            assert_eq!(node.state, LinkState::Valid);
            assert!(node.children.is_empty()); // depth 1 = no children
        }
        assert_eq!(result.outgoing_count, 2);
        assert_eq!(result.outgoing_depth_count, 1);

        // Incoming: d.md (depth 1, no children)
        assert_eq!(result.incoming.len(), 1);
        assert_eq!(result.incoming[0].path, "d.md");
        assert_eq!(result.incoming[0].state, LinkState::Valid);
        assert!(result.incoming[0].children.is_empty());
        assert_eq!(result.incoming_count, 1);
        assert_eq!(result.incoming_depth_count, 1);
    }

    #[test]
    fn neighborhood_depth_2() {
        // a -> b -> c; d -> a, e -> d
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("b.md", vec![make_link("c", "C", 1, false)]),
            make_file("d.md", vec![make_link("a", "A", 1, false)]),
            make_file("e.md", vec![make_link("d", "D", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let known: HashSet<String> = ["a.md", "b.md", "c.md", "d.md", "e.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let result = neighborhood(&graph, &known, "a.md", 2);

        // Outgoing: a -> b -> c
        assert_eq!(result.outgoing.len(), 1);
        assert_eq!(result.outgoing[0].path, "b.md");
        assert_eq!(result.outgoing[0].children.len(), 1);
        assert_eq!(result.outgoing[0].children[0].path, "c.md");
        assert!(result.outgoing[0].children[0].children.is_empty());
        assert_eq!(result.outgoing_count, 2); // b + c
        assert_eq!(result.outgoing_depth_count, 2);

        // Incoming: d -> a, e -> d
        assert_eq!(result.incoming.len(), 1);
        assert_eq!(result.incoming[0].path, "d.md");
        assert_eq!(result.incoming[0].children.len(), 1);
        assert_eq!(result.incoming[0].children[0].path, "e.md");
        assert!(result.incoming[0].children[0].children.is_empty());
        assert_eq!(result.incoming_count, 2); // d + e
        assert_eq!(result.incoming_depth_count, 2);
    }

    #[test]
    fn neighborhood_with_cycle() {
        // a -> b -> c -> a (cycle)
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
            make_file("b.md", vec![make_link("c", "C", 1, false)]),
            make_file("c.md", vec![make_link("a", "A", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let known: HashSet<String> = ["a.md", "b.md", "c.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let result = neighborhood(&graph, &known, "a.md", 3);

        // Outgoing: a -> b -> c (c -> a is cycle, skipped)
        assert_eq!(result.outgoing.len(), 1);
        assert_eq!(result.outgoing[0].path, "b.md");
        assert_eq!(result.outgoing[0].children.len(), 1);
        assert_eq!(result.outgoing[0].children[0].path, "c.md");
        // c -> a would be a cycle (a is the root), so no children
        assert!(result.outgoing[0].children[0].children.is_empty());

        // Incoming: c -> a (backlink), then c's backlinks: b -> c, then b's backlinks: a -> b (cycle)
        assert_eq!(result.incoming.len(), 1);
        assert_eq!(result.incoming[0].path, "c.md");
        assert_eq!(result.incoming[0].children.len(), 1);
        assert_eq!(result.incoming[0].children[0].path, "b.md");
        // b's backlink is a -> b, but a is on the branch, so skipped
        assert!(result.incoming[0].children[0].children.is_empty());
    }

    #[test]
    fn neighborhood_missing_file() {
        // Query for a file not in the graph
        let files = vec![
            make_file("a.md", vec![make_link("b", "B", 1, false)]),
        ];
        let graph = build_link_graph(&files);
        let known: HashSet<String> = ["a.md", "b.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let result = neighborhood(&graph, &known, "missing.md", 2);

        assert_eq!(result.file, "missing.md");
        assert!(result.outgoing.is_empty());
        assert!(result.incoming.is_empty());
        assert_eq!(result.outgoing_count, 0);
        assert_eq!(result.incoming_count, 0);
        assert_eq!(result.outgoing_depth_count, 0);
        assert_eq!(result.incoming_depth_count, 0);
    }
}

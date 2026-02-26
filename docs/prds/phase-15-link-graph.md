# PRD: Phase 15 — Link Graph & Backlinks

## Overview

Extract internal markdown links (`[text](path.md)` and `[[wikilinks]]`) during parsing to build a persistent link graph stored in the index. Enable backlink queries ("what links to this file?"), orphan detection ("what files have no links?"), broken link checking, and optional link-aware search boosting. New CLI commands `mdvdb links`, `mdvdb backlinks`, and `mdvdb orphans` with Phase 12-style colored output.

## Problem Statement

Markdown knowledge bases are dense with cross-references — `[see Authentication](auth.md)`, `[[daily-notes]]`, `[API Reference](api/endpoints.md)`. The current system treats every file as isolated. Two documents that explicitly link to each other are clearly related, but this signal is invisible to search and browsing.

Users maintaining documentation, zettelkasten-style notes, or project wikis need to answer questions that pure semantic search cannot:

1. **"What links to this file?"** — Understanding a document's importance and context by seeing what references it. Critical for refactoring docs (will renaming break references?).
2. **"What files have no links?"** — Orphaned documents that nothing references are often stale, forgotten, or poorly integrated into the knowledge base.
3. **"Are there broken links?"** — Internal links pointing to files that don't exist or aren't indexed. Common after reorganizing a docs folder.
4. **"Show me related documents"** — Combining semantic similarity with explicit link relationships gives better recommendations than either signal alone.

The data is already flowing through the parser (`pulldown-cmark` encounters link events) — we're just discarding it.

## Goals

- Extract internal markdown links during parsing: `[text](relative-path.md)` and `[[wikilink]]` syntax
- Persist link graph in the index (as `Option<LinkGraph>` on `IndexMetadata`, following Schema/ClusterState pattern)
- `mdvdb links <file>` — show outgoing links and incoming backlinks for a file
- `mdvdb backlinks <file>` — show only files that link TO this file
- `mdvdb orphans` — show files with zero incoming AND zero outgoing internal links
- Broken link detection: flag links to files not in the index
- Link-aware search boost: optional `--boost-links` flag to promote results that are link-neighbors of top hits
- Incremental update: when a file changes, re-extract its links and update the graph
- Phase 12-style colored CLI output with tree rendering and state badges
- JSON output for all new commands (`--json`)
- `vdb.links()`, `vdb.backlinks()`, `vdb.orphans()` library API methods

## Non-Goals

- Tracking external URLs (only internal relative links between indexed markdown files)
- PageRank, betweenness centrality, or other graph algorithms beyond direct neighbors
- Transitive link traversal (multi-hop graph walks) — only depth-1 direct links
- Graphviz, SVG, or visual graph rendering — ASCII tree output only
- Link weight scoring or ranking outgoing links by importance
- Modifying markdown files (e.g., auto-fixing broken links) — read-only as always
- Storing links in `StoredFile` or `StoredChunk` (separate `LinkGraph` struct in `IndexMetadata`)
- Anchor/heading fragment resolution (`file.md#section`) — target is file-level, fragment is informational only

## Technical Design

### Data Model Changes

**New types in `src/links.rs`:**

```rust
/// A single link extracted from a markdown file.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, serde::Serialize)]
pub struct LinkEntry {
    pub target_path: String,        // Resolved relative path (e.g., "docs/api/auth.md")
    pub link_text: String,          // Display text (e.g., "Authentication Guide")
    pub source_line: usize,         // 1-based line number where link appears
    pub fragment: Option<String>,   // Optional heading fragment (e.g., "installation")
    pub is_wikilink: bool,          // true for [[wikilink]], false for [text](path)
}

/// The complete link graph for the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, serde::Serialize)]
pub struct LinkGraph {
    /// Forward links: source_path → list of outgoing links
    pub forward: HashMap<String, Vec<LinkEntry>>,
    pub last_updated: u64,
}

/// Computed at query time from forward links (not stored).
pub struct LinkQueryResult {
    pub file_path: String,
    pub outgoing: Vec<ResolvedLink>,
    pub incoming: Vec<ResolvedLink>,
    pub broken_outgoing: Vec<LinkEntry>,
}

/// A resolved link with status information.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolvedLink {
    pub path: String,               // Relative path of the linked file
    pub link_text: String,          // Display text from the link
    pub source_line: usize,         // Line number
    pub fragment: Option<String>,   // Heading fragment
    pub is_wikilink: bool,
    pub state: LinkState,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum LinkState {
    Valid,      // Target file exists in the index
    Broken,     // Target file not found in index or on disk
}

/// A file with no incoming and no outgoing internal links.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OrphanFile {
    pub path: String,
    pub file_size: u64,
    pub indexed_at: u64,
    pub chunk_count: usize,
}
```

**Addition to `IndexMetadata` in `src/index/types.rs`:**

```rust
pub struct IndexMetadata {
    pub chunks: HashMap<String, StoredChunk>,
    pub files: HashMap<String, StoredFile>,
    pub embedding_config: EmbeddingConfig,
    pub last_updated: u64,
    pub schema: Option<Schema>,
    pub cluster_state: Option<ClusterState>,
    pub link_graph: Option<LinkGraph>,          // NEW
}
```

This follows the exact pattern used by `schema` (Phase 7) and `cluster_state` (Phase 9). `Option` ensures backward compatibility — existing indices without link data load fine with `None`.

### Interface Changes

**Parser extension (`src/parser.rs`):**

```rust
pub struct MarkdownFile {
    pub path: PathBuf,
    pub frontmatter: Option<serde_json::Value>,
    pub headings: Vec<Heading>,
    pub body: String,
    pub content_hash: String,
    pub file_size: u64,
    pub links: Vec<RawLink>,               // NEW — extracted during parsing
}

/// Raw link as extracted from markdown, before path resolution.
#[derive(Debug, Clone)]
pub struct RawLink {
    pub raw_target: String,                // Original target string from markdown
    pub link_text: String,                 // Display text
    pub line_number: usize,                // 1-based
    pub is_wikilink: bool,                 // true for [[...]]
}
```

**Index methods (`src/index/state.rs`):**

```rust
impl Index {
    pub fn get_link_graph(&self) -> Option<LinkGraph>;
    pub fn update_link_graph(&self, graph: Option<LinkGraph>);
}
```

**Library API (`src/lib.rs`):**

```rust
impl MarkdownVdb {
    /// Get outgoing links and incoming backlinks for a specific file.
    pub fn links(&self, relative_path: &str) -> Result<LinkQueryResult>;

    /// Get only files that link TO the given file (incoming backlinks).
    pub fn backlinks(&self, relative_path: &str) -> Result<Vec<ResolvedLink>>;

    /// Get files with zero incoming AND zero outgoing internal links.
    pub fn orphans(&self) -> Result<Vec<OrphanFile>>;
}
```

**Search extension (`src/search.rs`):**

```rust
pub struct SearchQuery {
    pub query: String,
    pub limit: usize,
    pub min_score: f64,
    pub filters: Vec<MetadataFilter>,
    pub boost_links: bool,                 // NEW — boost link neighbors of top results
}

impl SearchQuery {
    pub fn with_boost_links(mut self, boost: bool) -> Self;
}
```

### New Commands / API / UI

#### `mdvdb links <file>`

Show outgoing links and incoming backlinks for a file.

```
mdvdb links docs/api/auth.md [--json]
```

Human-readable output (Phase 12 color scheme):

```
Links: docs/api/auth.md

  Outgoing (3 links)
  ├── docs/api/endpoints.md            "API Endpoints"
  │   Line 42
  ├── docs/getting-started.md          "Getting Started Guide"
  │   Line 15
  └── docs/api/middleware.md            "Middleware Reference"
      Line 78

  Incoming (2 backlinks)
  ├── docs/README.md                   "see Authentication"
  │   Line 23
  └── docs/guides/security.md          "Auth Module"
      Line 156

  3 outgoing, 2 incoming, 0 broken
```

Color assignments (per Phase 12 color scheme):
- `"Links:"` title → bold white
- File paths → bold
- Link text (quoted) → dimmed
- `"Line 42"` → dimmed
- `"Outgoing"` / `"Incoming"` labels → cyan
- Counts → yellow
- `[broken]` badge → red
- `[[wikilink]]` indicator → blue
- Tree connectors (`├──`, `└──`, `│`) → dimmed

With broken links:

```
  Outgoing (3 links, 1 broken)
  ├── docs/api/endpoints.md            "API Endpoints"
  │   Line 42
  ├── docs/nonexistent.md              "Missing Doc" [broken]
  │   Line 67
  └── docs/api/middleware.md            "Middleware Reference"
      Line 78
```

JSON output:

```json
{
  "file": "docs/api/auth.md",
  "outgoing": [
    {
      "path": "docs/api/endpoints.md",
      "link_text": "API Endpoints",
      "source_line": 42,
      "fragment": null,
      "is_wikilink": false,
      "state": "Valid"
    }
  ],
  "incoming": [
    {
      "path": "docs/README.md",
      "link_text": "see Authentication",
      "source_line": 23,
      "fragment": null,
      "is_wikilink": false,
      "state": "Valid"
    }
  ],
  "broken_outgoing": [
    {
      "target_path": "docs/nonexistent.md",
      "link_text": "Missing Doc",
      "source_line": 67,
      "fragment": null,
      "is_wikilink": false
    }
  ],
  "summary": {
    "outgoing_count": 3,
    "incoming_count": 2,
    "broken_count": 1
  }
}
```

#### `mdvdb backlinks <file>`

Shorthand for incoming-only view.

```
mdvdb backlinks docs/api/auth.md [--json]
```

Human output:

```
Backlinks: docs/api/auth.md (2 files)

  docs/README.md                       "see Authentication"
    Line 23
  docs/guides/security.md              "Auth Module"
    Line 156
```

#### `mdvdb orphans`

Show files with no links in or out.

```
mdvdb orphans [--json]
```

Human output:

```
Orphan Files (3 files with no links)

  docs/archive/old-notes.md            4.2 KB    3 months ago
  notes/scratch.md                     1.1 KB    2 days ago
  docs/deprecated/legacy-api.md        8.7 KB    6 months ago
```

Color assignments:
- File paths → bold
- File sizes → yellow (humanized via Phase 12's `format_file_size`)
- Timestamps → dimmed (humanized via Phase 12's `format_timestamp`)

#### `mdvdb search --boost-links`

```
mdvdb search "authentication" --boost-links [--json]
```

Behavior: After normal search, look at the top 3 results' link neighbors. If any other result is a link neighbor, boost its score by a fixed factor (1.2x). Re-sort. This is a simple, predictable boost — not a complex graph reranking.

### Migration Strategy

Fully backward compatible:

- Existing indices load with `link_graph: None` — all link commands return empty results with a hint to run `mdvdb ingest` to build the link graph.
- First `ingest` after upgrade extracts links from all files and populates the link graph.
- No changes to existing stored types (`StoredFile`, `StoredChunk`).
- New field on `MarkdownFile` (`links: Vec<RawLink>`) is additive — existing code that destructures `MarkdownFile` may need updating but this is internal.

## Implementation Steps

### Step 1: Add link extraction to the parser

**File: `src/parser.rs`**

1. Add `RawLink` struct (with `raw_target`, `link_text`, `line_number`, `is_wikilink` fields).
2. Add `pub links: Vec<RawLink>` field to `MarkdownFile`.
3. In `parse_markdown_file()`, extend the existing `pulldown_cmark::Parser` event loop to handle link events:
   ```rust
   Event::Start(Tag::Link { dest_url, .. }) => {
       // Capture dest_url, start collecting link_text
   }
   Event::End(TagEnd::Link) => {
       // Finalize RawLink with collected text, push to links vec
   }
   ```
4. Add wikilink detection: before `pulldown_cmark` parsing, scan for `[[target]]` patterns via regex `\[\[([^\]]+)\]\]` and extract them as `RawLink { is_wikilink: true }`. Replace `[[target]]` with `[target](target.md)` in the body before pulldown_cmark parsing, or extract separately — either approach works, but separate extraction avoids mutating body text.
5. Filter to only internal links: skip links starting with `http://`, `https://`, `mailto:`, `#` (same-file anchor). Keep relative paths only.
6. Add unit tests:
   - `test_extract_standard_links` — `[text](path.md)` extraction
   - `test_extract_wikilinks` — `[[page]]` extraction with `is_wikilink: true`
   - `test_skip_external_links` — `https://` URLs not included
   - `test_skip_anchor_links` — `#heading` links not included
   - `test_link_with_fragment` — `[text](path.md#section)` captures fragment
   - `test_link_line_numbers` — correct 1-based line numbers
   - `test_no_links_returns_empty` — file without links → empty vec

### Step 2: Create the links module

**File: `src/links.rs`** (new)

1. Define `LinkEntry`, `LinkGraph`, `LinkQueryResult`, `ResolvedLink`, `LinkState`, `OrphanFile` (all with appropriate derives: `rkyv::Archive/Serialize/Deserialize` for stored types, `serde::Serialize` for API types).
2. Implement link path resolution:
   ```rust
   /// Resolve a raw link target relative to the source file's directory.
   /// "endpoints.md" from "docs/api/auth.md" → "docs/api/endpoints.md"
   /// "../guide.md" from "docs/api/auth.md" → "docs/guide.md"
   /// "docs/guide.md" (already from root) → "docs/guide.md"
   pub fn resolve_link(source_path: &str, raw_target: &str) -> String;
   ```
3. Implement `build_link_graph(files: &[MarkdownFile]) -> LinkGraph`:
   - For each file, resolve its `RawLink`s to `LinkEntry`s using `resolve_link`
   - Normalize paths (strip leading `./`, collapse `../`, normalize separators)
   - Deduplicate links to the same target within a file
   - Store in `forward` HashMap
4. Implement `compute_backlinks(graph: &LinkGraph) -> HashMap<String, Vec<ResolvedLink>>`:
   - Invert the forward link map: for each `(source, links)`, for each link target, add source to target's backlink list
5. Implement `query_links(file_path: &str, graph: &LinkGraph, indexed_files: &HashSet<String>) -> LinkQueryResult`:
   - Look up forward links for the file
   - Compute backlinks by scanning all forward entries
   - Classify each link as `Valid` or `Broken` by checking against `indexed_files`
6. Implement `find_orphans(graph: &LinkGraph, all_files: &HashMap<String, StoredFile>) -> Vec<OrphanFile>`:
   - Files with no forward links AND not appearing as a target in any other file's forward links
7. Implement `update_file_links(graph: &mut LinkGraph, source_path: &str, new_links: Vec<LinkEntry>)`:
   - Replace the forward links for a single file (for incremental updates)
8. Implement `remove_file_links(graph: &mut LinkGraph, source_path: &str)`:
   - Remove all forward links for a deleted file
9. Add unit tests:
   - `test_resolve_link_same_dir` — relative link in same directory
   - `test_resolve_link_parent_dir` — `../sibling.md` resolution
   - `test_resolve_link_absolute_from_root` — `docs/file.md` stays as-is
   - `test_resolve_link_normalize` — `./file.md` → `file.md`, trailing slashes
   - `test_build_link_graph` — multiple files, verify forward map
   - `test_compute_backlinks` — verify inverted map
   - `test_query_links_valid` — all links point to indexed files
   - `test_query_links_broken` — link to non-indexed file → Broken state
   - `test_find_orphans` — file with no links in or out
   - `test_find_orphans_excludes_linked` — file with links is not orphan
   - `test_update_file_links` — incremental update replaces old links
   - `test_remove_file_links` — deletion clears forward links
   - `test_wikilink_resolution` — `[[page]]` resolved to `page.md`
   - `test_deduplicate_same_target` — multiple links to same file deduplicated in graph

### Step 3: Add link graph storage to the index

**File: `src/index/types.rs`**

1. Add `pub link_graph: Option<LinkGraph>` to `IndexMetadata`. Ensure `LinkGraph` and `LinkEntry` have all required rkyv derives.

**File: `src/index/state.rs`**

2. Add `get_link_graph(&self) -> Option<LinkGraph>` method — read lock, clone from metadata.
3. Add `update_link_graph(&self, graph: Option<LinkGraph>)` method — write lock, set in metadata.
4. Update any deserialization code if needed to handle the new `Option` field (rkyv should handle `None` for missing fields in old indices, but verify with a test).

**File: `src/index/storage.rs`**

5. Verify that the existing `save()`/`load()` round-trips work with the new field. Add a test: `test_link_graph_persistence` — create index with link graph, save, load, verify links preserved.

### Step 4: Integrate link extraction into ingest

**File: `src/lib.rs`**

1. In `MarkdownVdb::ingest()`, after parsing all files and before saving:
   - Call `links::build_link_graph(&parsed_files)` to build the full link graph
   - Call `self.index.update_link_graph(Some(link_graph))`
2. For single-file ingest (`options.file` is `Some`):
   - Parse the single file (links are now extracted)
   - Get existing link graph from index (or create empty)
   - Call `links::update_file_links(&mut graph, &relative_path, new_links)`
   - Store updated graph
3. For file removal (stale files):
   - Call `links::remove_file_links(&mut graph, &removed_path)`

### Step 5: Integrate link updates into watcher

**File: `src/watcher.rs`**

1. In `handle_event()` for `FileEvent::Modified` and `FileEvent::Created`:
   - After parsing and upserting the file, extract its links
   - Get existing link graph, call `update_file_links`, store back
2. In `handle_event()` for `FileEvent::Deleted`:
   - Call `remove_file_links` for the deleted file
3. In `handle_event()` for `FileEvent::Renamed`:
   - Remove old path links, add new path links

### Step 6: Add library API methods

**File: `src/lib.rs`**

1. Add `pub mod links;` to module declarations.
2. Add re-exports: `pub use links::{LinkGraph, LinkEntry, LinkQueryResult, ResolvedLink, LinkState, OrphanFile};`
3. Implement `links(&self, relative_path: &str) -> Result<LinkQueryResult>`:
   - Get link graph from index (return error if None with hint to ingest)
   - Get set of indexed file paths from index
   - Call `links::query_links(relative_path, &graph, &indexed_files)`
4. Implement `backlinks(&self, relative_path: &str) -> Result<Vec<ResolvedLink>>`:
   - Call `self.links(relative_path)` and return only `incoming`
5. Implement `orphans(&self) -> Result<Vec<OrphanFile>>`:
   - Get link graph and files from index
   - Call `links::find_orphans(&graph, &files)`

### Step 7: Add link-aware search boost

**File: `src/search.rs`**

1. Add `pub boost_links: bool` field to `SearchQuery`, default `false`.
2. Add `pub fn with_boost_links(mut self, boost: bool) -> Self` builder method.
3. At the end of `search()`, after normal result assembly and before final truncation:
   - If `boost_links` is `true` and link graph is available:
   - Get the top 3 results' file paths
   - Find their link neighbors (files they link to + files that link to them)
   - For any other result whose file is a link neighbor, multiply its score by 1.2
   - Re-sort by score descending
4. The link graph is passed via the existing `index` parameter (call `index.get_link_graph()`).

### Step 8: Add CLI commands

**File: `src/main.rs`**

1. Add `LinksArgs`, `BacklinksArgs`, `OrphansArgs` structs:
   ```rust
   #[derive(Parser)]
   struct LinksArgs {
       /// Path to the markdown file (relative to project root)
       file_path: PathBuf,
       /// Output as JSON
       #[arg(long)]
       json: bool,
   }

   #[derive(Parser)]
   struct BacklinksArgs {
       /// Path to the markdown file (relative to project root)
       file_path: PathBuf,
       /// Output as JSON
       #[arg(long)]
       json: bool,
   }

   #[derive(Parser)]
   struct OrphansArgs {
       /// Output as JSON
       #[arg(long)]
       json: bool,
   }
   ```
2. Add `Links(LinksArgs)`, `Backlinks(BacklinksArgs)`, `Orphans(OrphansArgs)` to `Commands` enum.
3. Add `--boost-links` flag to `SearchArgs`:
   ```rust
   /// Boost results that are link neighbors of top hits
   #[arg(long)]
   boost_links: bool,
   ```
4. Implement handlers:
   - `Commands::Links` — call `vdb.links(&path)`, format or JSON
   - `Commands::Backlinks` — call `vdb.backlinks(&path)`, format or JSON
   - `Commands::Orphans` — call `vdb.orphans()`, format or JSON
   - Update `Commands::Search` to pass `boost_links` to query builder

### Step 9: Add formatting functions

**Note:** If Phase 12 (`src/format.rs`) has not been implemented yet, add the formatting as inline `println!` blocks in `main.rs` following the current pattern. If Phase 12 IS implemented, add these functions to `src/format.rs`:

```rust
/// Print link query results with tree rendering, colored badges, and summary.
pub fn print_links(result: &LinkQueryResult);

/// Print backlinks list (incoming only).
pub fn print_backlinks(file_path: &str, backlinks: &[ResolvedLink]);

/// Print orphan files with humanized file size and timestamp.
pub fn print_orphans(orphans: &[OrphanFile]);
```

Human-readable output uses:
- Box-drawing tree connectors (`├──`, `└──`, `│`) for link lists — dimmed
- File paths in bold
- Link text in dimmed quotes
- `"Line N"` in dimmed
- `[broken]` badge in red
- `[wikilink]` badge in blue
- Count numbers in yellow
- Section labels (`"Outgoing"`, `"Incoming"`) in cyan
- Summary line at the bottom

### Step 10: Add integration tests

**File: `tests/links_test.rs`** (new)

All tests use `tempfile::TempDir`, `mock_config()` with `EmbeddingProviderType::Mock`, and the library API.

1. `test_links_after_ingest` — Create 3 markdown files where A links to B and C. Ingest. Call `vdb.links("a.md")`. Verify 2 outgoing, 0 incoming.
2. `test_backlinks_after_ingest` — Same setup. Call `vdb.backlinks("b.md")`. Verify 1 incoming from A.
3. `test_broken_links` — A links to `nonexistent.md`. Ingest. Call `vdb.links("a.md")`. Verify `broken_outgoing` contains the link.
4. `test_orphans` — Create 3 files: A links to B, C has no links. Ingest. Call `vdb.orphans()`. Verify C is in orphan list, A and B are not.
5. `test_wikilinks` — File contains `[[page-name]]`. Ingest. Verify link resolved to `page-name.md`.
6. `test_link_graph_persistence` — Ingest, drop VDB, reopen, verify links still available.
7. `test_incremental_link_update` — Ingest file A (links to B). Modify A to link to C instead. Ingest single file. Verify graph updated.
8. `test_empty_link_graph_before_ingest` — Open VDB without ingesting. Call `vdb.links("a.md")`. Verify appropriate error or empty result.
9. `test_bidirectional_links` — A links to B, B links to A. Verify both see each other in incoming/outgoing.
10. `test_self_link_excluded` — A contains `[self](a.md)`. Verify self-link is excluded from graph or marked appropriately.

**File: `tests/cli_test.rs`** (additions)

11. `test_links_json_output` — Run `mdvdb links <file> --json`, verify valid JSON with `outgoing`, `incoming`, `summary` fields.
12. `test_backlinks_json_output` — Run `mdvdb backlinks <file> --json`, verify JSON array.
13. `test_orphans_json_output` — Run `mdvdb orphans --json`, verify JSON array of orphan files.
14. `test_links_nonexistent_file` — Run `mdvdb links nonexistent.md`, verify error exit code.
15. `test_search_boost_links_flag` — Run `mdvdb search "query" --boost-links --json`, verify command succeeds.

**File: `tests/api_test.rs`** (additions)

16. `test_links_api` — Call `vdb.links()` after ingest, verify `LinkQueryResult` structure.
17. `test_orphans_api` — Call `vdb.orphans()` after ingest, verify result.

### Step 11: Update CLAUDE.md and ROADMAP.md

- Add `links.rs` to project structure in CLAUDE.md
- Add `links()`, `backlinks()`, `orphans()` to public API list
- Add `links`, `backlinks`, `orphans` to CLI commands list
- Update ROADMAP.md with Phase 15 entry

### Step 12: Final validation

- `cargo test` — all existing tests pass, all new tests pass
- `cargo clippy --all-targets` — zero warnings
- `cargo run -- ingest` on a test corpus, then `cargo run -- links <file>` to verify output

## Validation Criteria

- [ ] Parser extracts `[text](path.md)` links with correct line numbers
- [ ] Parser extracts `[[wikilink]]` syntax and resolves to `wikilink.md`
- [ ] External links (`https://`, `mailto:`, `#anchor`) are excluded
- [ ] Link paths resolved correctly: relative to source file's directory
- [ ] `../` parent directory references resolve correctly
- [ ] Link graph persisted in index and survives save/load round-trip
- [ ] Existing indices without link data load without error (`link_graph: None`)
- [ ] `mdvdb links <file>` shows outgoing and incoming links with tree rendering
- [ ] `mdvdb links <file>` flags broken links with `[broken]` badge
- [ ] `mdvdb links <file> --json` outputs valid JSON with no ANSI escapes
- [ ] `mdvdb backlinks <file>` shows only incoming links
- [ ] `mdvdb orphans` lists files with no incoming or outgoing links
- [ ] `mdvdb orphans --json` outputs valid JSON array
- [ ] `mdvdb search --boost-links` boosts results that are link neighbors
- [ ] Incremental ingest updates link graph for changed file only
- [ ] File deletion removes that file's links from graph
- [ ] Watcher updates link graph on file changes
- [ ] Self-links (`[text](self.md)` from within `self.md`) are excluded
- [ ] Duplicate links to same target within a file are deduplicated
- [ ] All existing tests pass (309+)
- [ ] All new tests pass (17+)
- [ ] `cargo clippy --all-targets` reports zero warnings

## Anti-Patterns to Avoid

- **Do not store backlinks explicitly.** Backlinks are computed by inverting forward links. Storing both creates a sync burden — every link change requires updating two data structures. Forward links are the source of truth; backlinks are derived.

- **Do not track external URLs.** External links (`https://`, `mailto:`) are not part of the knowledge graph — they point outside the indexed corpus. Including them would bloat the graph and confuse orphan detection. Only internal relative links between markdown files are tracked.

- **Do not modify `StoredFile` or `StoredChunk`.** These are rkyv-serialized and changing their schema breaks backward compatibility with existing indices. Store link data in `IndexMetadata.link_graph` (following the Schema/ClusterState pattern).

- **Do not resolve link paths at query time.** Links are resolved relative to the source file's directory during parsing. Deferring resolution to query time would require knowing the source file's location for every query, adding complexity for zero benefit.

- **Do not implement multi-hop traversal.** Walking the graph beyond depth 1 (e.g., "files linked by files that link to X") adds combinatorial complexity and unclear UX. Direct links are sufficient for the stated use cases. Multi-hop can be a future enhancement.

- **Do not use `chrono` for timestamp formatting.** Follow Phase 12's convention: manual UTC formatting with `std::time::SystemTime` arithmetic. No heavy dependencies for simple relative time strings.

- **Do not mutate markdown files.** The system is read-only with respect to source files. Broken link detection reports issues but never auto-fixes them. All computed data lives in the index.

- **Do not add graph algorithm crates.** PageRank, betweenness centrality, etc. are interesting but out of scope. The link graph is a simple adjacency list with forward/backward traversal. `HashMap` is sufficient.

## Patterns to Follow

- **`IndexMetadata` extension pattern (`src/index/types.rs`)** — Adding `link_graph: Option<LinkGraph>` follows exactly how `schema: Option<Schema>` (Phase 7) and `cluster_state: Option<ClusterState>` (Phase 9) were added. `Option` provides backward compatibility.

- **Parser event loop (`src/parser.rs`)** — The existing `extract_headings()` function iterates `pulldown_cmark::Event` variants. Link extraction follows the same pattern, handling `Event::Start(Tag::Link { dest_url, .. })` and collecting text until `Event::End(TagEnd::Link)`.

- **Ingest integration (`src/lib.rs`)** — Link graph building mirrors schema inference: called at the end of ingest after all files are parsed, stored via `self.index.update_link_graph()`, then `self.index.save()`.

- **Watcher integration (`src/watcher.rs`)** — Incremental link updates mirror incremental schema updates: after processing a file change, update the link graph for that file.

- **CLI command structure (`src/main.rs`)** — `LinksArgs`, `BacklinksArgs`, `OrphansArgs` follow the same `#[derive(Parser)]` pattern as `StatusArgs`, `GetArgs`, etc. Each has `--json` flag.

- **Library API pattern (`src/lib.rs`)** — `links()`, `backlinks()`, `orphans()` mirror `status()`, `schema()`, `clusters()`: thin methods that delegate to module functions, passing index data.

- **Test patterns** — Integration tests use `tempfile::TempDir` + `mock_config()` + `EmbeddingProviderType::Mock`. CLI tests use `std::process::Command` with `env!("CARGO_BIN_EXE_mdvdb")`. Unit tests in `#[cfg(test)] mod tests` blocks.

- **Phase 12 color scheme** — All human-readable output follows the color vocabulary: bold white titles, cyan labels, bold paths, yellow numbers, red errors, blue keywords, dimmed secondary info. If Phase 12 is implemented, add functions to `format.rs`. If not, use `println!` with the same visual hierarchy in plain text.

## Critical Files

| File | Action | Description |
|------|--------|-------------|
| `src/parser.rs` | Modify | Add `RawLink` struct, `links` field to `MarkdownFile`, link extraction in event loop |
| `src/links.rs` | Create | Core module: link types, resolution, graph building, queries, orphan detection |
| `src/index/types.rs` | Modify | Add `link_graph: Option<LinkGraph>` to `IndexMetadata` |
| `src/index/state.rs` | Modify | Add `get_link_graph()` / `update_link_graph()` methods |
| `src/lib.rs` | Modify | Register `links` module, add re-exports, add `links()`/`backlinks()`/`orphans()` methods, integrate into ingest |
| `src/search.rs` | Modify | Add `boost_links` field to `SearchQuery`, implement link-aware boost |
| `src/main.rs` | Modify | Add `Links`/`Backlinks`/`Orphans` commands, `--boost-links` on search |
| `src/watcher.rs` | Modify | Update `handle_event` to maintain link graph incrementally |
| `tests/links_test.rs` | Create | Integration tests for link graph |
| `tests/cli_test.rs` | Modify | CLI tests for new commands |
| `tests/api_test.rs` | Modify | Library API tests for link methods |

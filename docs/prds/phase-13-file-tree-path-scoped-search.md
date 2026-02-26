# PRD: Phase 13 — File Tree Index & Path-Scoped Search

## Overview

Add a file tree view of indexed documents with colored state indicators, path-scoped search to restrict results to a directory subtree, and `path_components` on search results for hierarchical context. The tree is computed on-the-fly from existing index data — no new stored structures.

## Problem Statement

Users working with large markdown collections need structural awareness: which files are indexed, which changed, and where they sit in the directory hierarchy. Currently, `mdvdb status` only shows aggregate counts (documents, chunks, vectors). There is no way to:

1. See the directory layout of indexed files (like `tree` but scoped to the index)
2. Know which files are out of date, new, or deleted without running a full ingest
3. Restrict a semantic search to a specific subtree (e.g., `docs/api/`)
4. Get path hierarchy context in search results

## Goals

- New `mdvdb tree` CLI command: ASCII tree view with colored file-state indicators
- File states: indexed (green), modified (yellow), new (blue), deleted (red)
- Path-scoped search via `--path docs/api/` on the search command
- `path_components` field on `SearchResultFile` for hierarchical context
- New `vdb.file_tree()` library API method
- JSON output for all new features (`--json`)
- Zero new crate dependencies (use `std::io::IsTerminal`, `sha2` already in deps)

## Non-Goals

- Storing a tree structure in the index binary (computed on-the-fly is fast enough)
- Storing `path_components` in `StoredChunk`/`StoredFile` (split at runtime from existing `relative_path`)
- Glob/regex path matching (prefix match only — keep it simple)
- Modifying any rkyv-serialized structs (`StoredChunk`, `StoredFile`, `IndexMetadata`)
- Tree view of search results in this phase (can be added later)

## Technical Design

### Data Model Changes

**No changes to stored/serialized types.** All new data is derived at runtime.

#### New types in `src/tree.rs`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum FileState {
    Indexed,   // In index, content hash matches disk
    Modified,  // In index, content hash differs from disk
    New,       // On disk, not in index
    Deleted,   // In index, not on disk
}

#[derive(Debug, Clone, Serialize)]
pub struct FileTreeNode {
    pub name: String,           // Filename or directory name
    pub path: String,           // Full relative path from project root
    pub is_dir: bool,
    pub state: Option<FileState>, // None for directories
    pub children: Vec<FileTreeNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileTree {
    pub root: FileTreeNode,
    pub total_files: usize,
    pub indexed_count: usize,
    pub modified_count: usize,
    pub new_count: usize,
    pub deleted_count: usize,
}
```

#### Addition to `SearchQuery` (`src/search.rs`)

```rust
pub struct SearchQuery {
    pub query: String,
    pub limit: usize,
    pub min_score: f64,
    pub filters: Vec<MetadataFilter>,
    pub path_prefix: Option<String>,  // NEW — restrict to subtree
}
```

Plus builder: `pub fn with_path_prefix(mut self, prefix: impl Into<String>) -> Self`

#### Addition to `SearchResultFile` (`src/search.rs`)

```rust
pub struct SearchResultFile {
    pub path: String,
    pub frontmatter: Option<Value>,
    pub file_size: u64,
    pub path_components: Vec<String>,  // NEW — e.g., ["docs", "api", "auth.md"]
}
```

`path_components` is populated during result assembly by splitting `path` on `/`.

### Interface Changes

#### Search pipeline (`src/search.rs:search()`)

Insert path prefix check **before** file metadata lookup (fast string prefix check short-circuits expensive lookups):

```rust
// After: let Some(chunk) = index.get_chunk(chunk_id)
// Before: let Some(file) = index.get_file_metadata(...)
if let Some(ref prefix) = query.path_prefix {
    if !chunk.source_path.starts_with(prefix) {
        continue;
    }
}
```

#### Library API (`src/lib.rs`)

```rust
impl MarkdownVdb {
    pub fn file_tree(&self) -> Result<FileTree> { ... }
}
// Re-exports: FileTree, FileTreeNode, FileState
```

### New Commands

#### `mdvdb tree`

```
mdvdb tree [--path <prefix>] [--json] [--no-color]
```

Human-readable output:
```
.
├── docs/
│   ├── api/
│   │   ├── auth.md
│   │   └── endpoints.md [modified]
│   └── getting-started.md
├── notes/
│   ├── ideas.md [new]
│   └── todo.md
└── README.md

7 files (5 indexed, 1 modified, 1 new, 0 deleted)
```

Color scheme (ANSI):
- Indexed → green (`\x1b[32m`)
- Modified → yellow (`\x1b[33m`)
- New → blue (`\x1b[34m`)
- Deleted → red (`\x1b[31m`)
- Directories → bold (`\x1b[1m`)
- State labels (`[modified]`, `[new]`, `[deleted]`) always shown — color is additive

TTY detection: use `std::io::IsTerminal` (stable since Rust 1.70). Disable color if `--no-color` flag or `NO_COLOR` env var is set.

#### `mdvdb search --path`

```
mdvdb search "authentication" --path docs/api/ --json
```

JSON output includes `path_components`:
```json
{
  "results": [{
    "score": 0.89,
    "chunk": { "chunk_id": "docs/api/auth.md#0", ... },
    "file": {
      "path": "docs/api/auth.md",
      "path_components": ["docs", "api", "auth.md"],
      "file_size": 2048
    }
  }],
  "query": "authentication",
  "total_results": 1
}
```

### Migration Strategy

Fully backward compatible. No stored data changes. `path_prefix` defaults to `None`. `path_components` is a new additive field in JSON output.

## Implementation Steps

### Step 1: Add `path_prefix` to SearchQuery and `path_components` to SearchResultFile

**File: `src/search.rs`**

1. Add `path_prefix: Option<String>` field to `SearchQuery` (line 19)
2. Initialize to `None` in `SearchQuery::new()` (line 29)
3. Add `with_path_prefix()` builder method (after line 49)
4. Add `path_components: Vec<String>` to `SearchResultFile` (line 107)
5. In `search()` function (line 146–148), after chunk lookup and before file metadata lookup, add path prefix check:
   ```rust
   if let Some(ref prefix) = query.path_prefix {
       if !chunk.source_path.starts_with(prefix) {
           continue;
       }
   }
   ```
6. Populate `path_components` in result assembly (line 175–179):
   ```rust
   path_components: chunk.source_path.split('/').map(String::from).collect(),
   ```
7. Add unit tests: `test_search_query_with_path_prefix`, `test_path_prefix_defaults_to_none`

### Step 2: Create `src/tree.rs` module

**File: `src/tree.rs`** (new)

1. Define `FileState`, `FileTreeNode`, `FileTree` structs (with `Serialize` derive)
2. Implement `build_file_tree(project_root: &Path, config: &Config, index: &Index) -> Result<FileTree>`:
   - Call `discovery::discover(project_root, config)` to get current disk files
   - Call `index.get_file_hashes()` to get indexed file→hash map
   - For each discovered file: compute SHA-256 hash (using `sha2` crate, read file content), compare against index hash → determine `FileState`
   - For each indexed file not discovered → `Deleted`
   - Build tree from flat `(path, state)` list
3. Implement `build_tree_from_entries(entries: &[(String, FileState)]) -> FileTreeNode`:
   - Create root node `"."`, `is_dir: true`
   - For each entry: split path on `/`, walk tree creating intermediate dir nodes, insert leaf
   - Sort children at each level: directories first (alpha), then files (alpha)
4. Implement `render_tree(tree: &FileTree, colored: bool) -> String`:
   - Recursive render using `├── `, `└── `, `│   `, `    ` box-drawing chars
   - Apply ANSI color codes when `colored == true`
   - Append state suffix (`[modified]`, `[new]`, `[deleted]`) for non-`Indexed` files
   - Append summary line: `N files (X indexed, Y modified, Z new, W deleted)`
5. Implement `filter_subtree(tree: &FileTreeNode, prefix: &str) -> Option<FileTreeNode>`:
   - Return subtree matching the prefix, preserving directory structure
6. Add unit tests:
   - `test_build_tree_single_file` — one file at root
   - `test_build_tree_nested` — `a/b/c.md` creates intermediate dirs
   - `test_build_tree_sorting` — dirs first, then files, alphabetical
   - `test_build_tree_empty` — empty input → empty root
   - `test_file_state_classification` — indexed/modified/new/deleted detection
   - `test_render_tree_ascii` — verify box-drawing output structure
   - `test_render_tree_no_color` — no ANSI escape codes present
   - `test_render_tree_colored` — ANSI escape codes present
   - `test_filter_subtree` — filter to prefix, verify result
   - `test_summary_counts` — verify total/indexed/modified/new/deleted

### Step 3: Register module and add library API

**File: `src/lib.rs`**

1. Add `pub mod tree;` declaration
2. Add re-exports: `pub use tree::{FileTree, FileTreeNode, FileState};`
3. Add `file_tree()` method to `MarkdownVdb`:
   ```rust
   pub fn file_tree(&self) -> Result<FileTree> {
       tree::build_file_tree(&self.root, &self.config, &self.index)
   }
   ```

### Step 4: Add CLI `tree` command and `--path` on search

**File: `src/main.rs`**

1. Add `TreeArgs` struct:
   ```rust
   #[derive(Parser)]
   struct TreeArgs {
       #[arg(long)]
       path: Option<String>,
       #[arg(long)]
       json: bool,
       #[arg(long)]
       no_color: bool,
   }
   ```
2. Add `Tree(TreeArgs)` variant to `Commands` enum (with doc comment `/// Show file tree of indexed documents`)
3. Add `--path` arg to `SearchArgs`:
   ```rust
   #[arg(long)]
   path: Option<String>,
   ```
4. In search handler, apply path prefix:
   ```rust
   if let Some(ref path) = args.path {
       query = query.with_path_prefix(path);
   }
   ```
5. Add tree command handler:
   - Open VDB, call `vdb.file_tree()`
   - JSON mode: serialize to stdout
   - Human mode: detect TTY via `use std::io::IsTerminal; std::io::stdout().is_terminal()`, respect `--no-color` and `NO_COLOR` env var
   - If `--path` given, use `filter_subtree()` before rendering
   - Print rendered tree + summary footer
6. Update shell completion scripts (bash/zsh/fish/powershell) to include `tree` subcommand

### Step 5: Add integration tests

**File: `tests/tree_test.rs`** (new)
- `test_file_tree_empty_index` — tree before ingest shows all files as `New`
- `test_file_tree_after_ingest` — tree after ingest shows all files as `Indexed`
- `test_file_tree_modified_file` — modify file after ingest → `Modified`
- `test_file_tree_deleted_file` — delete file after ingest → `Deleted`
- `test_file_tree_new_file` — add file after ingest → `New`
- `test_file_tree_nested_dirs` — verify correct nesting
- `test_file_tree_json_serialization` — verify JSON roundtrip

**File: `tests/search_test.rs`** (additions)
- `test_path_prefix_filters_results` — index files in `docs/` and `notes/`, search with `path_prefix: "docs/"`, verify only docs results
- `test_path_prefix_no_match` — nonexistent prefix → empty results
- `test_path_components_populated` — verify field is populated correctly in results

**File: `tests/cli_test.rs`** (additions)
- `test_tree_command_json` — run `mdvdb tree --json`, verify JSON structure
- `test_tree_command_human` — run `mdvdb tree`, verify output contains file names
- `test_search_path_flag` — run `mdvdb search "query" --path docs/ --json`, verify filtered results

**File: `tests/api_test.rs`** (additions)
- `test_file_tree_api` — call `vdb.file_tree()` after ingest, verify structure
- `test_search_with_path_prefix_api` — call `vdb.search()` with path_prefix, verify filtering

### Step 6: Final cleanup

- `cargo test` — all tests pass
- `cargo clippy --all-targets` — zero warnings
- Update CLAUDE.md: add `tree.rs` to project structure, `file_tree()` to public API, `tree` to CLI commands

## Validation Criteria

- [ ] `mdvdb tree` displays ASCII tree with box-drawing characters
- [ ] File states correctly classified: indexed (hash match), modified (hash mismatch), new (not in index), deleted (not on disk)
- [ ] Colors applied when stdout is TTY, suppressed with `--no-color` or `NO_COLOR`
- [ ] `mdvdb tree --json` outputs valid JSON with root, counts, and state per file
- [ ] `mdvdb tree --path docs/` shows only the `docs/` subtree
- [ ] `mdvdb search "query" --path docs/` returns only results from `docs/` subtree
- [ ] `path_components` populated in all search results (JSON and programmatic)
- [ ] `vdb.file_tree()` returns correct `FileTree` from library API
- [ ] Directories sort before files, both alphabetically
- [ ] Summary line shows correct counts
- [ ] `cargo test` passes with zero failures
- [ ] `cargo clippy --all-targets` passes with zero warnings
- [ ] No new crate dependencies added

## Anti-Patterns to Avoid

- **Do not store the tree in IndexMetadata** — The tree is ephemeral display data, not query-critical. Storing it would add rkyv complexity, increase index size, and create a sync burden with the `files` HashMap. Compute it on-the-fly.

- **Do not add `path_components` to `StoredChunk` or `StoredFile`** — Splitting `relative_path.split('/')` is trivially fast at runtime. Storing it would add redundancy to every chunk in the index and require rkyv schema changes.

- **Do not use `path_prefix` as a `MetadataFilter` variant** — Path scoping operates on `source_path` (a core structural field), not frontmatter metadata. Mixing these concepts would confuse the API. Keep it as a separate `SearchQuery` field.

- **Do not add `atty` or other TTY crates** — `std::io::IsTerminal` is stable since Rust 1.70 and sufficient. The project avoids unnecessary dependencies.

- **Do not compute full file hashes eagerly for every tree call** — If the index has no files (empty), skip hash computation. For files not in the index, they are `New` without needing a hash. Only compute hashes for files that exist both on disk and in the index.

- **Do not use `println!` in the tree module** — Return strings from render functions. The CLI layer handles printing. Library code returns data, never prints.

## Patterns to Follow

- **Existing search pipeline in `src/search.rs`** — The path prefix check follows the same `continue`-on-mismatch pattern used for `min_score` and `evaluate_filters`. Place it between chunk lookup and file metadata lookup for early short-circuit.

- **Existing discovery in `src/discovery.rs`** — Reuse `discover()` for current filesystem state. Don't re-implement file walking.

- **Existing hash comparison in `src/ingest.rs`** — The `get_file_hashes()` + SHA-256 comparison pattern is already used for incremental ingest. The tree module follows the same pattern.

- **Builder pattern on `SearchQuery`** — `with_path_prefix()` follows the existing `with_limit()`, `with_min_score()`, `with_filter()` chain.

- **CLI arg structure in `src/main.rs`** — `TreeArgs` follows the same `#[derive(Parser)]` pattern as `StatusArgs`, `SchemaArgs`, etc. `--json` and `--no-color` flags follow existing conventions.

- **Test patterns** — Integration tests use `tempfile::TempDir`, `mock_config()`, and `EmbeddingProviderType::Mock`. CLI tests use `env!("CARGO_BIN_EXE_mdvdb")` with `std::process::Command`.

## Critical Files

| File | Action | Description |
|------|--------|-------------|
| `src/tree.rs` | Create | Core module: tree building, state computation, ASCII rendering |
| `src/search.rs` | Modify | Add `path_prefix` to SearchQuery, `path_components` to SearchResultFile, prefix filter in pipeline |
| `src/main.rs` | Modify | Add `Tree` subcommand, `--path` on search, completions update |
| `src/lib.rs` | Modify | Register `tree` module, add re-exports, add `file_tree()` method |
| `tests/tree_test.rs` | Create | Integration tests for file tree |
| `tests/search_test.rs` | Modify | Path prefix search tests |
| `tests/cli_test.rs` | Modify | Tree command + search --path CLI tests |
| `tests/api_test.rs` | Modify | Library API tests for file_tree() and path-scoped search |

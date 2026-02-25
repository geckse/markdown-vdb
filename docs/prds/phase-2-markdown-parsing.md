# PRD: Phase 2 — Markdown Parsing & File Discovery

## Overview

Build the file discovery system that recursively scans directories for `.md` files (respecting `.gitignore`, built-in ignores, and custom ignore patterns), and the markdown parser that extracts frontmatter metadata, heading structure, and body content from each file. This phase produces the raw material that the chunking engine (Phase 3) and embedding pipeline (Phase 4) consume.

## Problem Statement

The system needs to find all relevant markdown files in a project and understand their structure. This requires navigating complex ignore rules (`.gitignore` + built-in IDE/build directory ignores + user-defined patterns) and parsing markdown into structured components (frontmatter, headings, body text) without altering the source files.

## Goals

- Recursively discover all `.md` files in configured source directories
- Respect `.gitignore` rules automatically via the `ignore` crate
- Apply built-in default ignore patterns for IDE, build, and cache directories (never overridable)
- Apply user-configured `MDVDB_IGNORE_PATTERNS` as additional excludes
- Parse YAML frontmatter into `serde_json::Value` for dynamic metadata
- Extract heading hierarchy (h1–h6) with line numbers
- Extract body content preserving original text
- Compute SHA-256 content hash per file for change detection
- Store all file paths as relative to the project root

## Non-Goals

- No chunking logic (Phase 3)
- No embedding generation (Phase 4)
- No index file creation or storage (Phase 5)
- No support for non-markdown file types
- No frontmatter modification — strictly read-only on source files

## Technical Design

### Data Model Changes

**`MarkdownFile` struct** — parsed representation of a single file:

```rust
pub struct MarkdownFile {
    /// Path relative to project root
    pub relative_path: PathBuf,
    /// SHA-256 hash of the full file content (hex-encoded)
    pub content_hash: String,
    /// Parsed YAML frontmatter as dynamic JSON value, None if no frontmatter
    pub frontmatter: Option<serde_json::Value>,
    /// Ordered list of headings found in the document
    pub headings: Vec<Heading>,
    /// Full body content (everything after frontmatter delimiters)
    pub body: String,
    /// File size in bytes
    pub file_size: u64,
}

pub struct Heading {
    /// Heading level: 1–6
    pub level: u8,
    /// The heading text content
    pub text: String,
    /// 1-based line number where the heading appears in the file
    pub line_number: usize,
}
```

**`FileDiscovery` struct** — file scanner:

```rust
pub struct FileDiscovery {
    config: Config,
    project_root: PathBuf,
}
```

### Interface Changes

**File discovery:**

```rust
impl FileDiscovery {
    pub fn new(config: &Config, project_root: PathBuf) -> Self;

    /// Scan all configured source directories and return paths to .md files
    /// Paths are relative to project_root
    pub fn discover(&self) -> Result<Vec<PathBuf>>;
}
```

**Markdown parsing:**

```rust
/// Parse a single markdown file into its structured components
pub fn parse_markdown_file(
    project_root: &Path,
    relative_path: &Path,
) -> Result<MarkdownFile>;

/// Extract YAML frontmatter from raw file content
/// Returns (frontmatter_value, body_after_frontmatter)
fn extract_frontmatter(content: &str) -> Result<(Option<serde_json::Value>, &str)>;

/// Parse headings from markdown body content
fn extract_headings(body: &str) -> Vec<Heading>;

/// Compute SHA-256 hex digest of content
fn compute_content_hash(content: &str) -> String;
```

### Built-in Ignore Patterns

These are always applied and cannot be overridden by user config:

```rust
const BUILTIN_IGNORE_PATTERNS: &[&str] = &[
    ".claude/",
    ".cursor/",
    ".vscode/",
    ".idea/",
    ".git/",
    "node_modules/",
    ".obsidian/",
    "__pycache__/",
    ".next/",
    ".nuxt/",
    ".svelte-kit/",
    "target/",
    "dist/",
    "build/",
    "out/",
];
```

### Migration Strategy

Not applicable — no prior data exists.

## Implementation Steps

1. **Create `src/discovery.rs`** — Implement `FileDiscovery`:
   - In `discover()`, use the `ignore` crate's `WalkBuilder` to traverse each directory in `config.source_dirs`
   - Set `WalkBuilder::standard_filters(true)` to respect `.gitignore`
   - Add built-in ignore patterns using `WalkBuilder::add_custom_ignore_filename()` or by building an override matcher with `ignore::overrides::OverrideBuilder` for the `BUILTIN_IGNORE_PATTERNS` constant
   - Add `config.ignore_patterns` entries as additional overrides
   - Filter results to only include files with `.md` extension
   - Convert all paths to relative paths using `path.strip_prefix(&self.project_root)`
   - Sort results for deterministic ordering

2. **Create `src/parser.rs`** — Implement markdown parsing:
   - `extract_frontmatter(content)`: Check if content starts with `---\n`, find the closing `---\n`, extract the YAML between delimiters, parse with `serde_yaml::from_str::<serde_json::Value>()`. Return `(None, full_content)` if no valid frontmatter found. Handle edge cases: empty frontmatter (`---\n---\n`), frontmatter with only whitespace, missing closing delimiter (treat as no frontmatter).
   - `extract_headings(body)`: Use `pulldown_cmark::Parser` with default options. Iterate events, detect `Event::Start(Tag::Heading { level, .. })` to start collecting heading text, `Event::Text` to accumulate the heading content, `Event::End(TagEnd::Heading(_))` to finalize. Track line numbers by counting newlines in the source up to each heading's byte offset.
   - `compute_content_hash(content)`: Use `sha2::Sha256` digest, format as lowercase hex string.
   - `parse_markdown_file(project_root, relative_path)`: Read the file, call `extract_frontmatter`, `extract_headings`, `compute_content_hash`, construct and return `MarkdownFile`.

3. **Update `src/lib.rs`** — Add module declarations:
   - `pub mod discovery;`
   - `pub mod parser;`

4. **Write discovery tests** — Create `tests/discovery_test.rs`:
   - Create a temp directory with nested `.md` files and non-`.md` files
   - Test: only `.md` files are returned
   - Test: files in `.git/`, `node_modules/`, `.obsidian/` are excluded (built-in ignores)
   - Test: files matching `MDVDB_IGNORE_PATTERNS` are excluded
   - Test: `.gitignore` rules are respected (create a `.gitignore` in the temp dir)
   - Test: multiple `MDVDB_SOURCE_DIRS` are all scanned
   - Test: returned paths are relative to project root
   - Test: empty directory returns empty vec

5. **Write parser tests** — Create `tests/parser_test.rs`:
   - Test: file with valid frontmatter parses correctly (title, tags extracted)
   - Test: file without frontmatter returns `None` for frontmatter field
   - Test: empty frontmatter (`---\n---\n`) returns empty object
   - Test: headings at all levels (h1–h6) are extracted with correct levels
   - Test: heading line numbers are accurate
   - Test: nested headings preserve hierarchy order
   - Test: content hash is deterministic (same content = same hash)
   - Test: content hash changes when content changes
   - Test: file with only frontmatter and no body works
   - Test: file with no headings returns empty headings vec
   - Test: frontmatter with various YAML types (string, number, boolean, list, nested object) all parse into `serde_json::Value`

6. **Create test fixtures** — Add `tests/fixtures/` directory with sample markdown files:
   - `simple.md` — basic file with frontmatter and a few headings
   - `no-frontmatter.md` — markdown with no YAML frontmatter
   - `complex-frontmatter.md` — frontmatter with nested YAML, lists, dates
   - `deep-headings.md` — file with h1 through h6 headings
   - `empty.md` — empty file

## Validation Criteria

- [ ] `FileDiscovery::discover()` returns only `.md` files from configured source directories
- [ ] Files inside `.git/`, `node_modules/`, `.claude/`, `.obsidian/`, `target/`, `dist/`, `build/`, `out/`, `.vscode/`, `.idea/`, `.cursor/`, `__pycache__/`, `.next/`, `.nuxt/`, `.svelte-kit/` are never returned regardless of config
- [ ] `.gitignore` patterns in the project root are respected
- [ ] `MDVDB_IGNORE_PATTERNS=drafts/**` excludes files in `drafts/` subdirectory
- [ ] Frontmatter is parsed into `serde_json::Value` preserving types (strings, numbers, booleans, arrays)
- [ ] Files without frontmatter parse successfully with `frontmatter: None`
- [ ] Malformed frontmatter (missing closing `---`) is treated as no frontmatter, not an error
- [ ] All heading levels (h1–h6) are extracted with correct text and line numbers
- [ ] SHA-256 hash is hex-encoded, 64 characters long, deterministic
- [ ] All returned paths are relative to project root (no absolute paths in `MarkdownFile`)
- [ ] Source files are never modified (read-only access)
- [ ] `cargo test` passes all discovery and parser tests
- [ ] `cargo clippy` reports no warnings

## Anti-Patterns to Avoid

- **Do NOT walk directories manually with `std::fs::read_dir`** — Use the `ignore` crate which handles `.gitignore` parsing, symlink following, and efficient directory pruning. Manual traversal will miss `.gitignore` rules and be slower.
- **Do NOT store absolute paths** — All paths in `MarkdownFile.relative_path` must be relative to the project root. Absolute paths break portability when the project is moved.
- **Do NOT panic on malformed frontmatter** — Files with invalid YAML between `---` delimiters should be treated as having no frontmatter. Log a warning and continue.
- **Do NOT read file content twice** — Read the file once into a `String`, then pass references to `extract_frontmatter`, `extract_headings`, and `compute_content_hash`.
- **Do NOT modify the `ignore` crate's `.gitignore` handling** — Use `standard_filters(true)` and add custom patterns on top. Don't try to reimplement `.gitignore` logic.

## Patterns to Follow

- **Error handling:** Return `Result<T, Error>` from all public functions; use `Error::MarkdownParse` for parse failures with the file path included for debugging (defined in `src/error.rs` from Phase 1)
- **Config usage:** Accept `&Config` as parameter, never read env vars directly in this module
- **Testing:** Use temp directories with `std::fs::create_dir_all` and `std::fs::write` to create test fixtures programmatically; also provide static fixture files in `tests/fixtures/` for complex cases
- **Module structure:** `src/discovery.rs` for file scanning, `src/parser.rs` for markdown parsing — separate concerns

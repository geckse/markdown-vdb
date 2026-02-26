# PRD: Phase 12 — Making CLI Great

## Overview

Transform the mdvdb CLI from plain unformatted text into a polished terminal experience with colored output, ASCII art branding, inline infographics, progress indicators, and humanized values. A new `src/format.rs` module centralizes all human-readable formatting, while JSON output remains completely untouched for backward compatibility.

## Problem Statement

The current CLI is functional but visually sparse. Every subcommand uses raw `println!` with no colors, no visual hierarchy, and no progress feedback. Users cannot quickly scan output because labels, values, scores, and paths all look identical — monochrome text with manual indentation.

Several data fields that are already computed and available in JSON mode are never shown in human-readable output. Cluster keywords (`Vec<String>` on `ClusterSummary`), document frontmatter (`Option<serde_json::Value>` on `DocumentInfo`), and search result file metadata (`SearchResultFile.frontmatter`, `SearchResultFile.file_size`) are invisible unless the user passes `--json` and reads raw JSON.

Timestamps are raw Unix epoch integers (e.g., `1706349000`) and file sizes are raw byte counts (e.g., `15728640 bytes`), requiring mental conversion. The ingest command provides no feedback during processing — the user sees nothing until the entire pipeline (discovery, parsing, chunking, embedding, indexing, clustering) completes, which can take minutes for large corpora.

Running `mdvdb` with no subcommand prints a two-line plaintext message that fails to convey what the tool is or create any brand recognition. There is no visual identity for the tool.

## Goals

- Colored output for all human-readable CLI subcommands with a consistent color scheme
- Automatic TTY detection: disable colors when stdout is piped or redirected
- `--no-color` global flag and `NO_COLOR` environment variable support (per https://no-color.org/)
- Compact ASCII logo shown when running `mdvdb` with no subcommand and in `--version`
- ASCII infographics: score bars for search results, distribution bars for clusters, occurrence bars for schema fields
- Progress spinner (`indicatif`) during ingest with activity indication
- Human-readable timestamps ("2 hours ago" or "2024-01-15 12:00:00") and file sizes ("1.5 MB")
- Surface cluster keywords, document frontmatter, and search file metadata in human output
- Zero changes to JSON output format (full backward compatibility)
- All existing tests continue to pass; new features are tested

## Non-Goals

- No TUI framework (ratatui, cursive) or full-screen interactive mode
- No color themes or user-configurable color palettes
- No emoji in output (ASCII only for maximum terminal compatibility)
- No changes to the library API (`MarkdownVdb`, `IngestResult`, etc.)
- No changes to the JSON output format or structure
- No animated spinners for search (search is sub-ms with HNSW)
- No log colorization (tracing output remains unchanged on stderr)
- No `chrono` dependency for timestamp formatting

## Technical Design

### Data Model Changes

None. All data structures (`SearchResult`, `IngestResult`, `IndexStatus`, `Schema`, `ClusterSummary`, `DocumentInfo`) remain unchanged. The formatting layer reads existing fields that are already populated.

### Interface Changes

**`Cli` struct in `src/main.rs`** — add `--no-color` global flag:

```rust
#[derive(Parser)]
#[command(name = "mdvdb", version, about)]
struct Cli {
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Disable colored output
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}
```

**Color initialization** — early in `run()`, before any output:

```rust
if cli.no_color || std::env::var("NO_COLOR").is_ok() {
    colored::control::set_override(false);
}
```

The `colored` crate already checks TTY status internally. `set_override(false)` forces colors off for the explicit flag and env var. In JSON mode, colors are also disabled before output.

### New Module: `src/format.rs`

A new module owned by the **binary crate** (`mod format;` in `main.rs`, NOT in `lib.rs`). All human-readable output formatting moves here, keeping `main.rs` as thin dispatch and making formatting testable in isolation.

```rust
// src/format.rs

/// Format a Unix timestamp as a human-readable string.
/// Within 60s: "just now", within 1h: "N minutes ago",
/// within 24h: "N hours ago", within 30d: "N days ago",
/// otherwise: "YYYY-MM-DD HH:MM:SS" (UTC, manual formatting).
pub fn format_timestamp(unix_secs: u64) -> String;

/// Format bytes as human-readable size.
/// Examples: "42 B", "1.5 KB", "15.0 MB", "2.1 GB"
pub fn format_file_size(bytes: u64) -> String;

/// Render an ASCII bar of `width` characters for `value` in [0.0, max].
/// Filled segments are green, unfilled are dimmed.
/// Example: "========------" for value=0.6, max=1.0, width=14
pub fn render_bar(value: f64, max: f64, width: usize) -> String;

/// Print the ASCII logo in bold cyan.
pub fn print_logo();

/// Print version banner: logo + "mdvdb {version}" + tagline.
pub fn print_version();

/// Print search results with score bars, colored hierarchy, file metadata, frontmatter.
pub fn print_search_results(results: &[SearchResult], query: &str);

/// Print ingest summary with colored success/failure indicators.
pub fn print_ingest_result(result: &IngestResult);

/// Print index status with humanized file size and timestamps, colored labels.
pub fn print_status(status: &IndexStatus);

/// Print schema fields with occurrence bars relative to total_docs.
pub fn print_schema(schema: &Schema, total_docs: usize);

/// Print cluster summaries with distribution bars and keywords.
pub fn print_clusters(clusters: &[ClusterSummary]);

/// Print document info with frontmatter, humanized file size and timestamp.
pub fn print_document(doc: &DocumentInfo);

/// Print watch startup message.
pub fn print_watch_started(dirs: &[String]);

/// Print init success message.
pub fn print_init_success(path: &str);
```

### Color Scheme

Consistent color vocabulary across all commands:

| Element | Color | Usage |
|---------|-------|-------|
| Headings / titles | Bold white | `Index Status`, `Document Clusters (3 clusters)` |
| Labels / field names | Cyan | `Documents:`, `File size:`, `Provider:` |
| Paths / file names | Bold | `docs/guide.md` |
| Scores / numbers | Yellow | `0.8534`, `342` |
| Success messages | Green | `Ingestion complete`, `Created .markdownvdb` |
| Errors | Red (stderr) | `error: index not found` |
| Section headings | Magenta | `Getting Started > Installation` |
| Keywords / tags | Blue | `rust`, `programming`, `systems` |
| Bars (filled) | Green | `========` |
| Bars (unfilled) | Dimmed | `------` |
| Secondary info | Dimmed | `Lines 45-62`, hash values, relative timestamps |

### Infographic Designs

**Search result score bars** — 10-character bar next to numeric score:

```
1. docs/guide.md | 4.1 KB
   [=========-] 0.8534  Getting Started > Installation
   Lines 45-62
   title: "Guide" | tags: tutorial, setup
   To install, run cargo install mdvdb...
```

**Cluster distribution bars** — proportional to max cluster size, 20 chars max:

```
Document Clusters (3 clusters)

  Cluster 0 (12 docs) ====================
    Label: rust / systems / programming
    Keywords: rust, systems, programming, memory, safety

  Cluster 1 (3 docs)  =====
    Label: docs / guide
    Keywords: docs, guide, tutorial
```

**Schema occurrence bars** — proportional to total document count, 20 chars max:

```
Metadata Schema (5 fields)

  title (String) [required]
    ==================== 42/42
    Samples: "Hello World", "Rust Guide"

  tags (List)
    ========             18/42
    Samples: ["rust", "cli"], ["web"]
```

### Progress Bar Integration

The ingest command wraps `vdb.ingest()` with an `indicatif` spinner. The library API returns a final `IngestResult` without streaming progress, so a spinner (not a progress bar) is the correct choice:

```rust
let spinner = if !json_mode && std::io::stdout().is_terminal() {
    let sp = indicatif::ProgressBar::new_spinner();
    sp.set_style(indicatif::ProgressStyle::default_spinner()
        .template("{spinner:.green} {msg}")
        .unwrap());
    sp.set_message("Ingesting markdown files...");
    sp.enable_steady_tick(std::time::Duration::from_millis(100));
    Some(sp)
} else {
    None
};

let result = vdb.ingest(options).await?;

if let Some(sp) = spinner {
    sp.finish_and_clear();
}
```

The spinner is only created when stdout is a TTY and `--json` is not specified. `indicatif` auto-handles non-TTY gracefully, but we skip creation entirely in JSON mode to avoid any interference.

### TTY Detection Strategy

Three layers, checked in order:

1. **`--no-color` flag** — `colored::control::set_override(false)`
2. **`NO_COLOR` env var** — `colored` respects this automatically; we also check for spinner creation
3. **TTY detection** — `colored` checks internally via `IsTerminal`; for `indicatif`, we check `std::io::stdout().is_terminal()` (Rust 1.70+ `IsTerminal` trait)
4. **JSON mode** — colors and spinners unconditionally disabled

### New Commands / API / UI

No new subcommands. The `--no-color` global flag is the only new CLI argument.

### Migration Strategy

No migration needed. All changes are additive to the CLI presentation layer. JSON output is untouched. Existing scripts and integrations are unaffected.

## Implementation Steps

1. **Add dependencies to `Cargo.toml`.** Add `colored = "3"` and `indicatif = "0.17"` to `[dependencies]`. These are the only new crates needed — no `chrono`, no `atty` (use `std::io::IsTerminal` instead).

2. **Create `src/format.rs` with utility functions.** Implement `format_timestamp`, `format_file_size`, and `render_bar` as pure functions returning `String`. For timestamps, use `std::time::SystemTime` arithmetic: difference < 60s → "just now", < 3600 → "N minutes ago", < 86400 → "N hours ago", < 30 days → "N days ago", otherwise manual UTC date formatting without `chrono`. For file sizes, use 1024-based units (B/KB/MB/GB). For bars, map `value/max` to filled vs unfilled characters. Add `#[cfg(test)] mod tests` with unit tests for all three functions.

3. **Add ASCII logo and version functions to `src/format.rs`.** Create `print_logo()` that prints a 3-5 line ASCII art spelling "mdvdb" in bold cyan. Create `print_version()` that calls `print_logo()` then prints `"mdvdb {version}"` and `"Markdown Vector Database"`. The logo must be under 40 characters wide.

4. **Add per-command print functions to `src/format.rs`.** Implement `print_search_results`, `print_ingest_result`, `print_status`, `print_schema`, `print_clusters`, `print_document`, `print_watch_started`, and `print_init_success`. Each function uses the color scheme defined above. Import types from `mdvdb::*` (the library crate). Key additions beyond current output: search results show file size and frontmatter summary; clusters show keywords; get/document shows frontmatter; status/get show humanized timestamps and file sizes; schema shows occurrence bars.

5. **Add `--no-color` flag and `mod format` to `src/main.rs`.** Add `mod format;` at the top. Add the `no_color: bool` field to `Cli` struct with `#[arg(long, global = true)]`. At the start of `run()`, check `cli.no_color` and `std::env::var("NO_COLOR")` and call `colored::control::set_override(false)` when either is set.

6. **Refactor the no-subcommand case in `src/main.rs`.** Replace the current two-line `println!` with `format::print_logo()` plus a dimmed usage hint: `"Run mdvdb --help for usage information."`.

7. **Refactor search output in `src/main.rs`.** In the `Commands::Search` arm, replace the `println!`-based formatting block (currently ~15 lines) with a single call to `format::print_search_results(&results, &args.query)`. Keep JSON mode unchanged.

8. **Refactor ingest output in `src/main.rs`.** In the `Commands::Ingest` arm, add spinner creation before `vdb.ingest(options).await` and `finish_and_clear()` after. Replace the `println!` summary block with `format::print_ingest_result(&result)`. Only create spinner when `!json && stdout.is_terminal()`.

9. **Refactor status, schema, clusters, get, watch, init output in `src/main.rs`.** Replace each command's human-readable `println!` block with the corresponding `format::*` call. For schema, also call `vdb.status()` to get `document_count` for the occurrence bars. Keep all JSON branches unchanged.

10. **Ensure colors are disabled in JSON mode.** Before any JSON output, call `colored::control::set_override(false)`. This is a safety net — JSON serialization doesn't use `colored`, but this prevents any accidental colored output from leaking.

11. **Write unit tests for `src/format.rs`.** In the `#[cfg(test)]` block: test `format_file_size` with values for B, KB, MB, GB boundaries. Test `format_timestamp` with known offsets (0s, 300s, 7200s, 259200s). Test `render_bar` with full, half, empty, and zero-max edge cases. Use `colored::control::set_override(false)` in tests to get predictable output without ANSI codes.

12. **Write CLI integration tests in `tests/cli_test.rs`.** Add: `test_no_subcommand_shows_logo` (run `mdvdb`, check stdout for logo text), `test_no_color_flag_disables_colors` (run with `--no-color`, verify no `\x1b[` in stdout), `test_no_color_env_var` (run with `NO_COLOR=1`, verify no `\x1b[`), `test_clusters_shows_keywords` (run clusters after ingest, verify keyword text appears), `test_get_shows_frontmatter` (run get, verify frontmatter field names in output).

13. **Run `cargo test` and `cargo clippy --all-targets`.** All existing 306+ tests must pass. All new tests must pass. Clippy must report zero warnings.

## Validation Criteria

- [ ] `mdvdb` (no subcommand) displays the ASCII logo and a usage hint
- [ ] `mdvdb --version` displays the ASCII logo plus version info
- [ ] `mdvdb search "query"` shows colored output with score bars next to scores
- [ ] `mdvdb search "query"` shows file size and frontmatter summary per result
- [ ] `mdvdb search "query" --json` output is unchanged (valid JSON, no ANSI escapes)
- [ ] `mdvdb ingest` shows a spinner during processing when stdout is a TTY
- [ ] `mdvdb ingest` shows colored completion summary with humanized values
- [ ] `mdvdb ingest --json` output is unchanged (valid JSON, no spinner artifacts)
- [ ] `mdvdb status` shows humanized file size (e.g., "1.5 MB") and timestamp (e.g., "2 hours ago")
- [ ] `mdvdb schema` shows occurrence bars for each field relative to total documents
- [ ] `mdvdb clusters` shows distribution bars, labels, AND keywords for each cluster
- [ ] `mdvdb get file.md` shows frontmatter, humanized file size, and humanized timestamp
- [ ] `mdvdb --no-color status` produces output with no ANSI escape sequences
- [ ] `NO_COLOR=1 mdvdb status` produces output with no ANSI escape sequences
- [ ] Piping output (e.g., `mdvdb status | cat`) produces no ANSI escape sequences
- [ ] Colors are disabled automatically in `--json` mode for all commands
- [ ] All 306+ existing tests still pass
- [ ] New unit tests for `format_timestamp`, `format_file_size`, `render_bar` pass
- [ ] New CLI integration tests for `--no-color`, logo, keywords, frontmatter pass
- [ ] `cargo clippy --all-targets` reports zero warnings

## Anti-Patterns to Avoid

**Do NOT use inline ANSI escape codes.** Use the `colored` crate's methods (`.red()`, `.bold()`, `.cyan()`, etc.) exclusively. Hand-coded escape sequences are fragile, platform-dependent, and bypass the color-disabling mechanism provided by `colored::control::set_override`.

**Do NOT add `chrono` for timestamp formatting.** Implement relative-time logic ("2 hours ago") with basic arithmetic on `std::time::SystemTime`. The absolute fallback can use manual UTC formatting. `chrono` is a heavy transitive dependency for something that needs only division.

**Do NOT colorize JSON output.** Always ensure `colored::control::set_override(false)` is called before JSON paths. JSON parsers will choke on embedded ANSI sequences. The `--json` flag must produce clean, parseable output.

**Do NOT put formatting logic in `lib.rs` or library modules.** The `format` module is a presentation concern for the binary crate. It imports library types but the library never imports from `format`. Formatting stays at the CLI boundary via `mod format;` in `main.rs`.

**Do NOT make the progress bar mandatory.** The spinner must be created only when stdout is a TTY and `--json` is not specified. Non-interactive environments (CI, pipes, JSON mode) must see clean output with no control characters.

**Do NOT change the JSON output format.** Fields like `file_size` must remain as raw `u64` in JSON. Humanization is only for human-readable output. Existing integrations depend on the current JSON schema.

**Do NOT colorize stderr / tracing output.** Tracing goes to stderr and is controlled by `tracing-subscriber`. Do not mix `colored` output into the tracing pipeline.

## Patterns to Follow

**Formatting module pattern.** All human-readable output in a single `src/format.rs` module, mirroring how `src/logging.rs` centralizes log setup. Each subcommand's human output is a single function call from `main.rs`: `format::print_search_results(...)`. This keeps `main.rs` thin and dispatch-only.

**Color toggle pattern.** Check `--no-color` flag and `NO_COLOR` env var once at startup, call `colored::control::set_override(false)` to globally disable. All subsequent `.red()`, `.bold()` calls become no-ops automatically. No per-call color checking needed.

**Existing test pattern.** CLI integration tests in `tests/cli_test.rs` use `std::process::Command` with `env!("CARGO_BIN_EXE_mdvdb")` and `tempfile::TempDir`. New tests follow the same `setup_and_ingest()` helper pattern already established in that file.

**Thin CLI handler pattern.** Each `Commands` match arm: call library API, then call format function. No inline formatting logic in `main.rs`. This matches the existing separation where `main.rs` dispatches and `lib.rs` does business logic.

**Unit-testable formatting.** Functions like `format_timestamp`, `format_file_size`, and `render_bar` are pure functions returning `String`. They are tested by calling `colored::control::set_override(false)` in tests to strip ANSI codes, then asserting on the plain text content.

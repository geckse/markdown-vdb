# PRD: Path-Scoped Decay Filtering & Search Defaults

## Overview

Adds path-based include and exclude controls for time decay in search, plus fills the remaining gap in configurable search defaults (`boost_links`). When decay is enabled, users can specify which files are affected by decay via path prefix patterns — either excluding specific paths (evergreen/pinned content) or restricting decay to only certain paths (whitelist mode). Exclude always takes precedence over include.

## Problem Statement

Time decay currently applies uniformly to all files when enabled. This is too coarse for real-world use:

- **Reference docs** (e.g., `docs/reference/`, `wiki/glossary/`) should never be penalized for age — they are evergreen.
- **Journal/log directories** (e.g., `journal/`, `daily/`) benefit from decay, but other content does not.
- Users need per-path control without disabling decay entirely or managing it per-query every time.

Additionally, `boost_links` is the only search behavior that cannot be configured as a project default — it requires `--boost-links` on every search command.

## Goals

- Allow config-level path prefixes that exclude files from decay
- Allow config-level path prefixes that restrict decay to only matching files (whitelist)
- Exclude takes precedence when both lists match a file
- Support per-query overrides for both lists via the library API
- Expose both lists as CLI flags on the `search` command
- Add `MDVDB_SEARCH_BOOST_LINKS` config var for default link boosting
- Zero performance impact when lists are empty (current behavior preserved exactly)

## Non-Goals

- Glob or regex pattern matching — simple prefix matching only (consistent with `--path`)
- Per-file or per-chunk decay half-life (different half-lives per path)
- Frontmatter-based decay control (e.g., `decay: false` in YAML)
- Decay exclude/include in the file watcher or ingest pipeline (search-time only)

## Technical Design

### Data Model Changes

**Config struct** (`src/config.rs`):

```rust
// Current
pub search_decay_enabled: bool,
pub search_decay_half_life: f64,

// Added
pub search_decay_exclude: Vec<String>,  // Path prefixes immune to decay
pub search_decay_include: Vec<String>,  // Path prefixes where decay applies (whitelist)
pub search_boost_links: bool,           // Default for link boosting (currently hardcoded false)
```

**SearchQuery struct** (`src/search.rs`):

```rust
// Added optional per-query overrides
pub decay_exclude: Option<Vec<String>>,
pub decay_include: Option<Vec<String>>,
```

### Interface Changes

**SearchQuery builder methods** (`src/search.rs`):

```rust
pub fn with_decay_exclude(mut self, patterns: Vec<String>) -> Self
pub fn with_decay_include(mut self, patterns: Vec<String>) -> Self
```

**New helper function** (`src/search.rs`):

```rust
/// Determines whether decay should be applied to a file at the given path.
/// Returns false if the path matches any exclude prefix (highest priority),
/// or if include list is non-empty and path matches no include prefix.
fn should_apply_decay(path: &str, exclude: &[String], include: &[String]) -> bool
```

### New Commands / API / UI

**Config env vars:**

```
MDVDB_SEARCH_DECAY_EXCLUDE=docs/reference,wiki/pinned
MDVDB_SEARCH_DECAY_INCLUDE=journal,daily
MDVDB_SEARCH_BOOST_LINKS=true
```

**CLI flags** on `mdvdb search`:

```
--decay-exclude <PATTERNS>   Comma-separated path prefixes excluded from decay
--decay-include <PATTERNS>   Comma-separated path prefixes where decay applies
```

### Migration Strategy

No migration needed. Empty lists (the default) preserve existing behavior exactly. The `MDVDB_SEARCH_BOOST_LINKS` default is `false`, matching current hardcoded behavior.

## Implementation Steps

1. **Add config fields** — In `src/config.rs`, add `search_decay_exclude: Vec<String>`, `search_decay_include: Vec<String>`, and `search_boost_links: bool` to the `Config` struct. Parse `search_decay_exclude` and `search_decay_include` from `MDVDB_SEARCH_DECAY_EXCLUDE` and `MDVDB_SEARCH_DECAY_INCLUDE` using the existing `parse_comma_list_string()` helper. Parse `search_boost_links` from `MDVDB_SEARCH_BOOST_LINKS` using `parse_env_bool()` with default `false`. Add all three to the constructor and `default_values_match_spec` test.

2. **Add SearchQuery fields and builders** — In `src/search.rs`, add `decay_exclude: Option<Vec<String>>` and `decay_include: Option<Vec<String>>` to `SearchQuery`. Add `with_decay_exclude()` and `with_decay_include()` builder methods. Default both to `None` in `SearchQuery::new()`.

3. **Add `should_apply_decay` helper** — In `src/search.rs`, add a pure function:
   ```rust
   fn should_apply_decay(path: &str, exclude: &[String], include: &[String]) -> bool {
       // Exclude takes precedence
       if exclude.iter().any(|p| path.starts_with(p.as_str())) {
           return false;
       }
       // If include is non-empty, path must match at least one
       if !include.is_empty() && !include.iter().any(|p| path.starts_with(p.as_str())) {
           return false;
       }
       true
   }
   ```

4. **Wire into `search()` and `assemble_results()`** — In the `search()` function, resolve the effective exclude/include lists (per-query overrides take priority over config). Pass them into `assemble_results()`. In `assemble_results()`, wrap the existing decay application:
   ```rust
   let effective_score = if decay_enabled
       && should_apply_decay(&chunk.source_path, &decay_exclude, &decay_include)
   {
       apply_time_decay(...)
   } else {
       *score
   };
   ```

5. **Wire `search_boost_links` config default** — In the `search()` function (or wherever `boost_links` is resolved), use `config.search_boost_links` as the default when `query.boost_links` is false and no CLI flag overrides it. Since `boost_links` is a plain `bool` (not `Option`), change it to `Option<bool>` in `SearchQuery` so `None` means "use config default". Update the CLI mapping: `--boost-links` sets `Some(true)`, `--no-boost-links` sets `Some(false)`, absence stays `None`.

6. **Add CLI flags** — In `src/main.rs`, add `--decay-exclude` and `--decay-include` to `SearchArgs` (comma-separated string args). Parse and wire to `SearchQuery` builders. Add `--no-boost-links` flag (conflicts with `--boost-links`).

7. **Update shell completions** — Add `--decay-exclude`, `--decay-include`, and `--no-boost-links` to bash, zsh, and fish completion strings in `completions()`.

8. **Add unit tests** — In `src/search.rs` `#[cfg(test)]` module:
   - `should_apply_decay` with empty lists → true
   - `should_apply_decay` with matching include → true
   - `should_apply_decay` with non-matching include → false
   - `should_apply_decay` with matching exclude → false
   - `should_apply_decay` with exclude overriding include → false
   - `should_apply_decay` with include-only, no exclude → correct filtering
   - `SearchQuery` builder tests for new methods

9. **Add integration tests** — In `tests/search_test.rs`:
   - Decay with exclude: excluded file retains original score, others decay
   - Decay with include: only included files decay, others retain score
   - Decay with both: exclude takes precedence over include
   - `search_boost_links` config default applies when CLI flag absent

10. **Add config tests** — In `src/config.rs` `#[cfg(test)]` module:
    - Verify `search_decay_exclude` and `search_decay_include` default to empty vec
    - Verify `search_boost_links` defaults to false
    - Verify comma parsing works for the new env vars

## Validation Criteria

- [ ] `cargo test` passes with zero failures
- [ ] `cargo clippy --all-targets` clean
- [ ] Empty exclude/include lists produce identical behavior to current (no regression)
- [ ] Excluded paths retain original scores when decay is enabled
- [ ] Included paths get decay applied; non-included paths retain original scores
- [ ] Exclude overrides include when both match the same path
- [ ] Per-query overrides take precedence over config defaults
- [ ] CLI `--decay-exclude` and `--decay-include` work correctly
- [ ] Config vars `MDVDB_SEARCH_DECAY_EXCLUDE` and `MDVDB_SEARCH_DECAY_INCLUDE` load correctly
- [ ] `MDVDB_SEARCH_BOOST_LINKS=true` enables link boosting by default
- [ ] `--no-boost-links` disables link boosting even when config enables it

## Anti-Patterns to Avoid

- **Do not use glob or regex** — Keep it as simple prefix matching via `starts_with()`, consistent with the existing `--path` filter. Adding regex complexity is unnecessary for path-based scoping.
- **Do not modify `apply_time_decay()`** — The decay math function stays pure. Path filtering is a separate concern that wraps the call.
- **Do not check patterns when decay is disabled** — The `should_apply_decay` call should be inside the `if decay_enabled` branch. No wasted work.
- **Do not use `unwrap()` in library code** — All new code must return `Result` or handle errors gracefully per project conventions.
- **Do not introduce global state** — Patterns flow through `Config` → `search()` → `assemble_results()` as parameters.

## Patterns to Follow

- **Config parsing**: Use `parse_comma_list_string()` in `src/config.rs` (line 294) — the exact same helper used for `MDVDB_IGNORE_PATTERNS`.
- **Config bool parsing**: Use `parse_env_bool()` in `src/config.rs` (line 270) — same as `MDVDB_SEARCH_DECAY`.
- **SearchQuery builder pattern**: Follow the existing `with_decay()` and `with_path_prefix()` methods in `src/search.rs` (lines 103-135).
- **Parameter threading**: Follow how `decay_enabled` and `decay_half_life` flow from `search()` into `assemble_results()` (lines 302-315, 361-362).
- **Integration test pattern**: Follow `test_decay_enabled_penalizes_old_files` in `tests/search_test.rs` which uses `populate_index_with_mtime()` to set up files with specific modification times.
- **CLI arg style**: Follow existing `--decay` / `--no-decay` / `--decay-half-life` args in `src/main.rs` (lines 163-173).
- **Conflicting flag pattern**: Follow `--decay` / `--no-decay` conflict style for `--boost-links` / `--no-boost-links`.

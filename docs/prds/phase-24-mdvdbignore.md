# PRD: Phase 24 — `.mdvdbignore` File Support

## Overview

Add support for a `.mdvdbignore` file that uses `.gitignore` syntax to exclude files from the mdvdb index. This complements the existing `.gitignore` respect and `MDVDB_IGNORE_PATTERNS` env var by giving users a project-local, version-controllable way to exclude files that should remain in git but not in search.

## Problem Statement

Users may have markdown files they want tracked in git but excluded from the vector index — drafts, templates, changelogs, meeting notes archives, or generated docs. Currently they must either:
1. Add patterns to `MDVDB_IGNORE_PATTERNS` env var (not project-portable, not version-controlled)
2. Add to `.gitignore` (removes from git too, which is undesirable)

A `.mdvdbignore` file solves this cleanly — same familiar syntax, lives in the project root, can be committed to git.

## Goals

- Support `.mdvdbignore` files using standard `.gitignore` syntax (globs, negation, comments, directory markers)
- Nested `.mdvdbignore` files in subdirectories apply to that subtree (same as `.gitignore` scoping)
- Works in both full discovery (`discover()`) and incremental watcher checks (`should_index()`)
- Composes with existing ignore sources: built-in dirs > `.gitignore` > `.mdvdbignore` > `MDVDB_IGNORE_PATTERNS`
- Zero config — just create the file and it works
- `.mdvdbignore` itself should never be indexed

## Non-Goals

- GUI or CLI command to manage `.mdvdbignore` entries (users edit the file directly)
- User-level ignore file at `~/.mdvdb/ignore` (can be added later)
- Overriding built-in ignores (the 15 hardcoded dirs are always excluded)
- Supporting non-`.gitignore` syntax (no regex, no extended globs beyond what `ignore` crate supports)

## Technical Design

### How It Works

The `ignore` crate (already a dependency) natively supports custom ignore filenames via `WalkBuilder::add_custom_ignore_filename()`. This means:
- The walker reads `.mdvdbignore` files exactly like `.gitignore` files
- Nested files scope to their directory subtree
- Negation patterns (`!important.md`) work
- Comments (`# explanation`) work
- No custom parsing code needed for discovery

For `should_index()` (used by the file watcher), we use `ignore::gitignore::Gitignore` to parse the `.mdvdbignore` file and check paths against it.

### Interface Changes

**`FileDiscovery` in `src/discovery.rs`:**

```rust
// In discover(), after creating WalkBuilder:
walker.add_custom_ignore_filename(".mdvdbignore");

// FileDiscovery struct gains a cached gitignore field:
mdvdb_ignore: Option<ignore::gitignore::Gitignore>,

// In should_index(), check against cached .mdvdbignore patterns
```

No new public API methods. No config changes. No CLI changes.

### Data Model Changes

None. This only affects which files enter the pipeline — no index format changes.

### Migration Strategy

None needed. Adding a `.mdvdbignore` file is opt-in. Existing projects without the file see zero behavior change.

## Implementation Steps

1. **Update `FileDiscovery::new()` in `src/discovery.rs`** — Parse `.mdvdbignore` from project root using `ignore::gitignore::Gitignore::new()`. Store the result as `Option<Gitignore>` in the struct. If the file doesn't exist, store `None`.

2. **Update `discover()` in `src/discovery.rs`** — After creating `WalkBuilder` (line ~63), call `.add_custom_ignore_filename(".mdvdbignore")`. This is a single method call that makes the walker automatically read and apply `.mdvdbignore` files during directory traversal.

3. **Update `should_index()` in `src/discovery.rs`** — After built-in and user-pattern checks, check the cached `mdvdb_ignore` (if `Some`). Call `gitignore.matched(path, is_dir).is_ignore()` to test whether the path matches any `.mdvdbignore` pattern. Return `false` if matched.

4. **Add integration tests in `tests/discovery_test.rs`:**
   - `discover_mdvdbignore`: Create `.mdvdbignore` with `drafts/` pattern, verify `drafts/note.md` excluded but `docs/note.md` included
   - `discover_mdvdbignore_with_negation`: Exclude `archive/` but `!archive/important.md` keeps one file
   - `discover_mdvdbignore_and_gitignore_compose`: Both files' patterns are applied together
   - `should_index_respects_mdvdbignore`: Watcher check also excludes `.mdvdbignore` patterns

5. **Add unit tests in `src/discovery.rs`:**
   - `should_index_with_mdvdbignore` — verify `should_index()` respects parsed `.mdvdbignore` patterns

6. **Update `CLAUDE.md`** — Add `.mdvdbignore` to the discovery description.

7. **Update `docs/prds/ROADMAP.md`** — Add Phase 24 entry.

## Files Modified

| File | Change |
|---|---|
| `src/discovery.rs` | Add `mdvdb_ignore` field, `.mdvdbignore` to `WalkBuilder`, update `should_index()` |
| `tests/discovery_test.rs` | 4 new integration tests |
| `docs/prds/phase-24-mdvdbignore.md` | This PRD |
| `docs/prds/ROADMAP.md` | Add Phase 24 |
| `CLAUDE.md` | Document `.mdvdbignore` |

## Validation Criteria

- [ ] `cargo test` passes — all existing + new tests
- [ ] `cargo clippy --all-targets` — zero warnings
- [ ] `.mdvdbignore` with `drafts/` excludes `drafts/*.md` from discovery
- [ ] `.mdvdbignore` with `*.draft.md` excludes matching files
- [ ] Negation patterns (`!keep-this.md`) work correctly
- [ ] Comments in `.mdvdbignore` are ignored
- [ ] Empty or missing `.mdvdbignore` causes no errors
- [ ] `.gitignore` + `.mdvdbignore` compose (both applied)
- [ ] `should_index()` (watcher) respects `.mdvdbignore`
- [ ] Built-in ignores still apply regardless of `.mdvdbignore`
- [ ] `MDVDB_IGNORE_PATTERNS` still applies alongside `.mdvdbignore`

## Anti-Patterns to Avoid

- **Do not write a custom gitignore parser** — The `ignore` crate handles all `.gitignore` syntax (globs, negation, comments, directory markers). Use `WalkBuilder::add_custom_ignore_filename()` for discovery and `ignore::gitignore::Gitignore` for `should_index()`.

- **Do not read `.mdvdbignore` in `Config`** — This is a discovery concern, not a config concern. The file is read by `FileDiscovery`, not by `Config::load()`.

- **Do not make `.mdvdbignore` override built-in ignores** — The 15 hardcoded directories (`.git/`, `node_modules/`, etc.) are always excluded regardless of negation patterns in `.mdvdbignore`.

- **Do not re-parse `.mdvdbignore` on every `should_index()` call** — Parse once when constructing `FileDiscovery` and cache the result.

## Patterns to Follow

- **`WalkBuilder` configuration:** See existing `discover()` in `src/discovery.rs:51-103` for how the walker is set up with overrides and standard filters.
- **`should_index()` pattern matching:** See existing built-in dir checks and user pattern checks in `src/discovery.rs:110-145`.
- **Test structure:** See `discover_gitignore` test in `tests/discovery_test.rs:103-119` for how `.gitignore` is tested (create `.git/` dir + ignore file + verify exclusion).
- **`ignore` crate gitignore API:** `ignore::gitignore::Gitignore::new(path)` returns `(Gitignore, Option<Error>)`. Call `gitignore.matched(path, is_dir).is_ignore()` to check.

# PRD: Phase 11 — Environment Variable & Config Loading

## Overview

Add `.env` file support as a fallback configuration source, so shared secrets like `OPENAI_API_KEY` can live in a standard `.env` file (gitignored) without being duplicated into `.markdownvdb`. The resolution priority becomes: **shell env > `.markdownvdb` > `.env` > built-in defaults**.

## Problem Statement

Users typically keep API keys in a `.env` file that is gitignored and shared across multiple tools (Docker Compose, direnv, application runtimes, etc.). Before this change, mdvdb only loaded configuration from `.markdownvdb` and shell environment variables. This forced users to either:

1. Duplicate their `OPENAI_API_KEY` into `.markdownvdb` (risking accidental commits of secrets), or
2. Manually export the key into their shell before running mdvdb.

Both options create friction, especially for teams where `.env` is the established secret-sharing mechanism.

## Goals

- Load `.env` file automatically as a fallback config source
- Maintain existing priority: shell env always wins, `.markdownvdb` overrides `.env`
- Zero breaking changes to existing configurations
- No new syntax or special interpolation — just standard dotenv loading

## Non-Goals

- Custom interpolation syntax (e.g., `${.env.VAR}`) — unnecessary given the fallback approach
- Loading `.env` files from parent directories or other locations
- Encrypted `.env` support or vault integration
- Modifying the `.env` file programmatically

## Technical Design

### Data Model Changes

None. The `Config` struct is unchanged. Only the loading order in `Config::load()` is modified.

### Interface Changes

`Config::load()` signature is unchanged. The only behavioral change is that it now reads from an additional file.

**Before:**
```
Config::load(project_root) → reads shell env + .markdownvdb + defaults
```

**After:**
```
Config::load(project_root) → reads shell env + .markdownvdb + .env + defaults
```

### New Commands / API / UI

None. This is a transparent enhancement to the config loading pipeline.

### Migration Strategy

Fully backward compatible. If no `.env` file exists, behavior is identical to before. If a `.env` file exists, its values are only used for variables not already set by the shell or `.markdownvdb`.

## Implementation Steps

1. **Add `.env` loading to `Config::load()`** — In `src/config.rs`, after the existing `dotenvy::from_path(project_root.join(".markdownvdb"))` call, add a second `dotenvy::from_path(project_root.join(".env"))` call. Since `dotenvy::from_path` does not override existing environment variables, loading `.markdownvdb` first ensures it takes priority over `.env`. Shell environment variables (set before either file is loaded) always win.

   ```rust
   // src/config.rs — Config::load()
   // Load .markdownvdb file first (ignore if missing).
   let _ = dotenvy::from_path(project_root.join(".markdownvdb"));
   // Load .env as a fallback for shared secrets (e.g., OPENAI_API_KEY).
   let _ = dotenvy::from_path(project_root.join(".env"));
   ```

2. **Update `.markdownvdb.example`** — Update the header comment to document the new priority order and add a tip on the `OPENAI_API_KEY` line suggesting users put secrets in `.env` instead.

   ```
   # Priority: shell environment > .markdownvdb > .env file > built-in defaults
   #
   # Shared secrets like OPENAI_API_KEY can live in a .env file (gitignored)
   # and will be picked up automatically — no need to duplicate them here.
   ```

3. **Add integration tests** — In `tests/config_test.rs`, add three tests:

   - `env_file_provides_fallback_values` — Create a `.markdownvdb` with mdvdb settings and a `.env` with `OPENAI_API_KEY`. Verify the API key is loaded from `.env`.
   - `markdownvdb_overrides_env_file` — Set `MDVDB_EMBEDDING_DIMENSIONS` in both files with different values. Verify `.markdownvdb` wins.
   - `shell_env_overrides_both_files` — Set a variable in `.env`, `.markdownvdb`, and shell env. Verify shell env wins.

4. **Verify no regressions** — Run `cargo test` and `cargo clippy --all-targets` to confirm all existing tests pass and no warnings are introduced.

## Validation Criteria

- [x] `Config::load()` reads values from `.env` when they are not set in `.markdownvdb` or shell env
- [x] `.markdownvdb` values take priority over `.env` values for the same variable
- [x] Shell environment variables take priority over both files
- [x] Missing `.env` file does not cause an error (existing `missing_dotenv_file_ok` test still passes)
- [x] `.markdownvdb.example` documents the new priority order
- [x] `cargo test` passes with zero failures
- [x] `cargo clippy --all-targets` passes with zero warnings

## Anti-Patterns to Avoid

- **Do not use `dotenvy::from_path_override`** for either file — this would allow file values to override shell environment variables, breaking the "shell always wins" invariant. Use `dotenvy::from_path` (non-override) for both files, relying on load order for priority.

- **Do not load `.env` before `.markdownvdb`** — `dotenvy::from_path` sets variables that aren't already in the environment. Loading `.env` first would give it priority over `.markdownvdb`, which is wrong. The mdvdb-specific config file must always win over the general-purpose `.env`.

- **Do not add variable interpolation syntax** — While `${VAR}` interpolation might seem useful, it adds complexity and is unnecessary. The fallback loading approach solves the actual problem (shared secrets) without any new syntax to learn or maintain.

- **Do not load `.env` from parent directories** — Only load from the project root (same directory as `.markdownvdb`). Walking up the directory tree would create surprising behavior and potential security issues.

## Patterns to Follow

- **Existing config loading in `src/config.rs`** — The new `.env` loading follows the exact same pattern as the existing `.markdownvdb` loading: `let _ = dotenvy::from_path(...)` with the error silently ignored via `let _ =`.

- **Test structure in `tests/config_test.rs`** — All config tests use `clear_env()` for isolation, `#[serial]` to prevent env var conflicts between tests, and `TempDir` for filesystem isolation. New tests follow this exact pattern.

- **Comment style in `Config::load()`** — Each file-loading line has a doc comment explaining what it does and why. The new `.env` line follows suit with a comment explaining it's a fallback for shared secrets.

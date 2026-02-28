# PRD: User-Level Settings (~/.mdvdb/config)

## Overview

Add a user-level configuration file at `~/.mdvdb/config` that serves as a machine-wide fallback for settings like API keys and default embedding provider. Project-level `.markdownvdb` files continue to override user-level settings, so per-client or per-project configuration still works. This eliminates the need to duplicate credentials in every project.

## Problem Statement

Currently, every mdvdb project requires its own `.markdownvdb` file (or shell env vars) to configure credentials like `OPENAI_API_KEY`. For users with many projects on the same machine, this means repeating the same API key in every project root. This is tedious, error-prone (keys get out of sync), and unnecessary for settings that are inherently machine-scoped (credentials, preferred provider).

## Goals

- Add `~/.mdvdb/config` as a fourth file source in the config cascade, below project files but above built-in defaults
- Add `mdvdb init --global` to create the user config file with commented-out defaults
- Add `mdvdb config` subcommand to display the resolved configuration
- Add `mdvdb doctor` diagnostic command to verify config, provider connectivity, and index integrity
- Maintain full backward compatibility — users who don't create `~/.mdvdb/config` see zero behavior change
- Allow any setting to be placed at any level (no setting restrictions)

## Non-Goals

- No GUI or interactive config editor
- No config provenance tracking (`--sources` flag showing where each value came from) — this can be a follow-up
- No config file format change — user config uses the same dotenv format as `.markdownvdb`
- No remote/team-shared config support
- No automatic migration of existing project configs to user config

## Technical Design

### Data Model Changes

No changes to the `Config` struct itself. The struct remains identical; only the loading logic changes.

**New resolution order:**

```
shell env > project .markdownvdb > project .env > user ~/.mdvdb/config > built-in defaults
```

This works because `dotenvy::from_path()` does NOT override existing env vars. Files loaded earlier set env vars first, so they take priority over files loaded later. Adding `~/.mdvdb/config` as a fourth `from_path` call is the entire core change.

### Interface Changes

**`Config::load()` in `src/config.rs`** — Add one block after `.env` loading:

```rust
// Load user-level config (~/.mdvdb/config) as lowest-priority file source.
if std::env::var("MDVDB_NO_USER_CONFIG").is_err() {
    if let Some(config_dir) = user_config_dir() {
        let _ = dotenvy::from_path(config_dir.join("config"));
    }
}
```

**New helper `user_config_dir()` in `src/config.rs`:**

```rust
/// Resolve the user-level config directory.
/// Priority: MDVDB_CONFIG_HOME env var > dirs::home_dir()/.mdvdb
fn user_config_dir() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("MDVDB_CONFIG_HOME") {
        if !custom.is_empty() {
            return Some(PathBuf::from(custom));
        }
    }
    dirs::home_dir().map(|h| h.join(".mdvdb"))
}
```

**New public method `Config::user_config_path()`:**

```rust
pub fn user_config_path() -> Option<PathBuf> {
    user_config_dir().map(|d| d.join("config"))
}
```

### New Commands / API / UI

**`mdvdb init --global`** — Creates `~/.mdvdb/config` with credential-focused commented-out defaults. Returns `ConfigAlreadyExists` if the file already exists. Creates `~/.mdvdb/` directory if needed.

**`mdvdb config [--json]`** — Prints the fully resolved configuration. In JSON mode, serializes the `Config` struct. In human mode, prints labeled key-value pairs.

**`mdvdb doctor [--json]`** — Runs a suite of diagnostic checks and reports pass/fail for each. Checks include:
1. **Config resolution** — Can config load without errors? Show resolved provider, model, dimensions.
2. **User config** — Does `~/.mdvdb/config` exist? Is it readable?
3. **Project config** — Does `.markdownvdb` directory exist? Is there a config file?
4. **API key present** — Is `OPENAI_API_KEY` (or relevant provider key) set?
5. **Provider connectivity** — Can we reach the embedding API? Send a minimal test embedding request (single word like "test") and verify a vector comes back. Timeout after 5 seconds.
6. **Index integrity** — Does the index file exist? Can it be opened? Do vector count and chunk count match? Are HNSW keys consistent with metadata?
7. **Source directories** — Do configured source dirs exist and contain `.md` files?

Output format (human):
```
  ● mdvdb doctor

  ✓ Config loaded                  openai / text-embedding-3-small / 1536
  ✓ User config                    ~/.mdvdb/config
  ✗ Project config                 .markdownvdb not found
  ✓ API key                        OPENAI_API_KEY is set
  ✓ Provider reachable             200 OK (124ms)
  ✓ Index                          42 docs, 128 chunks, 128 vectors
  ✓ Source directories             docs/ notes/ (67 .md files)

  6/7 checks passed
```

JSON mode outputs a `DoctorResult` struct with per-check status and details.

**`MarkdownVdb::doctor()`** — New async method in `src/lib.rs` that runs all checks and returns `DoctorResult`. The provider connectivity check reuses the existing `EmbeddingProvider::embed_batch()` with a single-item input.

**`MarkdownVdb::init_global(path)`** — New static method in `src/lib.rs` that creates the user config file at the given path.

### Migration Strategy

No migration needed. This is purely additive:
- Existing projects with `.markdownvdb` continue to work identically
- Users who never create `~/.mdvdb/config` see no change
- `dotenvy::from_path` silently ignores missing files (existing pattern)

**New env vars:**
- `MDVDB_CONFIG_HOME` — Override the user config directory (primarily for testing, also useful for non-standard setups)
- `MDVDB_NO_USER_CONFIG` — Set to any value to disable loading user config entirely

## Implementation Steps

1. **Add `dirs` dependency to `Cargo.toml`** — Add `dirs = "6"` to `[dependencies]`. This lightweight crate provides cross-platform `home_dir()`. `std::env::home_dir()` is deprecated since Rust 1.29.

2. **Add `user_config_dir()` helper and `user_config_path()` method to `src/config.rs`** — The private `user_config_dir()` function checks `MDVDB_CONFIG_HOME` first, then falls back to `dirs::home_dir().join(".mdvdb")`. The public `Config::user_config_path()` method appends `"config"` to the directory for use by `init --global` and `config` commands.

3. **Modify `Config::load()` in `src/config.rs`** — After the existing `.env` loading line and before the first `env_or_default()` call, add the user config loading block. Check `MDVDB_NO_USER_CONFIG` first, then call `dotenvy::from_path(config_dir.join("config"))` with `let _ =` to ignore errors (matching the existing pattern for `.markdownvdb` and `.env`).

4. **Add `--global` flag to `InitArgs` in `src/main.rs`** — Add `#[arg(long)] global: bool` to the existing `InitArgs` struct. In the `Init` match arm, branch: if `--global`, resolve `Config::user_config_path()`, call `MarkdownVdb::init_global()`, and print success via `format::print_init_global_success()`. Otherwise, existing `init` behavior unchanged.

5. **Add `MarkdownVdb::init_global()` to `src/lib.rs`** — Static method that takes a `&Path` for the config file location. Creates parent directories with `fs::create_dir_all()`. Writes a template focused on credentials and shared defaults (all values commented out). Returns `Error::ConfigAlreadyExists` if file exists. Template content:
   ```
   # mdvdb user-level configuration
   # Values here apply to all projects unless overridden by project .markdownvdb

   # API credentials
   # OPENAI_API_KEY=sk-...

   # Default embedding provider
   # MDVDB_EMBEDDING_PROVIDER=openai
   # MDVDB_EMBEDDING_MODEL=text-embedding-3-small
   # MDVDB_EMBEDDING_DIMENSIONS=1536

   # Ollama host (if using Ollama)
   # OLLAMA_HOST=http://localhost:11434
   ```

6. **Add `Config` subcommand to `src/main.rs`** — New `Config(ConfigArgs)` variant in `Commands` enum. `ConfigArgs` has `#[arg(long)] json: bool`. In the handler: if `--json`, serialize `config` with `serde_json::to_writer_pretty`. Otherwise, call `format::print_config()`. Note: this command needs the config but does NOT need to open the index, so it should load config and print before the `MarkdownVdb::open_with_config` call. Restructure the match so that `Config` and `Init` are handled before the `vdb` is opened.

7. **Add `print_config()` and `print_init_global_success()` to `src/format.rs`** — `print_config()` displays all resolved fields in the existing colored label/value style (matching `print_status()`). Show the user config path at the bottom if resolvable. `print_init_global_success()` follows the `print_init_success()` pattern with a green checkmark, the config path, and a one-line explanation.

8. **Add `Doctor` subcommand to `src/main.rs`** — New `Doctor(DoctorArgs)` variant in `Commands` enum. `DoctorArgs` has `#[arg(long)] json: bool`. The handler calls `vdb.doctor().await?` and prints via `format::print_doctor()` or JSON serialize. Note: `doctor` needs an open VDB to check the index, but should also handle the case where the index doesn't exist (report as a finding, not an error).

9. **Add `DoctorResult`, `DoctorCheck`, `CheckStatus` types to `src/lib.rs`** — Define the result types:
   ```rust
   #[derive(Debug, Clone, Serialize)]
   pub struct DoctorResult {
       pub checks: Vec<DoctorCheck>,
       pub passed: usize,
       pub total: usize,
   }

   #[derive(Debug, Clone, Serialize)]
   pub struct DoctorCheck {
       pub name: String,
       pub status: CheckStatus,
       pub detail: String,
   }

   #[derive(Debug, Clone, Serialize)]
   pub enum CheckStatus {
       Pass,
       Fail,
       Warn,
   }
   ```

10. **Implement `MarkdownVdb::doctor()` in `src/lib.rs`** — Async method that runs each check sequentially:
    - Config check: always passes (config already loaded at this point), report provider/model/dimensions
    - User config: check `Config::user_config_path()` existence
    - Project config: check `.markdownvdb` directory existence
    - API key: check relevant env var based on provider type (skip for Mock provider)
    - Provider connectivity: `provider.embed_batch(&["test".to_string()])` with a 5-second `tokio::time::timeout`. Report latency on success. Mark as `Warn` (not Fail) for Mock provider.
    - Index integrity: check vector count matches chunk count in metadata. Check HNSW key consistency.
    - Source dirs: check each configured source dir exists, count `.md` files via `discovery::discover_files()`

11. **Add `print_doctor()` to `src/format.rs`** — Format each check as a colored line: green `✓` for Pass, red `✗` for Fail, yellow `!` for Warn. Summary line at the bottom with pass/total count.

12. **Update shell completion scripts in `src/main.rs`** — Add `config` and `doctor` to the bash, zsh, fish, and PowerShell completion scripts alongside the existing commands.

13. **Add unit tests to `src/config.rs`** — Test `user_config_dir()` with `MDVDB_CONFIG_HOME` set, unset, and empty. Test that `MDVDB_NO_USER_CONFIG` prevents loading. All tests use `ENV_MUTEX` (existing pattern).

14. **Add integration tests to `tests/config_test.rs`** — Key test cases:
    - User config provides fallback values (set `MDVDB_CONFIG_HOME` to tempdir, write config, verify values load)
    - Project `.markdownvdb` overrides user config (both set different values for same key, project wins)
    - Shell env overrides user config
    - `.env` overrides user config
    - Missing user config dir is silently skipped
    - `MDVDB_NO_USER_CONFIG` skips user config
    - Full four-level cascade (different keys from each source, all resolve correctly)
    All tests clear `MDVDB_CONFIG_HOME` and `MDVDB_NO_USER_CONFIG` in addition to the existing env var cleanup list.

15. **Add CLI integration tests to `tests/cli_test.rs`** — Key test cases:
    - `mdvdb init --global` creates config file at `MDVDB_CONFIG_HOME/config`
    - `mdvdb init --global` twice fails with error
    - `mdvdb config --json` outputs valid JSON with expected fields
    - `mdvdb config` (human mode) contains expected labels
    - `mdvdb doctor --json` outputs valid JSON with checks array
    - `mdvdb doctor` shows pass/fail lines for each check
    - CLI process inherits `MDVDB_CONFIG_HOME` and loads user config correctly

16. **Add doctor API tests to `tests/api_test.rs`** — Key test cases:
    - `doctor()` with mock provider returns all Pass/Warn (no Fail since mock needs no API key)
    - `doctor()` reports correct document/chunk counts after ingest
    - `doctor()` reports missing source dirs as Fail

17. **Update documentation** — Update the `Config::load()` doc comment to document the new four-file resolution order and the two new env vars (`MDVDB_CONFIG_HOME`, `MDVDB_NO_USER_CONFIG`).

## Validation Criteria

- [ ] `cargo test` passes with zero failures (all existing + new tests)
- [ ] `cargo clippy --all-targets` passes with zero warnings
- [ ] `mdvdb init --global` creates `~/.mdvdb/config` with correct content
- [ ] `mdvdb init --global` a second time returns `ConfigAlreadyExists` error
- [ ] `mdvdb config --json` outputs valid JSON matching the `Config` struct
- [ ] `mdvdb config` shows human-readable output with all fields
- [ ] API key set only in `~/.mdvdb/config` is picked up by `mdvdb ingest` in a project with no `.markdownvdb`
- [ ] API key set in project `.markdownvdb` overrides the one in `~/.mdvdb/config`
- [ ] Shell env var overrides both file sources
- [ ] `MDVDB_NO_USER_CONFIG=1` prevents user config from loading
- [ ] Missing `~/.mdvdb/` directory does not cause errors
- [ ] Existing tests continue to pass (no env var pollution from user config)
- [ ] `mdvdb doctor` runs all checks and reports pass/fail with details
- [ ] `mdvdb doctor --json` outputs valid JSON matching `DoctorResult` struct
- [ ] Doctor reports `Fail` for unreachable provider (invalid API key)
- [ ] Doctor reports `Pass` for valid, fully-configured setup after ingest
- [ ] Doctor reports correct vector/chunk count consistency

## Anti-Patterns to Avoid

- **Do NOT change the Config struct** — The resolution happens at the `dotenvy::from_path` level, not in the struct. Adding a "user config" field or separate loading path is unnecessary complexity.
- **Do NOT restrict which settings can be in user config** — All settings use the same dotenv format. Artificially restricting which keys are "allowed" at the user level adds complexity for no benefit.
- **Do NOT use `std::env::home_dir()`** — It's deprecated since Rust 1.29 and behaves incorrectly on Windows. Use the `dirs` crate.
- **Do NOT create `~/.mdvdb/` directory automatically during config loading** — Only create it when the user explicitly runs `mdvdb init --global`. The loader should silently skip missing paths (existing `let _ =` pattern).
- **Do NOT load user config before project config** — The priority order is critical. Project files must be loaded first so their values take precedence via dotenvy's no-override behavior.
- **Do NOT add a CLI flag for `--no-user-config`** — CLI flags are parsed after config loading in the current architecture. Use the `MDVDB_NO_USER_CONFIG` env var instead, which is checked during `Config::load()`.

## Patterns to Follow

- **dotenvy ignore pattern** (`src/config.rs:62-66`) — Use `let _ = dotenvy::from_path(...)` to silently ignore missing files. This is the existing pattern for `.markdownvdb` and `.env`.
- **Init method pattern** (`src/lib.rs:473-517`) — `MarkdownVdb::init()` checks for existing file, writes a template, logs with tracing. `init_global()` should follow the exact same structure with `create_dir_all` added for the parent directory.
- **CLI subcommand pattern** (`src/main.rs:47-78`) — Each command has an `Args` struct with `#[derive(Parser)]` and optional `--json` flag. The handler branches on `args.json` for output format.
- **Format function pattern** (`src/format.rs:293-334`) — `print_status()` shows the pattern: section header with cyan bullet, labeled fields with colored values, trailing newline. `print_config()` should follow this style.
- **Init success pattern** (`src/format.rs:533-549`) — `print_init_success()` shows green checkmark, path, and one-line guidance. `print_init_global_success()` should mirror this.
- **Test env cleanup pattern** (`src/config.rs:256-280`) — Tests clear all MDVDB env vars before running. New tests must add `MDVDB_CONFIG_HOME` and `MDVDB_NO_USER_CONFIG` to the cleanup list.
- **CLI test pattern** (`tests/cli_test.rs`) — Uses `Command::new(env!("CARGO_BIN_EXE_mdvdb"))` with `.env("key", "value")` for env var injection. Use `.env("MDVDB_CONFIG_HOME", tempdir.path())` to control user config location in tests.

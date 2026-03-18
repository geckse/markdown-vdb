# PRD: Phase 26 — CLI Releases, Install Script, and Update Check

## Overview

Add cross-platform release builds, a one-line installer, and automatic update notifications for the `mdvdb` CLI. Currently users must compile from source with `cargo install --path .`, which requires a Rust toolchain. This phase makes the CLI installable in seconds and self-aware of new versions.

## Problem Statement

The `mdvdb` CLI has no distribution pipeline:
1. **No pre-built binaries** — users need Rust installed to compile from source, which is a major adoption barrier
2. **No quick install** — there's no `curl | sh` one-liner; users must clone the repo and build manually
3. **No update awareness** — users have no way to know when a new version is available unless they check GitHub manually

The Electron desktop app already has CI/CD (`.github/workflows/build-app.yml` on `app-v*` tags with GitHub Releases), but the CLI has nothing comparable.

## Goals

- Build and publish CLI binaries for macOS (ARM64 + x86_64), Linux (x86_64 + ARM64), and Windows (x86_64) on every release tag
- Provide a POSIX shell install script that detects OS/arch and downloads the right binary
- Check for updates in the background on CLI invocation, cached to avoid API spam, with a colored notice on stderr
- Never block, slow down, or break the CLI — update checks are best-effort

## Non-Goals

- Homebrew formula, apt/rpm packages, or other package manager distribution (can be added later)
- Windows install script (Windows users download the `.zip` manually or use `cargo install`)
- Code signing for CLI binaries (Electron app has signing; CLI can add it later)
- Auto-update / self-update (user must re-run the install script or download manually)
- Changelog generation or release notes authoring

## Technical Design

### Part A: GitHub Actions Workflow

**File:** `.github/workflows/release-cli.yml`

**Trigger:** `push.tags: ['v*']` — distinct from the Electron app's `app-v*` trigger.

**Architecture:** Two-phase workflow:
1. `create-release` job creates the GitHub Release (or finds existing one)
2. `build` matrix jobs compile for each target and upload assets

**Build matrix:**

| Target | Runner | Build Method |
|--------|--------|-------------|
| `aarch64-apple-darwin` | `macos-latest` | native (ARM64 runner) |
| `x86_64-apple-darwin` | `macos-latest` | `rustup target add` + `--target` flag |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | native |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest` | `cross` (Docker-based cross-compilation) |
| `x86_64-pc-windows-msvc` | `windows-latest` | native |

**Artifact naming:** `mdvdb-{tag}-{target}.tar.gz` (unix) or `.zip` (windows)

**Key tools:**
- `dtolnay/rust-toolchain@stable` for Rust installation
- `cross` (installed via cargo) for Linux ARM64 only
- `softprops/action-gh-release@v2` for release creation and asset upload
- `GITHUB_TOKEN` (auto-available, no additional secrets needed)

**Packaging steps (per target):**
- Unix: `tar -czf mdvdb-{tag}-{target}.tar.gz -C target/{target}/release mdvdb`
- Windows: `7z a mdvdb-{tag}-{target}.zip target/{target}/release/mdvdb.exe`

### Part B: Install Script

**File:** `install.sh` (project root)

**Usage:** `curl -fsSL https://raw.githubusercontent.com/geckse/markdown-vdb/main/install.sh | sh`

**Design constraints:**
- POSIX shell (`#!/bin/sh`) — no bashisms, works on dash/ash/zsh
- `set -eu` for strict error handling
- Dependencies: only `curl`, `tar`, `uname`, `grep`, `sed` (standard on macOS and Linux)
- No `jq` dependency — parse GitHub API JSON with grep/sed

**Logic flow:**
1. Detect OS via `uname -s` → map `Darwin` → `apple-darwin`, `Linux` → `unknown-linux-gnu`
2. Detect arch via `uname -m` → map `x86_64`/`amd64` → `x86_64`, `arm64`/`aarch64` → `aarch64`
3. Construct target triple: `{arch}-{os}`
4. Fetch latest release tag: `GET https://api.github.com/repos/geckse/markdown-vdb/releases/latest` → extract `tag_name`
5. Construct download URL: `https://github.com/geckse/markdown-vdb/releases/download/{tag}/mdvdb-{tag}-{target}.tar.gz`
6. Download to temp dir (`mktemp -d` with `trap` cleanup)
7. Extract and move binary to `$INSTALL_DIR` (default: `/usr/local/bin`)
8. `chmod +x` and verify with `mdvdb --version`

**Customization:** `INSTALL_DIR=/custom/path curl -fsSL ... | sh`

### Part C: Update Check Module

**New file:** `src/update.rs`

**New dependency:** `semver = "1"` in Cargo.toml (only new crate; `reqwest`, `dirs`, `colored`, `serde_json` already present)

#### How It Works

1. On every CLI invocation, `run()` in `main.rs` spawns a background tokio task via `update::spawn_check()`
2. The task checks a cache file at `~/.mdvdb/last-update-check` (same `~/.mdvdb/` directory used by user config in `src/config.rs:117`)
3. If cache is less than 24 hours old, use the cached version; otherwise hit the GitHub API
4. API call: `GET https://api.github.com/repos/geckse/markdown-vdb/releases/latest` with 5-second timeout
5. Parse `tag_name` from JSON response, strip `v` prefix, compare with `env!("CARGO_PKG_VERSION")` using `semver::Version`
6. Write result to cache file (unix timestamp + version, two lines)
7. If newer version exists, return a colored message; otherwise return `None`
8. Back in `run()`, after the command completes, await the handle and print any message to stderr

#### API

```rust
/// Spawn a non-blocking update check. Returns JoinHandle<Option<String>>.
/// All errors silently swallowed — never causes CLI failures.
pub fn spawn_check() -> JoinHandle<Option<String>>;
```

Internal functions (not public):
- `async fn check_for_update() -> Result<Option<String>, Box<dyn Error + Send + Sync>>`
- `fn cache_path() -> Option<PathBuf>` — returns `~/.mdvdb/last-update-check`
- `fn format_update_message(latest: &str) -> Option<String>` — semver comparison + colored output

#### Cache File Format

`~/.mdvdb/last-update-check`:
```
1710700000
1.2.3
```
Line 1: unix timestamp of last check. Line 2: latest version found. Intentionally simple — no serde, just `split_once('\n')`.

#### Opt-Out

Set `MDVDB_NO_UPDATE_CHECK=1` to disable. Checked first in `check_for_update()` before any I/O.

#### Output Example

After command output completes:
```
Update available: 0.1.0 -> 0.2.0 (run `curl -fsSL https://raw.githubusercontent.com/geckse/markdown-vdb/main/install.sh | sh` to update)
```
Printed to stderr in yellow, never interferes with stdout JSON output.

### Interface Changes

**`src/main.rs`:**
- Add `mod update;` declaration alongside existing `mod format;`
- In `run()`, after `Cli::parse()`: `let update_handle = update::spawn_check();`
- At end of `run()`, before `Ok(())`: await handle, print message if `Some`

**No changes to the public library API (`lib.rs`)**. The update check is CLI-only behavior.

### Data Model Changes

None. No index format changes.

### Migration Strategy

None needed. All three features are additive:
- The workflow only triggers on new `v*` tags
- The install script is a new file
- The update check gracefully handles missing cache files

## Implementation Steps

1. **Add `semver` dependency** — Edit `Cargo.toml`, add `semver = "1"` to `[dependencies]`

2. **Create `src/update.rs`** — Implement `spawn_check()`, `check_for_update()`, `cache_path()`, `format_update_message()` with cache logic and GitHub API call. Include unit tests for version comparison.

3. **Integrate update check in `src/main.rs`** — Add `mod update;`, spawn check early in `run()`, await and print after command completes.

4. **Create `.github/workflows/release-cli.yml`** — Two-phase workflow: create release, then matrix build for 5 targets with asset upload.

5. **Create `install.sh`** — POSIX shell installer at project root, mark executable.

6. **Run verification** — `cargo test`, `cargo clippy --all-targets`, manual `mdvdb --version` test.

## Files Modified

| File | Change |
|---|---|
| `Cargo.toml` | Add `semver = "1"` dependency |
| `src/update.rs` | New module: background update checker with cache and semver comparison |
| `src/main.rs` | Add `mod update;`, spawn/await update check in `run()` |
| `.github/workflows/release-cli.yml` | New workflow: multi-platform CLI release builds |
| `install.sh` | New file: POSIX shell installer script |
| `docs/prds/phase-26-cli-releases-install-update.md` | This PRD |

## Validation Criteria

- [ ] `cargo test` passes — all existing + new update module tests
- [ ] `cargo clippy --all-targets` — zero warnings
- [ ] `mdvdb --version` still prints logo + version correctly
- [ ] Unit tests: `format_update_message` handles current version (no update), newer version (shows update), older version (no update), invalid semver (no update)
- [ ] `MDVDB_NO_UPDATE_CHECK=1 mdvdb status` produces no update check
- [ ] Update check never blocks or slows down command execution
- [ ] Update check failures (network down, API error) are silently ignored
- [ ] Cache file created at `~/.mdvdb/last-update-check` after first check
- [ ] Cache prevents API calls within 24 hours
- [ ] `install.sh` passes `shellcheck`
- [ ] GitHub workflow syntax is valid YAML
- [ ] Workflow triggers on `v*` tags but NOT on `app-v*` tags

## Anti-Patterns to Avoid

- **Do not block the CLI on update checks** — The check runs as a background tokio task. The main command executes immediately regardless of network conditions.

- **Do not add update check to the library API** — This is CLI-only behavior. `src/lib.rs` and `MarkdownVdb` should not know about update checking.

- **Do not use `jq` in the install script** — It's not installed by default on many systems. Use `grep`/`sed` to parse the single `tag_name` field from the JSON response.

- **Do not make update check errors visible** — All errors are swallowed with `unwrap_or(None)`. Users should never see network errors, parse failures, or cache write errors from the update checker.

- **Do not use `cross` for macOS x86_64** — macOS runners can cross-compile to x86_64 natively via `rustup target add`. `cross` is only needed for Linux ARM64 which requires a different toolchain/linker.

## Patterns to Follow

- **`reqwest::Client` usage:** See `src/embedding/openai.rs` for existing HTTP client patterns with proper error handling.
- **`dirs::home_dir()` usage:** See `src/config.rs:117` for the existing pattern of locating `~/.mdvdb/`.
- **GitHub Actions:** See `.github/workflows/build-app.yml` for the existing Electron app build pattern (matrix strategy, GitHub token, `--publish always`).
- **stderr for notices:** The CLI already uses stderr for logs and errors, stdout for data. Update notices go to stderr.
- **Colored output:** Use the `colored` crate (already a dependency) with `.yellow()` for the update notice, consistent with existing format.rs patterns.

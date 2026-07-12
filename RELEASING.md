# Releasing

This project ships two independently versioned artifacts from **two separate repositories**:

| Artifact | Repository | Tag pattern | Workflow | Outputs |
|----------|------------|-------------|----------|---------|
| **CLI** (`mdvdb`) | `geckse/markdown-vdb` (this repo) | `v*` (e.g. `v0.2.0`) | `.github/workflows/release-cli.yml` | Binaries for macOS, Linux, Windows |
| **Desktop App** (Tesseract) | `geckse/tesseract-md-app` (the `app/` submodule) | `v*` (e.g. `v0.2.0`) | `app/.github/workflows/build-app.yml` | DMG + ZIP, NSIS installer, AppImage + deb |

Each workflow triggers on tags pushed to **its own repo**. Tagging the parent repo never triggers an app build; the old `app-v*` tag scheme is dead — the app workflow only matches `v*` on `geckse/tesseract-md-app`.

CLI and app versions are independent [semver](https://semver.org/) — they don't need to match. The CLI version lives in `Cargo.toml`, the app version in `app/package.json`.

---

## CLI Release (`geckse/markdown-vdb`)

### 0. Preconditions

- The `Test` workflow (`.github/workflows/test.yml`) must be **green on `main`**, in particular the **`windows-latest` leg** (`cargo test --no-default-features` with the MSVC `MAP_FAILED` workaround) — Windows is the most fragile target and the release build uses the same flags. This is a hard precondition for the v0.2.0 release.
- `cargo clippy --all-targets -- -D warnings` clean locally.

### 1. Bump the version

Edit `Cargo.toml`:

```toml
[package]
version = "0.2.0"  # ← update this
```

### 2. Commit, tag, push

```bash
git add Cargo.toml Cargo.lock
git commit -m "release: v0.2.0"
git tag v0.2.0
git push origin main --tags
```

### 3. What happens next

The `release-cli.yml` workflow creates a GitHub Release (auto-generated release notes) and builds the `mdvdb` binary for 5 targets in parallel:

| Target | Runner | Method |
|--------|--------|--------|
| `aarch64-apple-darwin` (macOS ARM64) | `macos-latest` | native |
| `x86_64-apple-darwin` (macOS Intel) | `macos-latest` | cross-target |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | native |
| `aarch64-unknown-linux-gnu` (Linux ARM64) | `ubuntu-latest` | `cross` |
| `x86_64-pc-windows-msvc` | `windows-latest` | native |

### 4. Dual asset layout (CONTRACT — do not rename)

Every target uploads **two** assets:

1. **Versioned archive** — consumed by `install.sh` (`mdvdb-<tag>-<target>.tar.gz` / `.zip`):

   ```
   mdvdb-v0.2.0-aarch64-apple-darwin.tar.gz
   mdvdb-v0.2.0-x86_64-apple-darwin.tar.gz
   mdvdb-v0.2.0-x86_64-unknown-linux-gnu.tar.gz
   mdvdb-v0.2.0-aarch64-unknown-linux-gnu.tar.gz
   mdvdb-v0.2.0-x86_64-pc-windows-msvc.zip
   ```

2. **Raw unversioned binary** — consumed by the desktop app's in-app CLI installer:

   ```
   mdvdb-aarch64-apple-darwin
   mdvdb-x86_64-apple-darwin
   mdvdb-x86_64-unknown-linux-gnu
   mdvdb-aarch64-unknown-linux-gnu
   mdvdb-x86_64-pc-windows-msvc.exe
   ```

   These names are a **contract** with `app/src/main/cli-install.ts` `getAssetName()`, which looks them up by exact name on the *latest* GitHub release. Renaming them (or dropping one) silently breaks the app's "Install CLI" flow. Change both sides together or not at all.

### 5. Users install or update

```bash
curl -fsSL https://raw.githubusercontent.com/geckse/markdown-vdb/main/install.sh | sh
```

Users running an older version see an update notice within 24 hours (disable with `MDVDB_NO_UPDATE_CHECK=1`). App users install/update via the in-app installer, which downloads the raw binary directly.

---

## Desktop App Release (`geckse/tesseract-md-app`)

The app is a git submodule at `app/` with its **own origin** (`github.com/geckse/tesseract-md-app`). Release commits and tags happen **inside the submodule** and are pushed to the app repo — then the parent repo records the new submodule pointer. The `/publish-app` command (`.claude/commands/publish-app.md`) automates this flow.

### 1. Preflight

`git -C app status --porcelain` must be empty and the submodule must be on `main` (not detached HEAD).

### 2. Bump, commit, tag, push (inside the submodule)

```bash
# edit app/package.json "version" first
git -C app add package.json
git -C app commit -m "chore: bump version to 0.2.0"
git -C app tag v0.2.0
git -C app push origin main
git -C app push origin v0.2.0
```

### 3. What happens next

`build-app.yml` in the app repo builds on 3 platforms in parallel and publishes everything to a **draft** GitHub release on `geckse/tesseract-md-app`:

| Platform | Runner | Artifacts | Signing |
|----------|--------|-----------|---------|
| macOS | `macos-latest` | DMG + ZIP | signed + notarized |
| Windows | `windows-latest` | NSIS installer + ZIP | **unsigned** (beta) |
| Linux | `ubuntu-latest` | AppImage + .deb | n/a |

### 4. Publish the draft

After all three matrix jobs succeed:

```bash
gh release edit v0.2.0 -R geckse/tesseract-md-app --draft=false --generate-notes
```

### 5. Bump the submodule pointer in the parent repo

```bash
git add app
git commit -m "chore: bump app submodule to v0.2.0"
git push
```

### Secrets (configured in `geckse/tesseract-md-app`, mac job only)

| Secret | Purpose |
|--------|---------|
| `CSC_LINK` | Base64-encoded Developer ID Application certificate |
| `CSC_KEY_PASSWORD` | Certificate password |
| `APPLE_ID` | Apple Developer account email |
| `APPLE_APP_SPECIFIC_PASSWORD` | App-specific password (generate at appleid.apple.com) |
| `APPLE_TEAM_ID` | Apple Developer Team ID |

Without these, macOS builds are unsigned/unnotarized and trigger Gatekeeper warnings on macOS Sequoia+.

### Windows: unsigned for beta

Windows builds are intentionally **not code-signed** during the beta. SmartScreen will warn on first run — users click **"More info" → "Run anyway"**. Revisit (Authenticode or Azure Trusted Signing) before a stable release.

---

## Pre-release checklist

```bash
# CLI (this repo)
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
# + Test workflow green on main, including windows-latest

# App (inside app/)
npm ci
npm run typecheck
npm test
npm run lint
npm run build
```

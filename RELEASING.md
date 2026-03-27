# Releasing

This project has two independently versioned release artifacts:

| Artifact | Tag pattern | Workflow | Outputs |
|----------|-------------|----------|---------|
| **CLI** (`mdvdb`) | `v*` (e.g. `v0.2.0`) | `release-cli.yml` | Binaries for macOS, Linux, Windows |
| **Desktop App** (Tesseract) | `app-v*` (e.g. `app-v0.2.0`) | `build-app.yml` | DMG, NSIS installer, AppImage, deb |

Both publish to **GitHub Releases** automatically when you push a tag.

---

## CLI Release

### 1. Bump the version

Edit `Cargo.toml`:

```toml
[package]
version = "0.2.0"  # ← update this
```

### 2. Commit and tag

```bash
git add Cargo.toml Cargo.lock
git commit -m "release: v0.2.0"
git tag v0.2.0
```

### 3. Push

```bash
git push origin main --tags
```

### 4. What happens next

The `release-cli.yml` workflow:

1. Creates a GitHub Release with auto-generated release notes
2. Builds the `mdvdb` binary for 5 targets in parallel:

| Target | Runner | Method |
|--------|--------|--------|
| `aarch64-apple-darwin` (macOS ARM64) | `macos-latest` | native |
| `x86_64-apple-darwin` (macOS Intel) | `macos-latest` | cross-target |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | native |
| `aarch64-unknown-linux-gnu` (Linux ARM64) | `ubuntu-latest` | `cross` |
| `x86_64-pc-windows-msvc` | `windows-latest` | native |

3. Uploads release assets:

```
mdvdb-v0.2.0-aarch64-apple-darwin.tar.gz
mdvdb-v0.2.0-x86_64-apple-darwin.tar.gz
mdvdb-v0.2.0-x86_64-unknown-linux-gnu.tar.gz
mdvdb-v0.2.0-aarch64-unknown-linux-gnu.tar.gz
mdvdb-v0.2.0-x86_64-pc-windows-msvc.zip
```

### 5. Users install or update

```bash
curl -fsSL https://raw.githubusercontent.com/geckse/markdown-vdb/main/install.sh | sh
```

Users running an older version will see a notice within 24 hours:

```
Update available: 0.1.0 → 0.2.0 (run `curl -fsSL ... | sh` to update)
```

The update check can be disabled with `MDVDB_NO_UPDATE_CHECK=1`.

---

## Desktop App Release (Tesseract)

### 1. Bump the version

Edit `app/package.json`:

```json
{
  "version": "0.2.0"
}
```

### 2. Commit and tag

```bash
git add app/package.json
git commit -m "release: app-v0.2.0"
git tag app-v0.2.0
```

### 3. Push

```bash
git push origin main --tags
```

### 4. What happens next

The `build-app.yml` workflow:

1. Builds the Electron app on 3 platforms in parallel:

| Platform | Runner | Artifacts |
|----------|--------|-----------|
| macOS | `macos-latest` | DMG + ZIP (universal arm64/x64) |
| Windows | `windows-latest` | NSIS installer + ZIP |
| Linux | `ubuntu-latest` | AppImage + .deb |

2. Publishes all artifacts to a GitHub Release via `electron-builder --publish always`

### Code signing

macOS and Windows builds are code-signed when the following secrets are configured in the repository:

| Secret | Purpose |
|--------|---------|
| `CSC_LINK` | Base64-encoded code signing certificate |
| `CSC_KEY_PASSWORD` | Certificate password |

Without these secrets, builds are produced unsigned (functional but may trigger OS warnings).

### macOS notarization

macOS builds are notarized via an `afterSign` hook (`app/scripts/notarize.js`) when the following secrets are configured:

| Secret | Purpose |
|--------|---------|
| `APPLE_ID` | Apple Developer account email |
| `APPLE_APP_SPECIFIC_PASSWORD` | App-specific password (generate at appleid.apple.com) |
| `APPLE_TEAM_ID` | Apple Developer Team ID (found in developer portal) |

Without these secrets, notarization is skipped. Unsigned/unnotarized macOS builds will trigger Gatekeeper warnings on macOS Sequoia+.

---

## Releasing both at once

If a release includes changes to both the CLI and the app, create two separate tags:

```bash
git commit -m "release: v0.2.0 / app-v0.2.0"
git tag v0.2.0
git tag app-v0.2.0
git push origin main --tags
```

Both workflows run independently in parallel.

---

## Pre-release checklist

Before tagging a release, verify:

```bash
# CLI
cargo test
cargo clippy --all-targets
cargo build --release

# App
cd app
npm ci
npm run typecheck
npm test
npm run lint
npm run build
```

---

## Versioning

Both artifacts use [semver](https://semver.org/). CLI and app versions are independent — they don't need to match. The CLI version lives in `Cargo.toml`, the app version in `app/package.json`.

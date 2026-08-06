---
title: "Installation"
description: "Install a tagged mdvdb release or build the current development version"
category: "guides"
---

# Installation

Choose between a tagged GitHub release and the current `main` branch:

- **Tagged release** — a published, reproducible version with pre-built binaries.
- **Current main** — the newest code in the repository. It may contain features that have not
  reached a release tag yet.

Do not infer the latest release from examples or from the version in a checkout. Check
[GitHub Releases](https://github.com/geckse/markdown-vdb/releases), and verify the installed binary
with `mdvdb --version`.

## Install the latest tagged release

On macOS or Linux, the repository install script detects your operating system and architecture,
downloads the latest tagged release, and installs `mdvdb`:

```bash
curl -fsSL https://raw.githubusercontent.com/geckse/markdown-vdb/main/install.sh | sh
```

The default destination is `/usr/local/bin`. To install without administrator access, choose a
directory you own and add it to `PATH`:

```bash
mkdir -p "$HOME/.local/bin"
curl -fsSL https://raw.githubusercontent.com/geckse/markdown-vdb/main/install.sh \
  | INSTALL_DIR="$HOME/.local/bin" sh
export PATH="$HOME/.local/bin:$PATH"
```

If you prefer to inspect scripts before running them, download
[`install.sh`](https://github.com/geckse/markdown-vdb/blob/main/install.sh), review it, and then run
it locally.

### Manual release download

Release archives and raw binaries are published at
[github.com/geckse/markdown-vdb/releases](https://github.com/geckse/markdown-vdb/releases).
Assets use the release tag and Rust target in their names, for example:

| Platform | Target |
|---|---|
| macOS, Apple Silicon | `aarch64-apple-darwin` |
| macOS, Intel | `x86_64-apple-darwin` |
| Linux, x86-64 | `x86_64-unknown-linux-gnu` |
| Linux, ARM64 | `aarch64-unknown-linux-gnu` |
| Windows, x86-64 | `x86_64-pc-windows-msvc` |

Unix archives follow `mdvdb-<TAG>-<TARGET>.tar.gz`; Windows archives use `.zip`. Extract the
archive and put `mdvdb` or `mdvdb.exe` in a directory on `PATH`.

## Install current main

Install the latest development snapshot with the current stable Rust toolchain:

```bash
cargo install \
  --git https://github.com/geckse/markdown-vdb.git \
  --branch main \
  --locked
```

This follows `main`; it is not the same promise as installing the newest release tag. To build an
exact tag from source, replace `--branch main` with `--tag <TAG>`:

```bash
cargo install \
  --git https://github.com/geckse/markdown-vdb.git \
  --tag <TAG> \
  --locked
```

For development, clone the repository instead:

```bash
git clone https://github.com/geckse/markdown-vdb.git
cd markdown-vdb
cargo build --release --locked
```

The binary is written to `target/release/mdvdb` (`mdvdb.exe` on Windows).

### Source-build requirements

Use a current stable Rust toolchain. Native dependencies also require a C/C++ toolchain and CMake.
Typical setup commands are:

```bash
# macOS
xcode-select --install

# Debian / Ubuntu
sudo apt-get install build-essential cmake pkg-config

# Fedora / RHEL
sudo dnf install gcc gcc-c++ cmake pkg-config
```

On Windows, install Visual Studio Build Tools with the C++ workload and CMake.

## Verify and update

```bash
mdvdb --version
mdvdb --help
```

To update, repeat the method you originally used: rerun the release installer for the newest tag,
or rerun `cargo install --git ... --branch main --locked` for a newer development snapshot.

## Next steps

- [Quick Start](./quickstart.md) — initialize, configure, ingest, and query a collection
- [Configuration](./configuration.md) — YAML settings, secrets, and embedding providers
- [Shell Completions](./shell-completions.md) — enable completions for your shell
- [Command Reference](./commands/index.md) — browse CLI commands

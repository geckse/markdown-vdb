---
title: "Installation"
description: "How to install mdvdb via cargo, GitHub releases, or from source"
category: "guides"
---

# Installation

There are three ways to install **mdvdb**: from crates.io via `cargo install`, from pre-built GitHub Release binaries, or by building from source.

## Install via Cargo (Recommended)

If you have a [Rust toolchain](https://rustup.rs/) installed (Rust 1.70+), the simplest method is:

```bash
cargo install mdvdb
```

This downloads the latest release from [crates.io](https://crates.io/crates/mdvdb), compiles it for your platform, and installs the `mdvdb` binary into `~/.cargo/bin/`.

Make sure `~/.cargo/bin` is in your `PATH`:

```bash
# bash / zsh
export PATH="$HOME/.cargo/bin:$PATH"

# fish
fish_add_path ~/.cargo/bin
```

### Updating

To update to the latest version:

```bash
cargo install mdvdb --force
```

## Pre-built Binaries (GitHub Releases)

Pre-built binaries for major platforms are available on the [GitHub Releases](https://github.com/nicholasgasior/markdown-vdb/releases) page.

1. Download the archive for your platform:

   | Platform | Archive |
   |----------|---------|
   | macOS (Apple Silicon) | `mdvdb-aarch64-apple-darwin.tar.gz` |
   | macOS (Intel) | `mdvdb-x86_64-apple-darwin.tar.gz` |
   | Linux (x86_64) | `mdvdb-x86_64-unknown-linux-gnu.tar.gz` |
   | Linux (ARM64) | `mdvdb-aarch64-unknown-linux-gnu.tar.gz` |
   | Windows (x86_64) | `mdvdb-x86_64-pc-windows-msvc.zip` |

2. Extract the binary:

   ```bash
   # macOS / Linux
   tar -xzf mdvdb-*.tar.gz
   chmod +x mdvdb

   # Move to a directory in your PATH
   sudo mv mdvdb /usr/local/bin/
   ```

   On Windows, extract the `.zip` and move `mdvdb.exe` to a directory in your `PATH`.

## Build from Source

Clone the repository and build a release binary:

```bash
git clone https://github.com/nicholasgasior/markdown-vdb.git
cd markdown-vdb
cargo build --release
```

The compiled binary is at `target/release/mdvdb`. Copy it to a location in your `PATH`:

```bash
# macOS / Linux
sudo cp target/release/mdvdb /usr/local/bin/

# Or add the target directory to PATH
export PATH="$(pwd)/target/release:$PATH"
```

### Build Dependencies

Building from source requires:

- **Rust 1.70+** (install via [rustup](https://rustup.rs/))
- **A C compiler** (`cc` / `gcc` / `clang`) for native dependencies (`usearch`, `tiktoken-rs`)
- **CMake** (required by `usearch` for HNSW index compilation)
- **pkg-config** (Linux only, for system library discovery)

#### Platform-Specific Notes

**macOS:**

Xcode Command Line Tools provide the C compiler and CMake:

```bash
xcode-select --install
```

Or install CMake separately via Homebrew:

```bash
brew install cmake
```

**Linux (Debian/Ubuntu):**

```bash
sudo apt-get update
sudo apt-get install build-essential cmake pkg-config
```

**Linux (Fedora/RHEL):**

```bash
sudo dnf install gcc gcc-c++ cmake pkg-config
```

**Windows:**

Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "C++ build tools" workload, and install [CMake](https://cmake.org/download/).

## Verify Installation

After installing via any method, confirm that `mdvdb` is available:

```bash
mdvdb --version
```

You should see output like:

```
mdvdb 0.1.0
```

## Next Steps

- [Quick Start](./quickstart.md) -- Go from zero to your first search in 5 minutes
- [Configuration](./configuration.md) -- Set up embedding providers and customize behavior
- [Shell Completions](./shell-completions.md) -- Enable tab completions for your shell
- [Command Reference](./commands/index.md) -- Browse all available commands

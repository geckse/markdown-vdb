---
title: "mdvdb init"
description: "Initialize a new markdown-vdb project by creating a configuration file"
category: "commands"
---

# mdvdb init

Initialize a new markdown-vdb project by creating a configuration file. By default, creates a project-level config at `.markdownvdb/.config` in the current directory. With `--global`, creates a user-level config at `~/.mdvdb/config` that applies to all projects.

## Usage

```bash
mdvdb init [OPTIONS]
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `--global` | `false` | Create user-level config at `~/.mdvdb/config` instead of project config |

### Option Details

#### `--global`

Creates a user-level configuration file at `~/.mdvdb/config` (or the path specified by `MDVDB_CONFIG_HOME`). User-level settings apply to all projects as the lowest-priority file source -- any project-level `.markdownvdb/.config` or `.env` file will override them.

This is ideal for storing shared credentials (like `OPENAI_API_KEY`) or default preferences that you want across all your projects.

```bash
# Create user-level config
mdvdb init --global
```

## Global Options

These options apply to all commands. See [Commands Index](./index.md) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (-v info, -vv debug, -vvv trace) |
| `--root` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |

## Project-Level Init (Default)

Running `mdvdb init` without `--global` creates a `.markdownvdb/.config` file in the current directory (or the directory specified by `--root`) with a default configuration template:

```bash
mdvdb init
```

### Generated Project Config

The generated `.markdownvdb/.config` file contains:

```bash
# markdown-vdb configuration
# See https://github.com/example/markdown-vdb for documentation

# Embedding provider: openai, ollama, or custom
MDVDB_EMBEDDING_PROVIDER=openai
MDVDB_EMBEDDING_MODEL=text-embedding-3-small
MDVDB_EMBEDDING_DIMENSIONS=1536
MDVDB_EMBEDDING_BATCH_SIZE=100

# Source directories (comma-separated)
MDVDB_SOURCE_DIRS=.

# Chunking
MDVDB_CHUNK_MAX_TOKENS=512
MDVDB_CHUNK_OVERLAP_TOKENS=50

# Search defaults
MDVDB_SEARCH_DEFAULT_LIMIT=10
MDVDB_SEARCH_MIN_SCORE=0.0
MDVDB_SEARCH_MODE=hybrid
MDVDB_SEARCH_RRF_K=60.0

# File watching
MDVDB_WATCH=true
MDVDB_WATCH_DEBOUNCE_MS=300

# Clustering
MDVDB_CLUSTERING_ENABLED=true
MDVDB_CLUSTERING_REBALANCE_THRESHOLD=50
```

### Directory Structure After Init

```
your-project/
  .markdownvdb/
    .config          # <-- created by mdvdb init
```

The `.markdownvdb/` directory will also contain the `index` file (binary) and `fts/` directory (Tantivy segments) after running [`mdvdb ingest`](./ingest.md).

## User-Level Init (`--global`)

Running `mdvdb init --global` creates a `~/.mdvdb/config` file with a minimal template focused on credentials and provider defaults:

```bash
mdvdb init --global
```

### Generated User Config

The generated `~/.mdvdb/config` file contains:

```bash
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

Note that user-level config values are commented out by default. Uncomment and set the values you want to apply globally.

### Config Resolution Priority

User-level config (`~/.mdvdb/config`) has the **lowest priority** among file sources. The full resolution order is:

1. **Shell environment variables** (highest priority)
2. **`.markdownvdb/.config`** (project config)
3. **`.markdownvdb`** (legacy flat file)
4. **`.env`** (shared secrets)
5. **`~/.mdvdb/config`** (user-level defaults)
6. **Built-in defaults** (lowest priority)

See [Configuration](../configuration.md) for the complete config resolution documentation.

## Human-Readable Output

### Project Init

```
  ✓ Initialized

  Config: /path/to/project/.markdownvdb
  Edit it to configure your embedding provider and other settings.
```

### Global Init

```
  ✓ User config initialized

  Config: /home/user/.mdvdb/config
  Uncomment and set your API key and default settings.
```

## JSON Output

The `init` command does not produce JSON output. It always prints a human-readable success message regardless of the `--json` flag. To verify the resulting configuration after init, use [`mdvdb config --json`](./config.md).

## Error Handling

### Config Already Exists

If the configuration file already exists, `mdvdb init` returns an error instead of overwriting:

```
Error: config already exists at .markdownvdb/.config
```

This applies to both project-level and user-level (`--global`) initialization. To modify an existing config, edit the file directly.

### Legacy Config Detected

If a legacy flat `.markdownvdb` file exists (from an older version), project-level init also returns a "config already exists" error. To migrate, rename or remove the legacy file, then run `mdvdb init` again.

## Examples

```bash
# Initialize a project in the current directory
mdvdb init

# Initialize a project at a specific path
mdvdb init --root /path/to/project

# Create user-level config for shared credentials
mdvdb init --global

# Typical workflow: init, configure, ingest
mdvdb init
# Edit .markdownvdb/.config to set OPENAI_API_KEY
mdvdb ingest
```

## Related Commands

- [`mdvdb config`](./config.md) -- View the resolved configuration after init
- [`mdvdb doctor`](./doctor.md) -- Verify configuration and provider connectivity
- [`mdvdb ingest`](./ingest.md) -- Index markdown files after initializing
- [`mdvdb status`](./status.md) -- Check index status

## See Also

- [Configuration](../configuration.md) -- All environment variables, config files, and resolution order
- [Quick Start](../quickstart.md) -- Getting started from zero to first search
- [Embedding Providers](../concepts/embedding-providers.md) -- Configure OpenAI, Ollama, or custom providers
- [Index Storage](../concepts/index-storage.md) -- The `.markdownvdb/` directory structure

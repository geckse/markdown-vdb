---
title: "Command Reference"
description: "Overview of all mdvdb CLI commands with descriptions and links to detailed reference pages"
category: "commands"
---

# Command Reference

This page lists all available `mdvdb` commands. Click a command name for its full reference page with options, examples, and JSON output format.

## All Commands

| Command | Description |
|---------|-------------|
| [`mdvdb search`](./search.md) | Semantic search across indexed markdown files |
| [`mdvdb ingest`](./ingest.md) | Ingest markdown files into the index |
| [`mdvdb status`](./status.md) | Show index status and configuration |
| [`mdvdb schema`](./schema.md) | Show inferred metadata schema |
| [`mdvdb clusters`](./clusters.md) | Show document clusters |
| [`mdvdb tree`](./tree.md) | Show file tree with sync status indicators |
| [`mdvdb get`](./get.md) | Get metadata for a specific file |
| [`mdvdb watch`](./watch.md) | Watch for file changes and re-index automatically |
| [`mdvdb init`](./init.md) | Initialize a new `.markdownvdb` config file |
| [`mdvdb config`](./config.md) | Show resolved configuration |
| [`mdvdb doctor`](./doctor.md) | Run diagnostic checks on config, provider, and index |
| [`mdvdb links`](./links.md) | Show links originating from a file |
| [`mdvdb backlinks`](./backlinks.md) | Show backlinks pointing to a file |
| [`mdvdb orphans`](./orphans.md) | Find orphan files with no links |
| [`mdvdb edges`](./edges.md) | Show semantic edges between linked files |
| [`mdvdb graph`](./graph.md) | Show graph data (nodes, edges, clusters) for visualization |

## Commands by Category

### Core Workflow

These commands form the primary usage loop. See [Search Modes](../concepts/search-modes.md) for how search works, [Embedding Providers](../concepts/embedding-providers.md) for provider setup, and [Chunking](../concepts/chunking.md) for how files are split.

| Command | Purpose |
|---------|---------|
| [`search`](./search.md) | Find relevant content using semantic, lexical, or hybrid search |
| [`ingest`](./ingest.md) | Index markdown files (supports incremental and full re-indexing) |
| [`status`](./status.md) | Check how many files, chunks, and vectors are in the index |

### Setup & Configuration

See [Configuration](../configuration.md) for the full environment variable and config file reference.

| Command | Purpose |
|---------|---------|
| [`init`](./init.md) | Create a `.markdownvdb` config file (project or global) |
| [`config`](./config.md) | Display the fully resolved configuration with all values |
| [`doctor`](./doctor.md) | Diagnose issues with config, embedding provider, and index |

### Data Inspection

See [Clustering](../concepts/clustering.md) for how clusters are computed and [Index Storage](../concepts/index-storage.md) for how data is stored.

| Command | Purpose |
|---------|---------|
| [`schema`](./schema.md) | View auto-inferred frontmatter metadata schema |
| [`clusters`](./clusters.md) | View document clusters with TF-IDF keyword labels |
| [`tree`](./tree.md) | View file tree with per-file sync status (New/Modified/Synced/Deleted) |
| [`get`](./get.md) | Retrieve metadata and frontmatter for a single file |

### Link Graph

See [Link Graph](../concepts/link-graph.md) for how links are extracted and the graph is built.

| Command | Purpose |
|---------|---------|
| [`links`](./links.md) | Show outgoing links from a file (with multi-hop traversal) |
| [`backlinks`](./backlinks.md) | Show files that link to a given file |
| [`orphans`](./orphans.md) | Find files with no incoming or outgoing links |
| [`edges`](./edges.md) | Show semantic edges (relationship labels) between linked files |
| [`graph`](./graph.md) | Export full graph data for visualization tools |

### Automation

See [Ignore Files](../concepts/ignore-files.md) for how the watcher determines which files to monitor.

| Command | Purpose |
|---------|---------|
| [`watch`](./watch.md) | Monitor filesystem for changes and re-index automatically |

## Hidden Commands

The following commands are available but hidden from `--help` output:

| Command | Description |
|---------|-------------|
| `mdvdb completions <shell>` | Generate shell completions (see [Shell Completions](../shell-completions.md)) |
| `mdvdb chunks <dir>` | Dump chunks as JSON for benchmarking (internal use) |

## Global Options

Every command accepts these global flags. See [Global Options](../index.md#global-options) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (`-v` info, `-vv` debug, `-vvv` trace) |
| `--root <PATH>` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |
| `--version` | | Print version information with logo |

## See Also

- [Quick Start](../quickstart.md) - Get started with mdvdb in 5 minutes
- [Configuration](../configuration.md) - Environment variables and config file reference
- [JSON Output Reference](../json-output.md) - JSON schemas for `--json` output
- [Search Modes](../concepts/search-modes.md) - Hybrid, semantic, lexical, and edge search
- [Embedding Providers](../concepts/embedding-providers.md) - OpenAI, Ollama, and custom providers
- [Link Graph](../concepts/link-graph.md) - Link extraction, backlinks, and graph traversal
- [Time Decay](../concepts/time-decay.md) - Time-based scoring for search results

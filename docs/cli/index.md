---
title: "mdvdb CLI Documentation"
description: "Complete reference documentation for the mdvdb command-line interface"
category: "guides"
---

# mdvdb CLI Documentation

**mdvdb** is a filesystem-native vector database built around Markdown files. It provides semantic search, link graph analysis, clustering, and more — all from the command line with zero infrastructure.

## Getting Started

| Page | Description |
|------|-------------|
| [Installation](./installation.md) | Install mdvdb via cargo, GitHub releases, or from source |
| [Quick Start](./quickstart.md) | Go from zero to your first search in 5 minutes |
| [Configuration](./configuration.md) | Environment variables, config files, and resolution order |
| [Shell Completions](./shell-completions.md) | Set up tab completions for bash, zsh, fish, and PowerShell |

## Command Reference

All CLI commands are documented individually under [Commands](./commands/index.md).

| Category | Commands | Description |
|----------|----------|-------------|
| Core | [search](./commands/search.md), [ingest](./commands/ingest.md), [status](./commands/status.md) | Index files and search across them |
| Setup | [init](./commands/init.md), [config](./commands/config.md), [doctor](./commands/doctor.md) | Initialize, configure, and diagnose |
| Inspection | [schema](./commands/schema.md), [clusters](./commands/clusters.md), [tree](./commands/tree.md), [get](./commands/get.md) | Explore index contents and metadata |
| Graph | [links](./commands/links.md), [backlinks](./commands/backlinks.md), [orphans](./commands/orphans.md), [edges](./commands/edges.md), [graph](./commands/graph.md) | Navigate the link graph between files |
| Automation | [watch](./commands/watch.md) | Automatically re-index on file changes |

## Concepts

Deeper explanations of how mdvdb works under the hood.

| Page | Description |
|------|-------------|
| [Search Modes](./concepts/search-modes.md) | Hybrid, semantic, lexical, and edge search explained |
| [Embedding Providers](./concepts/embedding-providers.md) | OpenAI, Ollama, and custom provider setup |
| [Chunking](./concepts/chunking.md) | How Markdown files are split into chunks for embedding |
| [Link Graph](./concepts/link-graph.md) | Link extraction, backlinks, orphans, and semantic edges |
| [Time Decay](./concepts/time-decay.md) | Time-based scoring decay for search results |
| [Clustering](./concepts/clustering.md) | K-means document clustering with TF-IDF labels |
| [Ignore Files](./concepts/ignore-files.md) | `.gitignore`, `.mdvdbignore`, and built-in exclusions |
| [Index Storage](./concepts/index-storage.md) | The `.markdownvdb/` directory and binary index format |

## Output Formats

| Page | Description |
|------|-------------|
| [JSON Output Reference](./json-output.md) | JSON schemas for every command that supports `--json` |

## Command Overview

```mermaid
graph LR
    subgraph Setup
        init["mdvdb init"]
        config["mdvdb config"]
        doctor["mdvdb doctor"]
    end

    subgraph Indexing
        ingest["mdvdb ingest"]
        watch["mdvdb watch"]
    end

    subgraph Search
        search["mdvdb search"]
    end

    subgraph Inspection
        status["mdvdb status"]
        schema["mdvdb schema"]
        clusters["mdvdb clusters"]
        tree["mdvdb tree"]
        get["mdvdb get"]
    end

    subgraph Graph
        links["mdvdb links"]
        backlinks["mdvdb backlinks"]
        orphans["mdvdb orphans"]
        edges["mdvdb edges"]
        graph["mdvdb graph"]
    end

    init --> ingest
    ingest --> search
    ingest --> status
    watch -.-> ingest
    search --> get
    links --> backlinks
```

## Global Options

These flags are available on every command:

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (`-v` info, `-vv` debug, `-vvv` trace) |
| `--root <PATH>` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |
| `--version` | | Print version information with logo |

Running `mdvdb` with no subcommand prints a logo and usage hint.

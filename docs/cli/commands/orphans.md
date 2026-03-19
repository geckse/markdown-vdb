---
title: "mdvdb orphans"
description: "Find markdown files with no incoming or outgoing links (orphan files)"
category: "commands"
---

# mdvdb orphans

Find orphan files with no links. An orphan file is a markdown file in the index that has no outgoing links to other files and no incoming links (backlinks) from other files. These are isolated nodes in the link graph -- they neither reference nor are referenced by any other indexed file.

## Usage

```bash
mdvdb orphans [OPTIONS]
```

## Arguments

This command takes no arguments.

## Options

This command has no command-specific options. Only [global options](#global-options) apply.

## Global Options

These options apply to all commands. See [Commands Index](./index.md) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (-v info, -vv debug, -vvv trace) |
| `--root` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |

## Human-Readable Output

When run without `--json`, orphans displays a formatted list of isolated files:

```
  ● Orphan Files (3) — no incoming or outgoing links

  • docs/standalone-notes.md
  • archive/old-design.md
  • scratch/temp-ideas.md

  Total: 3 orphan files
```

If no orphan files are found:

```
  ✓ No orphan files found — all files are connected.
```

### Output Elements

| Element | Description |
|---------|-------------|
| **Orphan count** | Total number of files with no links (shown in header) |
| **File paths** | Relative paths of each orphan file, listed with bullet markers |
| **Total** | Summary count at the bottom |

## Examples

```bash
# Find all orphan files
mdvdb orphans

# Find orphan files as JSON
mdvdb orphans --json

# Find orphan files in a specific project
mdvdb orphans --root /path/to/project

# Find orphan files with debug logging
mdvdb orphans -vv
```

## JSON Output

### OrphansOutput (`--json`)

```json
{
  "orphans": [
    {
      "path": "docs/standalone-notes.md"
    },
    {
      "path": "archive/old-design.md"
    },
    {
      "path": "scratch/temp-ideas.md"
    }
  ],
  "total_orphans": 3
}
```

### OrphansOutput Fields

| Field | Type | Description |
|-------|------|-------------|
| `orphans` | `OrphanFile[]` | Array of orphan files |
| `total_orphans` | `number` | Total number of orphan files (equal to `orphans.length`) |

### OrphanFile Fields

| Field | Type | Description |
|-------|------|-------------|
| `path` | `string` | Relative path to the orphan file (relative to project root) |

## How Orphan Detection Works

A file is considered an orphan if it meets **both** of these conditions:

1. **No outgoing links** -- the file contains no markdown links (`[text](target.md)`) or wikilinks (`[[target]]`) pointing to other indexed files.
2. **No incoming links** -- no other indexed file contains a link pointing to this file.

The orphan check runs against the link graph built during ingestion. Only files that have been ingested are considered. Files excluded by `.gitignore` or `.mdvdbignore` are not in the index and therefore not part of the orphan analysis.

## Notes

- The `orphans` command opens the index in **read-only** mode. It never modifies the index.
- Orphan detection requires a populated link graph. Run [`mdvdb ingest`](./ingest.md) first to build the graph.
- A file is only an orphan if it has **neither** outgoing nor incoming links. A file with outgoing links but no incoming links is **not** an orphan.
- Links to files outside the index (broken links) do not count as outgoing connections for orphan purposes -- only links to indexed files are considered.
- To see the full link context for a specific file, use [`mdvdb links`](./links.md).
- To find files that link TO a specific file, use [`mdvdb backlinks`](./backlinks.md).

## Related Commands

- [`mdvdb links`](./links.md) -- Show outgoing and incoming links for a specific file
- [`mdvdb backlinks`](./backlinks.md) -- Show files linking TO a specific file
- [`mdvdb edges`](./edges.md) -- Show semantic edges between linked files
- [`mdvdb graph`](./graph.md) -- Visualization-ready graph data (nodes and edges)
- [`mdvdb tree`](./tree.md) -- File tree with sync status indicators
- [`mdvdb status`](./status.md) -- Index statistics including document count

## See Also

- [Link Graph](../concepts/link-graph.md) -- How mdvdb extracts links, builds the graph, and detects orphans
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference for all commands

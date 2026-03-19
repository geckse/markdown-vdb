---
title: "mdvdb backlinks"
description: "Show files that link TO a specific markdown file (incoming links / backlinks)"
category: "commands"
---

# mdvdb backlinks

Show files that link TO a specific file. Unlike [`mdvdb links`](./links.md) which shows both outgoing and incoming links, `backlinks` focuses exclusively on incoming links -- files in the index that contain a link pointing to the specified file.

## Usage

```bash
mdvdb backlinks <FILE_PATH> [OPTIONS]
```

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `FILE_PATH` | Yes | Relative path to the markdown file (e.g., `docs/readme.md`) |

The `FILE_PATH` must be the relative path as stored in the index (relative to the project root). The file must have been ingested for its backlinks to be available.

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

When run without `--json`, backlinks displays a formatted list of files linking to the target:

```
  ● Backlinks to docs/api/endpoints.md

  Incoming: 3 incoming links

  ├── docs/architecture.md "API Endpoints"
  │   line 12
  ├── docs/index.md "Endpoints Reference"
  │   line 34
  └── docs/api/overview.md "Endpoints" [wikilink]
      line 8
```

If no files link to the target:

```
  ● Backlinks to docs/orphan-page.md

  ✗ No files link to docs/orphan-page.md
```

### Output Elements

| Element | Description |
|---------|-------------|
| **Source file** | The file that contains a link pointing to the target |
| **Link text** | The display text of the markdown link (in quotes) |
| **Line number** | The line number in the source file where the link appears |
| `[wikilink]` | Blue badge indicating the link uses `[[wikilink]]` syntax |

## Examples

```bash
# Show backlinks for a file
mdvdb backlinks docs/api/endpoints.md

# Show backlinks as JSON
mdvdb backlinks docs/api/endpoints.md --json

# Show backlinks for a file in a specific project
mdvdb backlinks notes/meeting.md --root /path/to/project

# Show backlinks with debug logging
mdvdb backlinks docs/readme.md -vv
```

## JSON Output

### BacklinksOutput (`--json`)

```json
{
  "file": "docs/api/endpoints.md",
  "backlinks": [
    {
      "entry": {
        "source": "docs/architecture.md",
        "target": "docs/api/endpoints.md",
        "text": "API Endpoints",
        "line_number": 12,
        "is_wikilink": false
      },
      "state": "Valid"
    },
    {
      "entry": {
        "source": "docs/index.md",
        "target": "docs/api/endpoints.md",
        "text": "Endpoints Reference",
        "line_number": 34,
        "is_wikilink": false
      },
      "state": "Valid"
    },
    {
      "entry": {
        "source": "docs/api/overview.md",
        "target": "docs/api/endpoints.md",
        "text": "Endpoints",
        "line_number": 8,
        "is_wikilink": true
      },
      "state": "Valid"
    }
  ],
  "total_backlinks": 3
}
```

### BacklinksOutput Fields

| Field | Type | Description |
|-------|------|-------------|
| `file` | `string` | The queried file path |
| `backlinks` | `ResolvedLink[]` | Array of incoming links with link entry details and validity state |
| `total_backlinks` | `number` | Total number of backlinks (equal to `backlinks.length`) |

### ResolvedLink Fields

| Field | Type | Description |
|-------|------|-------------|
| `entry` | `LinkEntry` | The link entry with source, target, text, and line number |
| `state` | `string` | `"Valid"` if the source file exists in the index, `"Broken"` if not |

### LinkEntry Fields

| Field | Type | Description |
|-------|------|-------------|
| `source` | `string` | Source file path -- the file containing the link (relative to project root) |
| `target` | `string` | Target file path -- the queried file (resolved relative to project root) |
| `text` | `string` | Display text of the markdown link |
| `line_number` | `number` | Line number in the source file (1-based) |
| `is_wikilink` | `boolean` | `true` if the link uses `[[wikilink]]` syntax |

## Backlinks vs Links

| | `mdvdb links` | `mdvdb backlinks` |
|---|---|---|
| **Shows outgoing** | Yes | No |
| **Shows incoming** | Yes | Yes |
| **Multi-hop depth** | Yes (`--depth 2-3`) | No |
| **Use case** | Full link context for a file | Quickly find what references a file |

Use `mdvdb backlinks` when you want a focused answer to "who links to this file?" without the outgoing link information. Use [`mdvdb links`](./links.md) when you need the full bidirectional picture or want multi-hop traversal.

## Notes

- The `backlinks` command opens the index in **read-only** mode. It never modifies the index.
- Backlinks are computed from the link graph built during ingestion. Run [`mdvdb ingest`](./ingest.md) to populate the link graph.
- Both standard markdown links (`[text](target.md)`) and wikilinks (`[[target]]`) are tracked as backlinks.
- Link targets are resolved relative to the source file's directory, so a link `[text](../api/endpoints.md)` in `docs/guides/setup.md` correctly resolves to `docs/api/endpoints.md`.
- The `backlinks` array contains `ResolvedLink` objects (with an `entry` and `state` field), the same type used in the `outgoing` array of [`mdvdb links`](./links.md).
- For multi-hop backlink traversal (e.g., "what links to the files that link to this file?"), use [`mdvdb links --depth 2`](./links.md) which includes both outgoing and incoming multi-hop trees.

## Related Commands

- [`mdvdb links`](./links.md) -- Show both outgoing and incoming links with optional multi-hop depth
- [`mdvdb orphans`](./orphans.md) -- Find files with no links at all (no outgoing or incoming)
- [`mdvdb edges`](./edges.md) -- Show semantic edges between linked files
- [`mdvdb graph`](./graph.md) -- Visualization-ready graph data (nodes and edges)
- [`mdvdb get`](./get.md) -- Get metadata for a specific file
- [`mdvdb search`](./search.md) -- Search with `--boost-links` for link-aware scoring

## See Also

- [Link Graph](../concepts/link-graph.md) -- How mdvdb extracts links, builds the graph, computes backlinks, and detects orphans
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference for all commands
- [Configuration](../configuration.md) -- Link-related config vars (`MDVDB_SEARCH_BOOST_LINKS`, `MDVDB_SEARCH_BOOST_HOPS`)

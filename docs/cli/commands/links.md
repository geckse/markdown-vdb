---
title: "mdvdb links"
description: "Show outgoing and incoming links originating from a specific markdown file, with optional multi-hop neighborhood traversal"
category: "commands"
---

# mdvdb links

Show graph connections for a specific file. These include body Markdown links, body wikilinks, and
whole-value frontmatter Relations. The command displays both outgoing connections and incoming
backlinks. With `--depth 2` or `--depth 3`, it performs multi-hop traversal and returns the extended
neighborhood as a tree.

## Usage

```bash
mdvdb links <FILE_PATH> [OPTIONS]
```

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `FILE_PATH` | Yes | Relative path to the markdown file (e.g., `docs/readme.md`) |

The `FILE_PATH` must be the relative path as stored in the index (relative to the project root). The file must have been ingested for its links to be available.

## Options

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--depth` | | `1` | Link traversal depth (1-3). Depth 1 shows direct links, depth 2-3 shows multi-hop neighborhood. |

### `--depth <N>`

Controls how many hops of links to follow from the target file.

| Depth | Behavior | Output Format |
|-------|----------|---------------|
| `1` (default) | Shows only direct outgoing and incoming links | Flat list with link metadata (text, body line or frontmatter location, Relation field, state) |
| `2` | Shows direct links plus links from those linked files | Tree structure with nested children |
| `3` | Shows 3 levels of link traversal | Tree structure with deeper nesting |

- **Depth 1** returns a `LinksOutput` wrapping a `LinkQueryResult` -- a flat list of outgoing `ResolvedLink` entries and incoming `LinkEntry` entries. Every entry records its link text, body line or frontmatter location, wikilink flag, and optional Relation field. Validity state wraps outgoing entries; incoming entries are stored `LinkEntry` values.
- **Depth 2-3** recursively builds an outgoing and incoming neighborhood from the file, returning a `NeighborhoodResult` with `NeighborhoodNode.children` trees and per-branch cycle protection.

The depth value must be between 1 and 3 (inclusive). Values outside this range are rejected by the CLI parser.

## Global Options

These options apply to all commands. See [Commands Index](./index.md) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (-v info, -vv debug, -vvv trace) |
| `--root` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |

## Human-Readable Output

### Depth 1 (Direct Links)

When run with the default `--depth 1`, the output shows a flat list of outgoing and incoming links:

```
  ● docs/architecture.md

  Outgoing: 5
  ├── docs/api/endpoints.md "API Endpoints"
  │   line 12
  ├── docs/setup.md "Setup Guide"
  │   line 24
  ├── docs/missing.md "Missing Page" [broken]
  │   line 31
  ├── notes/ideas.md "Ideas" [wikilink]
  │   line 45
  └── docs/deploy.md "Deployment"
      line 58

  Incoming: 3
  ├── docs/index.md "Architecture Overview"
  │   line 8
  ├── projects/relaunch.md "Architecture" (design)
  │   frontmatter
  └── docs/api/overview.md "Architecture"
      line 15

  5 outgoing, 3 incoming, 1 broken
```

### Output Elements

| Element | Description |
|---------|-------------|
| **Outgoing** | Links found in this file pointing to other files |
| **Incoming** | Links in other files pointing to this file (backlinks) |
| Link target | The resolved relative path of the linked file |
| Link text | The display text of the body link or frontmatter Relation (in quotes) |
| Line number | For a body link, the 1-based line where the link appears |
| `frontmatter` | Location shown instead of a line number for a frontmatter Relation |
| `(field)` | Dimmed badge naming the originating frontmatter Relation field |
| `[broken]` | Red badge indicating the target file does not exist in the index |
| `[wikilink]` | Blue badge indicating the link uses `[[wikilink]]` syntax |

### Depth 2-3 (Multi-Hop Neighborhood)

When run with `--depth 2` or `--depth 3`, the output shows a tree of links with nested children:

```
  ● docs/architecture.md (depth: outgoing=2, incoming=2)

  Outgoing: 8 (2 level(s))
  ├── docs/api/endpoints.md
  │   ├── docs/api/auth.md
  │   └── docs/api/errors.md
  ├── docs/setup.md
  │   └── docs/prerequisites.md
  └── docs/deploy.md
      ├── docs/ci-cd.md
      └── docs/monitoring.md

  Incoming: 4 (2 level(s))
  ├── docs/index.md
  │   └── README.md
  └── docs/api/overview.md
      └── docs/api/index.md

  8 outgoing, 4 incoming
```

The multi-hop display includes:
- **Depth counts** in the header showing how many levels were explored for outgoing and incoming
- **Total counts** for outgoing and incoming tree nodes across all depths; a file may appear in different branches
- **Tree structure** with box-drawing characters showing the parent-child relationship
- **`[broken]`** badges on nodes whose target files don't exist

## Examples

```bash
# Show direct links for a file
mdvdb links docs/architecture.md

# Show 2-hop link neighborhood
mdvdb links docs/architecture.md --depth 2

# Show full 3-hop neighborhood as JSON
mdvdb links docs/architecture.md --depth 3 --json

# Show links for a file in a specific project
mdvdb links notes/meeting.md --root /path/to/project

# Show links with debug logging
mdvdb links docs/api.md -vv
```

## JSON Output

The JSON output format depends on the `--depth` value.

### Depth 1: LinksOutput (`--json`)

When `--depth` is 1 (the default), the output is a `LinksOutput` wrapper around a `LinkQueryResult`:

```json
{
  "file": "docs/architecture.md",
  "links": {
    "file": "docs/architecture.md",
    "outgoing": [
      {
        "entry": {
          "source": "docs/architecture.md",
          "target": "docs/api/endpoints.md",
          "text": "API Endpoints",
          "line_number": 12,
          "is_wikilink": false,
          "field": null
        },
        "state": "Valid"
      },
      {
        "entry": {
          "source": "docs/architecture.md",
          "target": "docs/missing.md",
          "text": "Missing Page",
          "line_number": 31,
          "is_wikilink": false,
          "field": null
        },
        "state": "Broken"
      },
      {
        "entry": {
          "source": "docs/architecture.md",
          "target": "clients/acme.md",
          "text": "Acme Corp",
          "line_number": 0,
          "is_wikilink": false,
          "field": "client"
        },
        "state": "Valid"
      }
    ],
    "incoming": [
      {
        "source": "docs/index.md",
        "target": "docs/architecture.md",
        "text": "Architecture Overview",
        "line_number": 8,
        "is_wikilink": false,
        "field": null
      }
    ]
  }
}
```

### LinksOutput Fields

| Field | Type | Description |
|-------|------|-------------|
| `file` | `string` | The queried file path |
| `links` | `LinkQueryResult` | The full link query result |

### LinkQueryResult Fields

| Field | Type | Description |
|-------|------|-------------|
| `file` | `string` | The queried file path |
| `outgoing` | `ResolvedLink[]` | Outgoing links from this file with validity state |
| `incoming` | `LinkEntry[]` | Incoming links (backlinks) to this file |

### ResolvedLink Fields

| Field | Type | Description |
|-------|------|-------------|
| `entry` | `LinkEntry` | The link entry with source, target, text, location, syntax, and Relation origin |
| `state` | `string` | `"Valid"` if the target exists in the index, `"Broken"` if not |

### LinkEntry Fields

| Field | Type | Description |
|-------|------|-------------|
| `source` | `string` | Source file path (relative to project root) |
| `target` | `string` | Target file path (resolved relative to project root) |
| `text` | `string` | Display text of the body link or frontmatter Relation |
| `line_number` | `number` | Body links use a 1-based source line; frontmatter Relations use the `0` sentinel |
| `is_wikilink` | `boolean` | `true` if the link uses `[[wikilink]]` syntax |
| `field` | `string \| null` | Originating frontmatter field for a Relation; always serialized and `null` for body links |

### Depth 2-3: NeighborhoodResult (`--json --depth 2`)

When `--depth` is 2 or 3, the output is a `NeighborhoodResult` with a tree structure:

```json
{
  "file": "docs/architecture.md",
  "outgoing": [
    {
      "path": "docs/api/endpoints.md",
      "state": "Valid",
      "children": [
        {
          "path": "docs/api/auth.md",
          "state": "Valid",
          "children": []
        },
        {
          "path": "docs/api/errors.md",
          "state": "Valid",
          "children": []
        }
      ]
    },
    {
      "path": "docs/setup.md",
      "state": "Valid",
      "children": [
        {
          "path": "docs/prerequisites.md",
          "state": "Broken",
          "children": []
        }
      ]
    }
  ],
  "incoming": [
    {
      "path": "docs/index.md",
      "state": "Valid",
      "children": [
        {
          "path": "README.md",
          "state": "Valid",
          "children": []
        }
      ]
    }
  ],
  "outgoing_count": 5,
  "incoming_count": 2,
  "outgoing_depth_count": 2,
  "incoming_depth_count": 2
}
```

### NeighborhoodResult Fields

| Field | Type | Description |
|-------|------|-------------|
| `file` | `string` | The queried file path |
| `outgoing` | `NeighborhoodNode[]` | Tree of outgoing (forward) links from this file |
| `incoming` | `NeighborhoodNode[]` | Tree of incoming (backlinks) to this file |
| `outgoing_count` | `number` | Total nodes in the outgoing tree across all depths |
| `incoming_count` | `number` | Total nodes in the incoming tree across all depths |
| `outgoing_depth_count` | `number` | Number of depth levels explored for outgoing links |
| `incoming_depth_count` | `number` | Number of depth levels explored for incoming links |

### NeighborhoodNode Fields

| Field | Type | Description |
|-------|------|-------------|
| `path` | `string` | Relative path to this file |
| `state` | `string` | `"Valid"` if the file exists in the index, `"Broken"` if not |
| `children` | `NeighborhoodNode[]` | Children discovered by following links from this file (empty at max depth or for leaf nodes) |

## Notes

- The `links` command opens the index in **read-only** mode. It never modifies the index.
- Links are extracted from Markdown files during ingestion. Run [`mdvdb ingest`](./ingest.md) to populate the link graph.
- Body Markdown links and wikilinks are detected, as are whole-value link-shaped strings and string-list elements in frontmatter. The latter carry their originating Relation `field`.
- Body link targets are resolved relative to the source file's directory. Relation targets follow
  the collection-root, overlay `target:`, and source-directory rules described in
  [Relations](../concepts/relations.md).
- The `.md` extension is automatically appended to link targets that don't have it.
- Fragment identifiers (e.g., `#section`) are stripped from link targets -- links are resolved at the file level.
- For depth 2-3, recursive traversal prevents cycles within each branch. The same file can still
  appear in separate branches.
- Use [`mdvdb backlinks`](./backlinks.md) for a simpler view of only the incoming links to a file.
- The `outgoing_count` and `incoming_count` in `NeighborhoodResult` count tree nodes, including a
  repeated file when it appears in different branches.

## Related Commands

- [`mdvdb backlinks`](./backlinks.md) -- Show only files linking TO a specific file
- [`mdvdb orphans`](./orphans.md) -- Find files with no links at all
- [`mdvdb edges`](./edges.md) -- Show semantic edges between linked files
- [`mdvdb graph`](./graph.md) -- Visualization-ready graph data (nodes and edges)
- [`mdvdb get`](./get.md) -- Get metadata for a specific file
- [`mdvdb search`](./search.md) -- Search with `--boost-links` for link-aware scoring

## See Also

- [Link Graph](../concepts/link-graph.md) -- How mdvdb extracts links, builds the graph, and supports multi-hop traversal
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference for all commands
- [Configuration](../configuration.md) -- Link-related config vars (`MDVDB_SEARCH_BOOST_LINKS`, `MDVDB_SEARCH_BOOST_HOPS`)

---
title: "mdvdb tree"
description: "Show file tree with sync status indicators showing which files are indexed, modified, new, or deleted"
category: "commands"
---

# mdvdb tree

Show a file tree of all markdown files with sync status indicators. Each file is classified by comparing the files on disk against the index: **Indexed** (synced, hash matches), **Modified** (hash changed since last ingest), **New** (on disk but not in the index), or **Deleted** (in the index but no longer on disk).

## Usage

```bash
mdvdb tree [OPTIONS]
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `--path <PREFIX>` | *(none)* | Restrict tree to files under this path prefix |

### Option Details

#### `--path <PREFIX>`

Filters the tree to show only files and directories under the given path prefix. The prefix matches against the relative file paths in the tree.

When `--path` is used, the tree is rooted at the matching subtree. If no files match the prefix, an empty tree is displayed with zero counts.

```bash
# Show tree for docs directory only
mdvdb tree --path docs

# Show tree for a nested directory
mdvdb tree --path src/components
```

## Global Options

These options apply to all commands. See [Commands Index](./index.md) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (-v info, -vv debug, -vvv trace) |
| `--root` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |

## Sync Status Indicators

Each file in the tree is classified into one of four sync states by comparing the file on disk with what is stored in the index:

| State | Indicator | Color | Description |
|-------|-----------|-------|-------------|
| **Indexed** | *(none)* | Green | File is in the index and its content hash matches the file on disk. Fully synced. |
| **Modified** | `[modified]` | Yellow | File is in the index but its content has changed since the last ingest (SHA-256 hash mismatch). |
| **New** | `[new]` | Blue | File exists on disk but is not yet in the index. Needs ingestion. |
| **Deleted** | `[deleted]` | Red | File is in the index but no longer exists on disk. Will be cleaned up on next ingest. |

### How Sync State Is Determined

1. **Discover** all markdown files on disk using the file scanner (respecting `.gitignore`, `.mdvdbignore`, and `MDVDB_IGNORE_PATTERNS`)
2. **Compare** each discovered file against the index:
   - If the file's relative path exists in the index, read the file and compute its SHA-256 content hash
   - If the hash matches the stored hash, the file is **Indexed**
   - If the hash differs, the file is **Modified**
   - If the path is not in the index, the file is **New**
3. **Check** for deleted files: any path in the index that is not found on disk is marked **Deleted**

## Human-Readable Output

When run without `--json`, tree displays an ASCII tree with box-drawing characters and colored status indicators:

```
.
├── docs
│   ├── api
│   │   ├── authentication.md
│   │   └── endpoints.md [modified]
│   ├── getting-started.md
│   └── overview.md [new]
├── blog
│   ├── 2024-01-15-intro.md
│   └── 2024-03-22-update.md
├── notes
│   └── meeting-notes.md [new]
└── README.md

12 files (8 indexed, 1 modified, 2 new, 1 deleted)
```

### Output Elements

| Element | Description |
|---------|-------------|
| **Tree structure** | Box-drawing characters (`├──`, `└──`, `│`) showing the directory hierarchy |
| **Directories** | Shown in bold, listed before files at each level |
| **Files** | Markdown files with optional `[state]` suffix for non-indexed states |
| **Summary line** | Total file count with breakdown by sync state |

### Color Coding

When colors are enabled (default), the tree uses ANSI colors:

| Element | Color |
|---------|-------|
| Directories | Bold |
| Indexed files | Green |
| Modified files | Yellow + `[modified]` suffix |
| New files | Blue + `[new]` suffix |
| Deleted files | Red + `[deleted]` suffix |

Colors can be disabled with `--no-color` or by setting the `NO_COLOR` environment variable.

### Sorting

Children at each directory level are sorted with:
1. **Directories first** (alphabetical)
2. **Files second** (alphabetical)

## Examples

```bash
# Show full file tree
mdvdb tree

# Show tree as JSON
mdvdb tree --json

# Show tree for a specific directory
mdvdb tree --path docs

# Show tree for a specific project
mdvdb tree --root /path/to/project

# Show tree without colors
mdvdb tree --no-color

# Show tree scoped to a directory as JSON
mdvdb tree --path blog --json
```

## JSON Output

### FileTree (`--json`)

```json
{
  "root": {
    "name": ".",
    "path": ".",
    "is_dir": true,
    "state": null,
    "children": [
      {
        "name": "docs",
        "path": "docs",
        "is_dir": true,
        "state": null,
        "children": [
          {
            "name": "api",
            "path": "docs/api",
            "is_dir": true,
            "state": null,
            "children": [
              {
                "name": "authentication.md",
                "path": "docs/api/authentication.md",
                "is_dir": false,
                "state": "indexed",
                "children": []
              },
              {
                "name": "endpoints.md",
                "path": "docs/api/endpoints.md",
                "is_dir": false,
                "state": "modified",
                "children": []
              }
            ]
          },
          {
            "name": "overview.md",
            "path": "docs/overview.md",
            "is_dir": false,
            "state": "new",
            "children": []
          }
        ]
      },
      {
        "name": "README.md",
        "path": "README.md",
        "is_dir": false,
        "state": "indexed",
        "children": []
      }
    ]
  },
  "total_files": 12,
  "indexed_count": 8,
  "modified_count": 1,
  "new_count": 2,
  "deleted_count": 1
}
```

### FileTree Fields

| Field | Type | Description |
|-------|------|-------------|
| `root` | `FileTreeNode` | Root node of the file tree |
| `total_files` | `number` | Total number of files (indexed + modified + new + deleted) |
| `indexed_count` | `number` | Number of files that are fully synced with the index |
| `modified_count` | `number` | Number of files modified since last ingest |
| `new_count` | `number` | Number of files on disk not yet in the index |
| `deleted_count` | `number` | Number of files in the index no longer on disk |

### FileTreeNode Fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | File or directory name (e.g., `"README.md"`, `"docs"`) |
| `path` | `string` | Relative path from project root (e.g., `"docs/api/endpoints.md"`) |
| `is_dir` | `boolean` | `true` for directories, `false` for files |
| `state` | `string \| null` | Sync state for files: `"indexed"`, `"modified"`, `"new"`, or `"deleted"`. Always `null` for directories. |
| `children` | `FileTreeNode[]` | Child nodes (files and subdirectories). Empty array for files. |

### FileState Values

| Value | Description |
|-------|-------------|
| `"indexed"` | File hash matches the index. Fully synced. |
| `"modified"` | File exists in the index but content has changed (hash mismatch). |
| `"new"` | File exists on disk but is not in the index. |
| `"deleted"` | File is in the index but no longer on disk. |

### Filtered Tree (`--path`)

When `--path` is used, the JSON output has the same structure but rooted at the matching subtree. If no files match the prefix, an empty tree is returned:

```json
{
  "root": {
    "name": ".",
    "path": ".",
    "is_dir": true,
    "state": null,
    "children": []
  },
  "total_files": 0,
  "indexed_count": 0,
  "modified_count": 0,
  "new_count": 0,
  "deleted_count": 0
}
```

## Notes

- The `tree` command opens the index in **read-only** mode. It never modifies the index.
- File discovery respects `.gitignore`, `.mdvdbignore`, and `MDVDB_IGNORE_PATTERNS` -- excluded files do not appear in the tree.
- The summary counts in `--path` mode still reflect the **full** tree counts, not just the filtered subtree. The tree structure is filtered but the aggregate counts come from the full scan.
- Deleted files appear in the tree even though they no longer exist on disk. They will be removed from the index on the next [`mdvdb ingest`](./ingest.md).
- The content hash comparison reads files from disk, so the `tree` command may be slightly slower than `status` for large projects.

## Related Commands

- [`mdvdb ingest`](./ingest.md) -- Index new and modified files, remove deleted files
- [`mdvdb status`](./status.md) -- Quick summary of index counts without the tree structure
- [`mdvdb schema`](./schema.md) -- View inferred metadata schema from indexed files
- [`mdvdb get`](./get.md) -- View detailed metadata for a specific file

## See Also

- [Index Storage](../concepts/index-storage.md) -- How the index tracks file hashes and content
- [Ignore Files](../concepts/ignore-files.md) -- How `.gitignore`, `.mdvdbignore`, and `MDVDB_IGNORE_PATTERNS` control file discovery
- [Configuration](../configuration.md) -- Environment variables and config options
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference

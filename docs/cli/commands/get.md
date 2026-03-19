---
title: "mdvdb get"
description: "Get metadata for a specific indexed file including path, content hash, frontmatter, chunk count, file size, and timestamps"
category: "commands"
---

# mdvdb get

Retrieve detailed metadata for a specific file from the index. Returns the file's path, content hash, frontmatter, chunk count, file size, and timestamps (indexed and modified).

## Usage

```bash
mdvdb get <FILE_PATH> [OPTIONS]
```

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `FILE_PATH` | Yes | Relative path to the markdown file (e.g., `docs/readme.md`) |

The `FILE_PATH` must be the relative path as stored in the index (relative to the project root). If the file is not in the index, the command exits with an error.

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

When run without `--json`, get displays a formatted summary of the document:

```
  ● docs/api/endpoints.md

  File size:  4.2 KB
  Indexed at: 2 hours ago
  Modified at: 45 minutes ago
  Hash:     a1b2c3d4e5f6...
  Chunks:   7

  Frontmatter:
    title: API Endpoints
    tags: ["api", "reference"]
    status: published
```

### Output Fields

| Field | Description |
|-------|-------------|
| **File size** | Size of the markdown file on disk (human-readable format) |
| **Indexed at** | When the file was last ingested (relative time format) |
| **Modified at** | Filesystem modification time of the file (relative time format). Only shown if available. |
| **Hash** | SHA-256 content hash used for change detection |
| **Chunks** | Number of text chunks the file was split into |
| **Frontmatter** | YAML frontmatter key-value pairs, if present in the file |

## Examples

```bash
# Get metadata for a specific file
mdvdb get docs/readme.md

# Get metadata as JSON
mdvdb get docs/readme.md --json

# Get metadata for a file in a specific project
mdvdb get notes/meeting.md --root /path/to/project

# Get metadata with debug logging
mdvdb get docs/api.md -vv
```

## JSON Output

### DocumentInfo (`--json`)

```json
{
  "path": "docs/api/endpoints.md",
  "content_hash": "a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890",
  "frontmatter": {
    "title": "API Endpoints",
    "tags": ["api", "reference"],
    "status": "published"
  },
  "chunk_count": 7,
  "file_size": 4301,
  "indexed_at": 1710849000,
  "modified_at": 1710854700
}
```

### DocumentInfo Fields

| Field | Type | Description |
|-------|------|-------------|
| `path` | `string` | Relative path to the markdown file from the project root |
| `content_hash` | `string` | SHA-256 hex digest of the file's content. Used for change detection during incremental ingest. |
| `frontmatter` | `object \| null` | Parsed YAML frontmatter as a JSON object. `null` if the file has no frontmatter block. |
| `chunk_count` | `number` | Number of text chunks this file was split into during ingestion |
| `file_size` | `number` | Size of the file on disk in bytes |
| `indexed_at` | `number` | Unix timestamp (seconds since epoch) when the file was last ingested |
| `modified_at` | `number \| null` | Unix timestamp of the file's filesystem modification time. `null` if the mtime is not available. |

### Frontmatter

The `frontmatter` field contains the parsed YAML frontmatter block from the top of the markdown file. The structure depends entirely on the file's content -- there is no fixed schema. Common fields include `title`, `tags`, `date`, `status`, `author`, etc.

If the file has no frontmatter (no `---` delimited block at the top), this field is `null`.

```json
// File with frontmatter
{ "frontmatter": { "title": "My Doc", "tags": ["rust", "cli"] } }

// File without frontmatter
{ "frontmatter": null }
```

## Error Handling

If the specified file is not in the index, the command exits with an error:

```
Error: file not in index: docs/nonexistent.md
```

This typically means the file either:
- Has not been ingested yet -- run [`mdvdb ingest`](./ingest.md) first
- Was excluded by `.gitignore`, `.mdvdbignore`, or `MDVDB_IGNORE_PATTERNS`
- Was deleted from disk and removed from the index on the last ingest

Use [`mdvdb tree`](./tree.md) to see which files are indexed and their sync status.

## Notes

- The `get` command opens the index in **read-only** mode. It never modifies the index.
- The `file_size` field is raw bytes in JSON mode, but displayed in human-readable format (e.g., "4.2 KB") in human-readable mode.
- The `indexed_at` and `modified_at` fields are Unix timestamps in JSON mode, but displayed as relative time strings (e.g., "2 hours ago") in human-readable mode.
- The `content_hash` is the SHA-256 hash of the file's raw content. It is used during incremental ingest to skip unchanged files.
- The `modified_at` field may be `null` if the filesystem modification time could not be read when the file was indexed.

## Related Commands

- [`mdvdb tree`](./tree.md) -- View file tree with sync status to find file paths
- [`mdvdb ingest`](./ingest.md) -- Index files so they appear in `get` results
- [`mdvdb status`](./status.md) -- Quick summary of index counts
- [`mdvdb schema`](./schema.md) -- View the inferred metadata schema across all files
- [`mdvdb links`](./links.md) -- View outgoing links from a specific file

## See Also

- [Index Storage](../concepts/index-storage.md) -- How files, chunks, and hashes are stored in the index
- [Chunking](../concepts/chunking.md) -- How files are split into chunks
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference
- [Configuration](../configuration.md) -- All environment variables and config options

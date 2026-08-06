---
title: "mdvdb info"
description: "Show collection or folder statistics, sync state, and a full-reindex estimate"
category: "commands"
---

# mdvdb info

Show statistics for the whole collection, a folder, or a configured Shard. The command compares Markdown files on disk with the current index and estimates the work required to reindex the selected scope.

## Usage

```bash
mdvdb info [OPTIONS] [PATH]
```

## Arguments and options

| Name | Default | Description |
|------|---------|-------------|
| `[PATH]` | `.` | Relative folder to inspect |
| `--shard <ID>` | | Inspect the folder configured for a named Shard; conflicts with `PATH` |

The command also accepts all [global options](./index.md#global-options), including `--json` and `--root`.

## Examples

```bash
# Whole collection
mdvdb info

# One folder
mdvdb info docs

# One named Shard
mdvdb info --shard research

# Machine-readable output
mdvdb info docs --json
```

## Reported values

| Field | Description |
|-------|-------------|
| `file_count` | Markdown files discovered on disk in the scope |
| `indexed_file_count` | Indexed files in the scope |
| `chunk_count` | Indexed chunks in the scope |
| `vector_count` | Whole collection: measured vector count; scoped view: chunk plus edge vectors attributed to the scope |
| `edge_count` | Edge vectors whose source file is in scope |
| `reindex_chunks` | Chunks a full reindex of the scope would produce |
| `reindex_estimated_tokens` | Estimated embedding input tokens |
| `reindex_estimated_api_calls` | Estimated provider calls using the configured batch size |
| `sync` | Counts of `new`, `changed`, `unchanged`, and `deleted` files |
| `index_file_size` | Size of the complete index file, even for a scoped report |
| `embedding` | Provider, model, and dimensions recorded by the index |
| `last_updated` | Unix timestamp of the last index save |

## JSON output

```json
{
  "scope": "docs/",
  "is_whole_vault": false,
  "file_count": 42,
  "indexed_file_count": 41,
  "chunk_count": 184,
  "vector_count": 197,
  "edge_count": 13,
  "reindex_chunks": 191,
  "reindex_estimated_tokens": 68320,
  "reindex_estimated_api_calls": 2,
  "index_file_size": 12345678,
  "embedding": {
    "provider": "OpenAI",
    "model": "text-embedding-3-small",
    "dimensions": 1536
  },
  "sync": {
    "new": 1,
    "changed": 1,
    "unchanged": 40,
    "deleted": 0
  },
  "last_updated": 1770000000
}
```

`scope` is `"."` and `is_whole_vault` is `true` for an unscoped call.

## Notes

- `info` is read-only: it does not ingest files or modify the index.
- Reindex counts are estimates based on the current files and chunking configuration.
- A Shard is a named folder scope over the shared collection index, not a separate index.

## Related commands

- [`mdvdb status`](./status.md) -- Quick whole-index health and compatibility summary
- [`mdvdb tree`](./tree.md) -- Per-file sync state
- [`mdvdb ingest`](./ingest.md) -- Update or rebuild the index
- [`mdvdb shards`](./shards.md) -- Manage named folder scopes

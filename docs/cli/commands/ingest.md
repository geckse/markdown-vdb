---
title: "mdvdb ingest"
description: "Incrementally index Markdown, refresh graph and topic analysis, and materialize computed fields"
category: "commands"
---

# mdvdb ingest

Discover and index Markdown files. Normal ingestion is incremental: unchanged files are skipped by content hash, while new, changed, and deleted files update the vector index, lexical index, link graph, schemas, analysis, and computed fields.

## Usage

```bash
mdvdb ingest [OPTIONS]
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `--reindex` | `false` | Re-embed all files, ignoring stored content hashes |
| `--file <PATH>` | | Ingest one project-relative Markdown file |
| `--preview` | `false` | Parse and estimate the operation without provider calls or writes |
| `--json-lines` | `false` | Stream progress and the final result as newline-delimited JSON |

`--full` remains a hidden, deprecated alias for `--reindex`. The command also accepts all [global options](./index.md#global-options).

## When to reindex

Use `--reindex` after changing the embedding provider, model, or dimensions. mdvdb also reports embedding incompatibility through [`mdvdb status`](./status.md).

```bash
mdvdb ingest --reindex
```

## Single-file and preview modes

```bash
# Update one file
mdvdb ingest --file docs/getting-started.md

# Re-embed one file
mdvdb ingest --file docs/getting-started.md --reindex

# Estimate files, chunks, tokens, and calls without mutating anything
mdvdb ingest --preview --json
```

Preview output contains per-file `New`, `Changed`, or `Unchanged` status plus aggregate chunk, token, and API-call estimates.

## What ingestion updates

At a high level, mdvdb:

1. Discovers files using source and ignore configuration.
2. Parses frontmatter and Markdown structure, chunks content, and computes hashes.
3. Embeds new or changed chunks and semantic edge context, with provider-aware batching and input limits.
4. Reconciles the vector snapshot and Tantivy lexical index, deleted files, and the link/relation graph.
5. Refreshes automatic communities (Leiden by default, with K-means available), user Topics, and Shard-local topic sidecars.
6. Rebuilds global and scoped schemas on a collection-wide ingest.
7. Runs Formula first, then Lookup/Rollup, materializing declared computed keys in frontmatter.
8. Atomically persists coherent index/module state and commits the lexical index.

Computed-field modules preserve document bodies and unrelated YAML, but they intentionally write declared computed keys. Their reports are included in ingest output.

## JSON output

```json
{
  "files_indexed": 12,
  "files_skipped": 45,
  "files_removed": 1,
  "chunks_created": 87,
  "api_calls": 3,
  "estimated_input_tokens": 18420,
  "files_failed": 0,
  "errors": [],
  "module_reports": [
    {
      "module": "formula",
      "event": "files_changed",
      "files_evaluated": 12,
      "fields_updated": 3,
      "diagnostics": [],
      "duration_ms": 5
    },
    {
      "module": "lookup_rollup",
      "event": "files_changed",
      "files_evaluated": 12,
      "fields_updated": 2,
      "diagnostics": [],
      "duration_ms": 7
    }
  ],
  "duration_secs": 4.235,
  "cancelled": false
}
```

| Field | Description |
|-------|-------------|
| `files_indexed`, `files_skipped`, `files_removed` | File reconciliation counts |
| `chunks_created` | Chunks created for files processed in this run |
| `api_calls` | Actual embedding provider calls |
| `estimated_input_tokens` | Provider-independent local estimate for successfully embedded inputs |
| `files_failed`, `errors` | Per-file parse/chunk failures; each error has `path` and `message` |
| `module_reports` | Ordered Formula and Lookup/Rollup outcomes |
| `duration_secs` | Total wall-clock duration |
| `timings` | Verbosity-gated phase timings, included with `-v` |
| `cancelled` | Whether cooperative cancellation was observed |

Each module report contains `module`, `event`, `files_evaluated`, `fields_updated`, `diagnostics`, and `duration_ms`. Diagnostics include their document path, field, stable code, message, and optional source span.

## NDJSON progress

`--json-lines` emits one JSON object per line, suitable for a long-running frontend or agent process:

```json
{"type":"progress","data":{"phase":"parsing","current":4,"total":20,"path":"docs/api.md","elapsed_ms":31,"accumulated_errors":0},"operation":"ingest"}
{"type":"progress","data":{"phase":"embedding","completed_batches":1,"total_batches":3,"completed_chunks":64,"total_chunks":142,"estimated_input_tokens":9200,"total_estimated_input_tokens":18420,"api_calls":1,"elapsed_ms":402,"accumulated_errors":0},"operation":"ingest"}
{"type":"result","data":{"files_indexed":12,"files_skipped":45,"files_removed":1,"chunks_created":87,"api_calls":3,"estimated_input_tokens":18420,"files_failed":0,"errors":[],"module_reports":[],"duration_secs":4.235,"cancelled":false},"operation":"ingest"}
```

Progress phase payloads vary by phase and may include `preparing`, `probing`, `discovering`, `parsing`, `skipped`, `file_error`, `embedding`, `saving`, `clustering`, `cleaning`, `cancelled`, and `done`.

## Notes

- Ctrl+C cancellation is cooperative and is observed at safe pipeline boundaries.
- Ordinary incremental ingestion avoids re-embedding an unchanged body; frontmatter-only changes can still refresh metadata and computed fields.
- `--preview` performs no network requests and does not modify Markdown or the index.

## Related commands

- [`mdvdb search`](./search.md) -- Query the updated index
- [`mdvdb status`](./status.md) -- Check compatibility and counts
- [`mdvdb info`](./info.md) -- Inspect sync state and reindex estimates
- [`mdvdb modules`](./modules.md) -- Manually recompute or diagnose computed fields
- [`mdvdb watch`](./watch.md) -- Apply incremental updates continuously

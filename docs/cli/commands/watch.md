---
title: "mdvdb watch"
description: "Continuously reconcile Markdown, schemas, graph data, and computed fields"
category: "commands"
---

# mdvdb watch

Watch configured source folders and apply incremental updates when Markdown files are created,
modified, renamed, or deleted. The watcher also observes the project-root
`.markdownvdb.schema.yml` so Formula, Lookup, Rollup, and Relation definitions can react without a
manual restart.

## Usage

```bash
mdvdb watch [GLOBAL OPTIONS]
```

There are no command-specific flags. Common forms are:

```bash
mdvdb watch
mdvdb watch --root /path/to/project
mdvdb watch --json
mdvdb watch -vv --no-color
```

`--json` is a long-lived NDJSON stream: one startup object followed by one object per processed
event.

## Incremental behavior

Every event is serialized through the project module lock so raw index changes, computed-field
evaluation, Markdown writeback, and the final save remain one coherent operation.

| Change | Work performed |
|--------|----------------|
| Duplicate/no-op filesystem event | Skip when both source and embedding-body hashes still match |
| Frontmatter-only edit | Refresh metadata, Relations, schemas, and computed modules without embedding, FTS, or cluster work |
| Body create/change | Parse, chunk, adaptively embed, update vector/FTS data, links/Relations, schemas, collection clusters/Topics, then Formula and Lookup/Rollup |
| Delete | Remove vector/FTS/link and collection analysis state, refresh schemas, and run dependent modules |
| Rename | Remove the old identity, index the new identity, and report both paths |
| Schema overlay edit | Refresh global/scoped schemas and relation classification, then run modules with `schema_changed`; no embedding call |

Formula runs before Lookup/Rollup. Successful computed values are atomically materialized into
declared frontmatter keys; bodies and unrelated YAML are preserved. The filesystem echo caused by
that writeback is recognized as a no-op, preventing a watch loop.

Automatic collection clusters and compatible collection Topic assignments are updated
incrementally. New or changed Topic definitions, and Shard-local Topic state, can still require a
collection-wide [`mdvdb ingest`](./ingest.md).

## Watched paths and debounce

Project YAML controls the source folders and debounce interval:

```yaml
# .markdownvdb/config.yaml
watch:
  debounce_ms: 300
sources:
  dirs:
    - .
```

Equivalent shell overrides are `MDVDB_WATCH_DEBOUNCE_MS` and comma-separated
`MDVDB_SOURCE_DIRS`. Source folders are watched recursively. Missing configured folders are skipped.

Markdown events still pass normal discovery and ignore rules, including `.gitignore`,
`.mdvdbignore`, built-in exclusions, and configured ignore patterns. The schema overlay is watched
at the project root even when the project root is not a source folder.

## Human-readable output

Startup lists the watched source folders:

```text
  ● Watching for changes

  →  .

  Press Ctrl+C to stop
```

Each processed event prints a compact status line:

```text
  ✓ indexed  docs/new-page.md (5 chunks) 142ms
  − deleted  docs/old-page.md 12ms
  ↻ renamed  docs/renamed.md (5 chunks) 156ms
  ✗ error    docs/broken.md 3ms — invalid frontmatter
```

## JSON stream

The first line is:

```json
{"status":"watching","message":"File watching started"}
```

Subsequent lines serialize `WatchEventReport` directly:

```json
{
  "event_type": "Modified",
  "path": "docs/api.md",
  "chunks_processed": 5,
  "estimated_input_tokens": 1180,
  "api_calls": 1,
  "duration_ms": 142,
  "success": true,
  "error": null,
  "module_reports": [
    {
      "module": "formula",
      "event": "files_changed",
      "files_evaluated": 1,
      "fields_updated": 1,
      "diagnostics": [],
      "duration_ms": 2
    },
    {
      "module": "lookup_rollup",
      "event": "files_changed",
      "files_evaluated": 2,
      "fields_updated": 1,
      "diagnostics": [],
      "duration_ms": 3
    }
  ]
}
```

Rename reports additionally contain `previous_path`:

```json
{"event_type":"Renamed","path":"docs/new-name.md","previous_path":"docs/old-name.md","chunks_processed":5,"estimated_input_tokens":1180,"api_calls":1,"duration_ms":156,"success":true,"error":null,"module_reports":[]}
```

| Field | Description |
|-------|-------------|
| `event_type` | `Created`, `Modified`, `Deleted`, or `Renamed`; schema changes report as `Modified` |
| `path` | Project-relative affected path |
| `previous_path` | Old project-relative path for a rename; omitted otherwise |
| `chunks_processed` | Chunks embedded/upserted; zero for deletes, no-ops, frontmatter-only, and schema-only work |
| `estimated_input_tokens` | Local count of successfully embedded input tokens |
| `api_calls` | Embedding calls made for the event |
| `duration_ms` | Event processing duration |
| `success`, `error` | Outcome and optional error text |
| `module_reports` | Ordered always-on module outcomes and diagnostics |

For an overlay edit, `path` is `.markdownvdb.schema.yml`, embedding counts are zero, and module
reports use the `schema_changed` event.

### Streaming examples

```bash
# Show failures while keeping the process attached
mdvdb watch --json | jq 'select(.success == false)'

# Observe computed-field updates
mdvdb watch --json | jq 'select(.module_reports | length > 0)'

# Append the NDJSON stream
mdvdb watch --json >> watch-events.jsonl
```

## Lifecycle and failures

- `watch` opens the index read-write and requires a usable embedding provider at startup.
- If an incompatible archived index had to be recreated, run an unscoped `mdvdb ingest` before
  starting the watcher.
- One event failure is reported and logged; the loop continues processing later events.
- Ctrl+C cancels the loop at an event boundary. Store updates use atomic/recoverable coordination,
  and startup repairs an interrupted vector/FTS reconciliation when required.

## Related commands

- [`mdvdb ingest`](./ingest.md) -- Bootstrap, rebuild, or catch up collection-wide derived state
- [`mdvdb modules`](./modules.md) -- Manually validate, run, or inspect computed modules
- [`mdvdb tree`](./tree.md) -- Compare disk and index sync state
- [`mdvdb status`](./status.md) -- Check embedding compatibility before watching
- [Ignore Files](../concepts/ignore-files.md) -- Discovery and ignore behavior

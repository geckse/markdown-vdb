---
title: "mdvdb collection"
description: "Query a folder as a table of Markdown documents and frontmatter columns"
category: "commands"
---

# mdvdb collection

Query a folder as a table: Markdown files are rows and frontmatter fields are columns. `mdvdb list` is an alias for this command.

The interface is SQL-like, not a SQL parser. A folder or Shard supplies the table scope, repeated filters act like equality predicates, and sorting plus limit/offset provide deterministic pagination.

## Usage

```bash
mdvdb collection [OPTIONS] [PATH]
```

## Arguments and options

| Name | Short | Default | Description |
|------|-------|---------|-------------|
| `[PATH]` | | `.` | Relative folder scope |
| `--shard <ID>` | | | Use a configured Shard; conflicts with `PATH` |
| `--recursive` | `-r` | `false` | Include all nested folders; otherwise return direct children only |
| `--sort <FIELD>` | | path | Sort by a top-level frontmatter field |
| `--order <ORDER>` | | `asc` | `asc` or `desc`; used with `--sort` |
| `--filter <KEY=VALUE>` | `-f` | | Equality filter; repeat for AND logic |
| `--limit <N>` | | all | Maximum rows in the page |
| `--offset <N>` | | `0` | Rows to skip after filtering and sorting |
| `--populate` | | `false` | Resolve frontmatter relations on returned rows, one level deep |

The command also accepts all [global options](./index.md#global-options).

## Query semantics

Without `--recursive`, `mdvdb collection projects` returns Markdown files directly inside `projects/`. With `--recursive`, it also includes documents in nested folders.

`--filter` exposes equality only in the CLI:

- Each filter addresses one top-level frontmatter field.
- Repeated filters use AND logic.
- Exact lowercase `true` and `false` become booleans; valid JSON numbers become numbers; other values remain strings.
- A list field matches when any item equals the filter value.
- Materialized Formula, Lookup, and Rollup values participate in filtering and sorting.
- Link-shaped relation values are syntax-normalized, so `clients/acme`, `clients/acme.md`, and `[[clients/acme]]` can match `[[clients/acme|Acme]]`.

Sorting is type-aware and keeps missing or null values last in either direction. If `--sort` is omitted, rows are ordered by path ascending.

## Examples

```bash
# Direct children of projects/
mdvdb collection projects

# SQL-like filtering, ordering, and pagination
mdvdb collection invoices \
  --filter status=paid \
  --sort total \
  --order desc \
  --limit 20 \
  --offset 0 \
  --populate \
  --json

# Query a named Shard recursively
mdvdb collection --shard research --recursive --json
```

The invoice example is analogous to:

```sql
SELECT * FROM invoices
WHERE status = 'paid'
ORDER BY total DESC
LIMIT 20 OFFSET 0;
```

`--populate` additionally resolves relation targets; it is not an arbitrary SQL join.

## JSON output

The response contains stable column metadata, the requested page, and `total_rows` after filtering but before pagination:

```json
{
  "scope": "invoices/",
  "recursive": false,
  "columns": [
    {
      "name": "total",
      "field_type": "Formula",
      "description": "Invoice total",
      "occurrence_count": 18,
      "sample_values": ["125.00", "240.00"],
      "allowed_values": null,
      "required": false,
      "in_schema": true,
      "relation_target": null,
      "formula": "price * quantity",
      "result_type": "Number",
      "relation_field": null,
      "target_field": null,
      "relation_direction": null,
      "relation_scope": null
    }
  ],
  "rows": [
    {
      "path": "invoices/invoice-104.md",
      "title": "Invoice 104",
      "title_source": "frontmatter",
      "frontmatter": {
        "status": "paid",
        "price": 120,
        "quantity": 2,
        "total": 240,
        "client": "[[clients/acme]]"
      },
      "computed_fields": {"total": 240},
      "computed_field_errors": {},
      "content_hash": "a1b2c3...",
      "file_size": 712,
      "modified_at": 1770000000,
      "indexed_at": 1770000100,
      "state": "indexed",
      "relations": {
        "client": [
          {
            "raw": "[[clients/acme]]",
            "path": "clients/acme.md",
            "exists": true,
            "title": "Acme",
            "frontmatter": {"title": "Acme"}
          }
        ]
      }
    }
  ],
  "total_rows": 18,
  "limit": 20,
  "offset": 0
}
```

Important response details:

- `columns` is calculated from the complete filtered result, so it stays stable across pages.
- `frontmatter` is always an object and is the authoritative materialized metadata.
- `computed_fields` mirrors successful computed values; `computed_field_errors` is keyed by field name.
- `relations` is present only with `--populate`. Target frontmatter is not populated recursively.
- `limit` is omitted when no limit was requested.

## Related commands

- [`mdvdb schema`](./schema.md) -- Inspect typed columns and computed definitions
- [`mdvdb modules`](./modules.md) -- Validate and recompute Formula, Lookup, and Rollup fields
- [`mdvdb get`](./get.md) -- Inspect one document
- [`mdvdb search`](./search.md) -- Combine retrieval with frontmatter equality filters

---
title: "mdvdb modules"
description: "Inspect, validate, run, and diagnose built-in computed-field modules"
category: "commands"
---

# mdvdb modules

Inspect and run the built-in modules that materialize computed frontmatter fields declared in `.markdownvdb.schema.yml`.

| Module ID | Owns | Execution order |
|-----------|------|-----------------|
| `formula` | `Formula` fields calculated from the current document | First |
| `lookup_rollup` | `Lookup` fields copied from related documents and `Rollup` aggregations | Second |

Both modules are always on during ingest, watch updates, and schema changes. `modules run` is the explicit recomputation surface.

## Usage

```bash
mdvdb modules <COMMAND> [OPTIONS]
```

| Command | Description |
|---------|-------------|
| `list` | List compiled-in modules, versions, and hooks |
| `validate <MODULE>` | Validate a Formula or Rollup expression without changing files or the index |
| `run <MODULE>` | Recompute persisted derived data |
| `status <MODULE>` | Read cached diagnostics without running expressions |

Valid module IDs are `formula` and `lookup_rollup`.

## List modules

```bash
mdvdb modules list
mdvdb modules list --json
```

JSON output is an array of descriptors:

```json
[
  {
    "id": "formula",
    "name": "Formula",
    "version": 1,
    "always_on": true,
    "hooks": ["full_ingest", "files_changed", "schema_changed", "manual_run"]
  },
  {
    "id": "lookup_rollup",
    "name": "Lookup & Rollup",
    "version": 1,
    "always_on": true,
    "hooks": ["full_ingest", "files_changed", "schema_changed", "manual_run"]
  }
]
```

## Validate an expression

```bash
mdvdb modules validate <MODULE> \
  --formula <EXPRESSION> \
  --result-type <TYPE>
```

Supported result types are `string`, `number`, `boolean`, `date`, `datetime`, `list`, and `json`. Use the `formula` module for a document Formula and `lookup_rollup` for a Rollup expression, where `values` is the reserved input list.

```bash
# Formula expression
mdvdb modules validate formula \
  --formula 'price * quantity' \
  --result-type number

# Rollup expression
mdvdb modules validate lookup_rollup \
  --formula 'values.reduce((sum, value) => sum + Number(value), 0)' \
  --result-type number \
  --json
```

JSON output:

```json
{
  "valid": false,
  "diagnostics": [
    {
      "module": "formula",
      "field": "",
      "code": "syntax_error",
      "message": "...",
      "span": {"start": 6, "end": 7}
    }
  ]
}
```

Validation uses mdvdb's deterministic JavaScript-like expression subset. It does not run a JavaScript VM and exposes no filesystem, network, random, or ambient-clock access.

## Run a module

```bash
mdvdb modules run <MODULE> [--path <PREFIX> | --shard <ID>]
```

```bash
# Recompute all Formula fields
mdvdb modules run formula

# Recompute Lookup and Rollup fields for one folder
mdvdb modules run lookup_rollup --path invoices --json

# Recompute in a named Shard
mdvdb modules run formula --shard operations
```

`--path` and `--shard` conflict. A manual run is dependency-aware: mdvdb may run prerequisite or downstream built-ins required to make the requested results coherent.

Unlike `list`, `validate`, and `status`, `run` is a materializing operation. It atomically updates declared computed keys in Markdown frontmatter and persists matching index state; document bodies and unrelated frontmatter are preserved.

The JSON response flattens the requested module report at the top level and also returns the ordered pipeline:

```json
{
  "module": "lookup_rollup",
  "event": "manual_run",
  "files_evaluated": 12,
  "fields_updated": 4,
  "diagnostics": [],
  "duration_ms": 8,
  "module_reports": [
    {
      "module": "formula",
      "event": "manual_run",
      "files_evaluated": 3,
      "fields_updated": 1,
      "diagnostics": [],
      "duration_ms": 2
    },
    {
      "module": "lookup_rollup",
      "event": "manual_run",
      "files_evaluated": 12,
      "fields_updated": 4,
      "diagnostics": [],
      "duration_ms": 8
    }
  ]
}
```

## Show cached diagnostics

```bash
mdvdb modules status <MODULE> [--path <PREFIX> | --shard <ID>]
```

With `--json`, the result is an array of diagnostics:

```json
[
  {
    "path": "invoices/invoice-104.md",
    "module": "formula",
    "field": "total",
    "code": "unknown_identifier",
    "message": "unknown identifier `quantity`",
    "span_start": 8,
    "span_end": 16
  }
]
```

## Schema examples

```yaml
# .markdownvdb.schema.yml
scopes:
  invoices:
    fields:
      total:
        field_type: formula
        formula: price * quantity
        result_type: number
      client:
        field_type: relation
        target: clients
      client_domain:
        field_type: lookup
        relation_field: client
        target_field: domain
```

See [`mdvdb schema`](./schema.md) to inspect the merged definitions and [`mdvdb get`](./get.md) to inspect values and per-field errors.

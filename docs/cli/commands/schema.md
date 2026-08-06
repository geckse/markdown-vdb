---
title: "mdvdb schema"
description: "Inspect inferred and declared frontmatter field types for a collection, folder, or Shard"
category: "commands"
---

# mdvdb schema

Show the merged metadata schema for indexed Markdown documents. mdvdb infers fields from YAML frontmatter and applies optional definitions from `.markdownvdb.schema.yml`, including relations and computed fields.

## Usage

```bash
mdvdb schema [--path <PREFIX> | --shard <ID>] [GLOBAL OPTIONS]
```

| Flag | Description |
|------|-------------|
| `--path <PREFIX>` | Restrict the schema to a relative folder prefix |
| `--shard <ID>` | Use the folder configured for a named Shard |

`--path` and `--shard` conflict. Without either option, the command returns the collection-wide schema. It also accepts all [global options](./index.md#global-options).

## Examples

```bash
mdvdb schema
mdvdb schema --path invoices
mdvdb schema --shard operations
mdvdb schema --path invoices --json
```

## Field types

JSON serializes these names in PascalCase:

| Type | Meaning |
|------|---------|
| `String` | Text value |
| `Number` | Integer or decimal value |
| `Boolean` | `true` or `false` |
| `List` | YAML sequence |
| `Date` | Date-shaped text such as `2026-08-06` |
| `Mixed` | More than one incompatible observed type |
| `Relation` | Whole-value reference to another Markdown document |
| `File` | Reference to a non-Markdown file |
| `Json` | Structured object or nested collection |
| `Formula` | Deterministic value calculated from the current document |
| `Lookup` | Value copied from a related document |
| `Rollup` | Formula evaluated over values collected from related documents |

## Schema overlay

The optional overlay adds annotations, type declarations, relation targets, and computed-field definitions. Global `fields` apply collection-wide; `scopes` apply to folder prefixes.

```yaml
# .markdownvdb.schema.yml
fields:
  status:
    description: Document lifecycle status
    allowed_values: [draft, review, published]
    required: true

scopes:
  invoices:
    fields:
      client:
        field_type: relation
        target: clients
      total:
        field_type: formula
        formula: price * quantity
        result_type: number
      client_domain:
        field_type: lookup
        relation_field: client
        target_field: domain
      related_total:
        field_type: rollup
        relation_field: related_invoices
        target_field: total
        formula: values.reduce((sum, value) => sum + Number(value), 0)
        result_type: number
```

Incoming Rollups additionally use `relation_direction: incoming` and `relation_scope` to select the source folder. Lookup traversal is always outgoing.

After changing the overlay, run an unscoped [`mdvdb ingest`](./ingest.md) to refresh persisted schemas and materialized computed values.

## JSON output

Unscoped output is a `Schema`:

```json
{
  "fields": [
    {
      "name": "client",
      "field_type": "Relation",
      "description": null,
      "occurrence_count": 18,
      "sample_values": ["[[clients/acme]]"],
      "allowed_values": null,
      "required": false,
      "relation_target": "clients",
      "formula": null,
      "result_type": null,
      "relation_field": null,
      "target_field": null,
      "relation_direction": null,
      "relation_scope": null
    },
    {
      "name": "total",
      "field_type": "Formula",
      "description": null,
      "occurrence_count": 18,
      "sample_values": ["125.00", "240.00"],
      "allowed_values": null,
      "required": false,
      "relation_target": null,
      "formula": "price * quantity",
      "result_type": "Number",
      "relation_field": null,
      "target_field": null,
      "relation_direction": null,
      "relation_scope": null
    }
  ],
  "last_updated": 1770000100
}
```

Scoped output wraps the same structure:

```json
{
  "scope": "invoices/",
  "schema": {
    "fields": [],
    "last_updated": 1770000100
  }
}
```

### SchemaField keys

| Key | Description |
|-----|-------------|
| `name`, `field_type` | Column name and resolved type |
| `description`, `allowed_values`, `required` | Overlay annotations |
| `occurrence_count`, `sample_values` | Inferred/materialized field statistics |
| `relation_target` | Declared target folder for a Relation |
| `formula`, `result_type` | Formula/Rollup expression and declared output type |
| `relation_field`, `target_field` | Relation to follow and exact field to copy/aggregate |
| `relation_direction` | `Outgoing` or `Incoming` for Lookup/Rollup definitions |
| `relation_scope` | Source folder used by an incoming Rollup |

Optional metadata keys are always present and serialize as `null` when not applicable.

## Notes

- `schema` is read-only; it does not evaluate or materialize fields.
- Fields are ordered alphabetically and samples are capped at 20 values.
- The human-readable view shows up to five samples and marks required fields.

## Related commands

- [`mdvdb collection`](./collection.md) -- Query folders using these fields as columns
- [`mdvdb modules`](./modules.md) -- Validate and materialize computed fields
- [`mdvdb search`](./search.md) -- Filter retrieval by frontmatter values
- [`mdvdb get`](./get.md) -- Inspect one document's values and diagnostics

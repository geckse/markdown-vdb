---
title: "mdvdb get"
description: "Inspect one indexed document, its frontmatter, computed fields, and optional relations"
category: "commands"
---

# mdvdb get

Retrieve index metadata for one Markdown document. The command reports its frontmatter, computed values and diagnostics, chunk count, file size, content hash, and timestamps.

## Usage

```bash
mdvdb get [OPTIONS] <FILE_PATH>
```

| Name | Required | Description |
|------|----------|-------------|
| `<FILE_PATH>` | Yes | Project-relative path as stored in the index |
| `--populate` | No | Resolve frontmatter relations and reverse references, one level deep |

The command also accepts all [global options](./index.md#global-options), including `--json` and `--root`.

## Examples

```bash
# Inspect metadata
mdvdb get docs/api.md

# Include resolved relation targets and documents that reference this file
mdvdb get invoices/invoice-104.md --populate

# Machine-readable output
mdvdb get invoices/invoice-104.md --populate --json
```

## JSON output

```json
{
  "path": "invoices/invoice-104.md",
  "content_hash": "a1b2c3d4e5f67890...",
  "frontmatter": {
    "title": "Invoice 104",
    "client": "[[clients/acme]]",
    "price": 120,
    "quantity": 2,
    "total": 240
  },
  "computed_fields": {
    "total": 240
  },
  "computed_field_errors": {},
  "chunk_count": 3,
  "file_size": 712,
  "indexed_at": 1770000100,
  "modified_at": 1770000000,
  "relations": {
    "client": [
      {
        "raw": "[[clients/acme]]",
        "path": "clients/acme.md",
        "exists": true,
        "title": "Acme",
        "frontmatter": {
          "title": "Acme",
          "domain": "acme.example"
        }
      }
    ]
  },
  "referenced_by": [
    {
      "source": "payments/payment-104.md",
      "field": "invoice",
      "title": "Payment 104"
    }
  ]
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `path` | `string` | Project-relative document path |
| `content_hash` | `string` | SHA-256 hash used for change detection |
| `frontmatter` | `object \| null` | Parsed, materialized YAML frontmatter; authoritative value surface |
| `computed_fields` | `object` | Successful Formula, Lookup, and Rollup values mirrored for provenance |
| `computed_field_errors` | `object` | Diagnostics keyed by computed field name |
| `chunk_count` | `number` | Number of indexed text chunks |
| `file_size` | `number` | File size in bytes |
| `indexed_at` | `number` | Unix timestamp of the last ingest |
| `modified_at` | `number \| null` | Filesystem modification timestamp |
| `relations` | `object` | Resolved frontmatter relations; present only with `--populate` |
| `referenced_by` | `array` | Reverse frontmatter references; present only with `--populate` |

A computed-field diagnostic has `module`, `field`, stable `code`, human-readable `message`, and optional `span_start`/`span_end` byte offsets.

Each populated relation contains:

- `raw`: the literal frontmatter value.
- `path`: resolved project-relative target, or `null` if it cannot be resolved.
- `exists`: whether the target exists in the indexed file set.
- `title`: target display title, or `null` for a missing target.
- `frontmatter`: raw target frontmatter, or `null`; it is never populated recursively.

When `--populate` is omitted, `relations` and `referenced_by` are absent rather than `null`.

## Notes

- `get` returns metadata and matched relation context, not the Markdown body.
- `get` is read-only and never recomputes modules or modifies the index.
- If the path is not indexed, run [`mdvdb ingest`](./ingest.md) and check [`mdvdb tree`](./tree.md).

## Related commands

- [`mdvdb collection`](./collection.md) -- Query a folder as frontmatter rows and columns
- [`mdvdb modules`](./modules.md) -- Recompute or inspect computed fields
- [`mdvdb schema`](./schema.md) -- Inspect field definitions
- [`mdvdb links`](./links.md) -- Traverse document links

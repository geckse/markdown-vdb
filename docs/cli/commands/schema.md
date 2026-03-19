---
title: "mdvdb schema"
description: "Show the auto-inferred metadata schema from frontmatter across all indexed markdown files"
category: "commands"
---

# mdvdb schema

Show the metadata schema auto-inferred from YAML frontmatter across all indexed markdown files. The schema reports every frontmatter key found, its inferred type, how many files contain it, sample values, and optional overlay annotations (descriptions, allowed values, required flags).

## Usage

```bash
mdvdb schema [OPTIONS]
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `--path <PREFIX>` | *(none)* | Restrict schema inference to files under this path prefix |

### Option Details

#### `--path <PREFIX>`

Restricts schema inference to files whose relative path starts with the given prefix. This is useful for projects with distinct content areas (e.g., `blog/`, `docs/`, `notes/`) where frontmatter conventions differ by directory.

When `--path` is used, the output includes the scope label and only considers files matching the prefix. The inferred types, occurrence counts, and sample values reflect only the scoped subset.

```bash
# Show schema for blog posts only
mdvdb schema --path blog

# Show schema for files under docs/api/
mdvdb schema --path docs/api
```

## Global Options

These options apply to all commands. See [Commands Index](./index.md) for details.

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Increase log verbosity (-v info, -vv debug, -vvv trace) |
| `--root` | | Project root directory (defaults to current directory) |
| `--no-color` | | Disable colored output |
| `--json` | | Output results as JSON |

## How Schema Inference Works

The schema is automatically inferred from the YAML frontmatter of all indexed markdown files. No configuration is required -- mdvdb examines every frontmatter key-value pair and determines:

1. **Field name** -- The frontmatter key (e.g., `title`, `tags`, `date`)
2. **Field type** -- Inferred from observed values across all files:
   - `string` -- Text values
   - `number` -- Integer or floating-point values
   - `boolean` -- `true` / `false` values
   - `list` -- Array/sequence values (e.g., `tags: [rust, cli]`)
   - `date` -- Strings matching `YYYY-MM-DD` format (with optional `T` suffix for datetime)
   - `mixed` -- Field has values of multiple different types across files
3. **Occurrence count** -- How many files contain this field
4. **Sample values** -- Up to 20 unique values observed across files

### Schema Overlay

You can enhance the inferred schema with a `.markdownvdb.schema.yml` overlay file that adds descriptions, type overrides, allowed values, and required flags. The overlay is merged with the inferred schema -- it does not replace it.

```yaml
# .markdownvdb.schema.yml
fields:
  status:
    description: "Document lifecycle status"
    allowed_values: ["draft", "review", "published"]
    required: true
  category:
    description: "Content category"
    field_type: "string"

scopes:
  blog:
    fields:
      author:
        description: "Blog post author"
        required: true
```

### Scoped Schemas

When the index contains scoped schemas (discovered from top-level directory prefixes), the `--path` flag retrieves the pre-computed scoped schema. If no pre-computed schema exists for the prefix, mdvdb infers one on-the-fly from discovered files matching the prefix.

## Human-Readable Output

When run without `--json`, schema displays a formatted field list with occurrence bars:

```
  ● Metadata Schema (5 fields)

  title (string) ████████████████████ 57/57
    Samples: Getting Started, API Reference, Configuration Guide

  tags (list) ████████████░░░░░░░░ 34/57
    Samples: [rust, cli], [documentation], [api, rest]

  date (date) ██████████████░░░░░░ 42/57
    Samples: 2024-01-15, 2024-03-22, 2024-06-01

  status (string) ████████░░░░░░░░░░░░ 23/57 [required]
    Document lifecycle status
    Samples: draft, review, published
    Allowed: draft, review, published

  author (string) ██████░░░░░░░░░░░░░░ 18/57
    Samples: Alice, Bob, Charlie
```

### Output Fields

| Element | Description |
|---------|-------------|
| **Field name** | The frontmatter key, shown in bold |
| **Type** | Inferred type in parentheses (string, number, boolean, list, date, mixed) |
| **Occurrence bar** | 20-character bar showing the proportion of files containing this field |
| **Count** | `occurrence_count / total_documents` |
| **[required]** | Shown if the field is marked required in the overlay |
| **Description** | From the overlay file, shown dimmed below the field name |
| **Samples** | Up to 5 sample values, shown dimmed |
| **Allowed** | Allowed values from the overlay, shown in cyan |

### Scoped Output

When `--path` is used, a scope label is shown above the schema:

```
  Scope: blog

  ● Metadata Schema (3 fields)

  ...
```

## Examples

```bash
# Show full metadata schema
mdvdb schema

# Show schema as JSON
mdvdb schema --json

# Show schema scoped to blog posts
mdvdb schema --path blog

# Show scoped schema as JSON
mdvdb schema --path docs --json

# Show schema for a specific project
mdvdb schema --root /path/to/project
```

## JSON Output

### Schema (full, `--json`)

```json
{
  "fields": [
    {
      "name": "title",
      "field_type": "String",
      "description": null,
      "occurrence_count": 57,
      "sample_values": ["Getting Started", "API Reference", "Configuration Guide"],
      "allowed_values": null,
      "required": false
    },
    {
      "name": "tags",
      "field_type": "List",
      "description": null,
      "occurrence_count": 34,
      "sample_values": ["[rust, cli]", "[documentation]"],
      "allowed_values": null,
      "required": false
    },
    {
      "name": "status",
      "field_type": "String",
      "description": "Document lifecycle status",
      "occurrence_count": 23,
      "sample_values": ["draft", "review", "published"],
      "allowed_values": ["draft", "review", "published"],
      "required": true
    }
  ],
  "last_updated": 1710856200
}
```

### ScopedSchema (with `--path`, `--json`)

```json
{
  "scope": "blog",
  "schema": {
    "fields": [
      {
        "name": "author",
        "field_type": "String",
        "description": "Blog post author",
        "occurrence_count": 12,
        "sample_values": ["Alice", "Bob"],
        "allowed_values": null,
        "required": true
      }
    ],
    "last_updated": 1710856200
  }
}
```

### Schema Fields

| Field | Type | Description |
|-------|------|-------------|
| `fields` | `SchemaField[]` | Array of schema fields, sorted alphabetically by name |
| `last_updated` | `number` | Unix timestamp (seconds since epoch) of when the schema was generated |

### SchemaField Fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Frontmatter key name |
| `field_type` | `string` | Inferred type: `"String"`, `"Number"`, `"Boolean"`, `"List"`, `"Date"`, or `"Mixed"` |
| `description` | `string \| null` | Human-readable description from overlay file |
| `occurrence_count` | `number` | Number of files containing this field |
| `sample_values` | `string[]` | Up to 20 unique sample values observed across files |
| `allowed_values` | `string[] \| null` | Allowed values from overlay file |
| `required` | `boolean` | Whether the field is marked required in the overlay (defaults to `false`) |

### ScopedSchema Fields

| Field | Type | Description |
|-------|------|-------------|
| `scope` | `string` | Path prefix used for scoping (e.g., `"blog"`) |
| `schema` | `Schema` | The schema for files under this scope |

### FieldType Values

| Value | Description |
|-------|-------------|
| `"String"` | Text values |
| `"Number"` | Integer or floating-point values |
| `"Boolean"` | `true` / `false` values |
| `"List"` | Array/sequence values |
| `"Date"` | Strings matching `YYYY-MM-DD` format |
| `"Mixed"` | Multiple different types observed across files |

## Notes

- The `schema` command opens the index in **read-only** mode. It never modifies the index.
- If no index exists or no files have been ingested, the schema will be empty.
- Fields are sorted alphabetically by name in both human-readable and JSON output.
- Sample values are capped at 20 unique values per field. The human-readable output shows up to 5.
- When using `--path`, the scope prefix is normalized to end with `/` internally (e.g., `blog` becomes `blog/`).
- The schema overlay file (`.markdownvdb.schema.yml`) is optional. Without it, the schema is purely inferred.

## Related Commands

- [`mdvdb status`](./status.md) -- Check index document count (used as denominator in occurrence ratios)
- [`mdvdb ingest`](./ingest.md) -- Index files to populate frontmatter data for schema inference
- [`mdvdb search`](./search.md) -- Use metadata filters (`-f KEY=VALUE`) based on discovered schema fields
- [`mdvdb clusters`](./clusters.md) -- View document clusters derived from the same indexed data
- [`mdvdb get`](./get.md) -- View frontmatter for a specific file

## See Also

- [Index Storage](../concepts/index-storage.md) -- How the schema is persisted in the index
- [Configuration](../configuration.md) -- Environment variables and config options
- [JSON Output Reference](../json-output.md) -- Complete JSON schema reference

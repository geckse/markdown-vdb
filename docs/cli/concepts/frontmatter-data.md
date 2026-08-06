---
title: "Frontmatter as structured data"
description: "Treat Markdown files as queryable records without giving up portable text files."
category: "concepts"
---

# Frontmatter as structured data

Markdown VDB treats a Markdown collection as both documents and structured
data:

- Each Markdown file is a record.
- Its collection-relative path is its record identifier.
- Frontmatter keys are typed fields.
- Folders and [Shards](./shards-and-topics.md) provide query scopes.
- [Relations](./relations.md) connect records.
- [Computed fields](./computed-fields.md) materialize derived values.

That gives a Markdown vault many of the useful properties of a small local
database while keeping the source of truth readable, editable Markdown.

## A record is still a Markdown file

For example, **projects/atlas.md** can contain:

~~~markdown
---
status: active
owner: "[[people/maya]]"
priority: 2
due: 2026-09-15
tags:
  - platform
  - migration
---

# Atlas

Move the billing platform to the new event pipeline.
~~~

The body remains useful for semantic and lexical search. The frontmatter adds
fields that can be filtered, sorted, inspected as a schema, or returned as
structured JSON.

## SQL-like, not SQL

The data model has deliberate SQL-like parallels:

| Markdown VDB | Database analogy |
| --- | --- |
| Markdown file | Row |
| Frontmatter key | Typed column |
| Collection folder or Shard | Table-like query scope |
| Relation | Foreign-key-like reference |
| Populate | One-level join-like expansion |
| Formula, Lookup, Rollup | Computed column |

This is a modeling analogy, not a SQL compatibility layer. Markdown VDB does
not expose a SQL parser, arbitrary JOIN syntax, a general query planner,
cross-Collection joins, or remote tables. The CLI provides purpose-built
filtering, sorting, pagination, semantic search, and relation expansion.

Use [collection](../commands/collection.md) for deterministic row-style
queries and [search](../commands/search.md) when the document text or semantic
meaning matters.

## Query records

List active projects, ordered by due date:

~~~bash
mdvdb collection projects \
  --recursive \
  --filter status=active \
  --sort due \
  --order asc \
  --json
~~~

The collection command returns files as rows with their full frontmatter.
Without **--recursive**, it returns direct children of the selected folder.
It also supports **--limit** and **--offset** for pagination.

Repeated filters are combined with AND:

~~~bash
mdvdb collection projects \
  --recursive \
  --filter status=active \
  --filter priority=2 \
  --json
~~~

CLI filters use exact **KEY=VALUE** equality. Values that look like numbers or
booleans are parsed as those types, and filtering a list checks whether it
contains the requested value. The CLI does not currently provide SQL-style
comparison operators such as greater-than or BETWEEN.

Frontmatter filters also narrow full-text and vector search:

~~~bash
mdvdb search "renewal risk" \
  --path projects \
  --filter status=active \
  --json
~~~

The filter is applied to records; the query still ranks matching document
chunks by the selected [search mode](./search-modes.md).

## Inspect the inferred schema

Markdown VDB infers a collection schema from observed frontmatter:

~~~bash
mdvdb schema --path projects --json
~~~

Common inferred field types include strings, numbers, booleans, lists, dates,
relations, files, JSON, and mixed fields. Formula, Lookup, and Rollup types
come from explicit computed-field definitions.

Inference lets a loose collection work immediately. Add
**.markdownvdb.schema.yml** when a field needs stronger intent, documentation,
allowed values, or a stable type:

~~~yaml
fields:
  owner:
    description: Person accountable for this record

scopes:
  projects:
    fields:
      status:
        field_type: string
        description: Current project lifecycle state
        allowed_values:
          - planned
          - active
          - done
        required: true
~~~

Global definitions under **fields** apply throughout the Collection. Entries
under **scopes** apply to that path and its descendants. More specific scopes
can refine inherited definitions.

See the [schema command](../commands/schema.md) for inspection and validation
details.

## Model connected data

A frontmatter field whose entire value is a Markdown path, wiki link, or
Markdown link can become a Relation:

~~~yaml
client: clients/acme.md
reviewer: "[[people/maya]]"
policy: "[Security policy](policies/security.md)"
~~~

Relations power one-level population, backlinks, graph exploration, and
link-aware search. Read [Relations](./relations.md) for the accepted shapes
and exact path resolution rules.

You can then derive values such as an invoice total, a related client's
domain, or the sum of all invoices referencing a client. These values are
written back to frontmatter and participate in the same filters and sorts as
authored fields. See [Computed fields](./computed-fields.md).

## Choose the right query surface

- Use **mdvdb collection** for predictable field filters, sorting, pagination,
  and complete records.
- Use **mdvdb search** for semantic, hybrid, lexical, or edge-aware retrieval
  with optional frontmatter filters.
- Use **mdvdb get FILE --populate** for one record plus resolved outgoing
  Relations and reverse references.
- Use **mdvdb schema** to discover fields before constructing a query.

For scripts and agents, add **--json** and follow the stable output contract
described in [JSON output](../json-output.md).

## Further reading

- [Relations](./relations.md)
- [Computed fields](./computed-fields.md)
- [Shards and Topics](./shards-and-topics.md)
- [Get a document](../commands/get.md)
- [Knowledge operations use case](../use-cases/knowledge-operations.md)

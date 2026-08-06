---
title: "Computed fields"
description: "Define Formula, Lookup, and Rollup fields that are deterministically materialized into Markdown frontmatter."
category: "concepts"
---

# Computed fields

Computed fields derive frontmatter values from the rest of the collection:

- **Formula** evaluates fields on the same record.
- **Lookup** follows an outgoing Relation and copies a target field.
- **Rollup** gathers related values and reduces them with a formula.

Definitions live in **.markdownvdb.schema.yml**. Successful results are
materialized into the owning Markdown file's frontmatter, so they can be
filtered, sorted, searched, exported, and read by tools that know nothing
about Markdown VDB.

Formula and Lookup/Rollup are built-in modules. They run during full and
incremental ingestion, while watching, after relevant schema changes, or when
requested explicitly.

## Formula

This definition computes an invoice total:

~~~yaml
scopes:
  invoices:
    fields:
      total:
        field_type: formula
        formula: quantity * unit_price
        result_type: number
~~~

Given:

~~~markdown
---
quantity: 4
unit_price: 125
---

# Invoice 1042
~~~

ingestion writes **total: 500** into the same frontmatter.

Validate an expression before adopting it:

~~~bash
mdvdb modules validate formula \
  --formula 'quantity * unit_price' \
  --result-type number
~~~

Run Formula for one scope:

~~~bash
mdvdb modules run formula --path invoices --json
~~~

Supported result types are string, number, boolean, date, datetime, list, and
JSON. Expressions use a deterministic, sandboxed JavaScript-like expression
language. They cannot access the filesystem, network, or host JavaScript
runtime.

## Lookup

Lookup follows a Relation on the current record and returns an exact top-level
field from the target:

~~~yaml
scopes:
  projects:
    fields:
      client:
        field_type: relation
        target: clients
      client_domain:
        field_type: lookup
        relation_field: client
        target_field: domain
~~~

With this project:

~~~yaml
client: clients/acme.md
~~~

and **clients/acme.md** containing:

~~~yaml
domain: acme.example
~~~

the project receives **client_domain: acme.example**.

A scalar Relation produces a scalar value or null. A Relation list produces
an ordered list. Lookup is outgoing only and does not accept a formula or
result type.

## Rollup

An incoming Rollup finds records in a source folder whose selected Relation
points to the current record. This client field totals all related invoices:

~~~yaml
scopes:
  clients:
    fields:
      invoice_total:
        field_type: rollup
        relation_direction: incoming
        relation_scope: invoices
        relation_field: client
        target_field: total
        formula: values.reduce((sum, value) => sum + Number(value), 0)
        result_type: number
~~~

Here **relation_scope** is the Collection-relative folder to scan,
**relation_field** is the ordinary Relation on each invoice, and
**target_field** is the invoice value to collect. The formula reduces the
collected **values** array.

Validate the reducer:

~~~bash
mdvdb modules validate lookup_rollup \
  --formula 'values.reduce((sum, value) => sum + Number(value), 0)' \
  --result-type number
~~~

Then run the module:

~~~bash
mdvdb modules run lookup_rollup --path clients --json
~~~

An outgoing Rollup instead follows a Relation on the owner. It omits
**relation_direction** and **relation_scope**:

~~~yaml
scopes:
  portfolios:
    fields:
      projects:
        field_type: relation
        target: projects
      total_budget:
        field_type: rollup
        relation_field: projects
        target_field: budget
        formula: values.reduce((sum, value) => sum + Number(value), 0)
        result_type: number
~~~

## Execution and dependencies

Computed values are module-owned output:

1. Formula fields run before Lookup and Rollup.
2. Lookup and Rollup definitions are dependency-ordered when one computed
   field consumes another.
3. All successful writes for a module run are applied atomically.
4. Failures produce diagnostics rather than a partial aggregate.
5. A stale owned output is removed when its definition no longer produces a
   valid value.

Inspect built-in modules and their latest state with:

~~~bash
mdvdb modules list --json
mdvdb modules status formula --path invoices --json
mdvdb modules status lookup_rollup --path clients --json
~~~

The Markdown frontmatter is authoritative after materialization; computed
results are not only an index cache. Rewriting a computed value preserves
unrelated YAML and the body, and an unchanged body does not need to be
re-embedded.

## Design safely

- Treat computed output fields as read-only. The next module run may replace a
  manual edit.
- Choose new field names carefully. Adding a computed definition with the same
  name as an authored field adopts that field as module-owned output.
- A Formula reads local fields only.
- Lookup and Rollup target exact top-level fields, not arbitrary nested
  selectors.
- Lookup cannot traverse incoming Relations.
- An incoming Rollup requires both **relation_scope** and an ordinary
  **relation_field** in that scope.
- Computed strings that look like Markdown links do not create Relation graph
  edges.

Use [schema](../commands/schema.md) to inspect the resolved definitions and
[Relations](./relations.md) to verify the fields used for traversal.

## Query computed values

Once materialized, computed fields behave like other frontmatter fields:

~~~bash
mdvdb collection invoices \
  --recursive \
  --sort total \
  --order desc \
  --json
~~~

This is part of Markdown VDB's [SQL-like structured-data model](./frontmatter-data.md):
Formula, Lookup, and Rollup values act like computed columns, but there is no
SQL expression or JOIN interface.

## Further reading

- [Frontmatter as structured data](./frontmatter-data.md)
- [Relations](./relations.md)
- [Schema command](../commands/schema.md)
- [Knowledge operations use case](../use-cases/knowledge-operations.md)


---
title: "Run knowledge operations from Markdown"
description: "Model projects, clients, and invoices as linked records with queryable and computed frontmatter."
category: "use-cases"
---

# Run knowledge operations from Markdown

Markdown VDB can support project operations, a lightweight CRM, or another
small team workflow without moving narrative context into database-only
records.

The useful combination is:

- Markdown bodies for plans, history, rationale, and meeting notes
- Frontmatter for state, dates, ownership, and amounts
- Relations for links among clients, projects, contacts, and invoices
- Formula, Lookup, and Rollup fields for derived values
- Collection queries for deterministic operational views
- Semantic search for questions that span the narrative

## Lay out the records

~~~text
clients/
├── acme.md
└── northwind.md
projects/
├── atlas.md
└── compass.md
invoices/
├── inv-1042.md
└── inv-1043.md
contacts/
└── maya.md
~~~

For example, **clients/acme.md**:

~~~markdown
---
status: active
domain: acme.example
account_owner: contacts/maya.md
---

# Acme

Acme is migrating its billing integration before renewal.
~~~

**projects/atlas.md**:

~~~markdown
---
status: active
client: clients/acme.md
due: 2026-09-15
priority: 2
---

# Atlas

Migrate Acme to the event-based billing integration.
~~~

And **invoices/inv-1042.md**:

~~~markdown
---
status: sent
client: clients/acme.md
quantity: 4
unit_price: 125
---

# Invoice 1042
~~~

The whole-value Markdown paths are Relations. The prose remains available to
hybrid search, while fields are available to deterministic queries.

## Declare the data model

Add **.markdownvdb.schema.yml** at the Collection root:

~~~yaml
scopes:
  projects:
    fields:
      status:
        field_type: string
        allowed_values:
          - planned
          - active
          - done
        required: true
      client:
        field_type: relation
        target: clients
      client_domain:
        field_type: lookup
        relation_field: client
        target_field: domain

  invoices:
    fields:
      client:
        field_type: relation
        target: clients
      total:
        field_type: formula
        formula: quantity * unit_price
        result_type: number

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

This schema adds three computed columns:

- **projects.client_domain** looks up the related client's domain.
- **invoices.total** multiplies local quantity and unit price.
- **clients.invoice_total** totals the computed values of invoices that point
  to that client.

Validate the formulas before ingesting:

~~~bash
mdvdb modules validate formula \
  --formula 'quantity * unit_price' \
  --result-type number

mdvdb modules validate lookup_rollup \
  --formula 'values.reduce((sum, value) => sum + Number(value), 0)' \
  --result-type number
~~~

Then build or refresh the collection:

~~~bash
mdvdb ingest
~~~

Formula runs before Lookup and Rollup. Successful results are atomically
materialized into the Markdown frontmatter, so the computed values are
portable and available to ordinary filters, sorts, and exports.

See [Computed fields](../concepts/computed-fields.md) for dependency and error
behavior.

## Build operational views

List active projects by due date:

~~~bash
mdvdb collection projects \
  --recursive \
  --filter status=active \
  --sort due \
  --order asc \
  --populate \
  --json
~~~

Rank clients by invoiced amount:

~~~bash
mdvdb collection clients \
  --sort invoice_total \
  --order desc \
  --json
~~~

Inspect one project and its related client:

~~~bash
mdvdb get projects/atlas.md --populate --json
~~~

Population is one level deep. It resolves the Relation and target
frontmatter; it is not an arbitrary recursive join.

## Ask narrative questions

Operational questions often mix state with text:

~~~bash
mdvdb search "renewal risks and unresolved integration blockers" \
  --path projects \
  --filter status=active \
  --populate \
  --json
~~~

The frontmatter filter provides the exact active-project boundary. Hybrid
search ranks the project prose by meaning and terms. Population attaches the
linked client record to the structured output.

Use collection queries when every result should be selected by fields. Use
search when relevance within the Markdown body matters.

## SQL-like, not SQL

This workflow resembles a relational data model:

| Markdown VDB | SQL analogy |
| --- | --- |
| File | Row |
| Frontmatter field | Column |
| Relation | Foreign key |
| Populate | One-level join-like expansion |
| Formula, Lookup, Rollup | Computed column |
| Filter, sort, limit, offset | Structured query operations |

It is intentionally not a SQL interface. There is no SQL parser, arbitrary
JOIN syntax, range-expression language in CLI filters, cross-Collection join,
or remote data source. A repeated **--filter KEY=VALUE** is an ANDed equality
constraint, while Lookup and Rollup implement declared, deterministic
relationship computations.

The tradeoff is useful for knowledge operations: data stays in understandable
Markdown and Git diffs, while common database-shaped workflows remain fast
and scriptable.

## Keep the collection healthy

~~~bash
mdvdb schema --json
mdvdb modules status formula --path invoices --json
mdvdb modules status lookup_rollup --path clients --json
mdvdb doctor --json
~~~

If a target file is renamed, update the frontmatter that points to it;
Relations are not silently rewritten. Doctor can surface dangling references.

Treat computed output fields as module-owned. Change their definitions or
input fields rather than manually editing the materialized values.

## Related pages

- [Frontmatter as structured data](../concepts/frontmatter-data.md)
- [Relations](../concepts/relations.md)
- [Computed fields](../concepts/computed-fields.md)
- [Collection](../commands/collection.md)
- [Schema](../commands/schema.md)
- [Get](../commands/get.md)
- [Search](../commands/search.md)
- [JSON output](../json-output.md)

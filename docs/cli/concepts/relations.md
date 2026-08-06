---
title: "Relations"
description: "Connect Markdown records with typed frontmatter links, population, backlinks, and graph-aware search."
category: "concepts"
---

# Relations

A Relation is a frontmatter value that points to another Markdown file.
Relations turn ordinary links into typed, queryable connections without
introducing opaque IDs or a separate database.

They support:

- Foreign-key-like fields in the inferred schema
- One-level expansion with **--populate**
- Reverse references from **mdvdb get**
- Backlinks, orphans, and graph traversal
- Link boosting and graph expansion during search
- Lookup and Rollup computed fields

## Accepted relation values

Markdown VDB recognizes three whole-value link shapes:

~~~yaml
# A Markdown path
client: clients/acme.md

# A wiki link; quote it so YAML does not interpret the brackets
reviewer: "[[people/maya]]"

# A Markdown link
policy: "[Security policy](policies/security.md)"
~~~

Lists are supported too:

~~~yaml
reviewers:
  - "[[people/maya]]"
  - people/noah.md
related:
  - "[Migration plan](projects/migration.md)"
~~~

The complete trimmed string must be link-shaped. A prose value such as
**Discuss with [[people/maya]] tomorrow** stays a string, because it is not
only a link. External URLs, mail links, and bare heading anchors are not
Relations.

Detection is value-driven. You do not need to declare every Relation in the
schema before ingestion can recognize it.

## Resolution rules

Relation targets are normalized to Collection-relative Markdown paths.
Resolution is deterministic and case-sensitive:

1. A target containing a slash is tried from the Collection root first. If
   that file does not exist, it is tried relative to the source file's folder.
2. A slash-less target on a field with an explicit schema target is resolved
   under that target folder.
3. Other slash-less targets are resolved relative to the source file's
   folder.

Fragments are removed and backslashes are normalized. Markdown VDB does not
perform a vault-wide basename search, so two same-named files cannot silently
change which target wins.

For example, in **projects/atlas.md**:

~~~yaml
client: clients/acme.md
brief: brief.md
~~~

The first value prefers **clients/acme.md** from the Collection root. The
second resolves to **projects/brief.md** unless its field has a schema target.

## Declare a target folder

Use the schema overlay when a Relation field has a known target collection:

~~~yaml
scopes:
  projects:
    fields:
      client:
        field_type: relation
        target: clients
~~~

Now a slash-less value such as this resolves beneath **clients**:

~~~yaml
client: acme.md
~~~

Declaring a target also documents the data model for tools and people. It
does not rewrite the stored frontmatter value.

## Populate related records

Use **--populate** on get, collection, or search:

~~~bash
mdvdb get projects/atlas.md --populate --json
~~~

~~~bash
mdvdb collection projects --recursive --populate --json
~~~

~~~bash
mdvdb search "migration risks" --path projects --populate --json
~~~

The original frontmatter remains intact. Populated output adds resolved
Relation data including the raw value, normalized path, whether the target
exists, target title, and target frontmatter.

Population is deliberately one level deep. A populated target's own Relations
are not recursively expanded. Broken targets are returned with an absent
target marker instead of being silently discarded.

For a single record, **mdvdb get --populate** also reports
**referenced_by**, the records whose frontmatter Relations point to it. See
[get](../commands/get.md).

## Relations join the link graph

Frontmatter Relations participate in the same graph used by body wiki links
and Markdown links. That means they appear in:

~~~bash
mdvdb backlinks clients/acme.md --json
mdvdb orphans --json
mdvdb graph --json
~~~

They also affect search when link-aware features are enabled:

~~~bash
mdvdb search "billing migration" \
  --boost-links \
  --hops 2 \
  --expand 1 \
  --json
~~~

Link boosting adjusts scores using graph connections. Graph expansion adds
connected documents as supplementary context. Read [The link graph](./link-graph.md)
for the graph model and [search](../commands/search.md) for flag interactions.

## Use Relations in computed fields

Lookup follows an outgoing Relation and copies a target field. Rollup follows
one or more Relations and aggregates target values; an incoming Rollup can
aggregate records that point back to its owner.

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

Read [Computed fields](./computed-fields.md) for complete Formula, Lookup, and
Rollup examples.

## Operational boundaries

- Keep wiki links quoted in YAML.
- Target matching is exact-case and path-based.
- Renaming a target file does not rewrite the frontmatter of files that point
  to it.
- **mdvdb doctor** can report dangling Relations after a move or deletion.
- A computed value that merely looks like a link does not become a Relation or
  create a graph edge.
- Population never crosses more than one Relation depth.

These constraints keep relation resolution deterministic and prevent an edit
from producing unexpected transitive reads.

## Further reading

- [Frontmatter as structured data](./frontmatter-data.md)
- [The link graph](./link-graph.md)
- [Computed fields](./computed-fields.md)
- [Schema command](../commands/schema.md)


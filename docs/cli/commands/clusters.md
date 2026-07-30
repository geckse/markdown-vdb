---
title: "mdvdb clusters"
description: "Inspect automatic communities and manage independent Collection or Shard Topics"
category: "commands"
---

# mdvdb clusters

Inspect automatic document communities or manage user-defined Topics. Without a Shard, results use
the Collection analysis stored in the shared index. With `--shard`, automatic communities and
Topic assignments belong only to that named recursive sub-collection.

## Usage

```bash
mdvdb clusters [--shard <ID>]
mdvdb clusters [--shard <ID>] --custom
mdvdb clusters [--shard <ID>] list
mdvdb clusters [--shard <ID>] add <NAME> [OPTIONS]
mdvdb clusters [--shard <ID>] update <NAME> [OPTIONS]
mdvdb clusters [--shard <ID>] remove <NAME>
mdvdb clusters [--shard <ID>] unassigned
```

## Options

| Flag | Description |
|------|-------------|
| `--shard <ID>` | Use the Shard's independent automatic clusters and Topics |
| `--custom` | Show computed Topic summaries instead of automatic clusters |

Topic definition options:

| Flag | Applies to | Description |
|------|------------|-------------|
| `--description <TEXT>` | `add`, `update` | Natural-language Topic description |
| `--seeds <A,B,...>` | `add`, `update` | Comma-separated seed phrases |
| `--threshold <0..1>` | `add`, `update` | Per-Topic similarity threshold |
| `--rename <NAME>` | `update` | Rename the Topic within its owner |

A Topic must have a non-empty description, at least one seed, or both. Names are unique
case-sensitively within their owner. The Collection and every Shard are separate owners, so two
scopes may intentionally use the same name with different definitions and assignments.

## Automatic clusters

```bash
# Communities across the complete Collection
mdvdb clusters

# Finer communities derived only from indexed documents in Research
mdvdb clusters --shard research
```

Collection automatic clusters are maintained by ingest. Shard automatic clusters are computed
lazily from the existing document vectors already stored in the shared index. They use the
Collection's clustering algorithm and settings but only the Shard corpus. For Leiden analysis,
`clustering.knn` remains the configured upper bound; a Shard caps its effective neighborhood at
`max(2, ceil(sqrt(document_count)))`. This prevents a small Shard from becoming a complete
similarity graph and collapsing otherwise useful local communities. Collection clustering is
unchanged.

No embedding provider is initialized by a read. The disposable Shard state is cached below
`.markdownvdb/cache/shards/`; it does not duplicate document embeddings or graph topology and does
not change index bytes or status.

Fewer than two indexed Shard documents produce no automatic partition and report `too_small`
through graph analysis metadata.

## Independent Topics

Collection definitions live under `clustering.custom`. Shard definitions live inside the raw
project-local Shard entry:

```yaml
shards:
  research:
    name: Research
    path: work/research
    topics:
      - name: Methods
        description: Research methods and experiments
        seeds: [methodology, experiment]
        threshold: 0.35
```

Manage them with the same CLI:

```bash
mdvdb clusters --shard research add Methods \
  --description "Research methods and experiments" \
  --seeds methodology,experiment \
  --threshold 0.35

mdvdb clusters --shard research list
mdvdb clusters --shard research update Methods --threshold 0.40
mdvdb clusters --shard research remove Methods
```

Definition mutations preserve unrelated YAML and prompt for a subsequent ingest. Ingest embeds new
or changed Topic definitions and assigns in-Shard documents. When only the Shard corpus changes,
compatible cached centroids are reused without re-embedding Topic text.

Read-only commands never initialize an embedding provider. Until stale or new definitions have
been ingested, graph topology and automatic clusters remain available while local Topics report
`needs_ingest`.

```bash
mdvdb ingest
mdvdb clusters --shard research --custom
mdvdb clusters --shard research unassigned
```

Topics use multi-label threshold assignment. A document joins every Topic whose similarity meets
the larger of the per-Topic threshold and Collection-wide
`clustering.topics.min_similarity`. `unassigned` returns documents matching none.

## Nested Shards

Nested Shards analyze themselves independently. A document contained by both an ancestor and a
descendant can have a different automatic cluster and different Topic memberships in each:

```bash
mdvdb clusters --shard research
mdvdb clusters --shard papers
```

No assignments inherit from the Collection, ancestor Shard, or sibling Shard. Links, backlinks,
relations, document identities, files, and embeddings remain Collection-wide and shared.

## JSON output

Automatic cluster output remains the existing `ClusterSummary[]` array:

```json
[
  {
    "id": 3,
    "document_count": 12,
    "label": "methods, experiments",
    "keywords": ["methods", "experiments", "evaluation"]
  }
]
```

Topic output remains `CustomClusterSummary[]`, including definition metadata, document counts, and
mean similarity. `unassigned` retains its existing object shape. Supplying `--shard` changes the
analysis owner, not these JSON contracts.

Shard-local `add`, `update`, and `remove` return
`{"action": string, "shard_id": string, "topics": TopicDef[]}`. The `topics` list is the complete
post-mutation local definition state. Collection-level mutation output remains unchanged.

IDs are opaque and need not be contiguous. Compatible Shard cache state is reused for best-effort
stable IDs and colors; a clustering-configuration change starts a fresh local state.

## Human output and empty states

Human output identifies the selected Shard and prints local distribution bars, labels, keywords,
and counts. Empty states distinguish:

- Clustering disabled by Collection settings.
- A Shard with fewer than two indexed documents.
- No local Topics configured.
- Local Topics that need ingest.
- A missing Shard folder, for which definitions remain editable but computed results are disabled.

## Safety and lifecycle

- Queries never modify the shared index or public version fields.
- Removing a Topic removes only its definition and derived local assignment state.
- Removing a Shard also removes its local Topic definitions and best-effort deletes its disposable
  cache, but never its folder or files.
- Shard path updates and retargeting preserve local Topic definitions.
- Corrupt or incompatible cache files are ignored and rebuilt safely.

## Examples

```bash
# Collection automatic clusters
mdvdb clusters

# Shard automatic clusters as JSON
mdvdb clusters --shard research --json

# Computed local Topics
mdvdb clusters --shard research --custom --json

# Two independent Topics with the same display name
mdvdb clusters add Methods --description "All collection methods"
mdvdb clusters --shard research add Methods --description "Research methods"

# Local Unassigned bucket
mdvdb clusters --shard research unassigned --json
```

## Related commands

- [`mdvdb shards`](./shards.md) — create and manage named sub-collection scopes
- [`mdvdb graph`](./graph.md) — render Shard-local graph clusters and Topics
- [`mdvdb ingest`](./ingest.md) — compute new or changed Topic centroids
- [Configuration](../configuration.md) — clustering settings and raw Shard Topic YAML
- [JSON output](../json-output.md) — response schemas and graph analysis status

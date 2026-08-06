---
title: "Shards and Topics"
description: "Create named folder lenses and semantic topic taxonomies over one shared Markdown collection."
category: "concepts"
---

# Shards and Topics

Shards and Topics organize a large Collection at two different levels:

- A **Shard** is a named, recursive folder lens.
- A **Topic** is a user-defined semantic category whose membership is based on
  similarity to a description and seed phrases.

They complement folders and frontmatter. Shards establish a reusable corpus
boundary; Topics provide overlapping semantic labels inside that corpus.

## A Shard is a lens, not another database

Every Shard uses the Collection's shared index, watcher, embeddings, and link
graph. A Shard does not:

- Copy files or embeddings
- Create a nested Markdown VDB database
- Partition ingestion
- Provide tenancy or access control

Ingestion and watch remain Collection-wide. Commands such as search, schema,
info, collection, graph, and clusters can use a Shard as their scope.

Create one for a folder:

~~~bash
mdvdb shards add research \
  --name Research \
  --path work/research
~~~

The folder must already exist unless **--create-dir** is included. The Shard
ID is immutable and kebab-case; its display name and path can be updated.

~~~bash
mdvdb shards list --json
mdvdb shards get research --json
mdvdb shards update research --name "Research library"
~~~

Removing a Shard removes only its definition, never its Markdown files:

~~~bash
mdvdb shards remove research
~~~

If a folder tree is renamed outside Markdown VDB, retarget affected Shards:

~~~bash
mdvdb shards retarget work/research knowledge/research
~~~

See the [shards command](../commands/shards.md) for the complete lifecycle.

## Define Shards in configuration

The equivalent YAML is:

~~~yaml
shards:
  research:
    name: Research
    path: work/research
    topics:
      - name: Methods
        description: Research methods and experimental design
        seeds:
          - methodology
          - experiment
        threshold: 0.35
~~~

Nested Shards are independent lenses. They do not inherit Topics from their
parent, siblings, or the Collection.

## Query through a Shard

Use **--shard** where a command supports it:

~~~bash
mdvdb search "evaluation methodology" --shard research --json
mdvdb collection --shard research --recursive --json
mdvdb schema --shard research --json
mdvdb info --shard research --json
~~~

For graph analysis, the Shard selects the analysis corpus. An additional path
projects the result within that corpus:

~~~bash
mdvdb graph \
  --shard research \
  --path work/research/drafts \
  --json
~~~

Shard graph topology is a strict induced subgraph: only files inside the Shard
participate in the analysis. Search graph expansion is different; it may add a
linked document outside the Shard as supplementary result context.

Read-only Shard graph and cluster calls reuse stored data and do not need to
initialize an embedding provider. Shard caches under
**.markdownvdb/cache/shards/** are disposable projections, not duplicate
indexes.

## Automatic clusters

Run:

~~~bash
mdvdb clusters --shard research --json
~~~

Automatic clusters discover structure from the documents in the selected
corpus. The default algorithm is deterministic Leiden community detection
over a similarity graph, with K-means available as a fallback. A Shard's
automatic clusters are recomputed from only that Shard's vectors.

## Topics

Topics are configured semantic labels. Each needs a description, seed
phrases, or both:

~~~yaml
clustering:
  topics:
    min_similarity: 0.30
  custom:
    - name: Reliability
      description: Reliability engineering, incidents, and resilience
      seeds:
        - postmortem
        - graceful degradation
      threshold: 0.38
~~~

Definitions under **clustering.custom** apply to the whole Collection.
Definitions under **shards.SHARD_ID.topics** belong only to that Shard.

Manage Collection Topics:

~~~bash
mdvdb clusters add Reliability \
  --description "Reliability engineering, incidents, and resilience" \
  --seeds "postmortem,graceful degradation" \
  --threshold 0.38

mdvdb clusters list --json
mdvdb clusters --custom --json
mdvdb clusters unassigned --json
~~~

Add and inspect a Shard-local Topic with **--shard**:

~~~bash
mdvdb clusters --shard research add Methods \
  --description "Research methods and experimental design" \
  --seeds "methodology,experiment" \
  --threshold 0.35

mdvdb clusters --shard research list --json
mdvdb clusters --shard research --custom --json
~~~

## How assignment works

Markdown VDB embeds the Topic description and seeds to create a centroid.
Each document can join every Topic whose similarity clears both:

- The Topic's own threshold, when present
- The global floor at **clustering.topics.min_similarity**, which defaults to
  0.30

This is multi-label classification: a document can belong to more than one
Topic. Documents that match none appear in the **Unassigned** bucket, and
membership scores are stored for inspection.

After adding or changing a Topic description or seeds, run ingestion so its
centroid is current:

~~~bash
mdvdb ingest
~~~

Until then, a Shard-local Topic report can indicate that ingestion is needed.
A corpus-only change may reuse an unchanged Topic centroid.

See [clusters](../commands/clusters.md) for Topic CRUD and output, and
[graph](../commands/graph.md) for topology analysis.

## When to use each tool

| Need | Use |
| --- | --- |
| Stable team or project boundary | Shard |
| Exact lifecycle or ownership state | Frontmatter field |
| Overlapping semantic categories | Topics |
| Discovered, unlabeled structure | Automatic clusters |
| Connections among files | Link graph |

Shards and Topics organize retrieval; they do not replace
[frontmatter filters](./frontmatter-data.md) or [Relations](./relations.md).

## Further reading

- [Shards command](../commands/shards.md)
- [Clusters command](../commands/clusters.md)
- [The link graph](./link-graph.md)
- [Frontmatter as structured data](./frontmatter-data.md)

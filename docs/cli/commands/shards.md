---
title: "mdvdb shards"
description: "Create and manage named recursive folder scopes that reuse a Collection index"
category: "commands"
---

# mdvdb shards

Shards give a collection-relative folder a stable name for humans, scripts, and agents. A Shard is
a recursive logical lens over one Collection: it reuses the Collection's watcher, link graph,
full-text index, and vector index. It is not a nested index or an access boundary.

## Configuration

Definitions live only in the project's `.markdownvdb/config.yaml`:

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
  papers:
    name: Papers
    path: work/research/papers
```

`papers` is displayed beneath `research` because its folder is contained by the Research folder.
The relationship is inferred; no parent field is stored.

When `--name` is omitted on `add`/`create`, the CLI derives a title-cased display name from the
ID (`research-papers` becomes `Research Papers`).

## Management

```bash
mdvdb shards list
mdvdb shards get research
mdvdb shards add research --name Research --path work/research
mdvdb shards create drafts --name Drafts --path work/drafts --create-dir
mdvdb shards update research --name "Research Library"
mdvdb shards update research --path archive/research
mdvdb shards retarget work/research archive/research
mdvdb shards remove research
mdvdb shards delete research
```

`remove` and `delete` never delete the folder or any Markdown files. Missing folders remain in
`list` with `exists: false`, which makes external renames repairable.

## Using a Shard

The following commands accept `--shard <ID>`:

```bash
mdvdb search "vector databases" --shard research --json
mdvdb tree --shard research --json
mdvdb info --shard research --json
mdvdb schema --shard research --json
mdvdb collection --shard research --recursive --json
mdvdb graph --shard research --json
mdvdb modules run formula --shard research --json
mdvdb modules status formula --shard research --json
```

Except for graph projection described below, `--shard` and the command's path selector are
mutually exclusive. Results keep their collection-root-relative paths and document identities.
`get`, `links`, and `backlinks` continue to take explicit collection-root-relative document paths
and therefore work across Shards.

For `collection`, the default is still direct children; pass `--recursive` for the complete Shard.
For `graph`, only nodes inside the Shard and edges whose two endpoints are inside are returned.
Automatic clusters are recomputed from the Shard's stored document vectors, and optional
Shard-local Topics are independent from every other scope. Leiden treats the Collection's
configured KNN as an upper bound and automatically reduces it for small Shards so their graph does
not become fully connected:

```bash
mdvdb clusters --shard research
mdvdb clusters --shard research add Methods \
  --description "Research methods and experiments" \
  --seeds methodology,experiment
mdvdb ingest
mdvdb graph --shard research --json
```

Graph also accepts an optional path together with a Shard. The Shard remains the analysis corpus;
the path only narrows visible topology:

```bash
mdvdb graph --shard research --path work/research/drafts --json
```

An ancestor path clamps to the Shard and a disjoint path returns an empty graph. Cluster and Topic
identities stay stable while navigating descendant folders.
Search graph expansion can include linked supplementary context outside the Shard.

## JSON

List:

```json
{
  "shards": [
    {
      "id": "research",
      "name": "Research",
      "path": "work/research",
      "parent_id": null,
      "exists": true
    }
  ],
  "total_shards": 1
}
```

Get emits one object from the `shards` array. Mutations emit the same `ShardInfo` objects:

```json
{
  "action": "add",
  "shards": [
    {
      "id": "research",
      "name": "Research",
      "path": "work/research",
      "parent_id": null,
      "exists": true
    }
  ]
}
```

Shard management works before ingest and never reads or writes the index. Removing a Shard also
removes its local Topic definitions and best-effort deletes disposable derived analysis cache; it
still never deletes the folder, Markdown files, or shared index.

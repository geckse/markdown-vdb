# Phase 34: Shard-Native Graph Analysis

## Summary

A Shard is an independent graph-analysis context over the documents already stored in one
Collection index. Its topology is the strict induced subgraph of indexed documents below the
Shard path. Automatic clusters are derived from only those documents, and Topic definitions are
owned by the Shard rather than inherited from the Collection, an ancestor Shard, or a sibling.

Documents, chunks, embeddings, Markdown files, links, and index metadata remain Collection-owned.
A document in nested Shards can therefore receive different local cluster and Topic assignments
without duplicating source data.

## Local Topic configuration

Optional Topic definitions are stored with the project-local Shard:

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

The definition shape and validation are the same as Collection Topics. Names are unique only
inside their owning Shard. Shard rename, path update, and retarget preserve the definitions.
Removing a Shard removes its local definitions but never its folder, files, embeddings, or links.

Malformed local Topics are isolated to Topic operations for their owning Shard and are reported by
Doctor. They do not prevent Shard discovery or unrelated Collection commands.

## Derived analysis cache

Shard analysis is disposable state under:

```text
.markdownvdb/cache/shards/<SHARD-ID>.json
```

The cache may contain automatic-cluster state, local Topic centroids and assignments, and
fingerprints. It never contains document embeddings or graph topology. The fingerprint covers:

- Shard ID and normalized path.
- In-scope indexed file hashes.
- Embedding model and dimensions.
- Collection-wide clustering settings used for automatic analysis.
- Local Topic definitions and the Collection-wide Topic similarity floor.

Writes hold a per-Shard advisory lock and replace the cache atomically. Corrupt, incompatible, or
stale files are ignored and rebuilt. Removing a Shard performs best-effort cache cleanup; cleanup
failure cannot block definition removal.

## Refresh model

Automatic clusters are computed lazily when `graph --shard` or `clusters --shard` needs them.
Compatible prior local state is supplied for best-effort stable IDs and colors. A change to the
clustering configuration starts a fresh local state. For Leiden, the configured KNN is retained as
an upper bound while the local effective KNN is capped at
`max(2, ceil(sqrt(clusterable_document_count)))`; this avoids complete similarity graphs for small
Shards without changing Collection analysis. A corpus with fewer than two indexed documents
reports `too_small` instead of presenting a misleading partition.

Read-only graph and cluster commands never initialize an embedding provider. Local Topic
definitions whose centroid fingerprint is stale report `needs_ingest`. The graph remains usable
with topology and automatic clusters while Topics are unavailable. Ingest embeds changed Topic
definitions. If definitions and centroids remain compatible but only Shard documents changed,
stored centroids are reused to assign the current documents without embedding Topic text again.

## Library and JSON contract

Shard-aware methods expose automatic cluster summaries, Topic summaries, unassigned documents,
and standard or compact graph data. Existing Collection methods and array shapes remain unchanged.

Graph payloads may include additive analysis metadata:

```text
GraphAnalysisInfo {
  context: collection | shard
  shard_id?
  shard_path?
  clusters: ready | disabled | too_small | error
  topics: ready | none | needs_ingest | error
  message?
}
```

The metadata is optional, so compact graph wire version 1 remains valid. No index type or metadata
layout changes.

## CLI contract

```text
mdvdb clusters --shard research
mdvdb clusters --shard research --custom
mdvdb clusters --shard research list
mdvdb clusters --shard research add <NAME> [OPTIONS]
mdvdb clusters --shard research update <NAME> [OPTIONS]
mdvdb clusters --shard research remove <NAME>
mdvdb clusters --shard research unassigned

mdvdb graph --shard research
mdvdb graph --shard research --path work/research/drafts
```

For graph, the Shard establishes the complete analysis corpus. An optional path only projects the
visible topology:

- A descendant path narrows the visible graph.
- An ancestor path clamps to the Shard boundary.
- A disjoint path returns an empty graph.

Cluster and Topic identities continue to come from the complete Shard while a descendant folder is
viewed. This is intentionally different from Collection `graph --path`, whose analysis remains
Collection-based.

## Graph projection rules

- Only in-Shard indexed document and chunk nodes are eligible.
- Only edges with both endpoints visible are returned.
- Chunk nodes inherit their document's local automatic cluster and Topic memberships.
- Cluster and Topic summaries are recounted from unique visible documents; zero-member entries are
  omitted.
- Semantic edge-cluster identities are not recomputed locally, but their summaries are pruned and
  recounted from visible edges.
- Direct graph search uses the effective Shard-plus-folder boundary. Expanded outside context can
  remain supplementary search context but is never inserted as a Shard graph node.
- Collection graph requests retain Collection analysis and public API behavior.

## Compatibility

This phase does not change the app version, CLI version, compact graph version, index version, or
index metadata. Graph and cluster reads do not change index bytes or `status`. Obsidian-imported
Topics remain Collection-owned. Watcher, links, backlinks, relations, local document graphs,
document identities, and Markdown files retain Collection-wide semantics.

## Acceptance criteria

- Nested and overlapping Shards produce independent deterministic clusters and Topics from shared
  stored document vectors.
- Collection, ancestor, and sibling Topics never leak into a Shard.
- Cache writes are locked and atomic; corruption and incompatible fingerprints recover safely.
- Topic reads make no embedding-provider calls and distinguish `none`, `needs_ingest`, and `error`.
- Descendant graph navigation preserves local identities while recounting visible membership.
- Standard and compact graph payloads have equivalent Shard semantics.
- Topic CRUD preserves unknown raw YAML, and Shard listing survives malformed local Topic data.
- Removing a Shard cannot delete content and cannot be blocked by cache cleanup.
- Index bytes and all public versions remain unchanged by Shard graph and cluster queries.

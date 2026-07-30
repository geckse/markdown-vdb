# Phase 33: Named Shards

## Summary

A **Shard** is a named, recursive folder lens inside a Markdown VDB Collection. Shards reuse
the collection's configuration, watcher, link graph, full-text index, and vector index. They are
not nested databases, index partitions, or access-control boundaries.

```yaml
# .markdownvdb/config.yaml
shards:
  research:
    name: Research
    path: work/research
```

The mapping key is an immutable kebab-case identifier. The display name and collection-relative
path are editable. Parent relationships are derived from nearest folder containment, so nested
Shards require no duplicated hierarchy metadata.

## Goals

- Give humans and agents stable names for commonly used folder scopes.
- Preserve one physical collection index and collection-root-relative document identities.
- Keep links, backlinks, relations, autocomplete, and explicit document navigation collection-wide.
- Make the same model available through the Rust library, CLI, and Tesseract app.
- Allow external CLI edits to become visible in open app windows.

## Non-goals

- Nested `.markdownvdb` directories or duplicated embeddings.
- Security, tenancy, or access-control isolation.
- Restricting ingest, watch, Doctor, topics, `get`, `links`, or `backlinks`.
- Changing the index, compact graph wire, CLI, or app version.

## Persistence and validation

Shard definitions are read from the project's raw `.markdownvdb/config.yaml`; the merged runtime
`Config` is deliberately not used, so user configuration cannot leak Shards into a project.
Read-modify-write operations preserve unrelated YAML keys, hold the shared advisory
`.markdownvdb/config.lock`, write atomically, and never touch the index.

Validation rules:

- IDs are unique, immutable kebab-case strings.
- Names are non-empty and case-insensitively unique.
- Paths are unique after slash normalization.
- Paths are relative, non-root, and cannot contain `..` or target `.markdownvdb`.
- Add/update requires an existing directory unless `--create-dir` is explicit.
- Missing folders remain listable with `exists: false`.
- Parent IDs are inferred using path-segment boundaries (`docs` never contains `docs-old`).

Malformed `shards` YAML fails Shard operations and appears as a Doctor warning, while unrelated
configuration and commands continue to work.

## Library contract

```rust
ShardDefinition { id, name, path }
ShardInfo { id, name, path, parent_id, exists }
ShardList { shards, total_shards }
```

The Shard store supports list, get, add, update, remove, and prefix retarget operations. Retarget
updates every Shard at or below the old path in one locked atomic mutation. Removing a definition
never removes its folder or files.

## CLI contract

```text
mdvdb shards list
mdvdb shards get <ID>
mdvdb shards add <ID> --path <PATH> [--name <NAME>] [--create-dir]
mdvdb shards update <ID> [--name <NAME>] [--path <PATH>] [--create-dir]
mdvdb shards remove <ID>
mdvdb shards retarget <OLD-PREFIX> <NEW-PREFIX>
```

`create` aliases `add`; `delete` aliases `remove`. JSON list output is
`{"shards": [...], "total_shards": N}`; get emits one `ShardInfo`; mutations emit
`{"action": "...", "shards": [...]}`.

`--shard <ID>` is mutually exclusive with existing path selectors on `search`, `tree`, `info`,
`schema`, `collection`, `graph`, `modules run`, and `modules status`. It resolves once to the
existing path-scoped APIs. `collection` continues to return direct children unless `--recursive`
is present.

All scoping uses one segment-safe matcher. Scoped search progressively grows candidate retrieval
until its requested limit is filled or the corpus is exhausted. Edge search scopes by source path;
the target may be outside. Scoped graph output is a strict induced subgraph. Expanded search
context remains supplementary and may cross the boundary.

## Ingest and schema

Ingest continues to process the complete collection. In addition to top-level and overlay scopes,
it caches schemas for configured Shard paths using the existing scoped-schema metadata field.
No stored type or index-format change is required.

## Tesseract behavior

Tesseract obtains definitions through the CLI and stores only the last selected Shard ID per
collection. The collection switcher is an accessible tree of collection roots and inferred nested
Shards. Selecting a Shard changes the working context without resetting the editor, tabs, watcher,
or index.

The active Shard is the default boundary for visible Markdown/assets, counts, search, Quick Open,
global graph, information/schema, table opening, and new-file destinations. The complete internal
file catalogs remain loaded for cross-Shard links, backlinks, relations, local graph, favorites,
and explicit navigation. An opened file outside the active Shard is labeled rather than causing
an implicit context switch.

In-app folder renames retarget the affected Shard and descendants; a failed manifest update rolls
back the filesystem rename. External renames leave a missing definition that can be edited or
retargeted. Folder deletion never deletes the definition; an active missing Shard falls back to
the collection root.

## Acceptance criteria

- CRUD preserves unknown YAML and does not create or change an index.
- Concurrent config writers coordinate through the shared advisory lock.
- Nested parent derivation and every scope boundary are segment-safe.
- Every supported `--shard` invocation is equivalent to its canonical path invocation.
- Scoped tree counters and requested search limits describe only the scope.
- Cross-Shard links/backlinks remain global while scoped graphs remain closed.
- Tesseract shows and manages nested Shards, restores a valid prior selection, and retains the
  complete catalog for link resolution.
- CLI/app versions, compact graph wire version, index metadata version, and index bytes remain
  unchanged by Shard management.


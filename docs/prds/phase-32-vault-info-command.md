# Phase 32: Vault Information Command

## Summary

Add `mdvdb info [path]`, a read-only command that reports whole-vault or folder-scoped content, sync, index, and full-reindex cost statistics. The command is the data contract used by the Tesseract collection information modal.

## Public API

`MarkdownVdb::info(path: Option<&str>) -> Result<VaultInfo>` is synchronous and makes no embedding-provider or other network calls. `None`, `.`, and `./` select the whole vault. Folder inputs are normalized to slash-terminated relative prefixes, so `blog`, `blog/`, and `./blog` are equivalent. A scope matching no files returns zero counts rather than an error.

`VaultInfo` serializes as:

```json
{
  "scope": ".",
  "is_whole_vault": true,
  "file_count": 2,
  "indexed_file_count": 2,
  "chunk_count": 4,
  "vector_count": 7,
  "edge_count": 3,
  "reindex_chunks": 4,
  "reindex_estimated_tokens": 820,
  "reindex_estimated_api_calls": 1,
  "index_file_size": 16384,
  "embedding": { "provider": "OpenAI", "model": "text-embedding-3-small", "dimensions": 1536 },
  "sync": { "new": 0, "changed": 0, "unchanged": 2, "deleted": 0 },
  "last_updated": 1770000000
}
```

All count and size fields are non-negative integers. `last_updated` is a Unix timestamp. `scope` is `.` for the whole vault and a trailing-slash prefix for folders.

## Semantics

- Disk files are discovered with the configured source directories and ignore rules.
- Sync state compares parsed content hashes with the index: new, changed, unchanged, and deleted.
- Full-reindex chunks and tokens include every in-scope disk file, including unchanged files. API calls are the chunk count divided by the configured embedding batch size, rounded up.
- Whole-vault `vector_count` is the measured HNSW size. HNSW has no per-path lookup, so scoped `vector_count` is derived as `chunk_count + edge_count`.
- Scoped edge vectors belong to their source file. Edge IDs use `edge:{source}->{target}@...`; filenames containing `->` are an accepted ambiguity.
- `index_file_size`, `last_updated`, and `embedding` describe the shared whole-vault index even for folder scopes.
- Like ingest preview, each call re-parses, re-chunks, and re-tokenizes in-scope files. It does not write the index.

## Status and doctor correction

`IndexStatus` adds `edge_count`. A healthy index satisfies:

```text
vector_count == chunk_count + edge_count
```

The doctor Index check must use that invariant. Semantic link embeddings live in HNSW but not in chunk metadata, so comparing vectors only with chunks incorrectly warned for every linked vault. Human `status` output includes the edge count, and status JSON includes the additive `edge_count` field.

## CLI

```text
mdvdb info [path] [--json] [--root <root>]
```

Human output groups content counts, sync state, full-reindex estimates, and index metadata. JSON output is the exact `VaultInfo` contract. Bash, zsh, fish, and PowerShell completions advertise `info`; shells with argument completion offer directories for `path`.

## Acceptance criteria

- Whole-vault, pre-ingest, scoped, empty-scope, changed, and deleted states have API coverage.
- `blog`, `blog/`, and `./blog` return identical scoped statistics.
- Human and JSON CLI forms and scoped JSON have integration coverage.
- A linked fixture produces a positive `edge_count`, satisfies the vector invariant, and passes the doctor Index check with edge detail.

---
title: "Searching the Vault"
tags: [search, mdvdb, workflow, retrieval]
category: guides
author: "Marcel Claus-Ahrens"
status: published
---
# Searching the Vault

How to find what you need fast using semantic, lexical, and hybrid search.

## Search Modes

The index supports three modes — pick the one that matches your intent:

- **Semantic** — finds meaning, not exact words. Use it when you remember *what* a note was about but not the phrasing.
- **Lexical (BM25)** — matches exact terms. Use it for identifiers, error codes, or specific names.
- **Hybrid** — fuses both via reciprocal rank fusion. The safe default when you're not sure.

```bash
mdvdb search "how do we handle expired tokens"        # hybrid (default)
mdvdb search "AES-256" --mode lexical                  # exact term
mdvdb search "incident response steps" --mode semantic # meaning
```

## Narrowing Results

- **Scope by path** — `--path guides/` limits the search to a folder.
- **Filter by metadata** — match on frontmatter fields like `category` or `status`.
- **Limit the count** — `--limit 5` keeps output focused; raise it when exploring.
- **Prefer recent notes** — time decay boosts recently modified files when enabled.

## Following the Link Graph

Notes are more useful connected than alone:

- `mdvdb links <path>` shows outgoing and incoming links for a file.
- Backlinks reveal which notes reference the one you're reading.
- `--expand` pulls in linked-file context alongside the direct matches.
- Orphan detection surfaces notes nothing links to — good candidates to connect or archive.

## Getting Better Matches

- Write descriptive headings — chunks split on headings, so a clear heading is a better search target.
- Use consistent frontmatter so metadata filters stay reliable.
- Keep notes focused; one idea per note retrieves more cleanly than a sprawling dump.
- Re-ingest after big edits so the index reflects reality (`mdvdb ingest`).

## Quick Reference

| Goal | Command |
| ---- | ------- |
| General question | `mdvdb search "..."` |
| Exact term or code | `mdvdb search "..." --mode lexical` |
| Within a folder | `mdvdb search "..." --path notes/` |
| See a file's links | `mdvdb links path/to/note.md` |
| Index status | `mdvdb status` |

&nbsp;

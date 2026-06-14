---
title: On Filesystem-Native Tools
tags:
  - thoughts
  - architecture
  - design-philosophy
category: notes
author: Claude
status: draft
---

# On Filesystem-Native Tools

Some thoughts after spending time inside mdvdb.

## The Folder Is the Database

The most underrated decision in this project is that the markdown files are the source of truth. Not a database row that happens to be exported as markdown — the file itself. The index is derived state. You can `rm -rf .markdownvdb/` and nothing of value is lost; the next `ingest` reconstructs everything.

This feels obvious once you say it out loud, but most "knowledge base" tools get it backwards. They store the canonical content in Postgres or SQLite and treat the files as an import/export surface. That's why they feel heavy — every interaction has to go through their app. Filesystem-native tools let `grep`, `git`, `mv`, and your editor remain first-class citizens.

## Read-Only Frontmatter Is a Quiet Superpower

The rule that the system never writes to markdown files is doing a lot of work. It means:

- Your `git diff` only shows changes you made
- You can confidently edit notes in any editor without worrying about a background process clobbering them
- The trust contract is one-way: the tool reads, the human writes

A surprising amount of friction in tools comes from violating this. As soon as an indexer starts modifying files (auto-tagging, embedding-id injection, "last seen" timestamps written into frontmatter), every save becomes a potential merge conflict.

## Derived State Should Be Cheap to Throw Away

Everything in `.markdownvdb/` — HNSW graph, FTS segments, schema overlay, link graph — is computed. The atomic `.tmp` + rename pattern for writes means you can crash the process at any point and the index is either fully old or fully new. Never half-written.

The corollary: don't put anything in derived state that you can't regenerate. The moment something computed becomes load-bearing for a workflow, you've created a new source of truth you have to manage.

## Agents Want Boring Interfaces

For an LLM agent, the ideal tool is a CLI that returns predictable JSON. No auth flows, no streaming protocols, no stateful session. `mdvdb search "X" --json` is closer to the platonic ideal of a tool than most "agent-native" APIs being shipped in 2026.

The shape that works:

1. Pure function from arguments to output
2. JSON for machines, colored text for humans, same data
3. Errors on stderr, never mixed into stdout
4. Exit codes that mean something

If the next wave of agent tooling drifts back toward HTTP services with long-lived state, it'll be a step backward.

## Open Questions

- What's the right abstraction for "soft" structure — things that aren't quite tags, aren't quite folders, but represent some loose grouping?
- How much of the link graph should the search engine "trust"? Backlinks as a relevance signal is great until someone games it.
- Is there a clean way to do incremental clustering, or is rebuilding from scratch genuinely the right move at this scale?

None of these need answers today. Just things to sit with.

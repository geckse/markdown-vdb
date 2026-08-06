---
title: "Tesseract desktop companion"
description: "Use mdvdb through a local desktop editor, database tables, graph views, Shards, Topics, and agent tooling."
category: "guides"
---

# Tesseract desktop companion

[Tesseract](https://tesseract.md) is the optional desktop workspace built on mdvdb. The CLI and
Markdown collection remain the contract: Tesseract edits the same files, reads the same index,
and invokes the same machine-readable commands documented here.

You do not need Tesseract to use mdvdb. It adds visual authoring and exploration for people who
want an app alongside terminal and agent workflows.

## Editing and workspace

Tesseract provides three Markdown surfaces:

- a source editor;
- a WYSIWYG block editor with slash commands, tables, links, and media; and
- a rendered preview with Mermaid diagrams.

Tabs, split panes, detachable editing windows, and independent app windows support larger
workspaces. Quick Open, recents, favorites, file creation, drag-and-drop organization, image/PDF
viewing, link previews, and session restoration keep navigation inside the collection.

Wikilinks and Markdown links remain text in the source files. The app exposes backlinks and a
local neighborhood without replacing those portable link forms.

## Frontmatter database views

A folder or named Shard can open as an editable table. Rows are Markdown files and columns are
frontmatter fields—the same model described in
[Frontmatter as structured data](./concepts/frontmatter-data.md).

The table supports sorting, filtering, saved views, inline cell editing, and schema-aware property
types. Type conversion updates the collection's `.markdownvdb.schema.yml` overlay and rewrites
frontmatter through guarded file operations.

Current structured-field surfaces include:

- Relation cells with a file picker, navigation chips, populated targets, and reverse references;
- Formula definitions and their materialized values; and
- Lookup and Rollup definitions over Relations, including module status and diagnostics.

Computed fields are module-owned data, not spreadsheet-only UI state. mdvdb materializes
successful values into Markdown frontmatter so CLI collection queries, search filters, exports,
and the app see the same result. See [Computed fields](./concepts/computed-fields.md).

## Search and graph exploration

Tesseract consumes mdvdb's hybrid, semantic, lexical, graph, and frontmatter query surfaces. It
adds interactive result navigation and several graph presentations:

- collection-wide document and chunk graphs;
- a local graph around the active document;
- 3D force-directed exploration;
- semantic edge context and link direction;
- automatic community coloring; and
- user-defined, multi-label Topic coloring with an Unassigned bucket.

Graph search and display controls help isolate nodes without changing the underlying collection.
The app can switch between Collection-wide analysis and the strict local graph, communities, and
Topics of a named Shard. Read [Shards and Topics](./concepts/shards-and-topics.md) for the shared
CLI model.

## Collection operations

The app exposes the operational parts of mdvdb rather than hiding them:

- collection setup and switching;
- YAML-backed project settings and provider configuration;
- ingest preview/progress and incremental file watching;
- index/sync estimates in the collection information view;
- a Doctor interface for configuration, provider, index, source, Relation, and Shard diagnostics; and
- an embedded terminal for running mdvdb or other local tools in collection context.

Native menus and keyboard shortcuts connect editor structure commands, export actions, terminal
creation, graph windows, and diagnostics to the desktop shell.

## Obsidian metadata and agent skills

For Obsidian-style vaults, Tesseract can derive mdvdb Topics from supported metadata and keep
provenance so app-managed definitions can be refreshed safely. This is a narrowly scoped Topic
import and synchronization workflow.

Tesseract can also install and update project-local agent skills for supported agent directories.
The files stay inside the collection, remain reviewable, and can be managed independently of the
desktop app.

## What remains authoritative

| Concern | Authority |
| --- | --- |
| Document content and authored properties | Markdown files |
| Field intent, Relations, Formula, Lookup, Rollup | `.markdownvdb.schema.yml` |
| Project runtime settings and Shards | `.markdownvdb/config.yaml` |
| Credentials | shell environment or `.env` secret files |
| Vector, lexical, graph, and Shard analysis | rebuildable `.markdownvdb/` state |
| Automation contract | mdvdb CLI JSON and NDJSON |

The separation matters: closing or uninstalling Tesseract does not strand the knowledge base in an
app-specific document format.

## Get the app

Tesseract publishes desktop builds separately from the mdvdb CLI. Visit
[tesseract.md](https://tesseract.md) for downloads and the app's release status. The app can locate
a compatible mdvdb executable and packaged builds can manage one when needed.

To begin with the CLI instead, continue to the [Quick Start](./quickstart.md).

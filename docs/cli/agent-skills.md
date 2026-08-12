---
title: "mdvdb Agent Skills"
description: "Install and use the mdvdb Agent Skills with Claude Code, Codex, Gemini CLI, Cursor, and GitHub Copilot"
category: "guides"
---

# mdvdb Agent Skills

The [mdvdb Agent Skills](https://github.com/geckse/markdown-vdb-skills) are 15 open-source,
portable instructions that teach an AI coding agent how to search, query, maintain, and visualize an
mdvdb collection. They use the `mdvdb` CLI already installed on your machine and do not add a
server or upload your Markdown files. The CLI does send document chunks to a hosted embedding
provider when you configure one; use a local Ollama model if content must stay on the machine.

The skills cover retrieval, cited synthesis, graph exploration, SQL-like frontmatter queries,
Relations, Shards, Topics, document authoring, and collection health. An agent can select the right
skill from a natural-language request, or you can name one explicitly where the agent supports it.

## Prerequisites

Install `mdvdb` **0.2.0 or newer** and make sure it is on `PATH`. For a new collection, configure
and probe an embedding provider before indexing; the [Quick Start](./quickstart.md#2-configure-an-embedding-provider)
covers hosted providers and local Ollama setup.

```bash
mdvdb --version
cd /path/to/your/markdown-collection
mdvdb init       # Skip this if the collection is already initialized
mdvdb embedding probe
mdvdb info
mdvdb ingest --preview
```

Review the provider, estimated work, and preview before running `mdvdb ingest`. A hosted provider
may receive document chunks and charge for embeddings. Ingest may also materialize declared
Formula, Lookup, and Rollup values into Markdown frontmatter; the preview estimates indexing work
but does not enumerate those computed-field patches.

Run your agent from the collection root when possible. Every mdvdb skill also supports
`--root <path-to-collection>` when the agent is working elsewhere.

## Claude Code

Claude Code can install the skills as the native `mdvdb` plugin. In a Claude Code session, add the
marketplace and install the plugin. Agent plugins are trusted instructions that can run commands or
edit files, so review the [skills repository](https://github.com/geckse/markdown-vdb-skills) before
installing or updating it.

```text
/plugin marketplace add geckse/markdown-vdb-skills
/plugin install mdvdb@mdvdb-skills
```

If the installation summary does not say the plugin is active, run `/reload-plugins` or start a new
session. Claude can choose a skill from your request, or you can call one explicitly with its plugin
namespace:

```text
/mdvdb:search-docs Find the decision that changed our authentication flow
/mdvdb:search-and-summarize Summarize the rollout plan and cite the source files
```

For a project-local manual install instead, copy the individual skill directories into
`.claude/skills`. On macOS or Linux:

```bash
git clone --depth 1 https://github.com/geckse/markdown-vdb-skills.git ../mdvdb-skills
mkdir -p .claude/skills
cp -R ../mdvdb-skills/plugins/mdvdb/skills/. .claude/skills/
```

On Windows PowerShell:

```powershell
git clone --depth 1 https://github.com/geckse/markdown-vdb-skills.git ..\mdvdb-skills
New-Item -ItemType Directory -Force .claude\skills | Out-Null
Copy-Item ..\mdvdb-skills\plugins\mdvdb\skills\* .claude\skills\ -Recurse -Force
```

See the [Claude Code plugin documentation](https://code.claude.com/docs/en/discover-plugins) for
marketplace management and the [Claude Code skills documentation](https://code.claude.com/docs/en/skills)
for skill discovery and invocation.

## Codex, Cursor, Gemini CLI, and GitHub Copilot

These agents understand portable Agent Skills from a project-local `.agents/skills` directory.
Before copying them, add `.agents/` to the collection's `.mdvdbignore`; otherwise a running
`mdvdb watch` process can ingest the `SKILL.md` instructions as collection content.

Clone the repository outside the collection, note the commit with `git rev-parse HEAD`, and review
the instructions before placing them in an agent discovery folder. On macOS or Linux, run this from
the collection root:

```bash
git clone --depth 1 https://github.com/geckse/markdown-vdb-skills.git ../mdvdb-skills
git -C ../mdvdb-skills rev-parse HEAD
mkdir -p .agents/skills
cp -R ../mdvdb-skills/plugins/mdvdb/skills/. .agents/skills/
```

On Windows PowerShell:

```powershell
git clone --depth 1 https://github.com/geckse/markdown-vdb-skills.git ..\mdvdb-skills
git -C ..\mdvdb-skills rev-parse HEAD
New-Item -ItemType Directory -Force .agents\skills | Out-Null
Copy-Item ..\mdvdb-skills\plugins\mdvdb\skills\* .agents\skills\ -Recurse -Force
```

The copy step matters: each subdirectory is one discoverable skill with its own `SKILL.md`. Keeping
only the outer Claude plugin directory in `.agents/skills` would hide those individual skills from
agents that scan one skill per folder.

Restart the agent or open a new session after copying the files. Then use the normal prompt box:

| Agent | How to use the installed skills |
|---|---|
| **Codex** | Ask naturally, or name a skill explicitly: `Use $search-docs to find the authentication decision and cite it.` |
| **Cursor** | Ask its Agent to use `search-docs`, or describe the mdvdb task naturally. |
| **Gemini CLI** | Run `/skills list` to confirm discovery, then ask Gemini to search or maintain the collection. Use `/skills reload` after changing skill files. |
| **GitHub Copilot** | Ask naturally in agent mode, or invoke a skill such as `/search-docs` where slash invocation is available. |

The portable folder is convenient when the same repository is used by more than one agent. If you
prefer each product's native project folder, copy the same 15 directories to the corresponding
location instead:

| Agent | Project-local skill folder |
|---|---|
| Claude Code | `.claude/skills` |
| Codex | `.agents/skills` |
| Cursor | `.cursor/skills` or `.agents/skills` |
| Gemini CLI | `.gemini/skills` or `.agents/skills` |
| GitHub Copilot | `.github/skills` or `.agents/skills` |

The vendor references describe the current discovery rules: [Codex skills](https://developers.openai.com/codex/build-skills),
[Cursor skills](https://cursor.com/docs/skills),
[Gemini CLI skills](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/skills.md), and
[GitHub Copilot skills](https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/customize-cloud-agent/add-skills).

### Keep skill instructions out of search results

mdvdb already excludes `.claude` and `.cursor` directories from ingestion. Before installing into
another folder, add the matching entry to `.mdvdbignore` so its `SKILL.md` files do not become
search results:

```text
# Keep only the entries that match your installation
.agents/
.gemini/
.github/skills/
```

If a watcher indexed the skills before you added the rule, run a normal incremental `mdvdb ingest`
afterward to remove those stale entries from the index.

## Ask for outcomes, not commands

The skills contain the command selection, safety checks, and output interpretation. A request can
describe the result you want:

```text
Search the vault for authentication decisions and cite the source files.

Show open invoices sorted by due date and populate each client Relation.

Explore everything related to the identity rollout, including linked documents.

Check collection health and show a repair plan before changing any files.

Use the research Shard and list notes not assigned to a Topic.
```

An agent may translate the invoice request into a deterministic, SQL-like frontmatter query with
`mdvdb collection`: Markdown files are rows, frontmatter keys are typed columns, Relations can be
populated, and computed Formula, Lookup, or Rollup values can be filtered and sorted after they are
materialized. For meaning-based questions, it can use hybrid search and then follow the link graph.

## Included skills

| Workflow | Skill | What it helps an agent do |
|---|---|---|
| Retrieve | `search-docs` | Run semantic, lexical, or hybrid search with filters, Relations, and `--populate` |
| Retrieve | `search-and-summarize` | Read top matches and produce a source-cited synthesis |
| Retrieve | `explore-topic` | Combine semantic search, graph expansion, and linked context |
| Retrieve | `find-related` | Follow Relations, semantic edges, links, backlinks, and multi-hop paths |
| Query | `query-collection` | Treat a folder as a frontmatter table with filters, sorting, pagination, and Relations |
| Organize | `manage-shards` | Create and use named recursive sub-collections over the shared index |
| Organize | `manage-topics` | Create, tune, and inspect multi-label Topics and the Unassigned bucket |
| Organize | `manage-relations` | Author, resolve, filter, and repair typed Markdown Relations |
| Maintain | `index-vault` | Preview, ingest, or re-index Markdown with cost estimates where available |
| Maintain | `vault-overview` | Inspect status, collection statistics, clusters, Topics, and the file tree |
| Maintain | `vault-health` | Run diagnostics, relation-integrity checks, orphan detection, and schema analysis |
| Author | `check-document` | Validate structure, schema fields, Relations, and link connectivity |
| Author | `enhance-document` | Improve frontmatter, headings, Relations, and links for retrieval |
| Author | `write-document` | Create an indexing-friendly Markdown file with the right metadata and links |
| Visualize | `graph-visualize` | Export and summarize the knowledge graph for visualization or analysis |

## Review write operations

Search, overview, and visualization workflows are generally read-only. Other skills can ingest an
index, edit Markdown or frontmatter, change mdvdb configuration, or manage Shards and Topics. The
agent's normal permission and approval rules still apply. Ask for a plan first when you want to
inspect proposed repairs or document edits before they are made.

`index-vault` is intentionally explicit-only because ingest changes the index and can materialize
computed frontmatter. Host-specific skill metadata is not interpreted identically by every agent,
so invoke that workflow explicitly outside Claude Code—for example, `$index-vault` in Codex—and
review its `mdvdb info` and `mdvdb ingest --preview` results before approving ingest.

## Refresh a manual installation

Review upstream changes, pull them, and copy the directories over the project installation. On
macOS or Linux:

```bash
git -C ../mdvdb-skills pull --ff-only
cp -R ../mdvdb-skills/plugins/mdvdb/skills/. .agents/skills/
```

Use `.claude/skills`, `.cursor/skills`, `.gemini/skills`, or `.github/skills` as the copy target if
you chose an agent-native folder. In PowerShell, rerun the `Copy-Item` command from the installation
section. These copy commands are additive: if an upstream skill is removed, compare the source and
target directories and remove only the obsolete target after review. For the Claude Code plugin
installation, review changes and manage updates through Claude Code's `/plugin` interface.

## Repositories

- [mdvdb Agent Skills](https://github.com/geckse/markdown-vdb-skills) — source for all 15 skills and the Claude Code plugin
- [mdvdb CLI](https://github.com/geckse/markdown-vdb) — the database and command-line tool the skills operate

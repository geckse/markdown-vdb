# Agent Memory Graph

Using markdown-vdb to turn plain Markdown files into a navigable AI agent knowledge graph.

---

AI agents that persist memory as Markdown files on disk can use mdvdb to make that memory **searchable** (semantic + lexical) and **connected** (link graph). The files are the source of truth — the model only "remembers" what gets written to disk. mdvdb indexes those files without ever modifying them, then exposes search, link traversal, and graph maintenance tools the agent can call at runtime.

No infrastructure required — just a directory of `.md` files and the `mdvdb` binary.

## Quick Setup

```bash
# In the agent's working directory:
mdvdb init          # Create .markdownvdb config
mdvdb ingest        # Index all markdown files
mdvdb status        # Confirm file/chunk/vector counts
```

## Memory as Plain Files

mdvdb treats Markdown files as read-only input. It indexes their content, frontmatter, and internal links — but never writes to them. The agent writes files; mdvdb indexes them. The agent queries mdvdb to recall what it wrote.

After writing a file, the agent can index it immediately with single-file ingest:

```bash
mdvdb ingest --file decisions/redis-cache.md
```

## Frontmatter as Memory Metadata

YAML frontmatter turns unstructured notes into queryable, filterable memory. mdvdb auto-infers a schema from all frontmatter fields across your files and exposes them as filter dimensions via `--filter KEY=VALUE`.

### Recommended Frontmatter Fields for Agent Memory

| Field | Purpose | Example values |
|-------|---------|----------------|
| `type` | Classify the memory entry | `log`, `decision`, `topic`, `error`, `user-preference` |
| `date` | When the memory was created | `2025-06-15` |
| `tags` | Semantic labels for cross-cutting concerns | `[auth, security]`, `[deployment, infra]` |
| `status` | Lifecycle state | `active`, `superseded`, `archived` |
| `confidence` | How confident the agent is in this information | `high`, `medium`, `low` |
| `source` | Where the information came from | `user`, `codebase`, `external-docs`, `inferred` |
| `related` | Explicit list of related files (supplements links in body) | `[topics/auth.md, topics/middleware.md]` |

### Example: Decision Record

```markdown
---
type: decision
date: 2025-06-15
tags: [architecture, caching]
status: active
confidence: high
source: user
---

# Decision: Use Redis for Session Cache

User confirmed Redis over Memcached. Key reasons:
- Need for data structures beyond simple key-value
- Pub/sub for cache invalidation across services

See also: [Performance Requirements](requirements/performance.md)
Supersedes: [[decisions/memcached-eval]]
```

### Example: Error Memory

```markdown
---
type: error
date: 2025-06-16
tags: [deployment, docker]
status: active
confidence: high
source: codebase
---

# Error: Container OOM on Deploy

The API container hits OOM with default 512MB limit during cold start.

**Fix:** Set memory limit to 1GB in `docker-compose.yml`, line 42.

First observed in [[logs/2025-06-16]]. Related: [[topics/deployment]]
```

### Example: User Preference

```markdown
---
type: user-preference
date: 2025-06-14
tags: [workflow, style]
status: active
confidence: high
source: user
---

# User Preferences

- Always use `bun` instead of `npm` for package management
- Prefer functional components over class components in React
- Never auto-commit — always ask first
- Test framework: vitest (not jest)

Updated from [[logs/2025-06-14]] session.
```

### Example: Topic File

```markdown
---
type: topic
tags: [architecture, backend]
status: active
---

# Authentication System

The app uses JWT tokens with refresh rotation.

## Flow
1. User submits credentials to `/api/auth/login`
2. Server returns `access_token` (15m) + `refresh_token` (7d)
3. Middleware validates tokens on every request — see [[topics/middleware]]

## Key Files
- `src/auth/jwt.rs` — token generation and validation
- `src/middleware/auth.rs` — request guard

## History
- Initial implementation: [[logs/2025-06-10]]
- Switched from session cookies to JWT: [[decisions/jwt-over-sessions]]
- Added refresh rotation: [[logs/2025-06-14]]
```

### Filtering with Frontmatter

The real power is in filtered search — the agent can narrow recall to specific memory types:

```bash
# Find all active decisions about caching:
mdvdb search "caching" --filter type=decision --filter status=active

# Find error memories related to deployment:
mdvdb search "OOM" --filter type=error --filter tags=deployment

# Find all user preferences:
mdvdb search "" --filter type=user-preference

# Find low-confidence memories that may need verification:
mdvdb search "" --filter confidence=low

# Combine with link boosting for richer recall:
mdvdb search "authentication" --filter type=topic --boost-links
```

### Schema Introspection

The agent can inspect what frontmatter fields exist across all indexed files:

```bash
mdvdb schema --json
```

This returns every field name, its inferred type, and example values — useful for an agent to discover what filters are available without hardcoding them.

## Headings as Memory Boundaries

mdvdb chunks files by headings — every `#`, `##`, `###` section becomes its own independently searchable unit with its own embedding vector. This is critical for agent memory because a single file often contains multiple distinct pieces of knowledge.

Consider a daily log:

```markdown
---
type: log
date: 2025-06-15
---

# Session Log: 2025-06-15

## Reviewed caching options
Compared Redis vs Memcached. Redis wins on data structures.
See [[decisions/redis-cache]] for the decision record.

## Fixed auth token expiry bug
The refresh token rotation was off by one. Changed `expires_at`
from `now + 7d` to `now + 7d - 1m` to prevent edge-case failures.
Related: [[topics/auth]]

## Deployed v2.3.1
Deployment went smoothly. Updated [[topics/deployment]] with new steps.
```

This file produces **three separate chunks**, one per `##` section. A search for "token expiry" returns the auth section specifically — not the entire day's log. The search result includes the section heading and file path, so the agent knows exactly where in the file the match is.

### Structure Memory Files for Precise Recall

Use this to your advantage when writing memory:

- **Use headings to separate distinct topics** within a single file. Each heading becomes a chunk boundary.
- **Keep each section focused** on one idea, decision, or observation. This produces tighter embeddings and more relevant search hits.
- **Put the key insight in the heading** — headings are included in the chunk text and influence the embedding. `## Fixed auth token expiry bug` is more searchable than `## Bug Fix #3`.
- **Use `##` sections in topic files** to separate sub-topics. A `topics/auth.md` with sections for "Flow", "Key Files", and "History" produces three targeted chunks instead of one broad one.

If a single section exceeds the token limit (configurable, defaults to 512 tokens), mdvdb applies a secondary size guard that splits it further — so long sections are still handled correctly.

### Searching at Section Level

Search results point to specific sections, not just files:

```bash
mdvdb search "token expiry" --json --limit 1
```

The result includes the `section` field (the heading text) and `chunk_index`, letting the agent pinpoint exactly which part of which file matched.

## Two-Layer Memory: Logs and Long-Term

A practical agent memory layout uses two layers connected by links.

**Layer 1: Daily logs** — append-only, date-stamped. Raw observations, decisions, and context written during a session.

**Layer 2: Curated memory** — a `MEMORY.md` file and topic-specific files. Distilled facts that link back to the daily logs for provenance.

```
memory/
  .markdownvdb
  MEMORY.md                       # Curated long-term memory (index file)
  logs/
    2025-06-14.md                 # Daily session logs (append-only)
    2025-06-15.md
    2025-06-16.md
  topics/
    architecture.md               # Topic-specific deep knowledge
    api-design.md
    deployment.md
    middleware.md
  decisions/
    redis-cache.md                # Decision records with rationale
    jwt-over-sessions.md
  preferences/
    workflow.md                   # User preferences and conventions
```

### Example: MEMORY.md

```markdown
---
type: memory
status: active
tags: [index, curated]
---

# Long-Term Memory

## Architecture
- Using Redis for session caching. Decision made 2025-06-15.
  See [[logs/2025-06-15]] for full context.
  Decision record: [[decisions/redis-cache]]
- API follows REST conventions with JSON responses.
  See [[topics/api-design]] for details.

## User Preferences
- Always use `bun`, never `npm`. See [[preferences/workflow]].
- Test with vitest, not jest.

## Key Contacts
- Infrastructure team owns deployment pipeline.
  See [[topics/deployment]] for runbook.
```

### Example: Daily Log

```markdown
---
type: log
date: 2025-06-15
tags: [architecture, deployment]
source: session
---

# Session Log: 2025-06-15

## 10:30 — Reviewed caching options
Compared Redis vs Memcached. Redis wins on data structures.
See [architecture notes](../topics/architecture.md) for broader context.
Created decision record: [[decisions/redis-cache]]

## 14:00 — Deployed v2.3.1
Deployment went smoothly. Updated [[topics/deployment]] with new steps.
```

The wikilinks (`[[logs/2025-06-15]]`) and standard links (`[text](path.md)`) are both extracted by mdvdb during ingest. They create an implicit graph connecting logs to curated memory to topic files.

## How Links Create a Knowledge Graph

Every `[text](path.md)` and `[[wikilink]]` inside a markdown file is extracted during `mdvdb ingest`. The link graph is stored in the index alongside embeddings and metadata.

Forward links (outgoing) are the source of truth. Backlinks (incoming) are computed from them — no duplication. External URLs (`https://`, `mailto:`) are filtered out. Self-links are excluded. Only internal relative links between markdown files are tracked.

The graph from the two-layer example above:

```
  MEMORY.md ─────────┬──→ topics/architecture.md
                     │        ↑
                     ├──→ topics/api-design.md
                     │
                     └──→ logs/2025-06-15.md
                               │
                               ├──→ topics/architecture.md
                               └──→ topics/deployment.md
```

A flat folder of `.md` files becomes a directed graph. mdvdb lets you traverse it in both directions.

**Wikilink resolution:**

| Syntax | Resolves to |
|--------|-------------|
| `[[page-name]]` | `page-name.md` |
| `[[path/to/note\|display text]]` | `path/to/note.md` |
| `[[logs/2025-06-15]]` | `logs/2025-06-15.md` |

## Navigating the Graph

### Links and Backlinks

`mdvdb links <file>` shows outgoing links and incoming backlinks for a file:

```bash
mdvdb links MEMORY.md
```

```
  ● MEMORY.md

  Outgoing: 3
  ├── topics/architecture.md  "architecture notes"
  │   line 5
  ├── topics/api-design.md    "details"  [wikilink]
  │   line 8
  └── logs/2025-06-15.md      "full context"  [wikilink]
      line 4

  Incoming: 1
  ├── logs/2025-06-16.md      "see long-term memory"  [wikilink]
      line 12

  3 outgoing, 1 incoming
```

Broken links (pointing to files that don't exist) are flagged with a `[broken]` badge.

`mdvdb backlinks <file>` shows only the incoming links:

```bash
mdvdb backlinks topics/architecture.md
```

```
  ● Backlinks to topics/architecture.md

  Incoming: 2 incoming links

  ├── MEMORY.md               "architecture notes"
  │   line 5
  └── logs/2025-06-15.md      "broader context"
      line 12
```

### JSON Mode for Agents

All commands support `--json` for machine-readable output:

```bash
mdvdb links MEMORY.md --json
```

```json
{
  "file": "MEMORY.md",
  "links": {
    "file": "MEMORY.md",
    "outgoing": [
      {
        "entry": {
          "source": "MEMORY.md",
          "target": "topics/architecture.md",
          "text": "architecture notes",
          "line_number": 5,
          "is_wikilink": false
        },
        "state": "Valid"
      }
    ],
    "incoming": [
      {
        "source": "logs/2025-06-16.md",
        "target": "MEMORY.md",
        "text": "see long-term memory",
        "line_number": 12,
        "is_wikilink": true
      }
    ]
  }
}
```

```bash
mdvdb backlinks topics/architecture.md --json
```

```json
{
  "file": "topics/architecture.md",
  "backlinks": [
    {
      "entry": {
        "source": "MEMORY.md",
        "target": "topics/architecture.md",
        "text": "architecture notes",
        "line_number": 5,
        "is_wikilink": false
      },
      "state": "Valid"
    }
  ],
  "total_backlinks": 1
}
```

### Finding Orphan Files

`mdvdb orphans` finds files with no incoming **and** no outgoing links — disconnected islands in the graph:

```bash
mdvdb orphans
```

```
  ● Orphan Files (2) — no incoming or outgoing links

  • scratch/ideas.md
  • old/deprecated-api-notes.md

  Total: 2 orphan files
```

For agent memory, orphans often indicate stale notes, forgotten context, or files that should be linked into the curated memory.

```bash
mdvdb orphans --json
```

```json
{
  "total_orphans": 2,
  "orphans": [
    { "path": "scratch/ideas.md" },
    { "path": "old/deprecated-api-notes.md" }
  ]
}
```

## Searching Agent Memory

### Semantic Search

```bash
# Find memories about caching decisions:
mdvdb search "caching strategy decision"

# Restrict to decision documents via frontmatter filter:
mdvdb search "caching" --filter type=decision

# Restrict to a specific directory:
mdvdb search "deployment" --path logs/

# JSON output for programmatic use:
mdvdb search "caching strategy" --json --limit 3
```

### Search Modes

```bash
mdvdb search "error handling" --semantic     # Pure vector similarity
mdvdb search "ERR_CONNECTION_REFUSED" --lexical  # Keyword/BM25
mdvdb search "auth middleware"               # Hybrid (default)
```

### Link-Boosted Search

The `--boost-links` flag activates link-aware scoring. After normal ranking, mdvdb checks the top results' link neighbors (files they link to, and files that link to them). Any other result that is a link neighbor gets a 1.2x score boost, then results are re-sorted.

```bash
mdvdb search "authentication" --boost-links
```

This is powerful for agent memory because linked documents are explicitly related by the agent. If the top result is `topics/auth.md` and it links to `topics/middleware.md`, and `topics/middleware.md` also appears in the results, its score gets boosted — documents the agent explicitly connected are more likely to be relevant.

### Recency Decay

Agent memory accumulates over time — old notes become less relevant as the project evolves. The `--decay` flag applies an exponential time-decay multiplier based on each file's last modification time:

```
score * 0.5 ^ (days_since_modified / half_life_days)
```

A file edited today scores at full strength. A file untouched for 90 days (the default half-life) scores at 50%. Older files fade further but are never fully excluded.

```bash
# Prefer recent memory (default 90-day half-life):
mdvdb search "deployment process" --decay

# Shorter half-life for fast-moving projects:
mdvdb search "deployment process" --decay --decay-half-life 30

# Combine with link boosting and filters:
mdvdb search "auth" --decay --boost-links --filter type=topic
```

This is especially useful for agent memory because:
- **Daily logs** naturally decay — last week's session is more relevant than last quarter's
- **Active topic files** that the agent keeps updating stay ranked high
- **Superseded decisions** fade without needing manual `status: archived` updates
- **Curated MEMORY.md** stays prominent as long as the agent keeps editing it

Decay is disabled by default. Enable it globally via `MDVDB_SEARCH_DECAY=true` in `.markdownvdb`, or per-query with `--decay` / `--no-decay`.

## Practical Workflows

### Session Start: Loading Context

At session start, the agent searches for relevant context and follows links to build a working set:

```bash
# 1. Search for the current task context:
mdvdb search "user authentication refactor" --json --limit 5

# 2. Follow links from the top result to find related memory:
mdvdb links topics/auth.md --json

# 3. Read the linked files to build full context.
```

### During Session: Writing and Linking

As the agent works, it appends observations and decisions to the daily log, linking to existing topic files:

```markdown
## 11:00 — Auth Refactor Progress
Moved session validation to middleware layer.
Updated [[topics/auth]] with the new flow.
Related: [middleware docs](../topics/middleware.md)
```

Both `[[topics/auth]]` (wikilink, resolves to `topics/auth.md`) and `[middleware docs](../topics/middleware.md)` (standard link, resolved relative to the source file) are extracted and indexed.

After writing, re-index incrementally:

```bash
mdvdb ingest --file logs/2025-06-16.md
mdvdb ingest --file topics/auth.md
```

### Session End: Curating Memory

At session end, the agent (or human) reviews the day's log and promotes important facts to `MEMORY.md` or topic files, adding links back to the log for provenance:

```bash
# 1. Review what was written and linked today:
mdvdb links logs/2025-06-16.md --json

# 2. Edit MEMORY.md to add distilled facts with links to the log.

# 3. Re-ingest:
mdvdb ingest
```

### Maintenance: Orphans and Broken Links

```bash
# Find disconnected files that should be linked in or archived:
mdvdb orphans

# Check a specific file for broken links (shown with [broken] badge):
mdvdb links topics/architecture.md
```

## Live Re-Indexing with the Watcher

For long-running agent sessions, `mdvdb watch` monitors the source directory and re-indexes automatically:

```bash
mdvdb watch
```

The link graph is updated incrementally — when a file changes, its links are re-extracted; when a file is deleted, its links are removed; when a file is renamed, old links are removed and new ones are added. The watcher debounces rapid changes so the agent can write freely.

## Library API

For deeper integration, agents can use the Rust library directly:

```rust
use mdvdb::{MarkdownVdb, SearchQuery, IngestOptions};

let vdb = MarkdownVdb::open("./memory")?;

// Index all files:
vdb.ingest(IngestOptions::default()).await?;

// Semantic search with link boosting:
let results = vdb.search(
    SearchQuery::new("authentication")
        .with_boost_links(true)
        .with_limit(5)
).await?;

// Traverse links from a result:
let link_info = vdb.links("topics/auth.md")?;
for link in &link_info.outgoing {
    println!("→ {} (\"{}\")", link.entry.target, link.entry.text);
}
for entry in &link_info.incoming {
    println!("← {} (\"{}\")", entry.source, entry.text);
}

// Find what links back to this file:
let backlinks = vdb.backlinks("topics/auth.md")?;

// Find orphan files:
let orphans = vdb.orphans()?;
```

## Tips

- **Use wikilinks for ease** — `[[topic-name]]` is faster to write than `[text](path.md)` and auto-resolves to `topic-name.md`.
- **Use frontmatter consistently** — at minimum, include `type` and `tags` on every memory file. This enables filtered search (`--filter type=decision`) and makes `mdvdb schema` useful for discoverability.
- **Keep curated memory small** — `MEMORY.md` should contain distilled facts, not raw logs. Link to logs for full context.
- **Run orphan checks regularly** — orphan files indicate forgotten knowledge. Link them in or archive them.
- **Use `--boost-links` for recall** — link boosting leverages the agent's own cross-references to improve search results.
- **Single-file ingest is fast** — `mdvdb ingest --file <path>` immediately after writing a file for near-instant indexing.
- **Always use `--json` for programmatic access** — all commands support it. Agents should parse JSON, not human-readable output.
- **Use the watcher for long sessions** — `mdvdb watch` keeps the index current without manual re-ingestion.
- **Paths are always relative** — all file paths in the index and in command arguments are relative to the project root.

## Complete Example

```
agent-workspace/
  .markdownvdb
  MEMORY.md                       # type: memory
  logs/
    2025-06-14.md                 # type: log
    2025-06-15.md                 # type: log
  topics/
    auth.md                       # type: topic
    deployment.md                 # type: topic
    performance.md                # type: topic
  decisions/
    redis-cache.md                # type: decision
    jwt-over-sessions.md          # type: decision
  preferences/
    workflow.md                   # type: user-preference
  scratch/
    ideas.md                      ← orphan (no links in or out)
```

```bash
cd agent-workspace
mdvdb init
mdvdb ingest

# Check what frontmatter schema was inferred:
mdvdb schema

# Inspect the graph:
mdvdb links MEMORY.md
mdvdb backlinks topics/auth.md
mdvdb orphans                     # → scratch/ideas.md

# Search by type:
mdvdb search "caching" --filter type=decision
mdvdb search "" --filter type=user-preference

# Search with link boosting:
mdvdb search "authentication flow" --boost-links

# Start watching for live changes:
mdvdb watch
```

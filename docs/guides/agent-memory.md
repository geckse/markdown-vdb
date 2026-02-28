# Agent Memory

Using markdown-vdb as a search layer for plain-file agent memory.

---

AI agents that write their memory to Markdown files can use mdvdb to search and recall that memory semantically. No graph traversal, no link extraction — just files, frontmatter, and search. The files are the source of truth; the model only "remembers" what gets written to disk.

## Quick Setup

```bash
cd agent-workspace
mdvdb init
mdvdb ingest
```

## How It Works

1. The agent writes Markdown files to disk during a session
2. `mdvdb ingest` indexes their content and frontmatter as embeddings
3. `mdvdb search` recalls relevant memory by meaning, not just keywords
4. mdvdb never modifies the files — it only reads and indexes

## Single-File Memory

The simplest pattern: one `MEMORY.md` file that the agent reads at session start and appends to during work.

```markdown
---
type: memory
status: active
---

# Agent Memory

## Project Stack
- Rust backend, React frontend, PostgreSQL database
- Deployment via Docker Compose on DigitalOcean

## User Preferences
- Always use `bun` instead of `npm`
- Never auto-commit — always ask first
- Prefer functional components in React
- Test framework: vitest (not jest)

## Key Decisions
- Using Redis for session caching (confirmed 2025-06-15)
- JWT with refresh rotation for auth (switched from session cookies 2025-06-10)
- API follows REST conventions with JSON responses

## Known Issues
- Container hits OOM with default 512MB limit during cold start — use 1GB
- Search indexing breaks if filenames contain unicode emoji
```

The agent reads this file directly at session start. As it grows, mdvdb makes it searchable — each `##` section becomes its own chunk with a separate embedding, so `mdvdb search "authentication"` returns the "Key Decisions" section specifically, not the whole file.

```bash
mdvdb ingest --file MEMORY.md
mdvdb search "what package manager" --json --limit 1
```

## Multi-File Memory

As memory grows, split into multiple files. A common layout:

```
memory/
  .markdownvdb
  MEMORY.md                   # Curated essentials (always loaded)
  logs/
    2025-06-14.md             # Daily session logs (append-only)
    2025-06-15.md
    2025-06-16.md
  topics/
    auth.md                   # Deep knowledge by subject
    deployment.md
    database.md
```

### MEMORY.md — The Index File

Keep this small. It holds distilled facts the agent should always have in context. When the agent learns something important, it adds a line here.

```markdown
---
type: memory
---

# Memory

## Stack
- Rust + React + PostgreSQL
- Docker Compose deploy on DigitalOcean

## Preferences
- bun (not npm), vitest (not jest)
- Never auto-commit

## Important
- Redis for caching, JWT for auth
- Container memory limit must be 1GB (not default 512MB)
```

### Daily Logs — Append-Only

One file per day. The agent appends observations, decisions, and context as it works. These are raw and chronological — not curated.

```markdown
---
type: log
date: 2025-06-15
tags: [architecture, deployment]
---

# 2025-06-15

## Reviewed caching options
Compared Redis vs Memcached. Redis wins on data structures and pub/sub.

## Fixed auth token expiry bug
Refresh token rotation was off by one. Changed `expires_at` from
`now + 7d` to `now + 7d - 1m` to prevent edge-case failures.

## Deployed v2.3.1
Went smoothly. Updated container memory limit to 1GB in docker-compose.yml.
```

### Topic Files — Deep Knowledge

One file per subject area. The agent creates these when a topic accumulates enough detail to warrant its own file.

```markdown
---
type: topic
tags: [architecture, backend]
---

# Authentication

The app uses JWT tokens with refresh rotation.

## Flow
1. User submits credentials to `/api/auth/login`
2. Server returns `access_token` (15m) + `refresh_token` (7d)
3. Middleware validates on every request

## Key Files
- `src/auth/jwt.rs` — token generation and validation
- `src/middleware/auth.rs` — request guard

## Gotchas
- Refresh rotation off-by-one was fixed 2025-06-15
- Token validation requires `RS256` — do not switch to `HS256`
```

## Frontmatter for Filtered Recall

YAML frontmatter makes memory filterable. mdvdb auto-infers a schema from all frontmatter fields and lets you filter with `--filter KEY=VALUE`.

### Recommended Fields

| Field | Purpose | Example values |
|-------|---------|----------------|
| `type` | What kind of memory | `memory`, `log`, `topic`, `error`, `decision`, `user-preference` |
| `date` | When it was written | `2025-06-15` |
| `tags` | Cross-cutting labels | `[auth, security]`, `[deployment, docker]` |
| `status` | Lifecycle | `active`, `superseded`, `archived` |
| `confidence` | How reliable | `high`, `medium`, `low` |
| `source` | Origin of the information | `user`, `codebase`, `inferred` |

### Example: Error Memory

```markdown
---
type: error
date: 2025-06-16
tags: [deployment, docker]
confidence: high
source: codebase
---

# Container OOM on Deploy

The API container hits OOM with default 512MB limit during cold start.

**Fix:** Set memory limit to 1GB in `docker-compose.yml`, line 42.
```

### Filtering in Search

```bash
# Find error memories:
mdvdb search "OOM" --filter type=error

# Find all user preferences:
mdvdb search "" --filter type=user-preference

# Find active decisions about caching:
mdvdb search "caching" --filter type=decision --filter status=active

# Find low-confidence memories to verify:
mdvdb search "" --filter confidence=low
```

## Headings as Chunk Boundaries

mdvdb splits files at headings — each `##` section gets its own embedding vector. This means a search for "token expiry" returns just the relevant section from a 200-line daily log, not the entire file.

Write memory with this in mind:

- **One topic per heading** — produces tight, focused embeddings
- **Descriptive headings** — `## Fixed auth token expiry bug` is more findable than `## Bug Fix #3`
- **Use `##` sections in topic files** — `topics/auth.md` with "Flow", "Key Files", and "Gotchas" sections produces three targeted search results

## Searching Memory

### Basic Search

```bash
# Semantic search — finds by meaning:
mdvdb search "what authentication method do we use"

# Keyword search — finds exact terms:
mdvdb search "RS256" --lexical

# Hybrid (default) — combines both:
mdvdb search "auth token rotation"
```

### Narrowing Results

```bash
# Only search within logs:
mdvdb search "deployment" --path logs/

# Only search topic files:
mdvdb search "deployment" --path topics/

# Filter by frontmatter:
mdvdb search "deployment" --filter type=topic

# Limit results:
mdvdb search "deployment" --limit 3
```

### JSON Output for Agents

Agents should always use `--json` to parse structured output:

```bash
mdvdb search "authentication" --json --limit 3
```

The result includes `file`, `section` (heading text), `score`, and `chunk_index` for each hit — enough to locate and read the exact section.

### Recency Decay

The `--decay` flag soft-boosts recently modified files. Old memory fades in ranking but is never excluded.

```bash
# Prefer recent memory (default 90-day half-life):
mdvdb search "deployment" --decay

# Shorter half-life for fast-moving projects:
mdvdb search "deployment" --decay --decay-half-life 30
```

Daily logs naturally decay over time. Topic files the agent keeps updating stay ranked high. Disabled by default — enable per-query with `--decay` or globally with `MDVDB_SEARCH_DECAY=true` in `.markdownvdb`.

## Agent Workflows

### Session Start

```bash
# 1. Read curated memory directly:
cat MEMORY.md

# 2. Search for task-relevant context:
mdvdb search "the current task description" --json --limit 5

# 3. Read the matched files/sections.
```

### During Session

```bash
# Agent writes to today's log, then indexes it:
mdvdb ingest --file logs/2025-06-16.md

# Agent updates a topic file, then indexes it:
mdvdb ingest --file topics/auth.md
```

Single-file ingest (`--file`) is fast — suitable for calling right after every write.

### Session End

The agent (or human) reviews the day's log and promotes important facts to `MEMORY.md` or topic files:

```bash
# Re-ingest everything after edits:
mdvdb ingest
```

### Long Sessions

For continuous work, use the file watcher instead of manual ingest:

```bash
mdvdb watch
```

The watcher picks up file changes automatically and re-indexes in the background.

## Schema Discovery

The agent can inspect what frontmatter fields exist without hardcoding them:

```bash
mdvdb schema --json
```

Returns every field name, inferred type, and example values — useful for dynamically building filter queries.

## Tips

- **Start with one file.** A single `MEMORY.md` is enough. Split into multiple files only when it grows unwieldy.
- **Use frontmatter on every file** — at minimum `type` and `tags`. This makes filtered search useful from day one.
- **Descriptive headings matter** — they're included in the chunk embedding and are the primary way mdvdb identifies section boundaries.
- **Single-file ingest after every write** — `mdvdb ingest --file <path>` keeps the index current without re-processing everything.
- **Always use `--json`** for agent consumption. Parse JSON, not human-readable output.
- **Use `--decay` for active projects** — recent memory is usually more relevant.
- **Paths are always relative** — all file references are relative to the project root.
- **mdvdb never writes to your files** — it's a read-only index. The agent owns the files.

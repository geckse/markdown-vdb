---
title: "Build an AI memory layer"
description: "Store inspectable agent memories in Markdown and retrieve recent events alongside durable knowledge."
category: "use-cases"
---

# Build an AI memory layer

An AI memory layer has at least two time horizons:

- Episodic memory records what happened in sessions, tool runs, and decisions.
- Durable memory holds consolidated facts, preferences, procedures, and topic
  summaries.

Markdown VDB can retrieve both from local Markdown while applying time decay
only to the fast-changing layer. Old memories remain searchable; they are
down-ranked rather than deleted.

## Separate decaying and durable memory

Use folders to make retention intent explicit:

~~~text
memory/
├── logs/          # episodic events; decay applies
├── sessions/      # session provenance
├── topics/        # consolidated summaries; evergreen
├── core/          # stable identity, constraints, preferences
└── decisions/     # durable decisions and rationale
~~~

An episodic record can be small and structured:

~~~markdown
---
type: event
status: resolved
confidence: 0.92
session: memory/sessions/2026-08-06-deploy.md
systems:
  - gateway
  - identity
---

# Gateway deployment failed

The deployment failed because the identity service rejected the rotated
client certificate. Reissuing the certificate resolved the failure.

## Evidence

The gateway logged TLS_CLIENT_CERT_EXPIRED during the rollout.
~~~

The body preserves evidence for semantic and lexical retrieval. Frontmatter
supports exact filters, and the session field becomes a Relation that keeps
provenance traversable.

## Configure the collection

After **mdvdb init**, select the memory folder and configure decay:

~~~yaml
sources:
  dirs:
    - memory

search:
  mode: hybrid
  boost_links: true
  expand_graph: 1
  decay:
    enabled: true
    half_life: 30
    include:
      - memory/logs
    exclude:
      - memory/core
      - memory/topics
~~~

A non-empty **include** list is a whitelist. Here only records beneath
**memory/logs** decay. **exclude** always wins if a path matches both lists,
which makes the evergreen intent explicit.

The default half-life is 90 days when no value is configured; decay itself is
off by default. At a 30-day half-life, a matching record's score multiplier is
0.5 after 30 days, 0.25 after 60 days, and so on.

Decay uses the Markdown file's filesystem modification time, not a date in
frontmatter or its filename. Editing a decayed file refreshes its age.
Evergreen folders should therefore be excluded or omitted from the whitelist.

Read [Time decay](../concepts/time-decay.md) for the scoring model.

## Write and refresh memories

The agent or application writes normal Markdown files. Refresh one new memory
without rescanning everything:

~~~bash
mdvdb ingest --file memory/logs/2026-08-06-gateway-deploy.md
~~~

For an interactive process that continually writes files, run:

~~~bash
mdvdb watch
~~~

Full and incremental ingestion update chunks, fields, Relations, search
indexes, and graph analysis. Keeping the write format as Markdown makes every
memory inspectable, editable, portable, and diffable.

## Retrieve recent episodes

Search only episodic records and apply an explicit per-query decay policy:

~~~bash
mdvdb search "what failed during the gateway deployment?" \
  --path memory \
  --filter type=event \
  --decay \
  --decay-half-life 30 \
  --decay-include memory/logs \
  --decay-exclude memory/core,memory/topics \
  --json
~~~

The include and exclude flags accept comma-separated path prefixes. Query
flags override the configured decay behavior for that search.

When recency would hide a historically important episode, disable it:

~~~bash
mdvdb search "first identity migration decision" \
  --path memory \
  --no-decay \
  --json
~~~

## Retrieve durable memory with context

Link stable summaries to the sessions, events, and decisions that support
them. Then combine ranking with graph context:

~~~bash
mdvdb search "current authentication constraints" \
  --path memory \
  --filter status=active \
  --boost-links \
  --hops 2 \
  --expand 1 \
  --populate \
  --json
~~~

Frontmatter filters prevent superseded or draft memories from entering the
answer. Link boosting rewards connected records, graph expansion supplies
supporting neighbors, and population resolves explicit provenance Relations.

## Consolidate instead of endlessly appending

A useful memory process is:

1. Append evidence-rich episodic records beneath **memory/logs**.
2. Retrieve related recent and historical records for a topic.
3. Update one durable summary beneath **memory/topics**.
4. Link that summary to the episodes and decisions that justify it.
5. Mark contradicted knowledge as superseded instead of silently deleting its
   history.

For example:

~~~yaml
type: topic
status: active
confidence: 0.96
evidence:
  - memory/logs/2026-08-06-gateway-deploy.md
  - memory/decisions/certificate-rotation.md
~~~

This combines relevance, recency, exact state, and provenance without turning
the model's hidden context window into the memory authority.

## Related pages

- [Time decay](../concepts/time-decay.md)
- [Search](../commands/search.md)
- [Frontmatter as structured data](../concepts/frontmatter-data.md)
- [Relations](../concepts/relations.md)
- [The link graph](../concepts/link-graph.md)
- [Chunking](../concepts/chunking.md)
- [JSON output](../json-output.md)


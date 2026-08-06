---
title: "Build an LLM-ready wiki"
description: "Turn a Markdown knowledge base into grounded, structured, graph-aware retrieval for agents and assistants."
category: "use-cases"
---

# Build an LLM-ready wiki

An LLM wiki needs more than vector similarity. It should answer a question
from the right passage, preserve file and heading provenance, respect
publication metadata, and expose links that help an agent gather adjacent
context.

Markdown VDB provides those pieces over an ordinary Markdown tree:

- Heading-aware chunks for precise retrieval
- Hybrid semantic and lexical search
- Frontmatter filters for state, audience, product, or owner
- Wiki-link, Markdown-link, and Relation graph context
- JSON output for agent pipelines
- Markdown files that remain reviewable in Git

## Organize the wiki

A practical layout is:

~~~text
wiki/
├── concepts/
├── guides/
├── decisions/
├── reference/
└── people/
~~~

Give important pages small, consistent frontmatter:

~~~markdown
---
type: guide
product: billing
audience: developer
status: published
owner: wiki/people/maya.md
prerequisites:
  - wiki/concepts/idempotency.md
  - wiki/guides/authentication.md
---

# Retrying billing requests

Use an idempotency key for every retried write.

## Backoff policy

Retry transient failures with capped exponential backoff.
~~~

The body is retrieval content. Frontmatter supplies exact constraints, and
the link-shaped owner and prerequisite values become
[Relations](../concepts/relations.md).

## Initialize and ingest

At the Collection root:

~~~bash
mdvdb init
~~~

Configure an embedding provider. This local example uses Ollama:

~~~yaml
embedding:
  provider: ollama
  model: nomic-embed-text
  dimensions: auto

sources:
  dirs:
    - wiki

search:
  mode: hybrid
  boost_links: true
  expand_graph: 1
~~~

Provider credentials and remote alternatives are covered in
[Embedding providers](../concepts/embedding-providers.md). Then probe and build the
index:

~~~bash
mdvdb embedding probe
mdvdb ingest
~~~

Ingestion parses frontmatter and links, splits content around Markdown
headings, embeds changed chunks, updates lexical search, and refreshes the
graph. Later ingests skip unchanged content.

## Give the corpus a stable name

Create a Shard when the repository contains more than the wiki:

~~~bash
mdvdb shards add wiki --name Wiki --path wiki
~~~

A Shard is a named folder lens over the shared Collection, not another index.
Now scripts can use **--shard wiki** without repeating or changing the folder
path. See [Shards and Topics](../concepts/shards-and-topics.md).

## Retrieve grounded context

A strong agent query combines relevance ranking, exact metadata, links, and
resolved frontmatter Relations:

~~~bash
mdvdb search "How should billing writes be retried?" \
  --shard wiki \
  --filter status=published \
  --boost-links \
  --hops 2 \
  --expand 1 \
  --populate \
  --json
~~~

This returns ranked document chunks with file and heading provenance.
**--boost-links** lets nearby authoritative pages influence ranking;
**--expand 1** returns connected documents as supplementary graph context;
**--populate** resolves frontmatter Relations one level deep.

Use a second filter when the assistant has a known audience or product:

~~~bash
mdvdb search "authentication setup" \
  --shard wiki \
  --filter status=published \
  --filter audience=developer \
  --filter product=billing \
  --json
~~~

Repeated filters are ANDed. For exact identifiers or error codes, lexical
search is often the clearest choice:

~~~bash
mdvdb search "BILLING_RETRY_EXHAUSTED" \
  --shard wiki \
  --lexical \
  --json
~~~

## A simple agent retrieval loop

1. Search with **--json**, adding metadata filters from the user's context.
2. Select the highest-value chunks and keep their path and heading as
   citations.
3. Inspect a key page with **mdvdb get FILE --populate --json**.
4. Follow graph context or populated prerequisites only when the answer needs
   more evidence.
5. Generate an answer that names the source files instead of treating the
   retrieved text as unattributed context.

The link graph is useful for questions whose answer spans a concept, a guide,
and a decision record. It is not a substitute for relevance ranking, so start
with search and expand deliberately.

## Maintain quality

Use frontmatter as a lightweight publishing workflow:

~~~bash
mdvdb collection wiki \
  --recursive \
  --filter status=published \
  --sort product \
  --json
~~~

Find broken links and isolated pages:

~~~bash
mdvdb doctor --json
mdvdb orphans --json
~~~

Run **mdvdb watch** during active editing, or ingest the changed file in a
content pipeline:

~~~bash
mdvdb ingest --file wiki/guides/retries.md
~~~

Because Markdown remains the source of truth, the same pull-request review
that checks prose can check ownership, status, prerequisites, and links.

## Related pages

- [Search](../commands/search.md)
- [Chunking](../concepts/chunking.md)
- [Search modes](../concepts/search-modes.md)
- [The link graph](../concepts/link-graph.md)
- [Relations](../concepts/relations.md)
- [Frontmatter as structured data](../concepts/frontmatter-data.md)
- [JSON output](../json-output.md)

---
title: "Feature Ideas & Brainstorming"
tags: [ideas, roadmap, brainstorming]
category: notes
author: "Alex Park"
status: draft
---

# Feature Ideas & Brainstorming

Running list of ideas to evaluate for future quarters.

## Real-Time Collaboration

Allow multiple users to edit the same document simultaneously. Would need:

- Operational Transform (OT) or CRDT for conflict resolution
- WebSocket connections for live updates
- Presence indicators (who's viewing/editing)
- Cursor position sharing

Complexity: high. Probably Q4 at the earliest. Look at how Figma handles this — they wrote a custom CRDT.

## Markdown Extensions

Support custom markdown blocks for richer content:

```markdown
:::warning
This endpoint is deprecated and will be removed in v3.
:::

:::code-playground lang=python
print("Hello, world!")
:::
```

Would need a custom markdown parser plugin. `pulldown-cmark` supports extensions, or we could preprocess the markdown before parsing.

## Webhook System

Let users register webhooks for events:

- Document created/updated/deleted
- Comment added
- User invited to workspace

Standard webhook pattern: POST to user's URL with event payload, retry with backoff on failure, circuit breaker after too many failures.

Should integrate with the existing event bus — webhooks become another consumer of the same events.

## AI-Powered Features

### Smart Summarization

Auto-generate a summary for long documents. Show it at the top as a collapsible section. Use an LLM API (OpenAI or self-hosted) to generate summaries on document save.

### Semantic Search

Replace keyword search with vector similarity search. Embed documents into a vector space and find similar documents by meaning rather than exact word matches. Would dramatically improve search quality for natural language queries.

### Auto-Tagging

Automatically suggest tags based on document content. Train a classifier on existing tagged documents to predict tags for new ones. Or use an LLM to extract key topics.

## Offline Mode

Progressive web app that works without internet:

- Service worker caches the app shell
- IndexedDB stores document data locally
- Sync queue for changes made offline
- Conflict resolution when reconnecting

This is a big investment but critical for users in areas with unreliable internet.

## Plugin System

Let third parties extend the platform:

- Define extension points (document renderers, sidebar widgets, slash commands)
- Sandboxed execution (iframe or Web Worker)
- Plugin marketplace
- API for plugins to read/write documents, listen to events

Look at VSCode's extension model for inspiration.

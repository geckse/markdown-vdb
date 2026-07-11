---
title: "Writing Good Notes"
tags: [notes, markdown, documentation, best-practices]
category: guides
author: "Marcel Claus-Ahrens"
status: published
---
# Writing Good Notes

Conventions for notes that are easy to read, link, and retrieve later.

## Frontmatter

Every note starts with YAML frontmatter. Consistent fields make metadata filters and search reliable:

```yaml
---
title: "Descriptive Title"
tags: [topic, subtopic]
category: notes
author: "Your Name"
status: draft   # draft | published | archived
---
```

- Use a real `title` — it's what search and link previews show.
- Keep `tags` lowercase and reuse existing ones instead of inventing near-duplicates.
- Set `status` honestly so stale drafts don't read as finished docs.

## Structure

- Open with a one-line summary of what the note is about.
- Use headings to break up content — the index chunks on headings, so they double as search anchors.
- One idea per note. Split a sprawling note rather than letting it grow unbounded.
- Put the most important information first; readers skim.

## Writing Style

- Short sentences. Short paragraphs. Whitespace is free.
- Prefer lists and tables over dense prose for anything scannable.
- Use fenced code blocks with a language tag for commands and snippets.
- Name things precisely — a good heading beats a paragraph of context.

## Linking

- Link related notes with `[[wikilinks]]` or standard Markdown links.
- Link generously — connected notes surface together and reduce orphans.
- When you reference a concept documented elsewhere, link it instead of re-explaining.

## Maintenance

- [ ] Title and summary reflect the current content
- [ ] `status` is accurate (not still `draft` after it shipped)
- [ ] Links point somewhere real
- [ ] Superseded content is archived or deleted, not left to rot
- [ ] Re-ingest after substantial edits so search stays current

## Anti-Patterns

- **Wall of text** — no headings, no lists, impossible to skim.
- **Mystery titles** — "Notes 3" tells no one anything.
- **Orphan notes** — nothing links in or out; they get lost.
- **Zombie drafts** — marked `draft` forever, trusted by no one.

&nbsp;

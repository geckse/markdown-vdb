# Quick Notes

No frontmatter here — just raw markdown. The system should handle this gracefully.

## TODO

- Fix the flaky test in the auth service
- Review Priya's PR on rate limiting
- Update the Helm chart for the new Redis version

## Random Thoughts

The current search implementation is too slow for large workspaces. We should look into vector similarity search instead of keyword matching. Something that understands meaning, not just exact words.

Maybe we could embed all our docs and use cosine similarity? There are some good open-source embedding models now that run locally.

## Links to Check

- HNSW algorithm paper for approximate nearest neighbor search
- Obsidian as a markdown knowledge base tool
- How Notion handles real-time collaboration

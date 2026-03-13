---
title: Debug Session Log
created: 2026-03-13
---

# Debug Log — Search Scoring Bug

Temporary notes from debugging session, don't index.

## Steps Reproduced

1. Query "embedding provider" with hybrid mode
2. Score for ollama.md came back as 0.0
3. Turns out BM25 normalization was dividing by zero when doc had no body text

## Fix

Added guard in `fts.rs` line 142 — check `total_tokens > 0` before normalizing.

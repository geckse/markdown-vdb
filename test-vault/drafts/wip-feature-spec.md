---
title: WIP Feature Spec
status: draft
author: gecko
created: 2026-03-10
---

# Feature Spec: Real-Time Collaboration

This is a rough draft — not ready for search indexing yet.

## Problem Statement

Multiple users editing the same vault simultaneously leads to conflicts.

## Proposed Solution

- CRDT-based conflict resolution
- WebSocket sync layer
- Presence indicators

## Open Questions

- How to handle offline edits?
- What's the latency budget?
- Do we need operational transforms or is CRDT sufficient?

## Scratch Notes

Just brainstorming here, ignore this...

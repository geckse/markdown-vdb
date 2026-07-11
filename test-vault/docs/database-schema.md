---
title: "Database Schema"
tags: [backend, data, postgres]
category: documentation
author: "Jane Chen"
status: draft
---
# Database Schema

The relational model behind the [Data Layer](architecture.md#data-layer): core tables, relationships, and the rules that keep them consistent.

## Core Tables

- **documents** — one row per document, with content hash, timestamps, and owner.
- **chunks** — child rows of `documents`, one per embedded section, cascading on delete.
- **users** — identities and their granted scopes (see [Authentication & Authorization](authentication.md#scopes)).
- **audit_log** — append-only record of every mutating operation, keyed by trace ID.

## Invariants

The schema enforces what the application must never violate:

- A `chunk` cannot exist without its parent `document` (foreign key, `ON DELETE CASCADE`).
- `content_hash` is unique per document version, so unchanged content is never re-embedded.
- `audit_log` rows are immutable — no updates or deletes, only inserts.

## Migrations

Schema changes ship as ordered, forward-only migrations applied during [deployment](deployment.md#database-migrations). Every migration must be backward-compatible with the currently running code so a [rolling update](deployment.md#rolling-updates) never breaks mid-deploy.

## Operations

Routine care — vacuuming, index maintenance, backups — lives in the [Database Maintenance Runbook](../runbooks/database-maintenance.md). Caches derived from these tables are invalidated per the [Caching Strategy](caching.md).

## See Also

- [System Architecture](architecture.md#data-layer) — how the database fits the whole
- [Database Maintenance Runbook](../runbooks/database-maintenance.md) — operational upkeep
- [Deployment Guide](deployment.md#database-migrations) — how schema changes roll out
- [API Versioning](versioning.md) — coordinating schema and API evolution

&nbsp;

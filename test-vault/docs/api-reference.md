---
title: "API Reference"
tags: [api, v2, rest]
category: documentation
author: "Jane Chen"
status: published
version: "2.4.0"
---

# API Reference

Complete reference for the platform REST API (v2).

## Authentication

All API requests require a Bearer token. Tokens are issued via the `/auth/token` endpoint using client credentials.

```
POST /auth/token
Content-Type: application/json

{
  "client_id": "your-client-id",
  "client_secret": "your-client-secret"
}
```

Response:

```json
{
  "access_token": "eyJhbGciOiJSUzI1NiIs...",
  "token_type": "bearer",
  "expires_in": 900,
  "refresh_token": "dGhpcyBpcyBhIHJlZnJl..."
}
```

Include the token in all subsequent requests:

```
Authorization: Bearer eyJhbGciOiJSUzI1NiIs...
```

## Documents

### List Documents

```
GET /api/v2/documents
```

Query parameters:

| Param | Type | Default | Description |
|---|---|---|---|
| `page` | integer | 1 | Page number |
| `per_page` | integer | 20 | Items per page (max 100) |
| `sort` | string | `created_at` | Sort field |
| `order` | string | `desc` | Sort order (`asc` or `desc`) |
| `tag` | string | — | Filter by tag |

### Get Document

```
GET /api/v2/documents/:id
```

Returns the full document with metadata and content.

### Create Document

```
POST /api/v2/documents
Content-Type: application/json

{
  "title": "New Document",
  "content": "# Hello\n\nThis is the document body.",
  "tags": ["draft"],
  "metadata": {
    "category": "notes",
    "priority": "high"
  }
}
```

### Update Document

```
PATCH /api/v2/documents/:id
Content-Type: application/json

{
  "title": "Updated Title",
  "tags": ["published", "v2"]
}
```

Only include fields you want to change. Omitted fields are not modified.

### Delete Document

```
DELETE /api/v2/documents/:id
```

Returns `204 No Content` on success. Deletion is soft — documents can be recovered within 30 days.

## Search

### Full-Text Search

```
GET /api/v2/search?q=authentication+flow&limit=10
```

Returns ranked results with relevance scores and highlighted excerpts.

### Semantic Search

```
POST /api/v2/search/semantic
Content-Type: application/json

{
  "query": "how does the login process work",
  "limit": 5,
  "filters": {
    "tags": ["api"],
    "category": "documentation"
  }
}
```

## Rate Limits

| Tier | Requests/min | Burst |
|---|---|---|
| Free | 60 | 10 |
| Pro | 600 | 50 |
| Enterprise | 6000 | 200 |

Rate limit headers are included in every response:

```
X-RateLimit-Limit: 600
X-RateLimit-Remaining: 594
X-RateLimit-Reset: 1709312400
```

## Error Handling

All errors follow a consistent format:

```json
{
  "error": {
    "code": "DOCUMENT_NOT_FOUND",
    "message": "Document with id 'abc123' not found",
    "status": 404
  }
}
```

Common error codes:

| Code | Status | Meaning |
|---|---|---|
| `UNAUTHORIZED` | 401 | Missing or invalid token |
| `FORBIDDEN` | 403 | Valid token but insufficient permissions |
| `NOT_FOUND` | 404 | Resource does not exist |
| `RATE_LIMITED` | 429 | Too many requests |
| `INTERNAL_ERROR` | 500 | Server error — retry with backoff |

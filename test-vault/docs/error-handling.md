---
title: "Error Handling Patterns"
tags: [api, backend, patterns]
category: documentation
author: "Jane Chen"
status: draft
---

# Error Handling Patterns

Conventions for how services handle, propagate, and report errors.

## Error Response Format

All services return errors in a consistent JSON envelope:

```json
{
  "error": {
    "code": "VALIDATION_FAILED",
    "message": "Field 'email' is required",
    "status": 422,
    "details": [
      { "field": "email", "reason": "required" }
    ]
  }
}
```

The `details` array is optional and used for validation errors where multiple fields may fail.

## Error Categories

### Client Errors (4xx)

These indicate the caller made a mistake. Do not retry without changing the request.

- **400 Bad Request** — malformed JSON, invalid content type
- **401 Unauthorized** — missing or expired token
- **403 Forbidden** — valid token but insufficient scope
- **404 Not Found** — resource doesn't exist
- **409 Conflict** — duplicate resource or version conflict
- **422 Unprocessable Entity** — valid JSON but failed validation
- **429 Too Many Requests** — rate limited, check `Retry-After` header

### Server Errors (5xx)

These indicate the service failed. Callers should retry with exponential backoff.

- **500 Internal Server Error** — unexpected failure, logged with trace ID
- **502 Bad Gateway** — upstream service unreachable
- **503 Service Unavailable** — overloaded or in maintenance mode
- **504 Gateway Timeout** — upstream service didn't respond in time

## Retry Strategy

For transient errors (5xx, network failures), use exponential backoff:

```
attempt 1: wait 100ms
attempt 2: wait 200ms
attempt 3: wait 400ms
attempt 4: wait 800ms
attempt 5: give up
```

Add jitter (random 0-50ms) to each wait to prevent thundering herd.

## Error Propagation Between Services

When service A calls service B and B returns an error:

1. **Log the full error** from B with the trace ID
2. **Map the error** to an appropriate error for A's caller — don't leak B's internal details
3. **Preserve the trace ID** so the error can be correlated across services

Example: if the document service calls the auth service and gets a 500, the document service returns a 502 to its caller (upstream failure), not a 500 (which would imply the document service itself failed).

## Trace IDs

Every request gets a unique trace ID (`X-Trace-Id` header). If not provided by the caller, the API gateway generates one. The trace ID is:

- Included in all log entries for the request
- Passed to downstream service calls
- Included in error responses
- Used to correlate logs across services in the log aggregator

## Circuit Breaker

Services use a circuit breaker for outbound calls to other services:

- **Closed** (normal): requests flow through
- **Open** (tripped): requests fail immediately with 503, no outbound call made
- **Half-open** (testing): one request allowed through to test if the downstream is back

Trip threshold: 5 failures in 30 seconds. Recovery probe: every 10 seconds.

---
title: "About Errors"
tags: [api, backend, concepts]
category: documentation
author: "Marcel Claus-Ahrens"
status: draft
---
# About Errors

A shared mental model for how we think about errors before we get into the mechanics. For the concrete conventions — response envelope, status codes, retries, and circuit breakers — see [Error Handling Patterns](error-handling.md).

## Errors Are Normal

An error is not an exception to the system; it's part of it. Networks drop, inputs are malformed, upstreams fall over, and callers ask for things that don't exist. A healthy service treats these as expected outcomes with defined behavior, not as surprises to be patched over later.

The goal is never to eliminate errors — it's to make them **legible**: easy to detect, easy to categorize, and easy to act on.

## Whose Fault Is It?

Every error answers one question first: who needs to change something?

- **Caller errors** mean the request was wrong. The fix is on the caller's side, so retrying the same request unchanged is pointless.
- **Service errors** mean the request was reasonable but the system failed to fulfill it. The fix is on our side, and retrying later may well succeed.

This split — the 4xx/5xx boundary — is the single most useful distinction in error handling, because it tells both humans and machines what to do next. The full breakdown of categories lives in [Error Handling Patterns](error-handling.md#error-categories).

## Three Things Every Error Should Do

1. **Explain itself.** A message a human can read and a code a machine can branch on. Never make a caller reverse-engineer intent from a stack trace.
2. **Stay traceable.** Carry a trace ID so a single failure can be followed across every service and log it touched.
3. **Fail honestly.** Report the actual state — don't swallow an error, don't dress a failure up as success, and don't leak an internal detail as if it were the caller's problem.

## Recoverable vs. Terminal

Before writing any handling logic, ask whether the error is recoverable:

- **Transient** — a timeout, a rate limit, a briefly-unavailable upstream. Back off and retry.
- **Permanent** — a validation failure, a missing resource, a bad token. Retrying only wastes both sides' time; surface it immediately. 

Guessing wrong in either direction is costly: retrying a permanent error hammers a dead endpoint, while giving up on a transient one turns a blip into an outage. The retry and circuit-breaker rules that encode this judgment are specified in [Error Handling Patterns](error-handling.md#retry-strategy).

## See Also

- [Error Handling Patterns](error-handling.md) — the concrete conventions this document sets up: response format, status codes, retries, propagation, and circuit breakers
- [API Reference](api-reference.md#error-handling) — the client-facing error contract
- [Incident Response Runbook](../runbooks/incident-response.md) — what to do when errors become an incident

&nbsp;
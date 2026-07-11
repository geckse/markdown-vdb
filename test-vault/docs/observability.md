---
title: "Observability & Logging"
tags: [backend, operations, monitoring]
category: documentation
author: "Marcus Riveras"
status: published
---
# Observability & Logging

The three signals — logs, metrics, and traces — that let us understand a running system. All three hang off the same [trace IDs](error-handling.md#trace-ids) that errors already carry.

## The Three Signals

- **Logs** — structured JSON events, one per meaningful action, always tagged with the trace ID.
- **Metrics** — numeric time series (request rate, latency percentiles, error ratios) scraped and stored for dashboards and alerts.
- **Traces** — the causal chain of a single request as it fans out across services.

## Correlation

The trace ID is the thread that ties the three together. Given one failing request you can pivot from its error response, to its logs, to the full distributed trace, without guessing. This is why [error propagation](error-handling.md#error-propagation-between-services) preserves the trace ID across service hops.

## Dashboards and Alerts

Dashboards are wired into the same metrics described in the [deployment monitoring](deployment.md#monitoring) section. Alerts fire on symptoms users feel — elevated error rate, latency regressions, saturation — not on internal causes.

## From Signal to Incident

When an alert crosses the line from noise to incident, the [Incident Response Runbook](../runbooks/incident-response.md) takes over. Good observability is what makes that handoff fast: the on-call engineer starts from a trace ID, not a blank page.

## See Also

- [Error Handling Patterns](error-handling.md#trace-ids) — the trace ID contract everything builds on
- [Deployment Guide](deployment.md#monitoring) — where dashboards and scrape targets are configured
- [Incident Response Runbook](../runbooks/incident-response.md) — acting on what observability surfaces
- [Performance & Scaling](performance.md) — the metrics that drive scaling decisions

&nbsp;

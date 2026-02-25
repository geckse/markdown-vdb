---
title: "Incident Response Runbook"
tags: [runbook, oncall, incidents, operations]
category: runbooks
author: "Marcus Rivera"
severity: critical
---

# Incident Response Runbook

Step-by-step procedures for handling production incidents.

## Severity Levels

| Level | Description | Response Time | Example |
|---|---|---|---|
| SEV1 | Complete outage, all users affected | 15 min | Database down, API unreachable |
| SEV2 | Major feature broken, many users affected | 30 min | Search not returning results, auth failing |
| SEV3 | Minor feature broken, workaround exists | 2 hours | Export button broken, notification delay |
| SEV4 | Cosmetic issue, no user impact | Next business day | Typo in UI, log formatting issue |

## Triage Checklist

When you get paged:

1. **Acknowledge** the alert within 5 minutes
2. **Check the dashboard** — is this a real issue or a flaky alert?
3. **Assess severity** — how many users are affected? Is there a workaround?
4. **Communicate** — post in `#incidents` with: what's happening, severity level, who's investigating
5. **Investigate** — follow the relevant service runbook below

## Service-Specific Procedures

### Auth Service Down

Symptoms: 401 errors across all services, login failing

1. Check pod status: `kubectl get pods -l app=auth -n production`
2. Check logs: `kubectl logs -f deployment/auth-service -n production`
3. Check database connectivity: `kubectl exec -it auth-pod -- pg_isready -h $DATABASE_HOST`
4. If pods are crash-looping: check recent deployments (`helm history platform`)
5. If database is down: escalate to DBA, switch to read-only mode if possible
6. Rollback if recent deploy: `helm rollback platform <last-good-revision>`

### Search Service Degraded

Symptoms: slow search results (>2s), timeout errors, empty results

1. Check Elasticsearch cluster health: `curl http://es-host:9200/_cluster/health`
2. Check for GC pauses in ES logs
3. If yellow/red cluster: check disk space and shard allocation
4. If GC pauses: increase heap size (requires rolling restart)
5. If index corrupted: trigger reindex from primary data store

### Event Bus Backlog

Symptoms: delayed notifications, stale search results, analytics lag

1. Check consumer lag: `kafka-consumer-groups --describe --group <service-group>`
2. Check if consumers are running: `kubectl get pods -l component=consumer`
3. If consumers crashed: restart them, check for poison messages
4. If throughput issue: scale up consumer replicas
5. If broker issue: check broker logs, disk space, replication status

## Communication Template

Post this in `#incidents` when declaring an incident:

```
🔴 INCIDENT — SEV[1/2/3]

What: [brief description]
Impact: [who/what is affected]
Status: [investigating/identified/mitigating/resolved]
Lead: [your name]
Timeline:
- HH:MM — [first observation]
- HH:MM — [action taken]
```

Update the thread every 15 minutes for SEV1, every 30 minutes for SEV2.

## Post-Incident

Within 48 hours of resolution:

1. Write a post-mortem (template in `docs/templates/post-mortem.md`)
2. Identify root cause and contributing factors
3. List action items with owners and deadlines
4. Share in `#engineering` for team review
5. Update monitoring/alerting to catch this earlier next time

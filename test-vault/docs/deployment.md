---
title: "Deployment Guide"
tags: [devops, kubernetes, deployment]
category: documentation
author: "Marcus Rivera"
status: published
---

# Deployment Guide

How to deploy, configure, and operate the platform in production.

## Prerequisites

- Kubernetes cluster (1.28+)
- Helm 3.x
- Container registry access
- DNS configured for your domain

## Environment Configuration

All services read configuration from environment variables. Secrets are mounted from Kubernetes secrets, never baked into images.

Required environment variables per service:

| Variable | Service | Description |
|---|---|---|
| `DATABASE_URL` | auth, documents | PostgreSQL connection string |
| `REDIS_URL` | notifications | Redis connection string |
| `S3_BUCKET` | documents | Document storage bucket |
| `JWT_PUBLIC_KEY` | all | RSA public key for token verification |
| `EVENT_BUS_URL` | all | Message broker connection string |

## Deployment Steps

### 1. Build and Push Images

```bash
docker build -t registry.example.com/auth-service:v2.4.0 ./services/auth
docker push registry.example.com/auth-service:v2.4.0
```

Repeat for each service. CI/CD typically handles this on merge to main.

### 2. Update Helm Values

Edit `deploy/values-production.yaml`:

```yaml
auth:
  image:
    tag: v2.4.0
  replicas: 3
  resources:
    requests:
      cpu: 250m
      memory: 512Mi
    limits:
      cpu: 1000m
      memory: 1Gi
```

### 3. Apply with Helm

```bash
helm upgrade --install platform ./deploy/chart \
  -f deploy/values-production.yaml \
  --namespace production
```

### 4. Verify

```bash
kubectl get pods -n production
kubectl logs -f deployment/auth-service -n production
```

## Rolling Updates

The default strategy is rolling update with:

- `maxSurge: 1` — at most 1 extra pod during update
- `maxUnavailable: 0` — no downtime

Rollback if something goes wrong:

```bash
helm rollback platform 1 --namespace production
```

## Scaling

Horizontal pod autoscaling is configured per service:

```yaml
autoscaling:
  enabled: true
  minReplicas: 2
  maxReplicas: 10
  targetCPUUtilization: 70
  targetRequestLatency: 200ms
```

The search service typically needs more replicas than other services due to heavy query loads.

## Monitoring

Each service exposes `/metrics` in Prometheus format. Key metrics to alert on:

- Request latency p99 > 500ms
- Error rate > 1%
- Pod restart count > 3 in 5 minutes
- Event bus consumer lag > 10,000 messages

## Database Migrations

Migrations run as Kubernetes jobs before deployment:

```bash
kubectl apply -f deploy/migrations/auth-migrate-job.yaml
```

Never run migrations during peak traffic. Always take a database snapshot before migrating.

## Disaster Recovery

- Database: automated daily snapshots, retained for 30 days
- S3: versioning enabled, cross-region replication
- Event bus: messages retained for 7 days
- Full recovery procedure documented in the runbook (see `runbooks/disaster-recovery.md`)

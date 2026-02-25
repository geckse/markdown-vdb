---
title: "Database Maintenance Runbook"
tags: [runbook, database, postgres, maintenance]
category: runbooks
author: "Marcus Rivera"
---

# Database Maintenance Runbook

Routine and emergency procedures for PostgreSQL databases.

## Routine Maintenance

### Weekly: Analyze and Vacuum

Run during low-traffic hours (Sunday 3am UTC):

```sql
VACUUM ANALYZE;
```

This reclaims dead row space and updates query planner statistics. Autovacuum handles most cases, but a manual pass catches tables with unusual write patterns.

### Monthly: Index Health Check

Check for bloated indexes:

```sql
SELECT
  schemaname,
  tablename,
  indexname,
  pg_size_pretty(pg_relation_size(indexrelid)) as index_size
FROM pg_stat_user_indexes
ORDER BY pg_relation_size(indexrelid) DESC
LIMIT 20;
```

If an index is significantly larger than expected, reindex it:

```sql
REINDEX INDEX CONCURRENTLY idx_documents_created_at;
```

Use `CONCURRENTLY` to avoid locking the table.

### Quarterly: Connection Pool Review

Check connection usage:

```sql
SELECT
  datname,
  numbackends,
  xact_commit,
  xact_rollback,
  blks_read,
  blks_hit,
  tup_returned,
  tup_fetched
FROM pg_stat_database
WHERE datname = 'platform';
```

If `numbackends` is consistently near the pool max, increase the pool size or investigate connection leaks.

## Backup and Recovery

### Automated Backups

- Daily full backup at 2am UTC via `pg_dump`
- Continuous WAL archiving to S3
- Retention: 30 daily backups, 12 monthly backups

### Point-in-Time Recovery

If you need to restore to a specific moment:

1. Stop the affected service
2. Create a new database instance from the latest base backup
3. Replay WAL files up to the target timestamp:

```bash
recovery_target_time = '2026-02-20 14:30:00 UTC'
```

4. Verify data integrity
5. Swap the connection string to point to the recovered instance
6. Restart the service

### Table-Level Recovery

If only specific tables are affected:

```bash
pg_restore -d platform -t documents backup_20260220.dump
```

## Emergency Procedures

### Database Out of Disk Space

1. Check disk usage: `SELECT pg_size_pretty(pg_database_size('platform'));`
2. Find the largest tables:

```sql
SELECT
  relname,
  pg_size_pretty(pg_total_relation_size(relid))
FROM pg_catalog.pg_statio_user_tables
ORDER BY pg_total_relation_size(relid) DESC
LIMIT 10;
```

3. If a table has excessive dead rows: `VACUUM FULL <table>` (locks the table!)
4. If logs are filling disk: rotate and compress logs immediately
5. Long term: add disk space or archive old data

### Slow Queries Blocking Others

Find blocking queries:

```sql
SELECT
  blocked.pid AS blocked_pid,
  blocked.query AS blocked_query,
  blocking.pid AS blocking_pid,
  blocking.query AS blocking_query,
  blocking.state
FROM pg_stat_activity blocked
JOIN pg_locks bl ON bl.pid = blocked.pid
JOIN pg_locks bll ON bll.locktype = bl.locktype
  AND bll.relation = bl.relation
  AND bll.pid != bl.pid
JOIN pg_stat_activity blocking ON blocking.pid = bll.pid
WHERE NOT bl.granted;
```

If a long-running query is blocking production traffic, terminate it:

```sql
SELECT pg_terminate_backend(<blocking_pid>);
```

### Replication Lag

Check replication status:

```sql
SELECT
  client_addr,
  state,
  sent_lsn,
  write_lsn,
  flush_lsn,
  replay_lsn,
  pg_wal_lsn_diff(sent_lsn, replay_lsn) AS byte_lag
FROM pg_stat_replication;
```

If lag is growing:
1. Check replica's disk I/O and CPU
2. Check network between primary and replica
3. If replica is too far behind, consider rebuilding from a fresh backup

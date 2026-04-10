# ADR-0010: Embargo Store Backend Selection

## Status

Proposed — open decision, to be resolved in Application Design

## Context

The embargo feature (FR-11) requires durable storage for embargo records that survives
process restarts and is consistent across all instances of Rendition running in a
Kubernetes deployment. Three candidates were evaluated:

| Store | Latency | Durability | Operational overhead | TTL native |
|---|---|---|---|---|
| **Redis** | < 1 ms | Configurable (AOF/RDB) | Moderate (HA needs Sentinel or Cluster) | Yes |
| **Amazon DynamoDB** | 1–5 ms | Fully managed, multi-AZ | Low (serverless) | Yes (TTL attribute) |
| **PostgreSQL** | 1–5 ms | ACID, WAL | Moderate (needs RDS or self-managed) | No (via cron job) |

The embargo check fires on every CDN request (hot path). Embargo records are few
(hundreds to thousands), written infrequently, but read on every request.

Constraints:
- The embargo check must complete in < 5 ms P99 (to meet the ≤ 10 ms cache-hit
  latency target when the in-process embargo cache misses).
- The store must be reachable from within the Kubernetes cluster.
- Adding a new infrastructure dependency should be justified by clear need.

## Decision

**Not yet made.** The following options remain under consideration:

**Option A — Redis (recommended for AWS deployments):**
Use Amazon ElastiCache (Redis OSS) in cluster mode. Embargo records stored as JSON
strings with `EXPIREAT` TTL. Sub-millisecond reads. Native TTL eliminates background
cleanup jobs. Requires ElastiCache cluster provisioning.

**Option B — DynamoDB:**
Embargo records as DynamoDB items with TTL attribute. Fully serverless; scales to
zero cost when idle. 1–5 ms read latency. AWS SDK dependency. Strongly consistent
reads available. Good fit if the rest of the system is already AWS-native.

**Option C — PostgreSQL:**
Embargo records as rows in an `embargoes` table. Full ACID guarantees. Enables rich
queries (e.g. "list all embargoes expiring this week"). Requires RDS or similar.
TTL-based cleanup via a background task or pg_cron. Higher per-query latency than
Redis/DynamoDB (5–10 ms) — in-process cache absorbs most load.

## Decision Criteria

The Application Design stage will evaluate:

1. **Existing infrastructure**: Does the deployment environment already include Redis
   or DynamoDB? Reuse existing infrastructure to avoid additional operational surface.
2. **Latency budget**: All three meet the < 5 ms P99 target when the in-process cache
   is warm. Only matters for cold cache hits.
3. **Feature requirements**: If future versions require complex queries over embargo
   records (reporting, bulk operations), PostgreSQL's SQL expressiveness has value.
4. **Cloud portability (QA-06)**: DynamoDB couples the store to AWS. Redis and
   PostgreSQL are portable. If portability is a priority, Redis or PostgreSQL is
   preferred.

## Consequences

The embargo store choice propagates to:
- `src/embargo/store.rs` — the `EmbargoStore` trait implementation selected.
- CI/CD — the chosen store must be available in the test environment (Docker Compose).
- Kubernetes manifests — a store deployment or external service reference is required.
- ADR-0010 will be updated to "Accepted" once the Application Design stage resolves
  this decision.

# ADR-0010: Embargo Store Backend Selection

## Status

Accepted — resolved in Application Design (2026-04-09)

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

**Redis (Amazon ElastiCache, Redis OSS).**

Embargo records and named presets (FR-18) are stored as JSON strings under namespaced
keys (`embargo:{path}`, `preset:{name}`) with native `EXPIREAT` TTL for automatic
expiry. The `fred` or `redis` Rust crate is used behind the `EmbargoStore` trait.

Key implementation details:

- ElastiCache cluster mode with at least one replica for HA.
- The `EmbargoStore` trait (`get`, `put`, `delete`, `list_active`) remains
  backend-agnostic. `RedisEmbargoStore` is the v1 implementation.
- A `docker-compose.yml` Redis service is provided for local development and CI.
- The in-process read-through cache (FR-11, 30 s TTL) absorbs CDN-path hot reads;
  Redis is only hit on cache cold misses and all admin writes.

DynamoDB was rejected because it couples the embargo store to AWS, reducing
cloud portability (QA-06). PostgreSQL was rejected because its query power is
not needed in v1 and TTL management adds operational complexity.

## Consequences

- `src/embargo/store.rs` — `RedisEmbargoStore` implements `EmbargoStore` trait.
- `Cargo.toml` — `fred` or `redis` crate added as a dependency.
- `docker-compose.yml` — Redis service added for local dev and CI integration tests.
- Kubernetes manifests — ElastiCache endpoint referenced via `RENDITION_REDIS_URL`
  environment variable.
- Operators not on AWS may substitute any Redis-compatible store (Upstash,
  Valkey, Redis Cloud) by setting `RENDITION_REDIS_URL`.

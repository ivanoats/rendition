# ADR-0015: `tower-governor` for Per-IP Rate Limiting

## Status

Accepted

## Context

FR-05 requires per-IP rate limiting on `/cdn/*` endpoints with configurable
requests-per-second and burst capacity. Clients exceeding the limit must receive
`HTTP 429 Too Many Requests` with a `Retry-After` header.

Tower's built-in `tower::limit::RateLimitLayer` is a global concurrency limiter —
it does not track individual client IPs, making it unsuitable for per-client
enforcement.

Two per-IP options were evaluated:

| Criterion | `tower-governor` | Custom `DashMap` + `governor` |
|---|---|---|
| Lines of implementation code | ~10 (middleware config) | ~80–120 (custom layer) |
| IP extraction | Built-in `PeerIpKeyExtractor` | Manual from `ConnectInfo` |
| X-Forwarded-For support | Via custom `KeyExtractor` trait | Manual parsing |
| Algorithm | GCRA (`governor` crate) | GCRA (`governor` crate) |
| Extra dependencies | `tower-governor`, `governor` | `governor`, `dashmap` |

## Decision

Use **`tower-governor`** as a Tower middleware layer on the CDN router.

The GCRA (Generic Cell Rate Algorithm) algorithm used by `governor` provides
smooth per-client rate limiting without the thundering-herd spikes of token bucket
leaky-bucket implementations. Rate parameters are configurable via
`RENDITION_RATE_LIMIT_RPS` (default: `100`) and `RENDITION_RATE_LIMIT_BURST`
(default: `200`).

For deployments behind a CDN or reverse proxy, a custom `KeyExtractor` is
implemented that reads the real client IP from `X-Forwarded-For` or
`X-Real-IP` headers rather than the TCP peer address. This extractor is
configured via `RENDITION_RATE_LIMIT_KEY` (`peer_ip` | `x_forwarded_for`;
default: `peer_ip`).

Rate limiting is applied only to `/cdn/*` routes, not to `/health/*` or
`/metrics` (which are internal and scraped by Prometheus).

## Consequences

**Benefits:**

- GCRA algorithm prevents bursty traffic from exhausting per-client quotas
  immediately, providing a fairer rate-limiting experience than a simple
  fixed-window counter.
- `429` response and `Retry-After` header are handled by `tower-governor`
  automatically, removing per-handler boilerplate.
- The `KeyExtractor` trait allows the IP extraction strategy to be changed
  without replacing the rate-limiting logic.

**Drawbacks:**

- `tower-governor` is a smaller community crate compared to `tower` itself.
  If it becomes unmaintained, the custom `DashMap` + `governor` approach is
  a straightforward in-house replacement using the same `governor` algorithm.
- In-process per-IP state is not shared across pods. In a multi-pod deployment,
  a single client can exceed the per-pod limit multiplied by the replica count.
  This is acceptable for v1 where the CDN edge (CloudFront/Fastly) provides
  the first layer of DDoS protection. A Redis-backed shared rate limiter is a
  future option.

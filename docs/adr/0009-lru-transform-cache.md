# ADR-0009: In-Process LRU Cache for Transformed Images

## Status

Accepted

## Context

Image transformations via libvips are CPU-intensive and add 50–500 ms of latency
depending on source size and operation. For a retail CDN, many requests are
repetitive: the same product image at the same dimensions is requested thousands of
times per minute during peak traffic (product listing pages, recommendation widgets).

Options considered for caching transformed output:

- **No cache** — every request pays full transform cost. Unacceptable at
  lululemon-scale traffic (OC-01 throughput targets require >200 req/s per instance).
- **In-process LRU cache** — bounded in-memory cache keyed on `(asset_path, params)`.
  Fast (sub-millisecond lookup), no network hop, no additional infrastructure.
  Lost on restart; not shared across instances.
- **External cache (Redis)** — shared across all instances, survives restarts,
  supports TTL natively. Adds a network hop (1–5 ms) and a new infrastructure
  dependency.
- **CDN edge caching** — transformed responses can be cached by the upstream CDN
  (CloudFront, Fastly) using `Cache-Control: public, max-age=…`. This is complementary
  to, not a replacement for, an origin cache.

At the scale of a single-node or small-cluster deployment, an in-process LRU cache
eliminates the vast majority of transform work. Transformed images are byte-identical
for the same input, making the result deterministic and safe to cache.

## Decision

Implement an **in-process LRU cache** (`src/cache.rs`) using a thread-safe LRU
implementation (e.g. `moka` or `lru` + `Mutex<LruCache>`).

- Cache key: `SHA-256(asset_path || canonical_serialisation_of_TransformParams)`.
  Canonical serialisation ensures parameter order does not affect cache hits.
- Maximum entries: `RENDITION_CACHE_MAX_ENTRIES` (default: 1 000).
- TTL per entry: `RENDITION_CACHE_TTL_SECONDS` (default: 3 600 s).
- Embargoed assets MUST NOT be stored in the cache (FR-14).
- Cache miss metrics (`rendition_cache_misses_total`) and hit metrics
  (`rendition_cache_hits_total`) are emitted as Prometheus counters.
- CDN `Cache-Control` and `Surrogate-Key` headers are set independently of the
  in-process cache — both layers are complementary.
- External Redis caching is deferred to a future version once horizontal scale drives
  the need for cross-instance cache sharing. The cache interface (`src/cache.rs` trait)
  MUST be designed to allow a Redis-backed implementation without changing callers.

## Consequences

**Benefits:**

- Cache hits serve responses in < 1 ms with no I/O, eliminating libvips overhead for
  repeated requests.
- No additional infrastructure required in v1 — reduces operational complexity.
- Bounded memory: LRU eviction prevents unbounded growth; combined with per-entry TTL
  it limits stale data.
- Deterministic cache keys mean identical parameter sets always hit the same entry,
  regardless of URL parameter order.

**Drawbacks:**

- Cache is lost on process restart or pod eviction. The first wave of requests after a
  restart pays full transform cost until the cache warms up. Mitigation: CDN edge cache
  absorbs most traffic before origin is hit.
- Not shared across instances. In a multi-pod deployment, each pod has its own cache.
  This is acceptable when a CDN sits in front — the CDN acts as the shared layer.
- Memory growth is bounded but not zero: 1 000 entries × average 200 KB = ~200 MB
  peak. This must be factored into pod memory limits.
- The SHA-256 key computation adds ~1 μs per request on cache miss. Negligible.

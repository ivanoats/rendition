# Components

## Overview

Rendition is structured as a single Rust crate with logically separated modules.
Each module maps to a bounded context with a clear responsibility boundary.
Dependencies flow inward: API and admin layers depend on domain modules; domain
modules depend on traits (ports); infrastructure adapters implement those traits.

---

## C-01 — Config (`src/config.rs`)

**Responsibility:** Load, validate, and expose all `RENDITION_*` environment
variables as a typed `AppConfig` struct. This is the foundation every other
component depends on; it is initialised once at startup before any other
component.

**Key types:**

- `AppConfig` — the root typed configuration struct
- `StorageBackendKind` — enum: `Local` | `S3`
- `S3Config` — nested struct for all `RENDITION_S3_*` fields
- `OidcConfig` — nested struct for all `RENDITION_OIDC_*` fields

**Boundaries:**

- No dependencies on any other Rendition component
- Consumed by `src/lib.rs` (`build_app`) and `src/main.rs`
- `envy::prefixed("RENDITION_")` is the only external call (ADR-0014)

---

## C-02 — Storage (`src/storage/`)

**Responsibility:** Abstract asset retrieval behind the `StorageBackend` trait.
Provide two implementations: `LocalStorage` for development and `S3Storage` for
production. Encapsulate all AWS SDK usage inside `S3Storage` — no SDK types
cross the module boundary (ADR-0004, NFR-06).

**Key types:**

- `StorageBackend` — async trait (port): `get`, `exists`, `get_range`
- `Asset` — data transfer type: `data: Vec<u8>`, `content_type`, `size`
- `LocalStorage` — filesystem adapter
- `S3Storage` — AWS S3 adapter with `CircuitBreaker`
- `CircuitBreaker` — tracks S3 error rate; opens/closes automatically

**Boundaries:**

- `StorageBackend` is defined here; implementations live in sub-modules
- AWS SDK (`aws-sdk-s3`) is imported only in `src/storage/s3.rs`
- `CircuitBreaker` is internal to `S3Storage`; its open/closed state is exposed
  via `is_healthy() -> bool` for health checks

---

## C-03 — Transform (`src/transform/`)

**Responsibility:** Apply the image transform pipeline — crop, resize, sharpen,
watermark, rotate, flip, encode — using libvips. Resolve `fmt=auto` to a
concrete format via `Accept`-header negotiation. Run all libvips work on a
blocking thread pool via `spawn_blocking`.

**Key types:**

- `TransformParams` — deserialisable query parameter struct
- `ImageFormat` — enum: `Jpeg` | `Webp` | `Avif` | `Png` | `Auto`
- `FitMode` — enum: `Constrain` | `Crop` | `Stretch` | `Fill` | `Smart`

**Boundaries:**

- `libvips` bindings are contained entirely within this module
- Consumes `Asset` from C-02 (raw bytes only — no trait dependency)
- Returns `(Vec<u8>, &'static str)` — bytes and MIME type
- No knowledge of HTTP, storage, cache, or embargo

---

## C-04 — Transform Cache (`src/cache.rs`)

**Responsibility:** Cache transformed image responses in-process to avoid
repeated libvips invocations for identical requests. Bounded LRU with TTL.
Thread-safe for concurrent Axum handlers.

**Key types:**

- `TransformCache` — trait (port): `get`, `put`, `invalidate`
- `MokaTransformCache` — `moka::future::Cache`-backed implementation (ADR-0009)
- `CachedResponse` — `bytes: Bytes`, `content_type: &'static str`
- `CacheKey` — `[u8; 32]` (SHA-256 of path + canonical params + resolved format)

**Boundaries:**

- No dependency on storage, transform, or HTTP layers
- Cache key computation takes `path`, `TransformParams`, and resolved `ImageFormat`
  so `fmt=auto` variants are cached separately per format
- Embargoed responses must never be inserted (enforced by the caller — C-07)

---

## C-05 — Embargo (`src/embargo/`)

**Responsibility:** Enforce asset embargoes on the CDN request path and provide
the data model and persistence layer for embargo records.

Sub-components:

- `EmbargoEnforcer` — in-process read-through cache (HashMap, configurable TTL).
  Answers `check(path) -> Option<EmbargoRecord>` with < 1 µs latency on a warm
  cache, falling back to `EmbargoStore` on a miss.
- `EmbargoStore` — async trait (port): `get`, `put`, `delete`, `list_active`
- `RedisEmbargoStore` — Redis-backed implementation via `fred` crate (ADR-0010)
- `EmbargoRecord` — data type: `asset_path`, `embargo_until`, `created_by`,
  `created_at`, `note`

**Boundaries:**

- `EmbargoEnforcer` depends on `EmbargoStore` trait only — no Redis import
- Redis client code is contained in `src/embargo/redis_store.rs`
- CDN path calls `EmbargoEnforcer::check` (read-only, fast)
- Admin path calls `EmbargoStore` directly for mutations

---

## C-06 — Preset (`src/preset/`)

**Responsibility:** Store and resolve named transform presets. A preset is a
named alias for a `TransformParams` set (FR-18). Shares the Redis backend with
C-05 under a separate key namespace (`preset:{name}`).

**Key types:**

- `PresetStore` — async trait (port): `get`, `put`, `delete`, `list`
- `RedisPresetStore` — Redis-backed implementation (reuses the Redis connection
  pool from C-05)
- `NamedPreset` — `name: String`, `params: TransformParams`, `created_by`,
  `created_at`

**Boundaries:**

- `PresetStore` is resolved before `TransformParams` is constructed in
  `serve_asset` — if the request contains a `preset` parameter, the stored
  `TransformParams` is loaded and merged with any explicit overrides

---

## C-07 — CDN API (`src/api/`)

**Responsibility:** Handle all `/cdn/*` requests. Orchestrate the per-request
pipeline: preset resolution → embargo check → cache lookup → storage fetch →
format negotiation → transform → cache store → response headers → respond.

**Key types:**

- `AppState<S>` — shared state: `storage: Arc<S>`, `cache: Arc<dyn TransformCache>`,
  `embargo: Arc<EmbargoEnforcer>`, `presets: Arc<dyn PresetStore>`,
  `config: Arc<AppConfig>`
- `serve_asset<S>` — the single CDN handler function

**Boundaries:**

- Depends on C-02 (storage), C-03 (transform), C-04 (cache), C-05 (embargo),
  C-06 (preset), C-01 (config)
- Sets `Surrogate-Key`, `Cache-Control`, `Vary`, `Accept-Ranges`,
  `X-Request-Id` response headers
- Returns `451` for active embargoes; `404` for missing assets; `400` for
  invalid params; `206`/`200` for assets

---

## C-08 — Admin API (`src/admin/`)

**Responsibility:** Handle all `/admin/*` requests behind authentication.
Provide CRUD endpoints for embargoes and presets, and a cache purge endpoint.

Sub-components:

- `AuthLayer` — Tower middleware: validates OIDC JWT (via `JwksCache`) or
  API key hash. Injects `AdminIdentity` into request extensions on success.
- `JwksCache` — fetches and caches JWKS keys from the OIDC provider (ADR-0016)
- `embargo_handlers` — Axum handlers for `/admin/embargoes/*`
- `preset_handlers` — Axum handlers for `/admin/presets/*`
- `purge_handlers` — `POST /admin/purge` — invalidates transform cache entries
  by asset path pattern

**Boundaries:**

- Mounted on `127.0.0.1:3001` (ADR-0013); never reachable from the CDN port
- `AuthLayer` calls no domain logic — it only validates identity
- `embargo_handlers` and `preset_handlers` call C-05 and C-06 traits directly

---

## C-09 — Middleware (`src/middleware/`)

**Responsibility:** Provide reusable Tower middleware layers applied to all
routes on the CDN listener.

Layers (outermost to innermost):

1. `RequestIdLayer` — generate and inject `X-Request-Id` (UUID v4)
2. `TraceLayer` — structured per-request span (method, path, status, latency)
3. `RateLimitLayer` — `tower-governor` per-IP GCRA limiter (ADR-0015)
4. `SecurityHeadersLayer` — HSTS, X-Content-Type-Options, X-Frame-Options,
   Referrer-Policy, CSP (FR-06)
5. `CompressionLayer` — gzip/br for non-binary responses

**Boundaries:**

- No business logic; no dependency on domain components
- `RateLimitLayer` is applied to CDN routes only
- `SecurityHeadersLayer` is applied to all routes on both listeners

---

## C-10 — Observability (`src/observability/`)

**Responsibility:** Initialise the Prometheus metrics registry and OpenTelemetry
OTLP exporter. Provide typed metric handles consumed by other components.
Expose `/health/live`, `/health/ready`, and `/metrics` handlers.

**Key types:**

- `Metrics` — holds `lazy_static` metric handles: counters, histograms, gauges
- `OtelGuard` — RAII guard for OTEL exporter flush on shutdown
- `health::liveness_handler` — always `200 OK`
- `health::readiness_handler` — checks S3 circuit breaker state and Redis
  reachability; returns `503` if either is unhealthy

**Boundaries:**

- `Metrics` is initialised once in `main()` and passed into `AppState`
- All other components call `metrics.record_*()` methods — no direct
  `prometheus` crate imports outside this module
- OTEL SDK initialised in `main()` before the Tokio runtime starts handlers

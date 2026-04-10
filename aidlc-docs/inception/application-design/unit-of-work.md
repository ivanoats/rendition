# Units of Work

## Decomposition Decisions

| Decision | Choice |
|---|---|
| Development strategy | Sequential on `main` — one unit at a time, always releasable |
| Integration test infrastructure | `testcontainers-rs` — real Docker containers in-process |
| Feature activation | Config-gated, always-on — no compile-time feature flags |

---

## Unit 1 — Config

**Goal:** Establish the typed configuration foundation every other unit depends on.
Nothing else can start until this unit is complete and tested.

**Deliverables:**

- `src/config.rs` — `AppConfig` struct with all `RENDITION_*` fields; nested
  `S3Config` and `OidcConfig`; `envy::prefixed("RENDITION_")` loading;
  `validate()` for cross-field rules
- `tests/config_test.rs` — unit tests for valid configs, missing required
  fields, invalid types; proptest round-trips for valid env var sets

**Acceptance criteria:**

- `AppConfig::load()` returns `Ok` for any valid env var set
- `AppConfig::load()` returns `Err` with a human-readable message for any
  invalid set (missing required field, wrong type, out-of-range value)
- Process exits at startup — no panics at request time due to config issues
- All `RENDITION_*` variables documented in `README.md` configuration table

**Entry condition:** None  
**Exit condition:** `cargo test --lib config` passes; config module reviewed

---

## Unit 2 — S3 Storage Backend

**Goal:** Replace the `todo!()` stub in `S3Storage` with a production-ready
implementation behind the existing `StorageBackend` trait. Add circuit breaker
for fault isolation.

**Deliverables:**

- `src/storage/s3.rs` — `S3Storage` implementing `get`, `exists`, `get_range`
  via `aws-sdk-s3` `GetObject` / `HeadObject`; `get_range` passes `Range`
  header to S3 natively (ADR-0018)
- `src/storage/circuit_breaker.rs` — `CircuitBreaker` with configurable
  threshold and cooldown; `is_open() -> bool` for health checks
- Integration tests using `testcontainers-rs` + LocalStack S3
- `S3Config` fields wired from `AppConfig` (Unit 1)

**Acceptance criteria:**

- `S3Storage::get` fetches correct bytes from a real LocalStack bucket
- `S3Storage::exists` uses `HeadObject` (no body download)
- `S3Storage::get_range` fetches only the requested byte slice (verified by
  checking response size against requested range width)
- Circuit breaker opens after `threshold` consecutive errors; auto-closes
  after `cooldown`; `is_healthy()` reflects state
- No `aws-sdk-s3` types visible outside `src/storage/s3.rs`

**Entry condition:** Unit 1 complete  
**Exit condition:** Integration tests pass against LocalStack; `cargo clippy` clean

---

## Unit 3 — Transform Cache

**Goal:** Introduce an in-process LRU transform cache so repeated identical
requests bypass libvips entirely.

**Deliverables:**

- `src/cache.rs` — `TransformCache` trait; `MokaTransformCache` implementation;
  `CacheKey = [u8; 32]`; `compute_cache_key(path, params, format) -> CacheKey`
  using SHA-256
- `AppState` updated to include `Arc<dyn TransformCache>`
- `serve_asset` wired: cache lookup before storage fetch; cache store after
  transform (skip on embargo)
- Prometheus counters: `rendition_cache_hits_total`, `rendition_cache_misses_total`
- Proptest: `compute_cache_key` is deterministic for identical inputs; distinct
  for differing inputs

**Acceptance criteria:**

- A second identical request returns bytes from cache; libvips is not invoked
- Cache is bounded — inserting beyond `max_capacity` evicts the LRU entry
- Cache entries expire after `ttl` seconds
- Cache key is identical regardless of URL query parameter order
- `rendition_cache_hits_total` increments on cache hit

**Entry condition:** Unit 1 complete  
**Exit condition:** E2E test confirms cache hit on second request; proptest passes

---

## Unit 4 — Transform Pipeline Enhancements

**Goal:** Close the Scene7 / Imgix feature gap: `fmt=auto`, smart crop,
sharpening, watermark compositing, named preset resolution, and HTTP 206
video byte-range delivery.

**Deliverables:**

- `src/transform/mod.rs` — `negotiate_format(accept, has_alpha) -> ImageFormat`;
  `ImageFormat::Auto` variant; `TransformParams` extended with `sharp`,
  `unsharp`, `layer`, `layer_pos`, `layer_opacity`, `preset` fields
- `src/transform/pipeline.rs` — sharpening step (`ops::sharpen`); watermark
  compositing step (`ops::composite`); `fit=smart` via `ops::smartcrop`
- `src/api/mod.rs` — `serve_asset` extended with: preset resolution via
  `PresetStore`; `Range` header parsing; `206`/`Accept-Ranges` response
- `src/preset/` — `PresetStore` trait; `RedisPresetStore`; `resolve_params()`
- `Vary: Accept` header on `fmt=auto` responses
- Full input validation for all new parameters (FR-09)
- Proptest: pipeline output dimensions satisfy fit-mode invariants for all
  valid input combinations

**Acceptance criteria:**

- `fmt=auto` with `Accept: image/avif` returns AVIF; response includes
  `Vary: Accept`
- `fit=smart` returns a crop centred on the detected subject (verified
  visually in integration test fixtures)
- `?preset=thumbnail` expands stored params; explicit URL params override
- `Range: bytes=0-1023` returns `206 Partial Content` with correct
  `Content-Range` and 1024 bytes
- Multi-range requests return `416 Range Not Satisfiable`

**Entry condition:** Units 1 and 3 complete  
**Exit condition:** All new params covered by integration tests; proptest passes

---

## Unit 5 — Embargo + Admin API

**Goal:** Implement the embargo enforcement system end-to-end: Redis persistence,
in-process enforcer cache, admin API with OIDC/API key authentication, and
CDN-path enforcement returning `HTTP 451`.

**Deliverables:**

- `src/embargo/mod.rs` — `EmbargoRecord`, `EmbargoStore` trait,
  `EmbargoEnforcer` with in-process HashMap cache (configurable TTL)
- `src/embargo/redis_store.rs` — `RedisEmbargoStore` via `fred` crate;
  `embargo:{path}` key namespace; `EXPIREAT` TTL
- `src/preset/mod.rs` and `src/preset/redis_store.rs` — `PresetStore` trait
  and `RedisPresetStore` sharing the Redis connection pool
- `src/admin/` — `admin_router()` bound to `RENDITION_ADMIN_BIND_ADDR`;
  `AuthLayer` (OIDC JWT via `jsonwebtoken` + JWKS cache, and SHA-256 API key);
  `embargo_handlers`, `preset_handlers`, `purge_handlers`
- `serve_asset` wired: `EmbargoEnforcer::check()` before every CDN request;
  `451` response; embargo results must not enter transform cache
- Audit log entries for all embargo mutations (structured log with request ID)
- Integration tests using `testcontainers-rs` Redis

**Acceptance criteria:**

- `POST /admin/embargoes` with valid OIDC JWT creates embargo in Redis
- `GET /cdn/{embargoed-path}` returns `451` with generic body; no storage I/O
- `DELETE /admin/embargoes/{path}` lifts embargo; next CDN request returns asset
- `EmbargoEnforcer` local cache invalidated immediately on admin delete
- `POST /admin/embargoes` on a path already embargoed returns `409 Conflict`
- Unauthenticated `POST /admin/embargoes` returns `401`
- JWT with wrong group returns `403`
- Invalid OIDC config → admin API only accepts API keys (config-gated)

**Entry condition:** Units 1 and 3 complete  
**Exit condition:** Integration tests pass; `cargo clippy` clean; audit entries verified

---

## Unit 6 — Middleware

**Goal:** Harden the CDN listener with production-grade middleware: per-IP rate
limiting, security headers, request ID injection, error response hardening,
and full input validation on all query parameters.

**Deliverables:**

- `src/middleware/mod.rs` — `cdn_middleware_stack()` composing Tower layers:
  `RequestIdLayer`, `TraceLayer`, `GovernorLayer` (tower-governor),
  `SecurityHeadersLayer`, `CompressionLayer`
- `SecurityHeadersLayer` setting HSTS, `X-Content-Type-Options`, `X-Frame-Options`,
  `Referrer-Policy`, `Content-Security-Policy`
- `TransformParams::validate()` enforcing all FR-09 constraints; `400` on violation
- Error handler ensuring no internal details (`serve_asset` `500` returns
  `{"error":"internal server error"}` only; details in structured log with
  request ID)
- `413 Payload Too Large` for requests exceeding `RENDITION_MAX_PAYLOAD_BYTES`
- `RENDITION_RATE_LIMIT_KEY` configures IP extraction strategy
  (`peer_ip` or `x_forwarded_for`)

**Acceptance criteria:**

- Every response includes all six security headers
- Every response includes `X-Request-Id`
- `?wid=99999` returns `400 Bad Request`
- `?qlt=0` returns `400 Bad Request`
- IP exceeding `RENDITION_RATE_LIMIT_RPS` receives `429` with `Retry-After`
- `500` response body contains no stack trace, file path, or libvips message
- Security baseline rules SECURITY-04, SECURITY-05, SECURITY-08, SECURITY-09
  all pass

**Entry condition:** Unit 1 complete  
**Exit condition:** All middleware acceptance tests pass; security header test matrix complete

---

## Unit 7 — Observability and Operations

**Goal:** Make Rendition production-deployable: Prometheus metrics, OTEL traces,
split health probes, graceful shutdown, Dockerfile, Kubernetes manifests,
`cargo-audit` in CI, and coverage gate.

**Deliverables:**

- `src/observability/mod.rs` — `Metrics` struct with all counters/histograms/
  gauges; `init_otel()` returning `OtelGuard`; `GET /metrics` handler
- `src/observability/health.rs` — `liveness_handler()` (always 200);
  `readiness_handler()` checking S3 circuit breaker and Redis reachability
- `main.rs` updated: `CancellationToken` for graceful shutdown on SIGTERM/SIGINT;
  drain in-flight requests before exit
- `Dockerfile` — multi-stage build; final image based on `debian:bookworm-slim`
  with libvips runtime; non-root user
- `k8s/` — `Deployment`, `Service` (CDN `:3000`, admin `:3001`), `HorizontalPodAutoscaler`,
  `ConfigMap` template, `ServiceMonitor` for Prometheus scraping
- `.github/workflows/ci.yml` — `cargo test`, `cargo clippy`, `cargo audit`,
  `cargo llvm-cov` with ≥ 80% line coverage gate, Docker build
- `docker-compose.yml` — Redis + LocalStack for local dev

**Acceptance criteria:**

- `GET /metrics` returns Prometheus text format with all registered metrics
- `GET /health/live` returns `200` always
- `GET /health/ready` returns `503` when S3 circuit breaker is open
- `GET /health/ready` returns `503` when Redis is unreachable
- SIGTERM causes graceful drain (in-flight requests complete; new connections refused)
- `cargo audit` reports zero high/critical CVEs
- `cargo llvm-cov` reports ≥ 80% line coverage
- `docker build` produces a working image; `docker run` starts and passes health check

**Entry condition:** All units 1–6 complete  
**Exit condition:** CI pipeline green end-to-end; K8s manifests reviewed

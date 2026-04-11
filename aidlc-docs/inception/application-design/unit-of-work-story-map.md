# Unit of Work — Story Map

Functional requirements (FR) and quality attributes (QA) mapped to units.
Each item lists the requirement, the component it lands in, and its
acceptance test type.

---

## Unit 1 — Config

| Requirement | Component | Test type |
|---|---|---|
| FR-02: All `RENDITION_*` env vars parsed into typed struct | `AppConfig` | Unit + proptest |
| FR-02: Fail-fast on missing required vars | `AppConfig::validate()` | Unit |
| FR-02: Fail-fast on invalid var values | `envy` + `validate()` | Unit |
| FR-02: S3 fields required only when `RENDITION_STORAGE_BACKEND=s3` | `validate()` cross-field | Unit |
| FR-02: OIDC fields validated when OIDC is configured | `OidcConfig::validate()` | Unit |
| NFR-02 (PBT): Valid env var sets always produce valid `AppConfig` | `AppConfig` | Proptest |
| NFR-02 (PBT): Invalid sets always fail with typed error | `AppConfig::load()` | Proptest |

---

## Unit 2 — S3 Storage Backend

| Requirement | Component | Test type |
|---|---|---|
| FR-01: `S3Storage::get` fetches bytes from S3 | `S3Storage` | Integration (LocalStack) |
| FR-01: `S3Storage::exists` uses `HeadObject` (no body) | `S3Storage` | Integration |
| FR-01: S3 endpoint configurable for MinIO / R2 | `S3Config::endpoint` | Integration |
| FR-01: No AWS SDK types outside `src/storage/s3.rs` | Module boundary | Compile-time |
| FR-10: Replace `todo!()` panics with typed errors | `S3Storage` | Unit |
| QA-04 (reliability): Circuit breaker opens after threshold errors | `CircuitBreaker` | Unit |
| QA-04: Circuit breaker auto-closes after cooldown | `CircuitBreaker` | Unit |
| QA-04: `is_healthy()` reflects circuit state | `CircuitBreaker` | Unit |
| NFR-06: `StorageBackend` trait unchanged by S3 impl | Trait boundary | Compile-time |

---

## Unit 3 — Transform Cache

| Requirement | Component | Test type |
|---|---|---|
| FR-03: Cache transformed outputs to avoid redundant libvips work | `MokaTransformCache` | Integration |
| FR-03: Cache key includes path + full transform params | `compute_cache_key` | Unit + proptest |
| FR-03: Cache bounded by `RENDITION_CACHE_MAX_ENTRIES` (LRU eviction) | `MokaTransformCache` | Unit |
| FR-03: Cache entries expire after `RENDITION_CACHE_TTL_SECONDS` | `MokaTransformCache` | Unit |
| FR-03: Cache safe for concurrent access | `MokaTransformCache` | Unit (threaded) |
| FR-03: Cache hits bypass libvips | `serve_asset` wire-up | Integration |
| FR-03: Cache hit/miss metrics emitted | `Metrics` counters | Unit |
| FR-14: Embargoed responses must not enter cache | `serve_asset` | Integration |
| NFR-02 (PBT): `compute_cache_key` deterministic for same inputs | `compute_cache_key` | Proptest |
| NFR-02 (PBT): Distinct keys for differing params | `compute_cache_key` | Proptest |

---

## Unit 4 — Transform Pipeline Enhancements

| Requirement | Component | Test type |
|---|---|---|
| FR-15: `fmt=auto` selects best format from `Accept` header | `negotiate_format()` | Unit |
| FR-15: `Vary: Accept` set on `fmt=auto` responses | `serve_asset` | Integration |
| FR-15: Preference order AVIF → WebP → JPEG | `negotiate_format()` | Unit |
| FR-16: `sharp=1` applies default unsharp mask | `apply_blocking` sharpen step | Integration |
| FR-16: `unsharp=r,s,a,t` applies full unsharp mask | `apply_blocking` | Integration |
| FR-16: Sharpening applied after resize, before encode | Pipeline order | Unit |
| FR-17: `layer=path` composites watermark onto source | `apply_blocking` composite step | Integration |
| FR-17: `layer_pos` controls overlay position | `apply_blocking` | Integration |
| FR-17: `layer_opacity` controls blend | `apply_blocking` | Integration |
| FR-18: `?preset=name` expands stored params | `resolve_params()` + `PresetStore` | Integration |
| FR-18: Explicit URL params override preset defaults | `resolve_params()` | Unit |
| FR-19: `fit=smart` uses libvips `smartcrop` | `apply_blocking` resize step | Integration |
| FR-22: `Range: bytes=start-end` returns `206 Partial Content` | `serve_asset` | Integration |
| FR-22: `Accept-Ranges: bytes` on all asset responses | `serve_asset` | Integration |
| FR-22: Multi-range request returns `416` | `serve_asset` | Unit |
| FR-22: `S3Storage::get_range` uses S3 native range fetch | `S3Storage` | Integration |
| FR-09: Validation of all new params | `TransformParams::validate()` | Unit + proptest |
| NFR-02 (PBT): Output dimensions satisfy fit-mode invariants | `apply_blocking` | Proptest |
| NFR-02 (PBT): `fmt=auto` always produces a valid format | `negotiate_format` | Proptest |

---

## Unit 5 — Embargo + Admin API

| Requirement | Component | Test type |
|---|---|---|
| FR-11: `EmbargoRecord` data model with all required fields | `EmbargoRecord` | Unit |
| FR-11: Embargo records persisted in Redis | `RedisEmbargoStore` | Integration (Redis) |
| FR-11: In-process cache with `RENDITION_EMBARGO_CACHE_TTL_SECONDS` | `EmbargoEnforcer` | Unit |
| FR-12: `POST /admin/embargoes` creates embargo | `create_embargo` handler | Integration |
| FR-12: `GET /admin/embargoes` lists active embargoes | `list_embargoes` handler | Integration |
| FR-12: `GET /admin/embargoes/{path}` fetches one | `get_embargo` handler | Integration |
| FR-12: `PUT /admin/embargoes/{path}` updates date/note | `update_embargo` handler | Integration |
| FR-12: `DELETE /admin/embargoes/{path}` lifts embargo | `delete_embargo` handler | Integration |
| FR-12: `POST` on existing path returns `409 Conflict` | `create_embargo` | Integration |
| FR-12: `embargo_until` in past returns `400` | `create_embargo` validation | Unit |
| FR-12: Delete immediately invalidates local cache | `EmbargoEnforcer::invalidate()` | Unit |
| FR-13: OIDC JWT validation via JWKS endpoint | `JwksCache::validate()` | Unit (mock JWKS) |
| FR-13: Group claim check for `RENDITION_OIDC_ADMIN_GROUP` | `AuthLayer` | Unit |
| FR-13: API key SHA-256 comparison | `AuthLayer` | Unit |
| FR-13: Unauthenticated request returns `401` | `AuthLayer` | Integration |
| FR-13: Wrong group returns `403` | `AuthLayer` | Integration |
| FR-13: Admin API on `127.0.0.1:3001` only | `admin_router` binding | Integration |
| FR-14: CDN path checks embargo before any storage I/O | `serve_asset` | Integration |
| FR-14: Active embargo returns `451` with generic body | `serve_asset` | Integration |
| FR-14: `embargo_until` not in `451` response body | `serve_asset` | Integration |
| FR-14: Embargoed responses not cached | `serve_asset` + cache | Integration |
| FR-18: `POST /admin/presets` creates preset | `create_preset` handler | Integration |
| FR-18: `GET /admin/presets` lists presets | `list_presets` handler | Integration |
| FR-18: `PUT /admin/presets/{name}` updates preset | `update_preset` handler | Integration |
| FR-18: `DELETE /admin/presets/{name}` removes preset | `delete_preset` handler | Integration |
| FR-14 (audit): Embargo mutations logged with request ID | Structured log | Integration |
| NFR-03 (SECURITY-07): API keys stored as SHA-256 hashes only | `AppConfig` + `AuthLayer` | Code review |

---

## Unit 6 — Middleware

| Requirement | Component | Test type |
|---|---|---|
| FR-04: `413` for requests exceeding `RENDITION_MAX_PAYLOAD_BYTES` | `RequestBodyLimitLayer` | Integration |
| FR-05: Per-IP rate limiting on `/cdn/*` | `GovernorLayer` | Integration |
| FR-05: `429` + `Retry-After` on breach | `GovernorLayer` | Integration |
| FR-05: Rate limit params configurable | `AppConfig` + middleware wiring | Unit |
| FR-06: `Strict-Transport-Security` on all responses | `SecurityHeadersLayer` | Integration |
| FR-06: `X-Content-Type-Options: nosniff` | `SecurityHeadersLayer` | Integration |
| FR-06: `X-Frame-Options: DENY` | `SecurityHeadersLayer` | Integration |
| FR-06: `Referrer-Policy` | `SecurityHeadersLayer` | Integration |
| FR-06: `Content-Security-Policy: default-src 'none'` | `SecurityHeadersLayer` | Integration |
| FR-07: Every request logged with method, path, status, latency, request ID | `TraceLayer` | Integration |
| FR-07: `X-Request-Id` in response header | `RequestIdLayer` | Integration |
| FR-07: No credentials or PII in logs | Log output | Code review |
| FR-08: `500` body contains no internal details | Error handler | Integration |
| FR-08: Internal details logged server-side with request ID | `tracing` spans | Integration |
| FR-09: `wid` / `hei` ≤ 8192 | `TransformParams::validate()` | Unit |
| FR-09: `qlt` in 1–100 | `TransformParams::validate()` | Unit |
| FR-09: `rotate` one of 0/90/180/270 | `TransformParams::validate()` | Unit |
| FR-09: `crop` four non-negative integers | `TransformParams::validate()` | Unit |
| NFR-03 (SECURITY-04): Security headers on all responses | `SecurityHeadersLayer` | Integration |
| NFR-03 (SECURITY-05): Input validation on all params | `TransformParams::validate()` | Unit + proptest |
| NFR-03 (SECURITY-08): Rate limiting on public endpoints | `GovernorLayer` | Integration |
| NFR-03 (SECURITY-09): Error responses hide internals | Error handler | Integration |

---

## Unit 7 — Observability and Operations

| Requirement | Component | Test type |
|---|---|---|
| NFR-05: `GET /metrics` in Prometheus text format | `metrics_handler` | Integration |
| NFR-05: `rendition_cache_hits_total` counter | `Metrics` | Integration |
| NFR-05: `rendition_cache_misses_total` counter | `Metrics` | Integration |
| NFR-05: `rendition_transform_duration_seconds` histogram | `Metrics` | Integration |
| NFR-05: `rendition_embargo_rejections_total` counter | `Metrics` | Integration |
| NFR-05: `rendition_storage_errors_total` counter | `Metrics` | Integration |
| NFR-05: `rendition_circuit_breaker_open` gauge | `Metrics` | Integration |
| QA-02 (observability): OTLP traces exported per request | `init_otel` + `TraceLayer` | Integration |
| QA-03 (health): `GET /health/live` always `200` | `liveness_handler` | Integration |
| QA-03: `GET /health/ready` returns `503` when S3 CB open | `readiness_handler` | Integration |
| QA-03: `GET /health/ready` returns `503` when Redis down | `readiness_handler` | Integration |
| QA-05 (graceful shutdown): SIGTERM drains in-flight requests | `main` + `CancellationToken` | Integration |
| FR-20: `RENDITION_PUBLIC_BASE_URL` used in API responses | `AppConfig` + preset list | Unit |
| FR-21: `Surrogate-Key: asset:{path}` on CDN responses | `serve_asset` | Integration |
| FR-21: `Cache-Control` value configurable | `AppConfig` + `serve_asset` | Unit |
| FR-21: `Vary: Accept` on `fmt=auto` responses | `serve_asset` | Integration |
| NFR-01: ≥ 80% line/branch coverage | `cargo llvm-cov` | CI gate |
| NFR-07: `cargo-audit` zero high/critical CVEs | `cargo-audit` | CI gate |
| NFR-07: `Cargo.lock` committed | Repository | Convention |
| QA-06 (portability): Dockerfile multi-stage build | `Dockerfile` | CI (docker build) |
| QA-06: K8s `Deployment` + `Service` + `HPA` manifests | `k8s/` | Review |
| QA-06: `docker-compose.yml` for local dev (Redis + LocalStack) | `docker-compose.yml` | Manual |

---

## Requirements Coverage Summary

| Range | Count | All assigned |
|---|---|---|
| FR-01 to FR-22 | 22 functional requirements | Yes |
| NFR-01 to NFR-07 | 7 non-functional requirements | Yes |
| QA-02 to QA-06 | 5 quality attributes (ops-relevant) | Yes |
| Security baseline (SECURITY-04, 05, 07, 08, 09) | 5 key rules | Yes |
| PBT rules (NFR-02) | Config, cache key, pipeline invariants, param validation | Yes |

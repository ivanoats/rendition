# Unit 2 — S3 Storage Backend: Code Summary

**Stage:** Code Generation (Part 2 — Generation, complete)
**Plan:** `aidlc-docs/construction/plans/s3-storage-code-generation-plan.md`
**Upstream artifacts:**
`aidlc-docs/construction/s3-storage/functional-design/`,
`aidlc-docs/construction/s3-storage/nfr-requirements/`,
`aidlc-docs/construction/s3-storage/nfr-design/`,
`aidlc-docs/construction/s3-storage/infrastructure-design/`

## Files created

- **`src/storage/local.rs`** — `LocalStorage` extracted from the
  previous monolithic `src/storage/mod.rs`. Uses the new
  `Result<T, StorageError>` trait signature and wraps filesystem reads
  in `tokio::time::timeout(local_timeout_ms, ...)`. Owns the
  `safe_join` path-traversal helper (filesystem-specific, moved here
  from `mod.rs`).
- **`src/storage/circuit_breaker.rs`** — ~240 lines implementing the
  ADR-0019 state machine. `std::sync::Mutex<State>`, critical sections
  never cross `.await`, `tokio::time::Instant` for deterministic
  `#[tokio::test(start_paused = true)]` tests. Eight unit tests cover
  R-06 transitions and the single-probe half-open rule.
- **`src/storage/s3.rs`** — ~450 lines wrapping `aws-sdk-s3` with:
  - A hand-rolled full-jitter retry loop (R-02) capped at 500 ms.
  - Per-attempt `tokio::time::timeout` (R-03).
  - Route through `CircuitBreaker::call` (Flow 1/2/3).
  - Error classification via `classify_get_object_error`,
    `classify_head_object_error`, `classify_sdk_shape` (R-01).
  - Content-type fallback chain (R-05) in `resolve_content_type`.
  - Range header assembly with post-fetch length verification (Flow 3).
  - `is_healthy()` O(1) breaker-state read.
  - Production `new(&S3Settings)` constructor that reads SDK default
    credentials chain; `new_for_test(endpoint, key, secret, bucket)`
    with static creds for LocalStack tests.
  - Pure-helper unit tests: `resolve_content_type` fallback variants,
    `full_jitter_delay` cap, `outcome_of` mapping.
- **`tests/s3_integration.rs`** — nine `#[ignore]`-gated LocalStack
  integration tests. Shared container via `tokio::sync::OnceCell`
  behind a `OnceLock`, per-test UUID bucket names via `uuid::Uuid`.
  Covers the full acceptance criterion matrix from the unit
  definition.
- **`tests/circuit_breaker_proptest.rs`** — three proptest properties:
  (1) breaker survives arbitrary sequences without panic, (2) only
  `Success` and `NotFound` events never open the breaker, (3)
  exactly `threshold` `Unavailable` calls always open the breaker.
  Each property runs 32 generated cases; all use `start_paused = true`
  tokio runtimes for deterministic time.
- **`aidlc-docs/construction/s3-storage/code/code-summary.md`** —
  this file.

## Files modified

- **`Cargo.toml`** — added runtime deps `aws-config`, `aws-sdk-s3`,
  `aws-smithy-runtime`, `aws-smithy-runtime-api`, `aws-smithy-types`,
  `aws-credential-types`, `rand`. Added dev-deps
  `testcontainers-modules` (`localstack` feature) and `uuid` (`v4`).
  Added `test-util` feature to `tokio` for `start_paused` support.
- **`Cargo.lock`** — ~200 new transitive entries for the AWS SDK and
  its `rustls` HTTP stack.
- **`src/config.rs`** — introduced `S3Settings` nested struct
  containing all `RENDITION_S3_*` fields plus seven new ones. The
  top-level `AppConfig` holds a single `s3: S3Settings` via
  `#[serde(skip)]` + manual two-pass `envy::prefixed("RENDITION_").from_env::<S3Settings>()`
  in `AppConfig::load()`. Added `local_timeout_ms` top-level field.
  `S3Settings::validate()` enforces per-field bounds (R-01..R-07 of
  domain-entities.md E4). `AppConfig::validate()` now delegates to
  `S3Settings::validate()` and checks `local_timeout_ms >= 100`.
  HTTPS-only enforcement on `s3_endpoint` unless
  `s3_allow_insecure_endpoint` (SECURITY-01 in-transit).
- **`src/storage/mod.rs`** — rewritten. Now contains only the trait,
  `Asset`, `StorageError`, `Outcome`, `StorageMetrics`,
  `NoopMetrics`, `compose_key` helper, `content_type_from_ext`
  helper, and re-exports for `LocalStorage`/`S3Storage`. Trait
  `StorageBackend` methods now return `Result<T, StorageError>`;
  `get_range` has a default full-fetch-and-slice impl so
  `LocalStorage` doesn't need to override.
- **`src/lib.rs`** — `build_app` is now `async` and returns
  `Result<Router, AppBuildError>`. Selects `LocalStorage` or
  `S3Storage::new(&cfg.s3).await?` based on
  `cfg.storage_backend`. Wires `NoopMetrics` (via the S3Storage
  constructor); Unit 7 replaces with `PrometheusMetrics`.
- **`src/main.rs`** — awaits `build_app` and handles the new
  `AppBuildError` fail-fast path.
- **`src/api/mod.rs`** — `serve_asset` handles `Result<bool, StorageError>`
  from `exists` and the `StorageError` variants from `get`. New
  `storage_error_response` helper maps errors to HTTP statuses per
  R-01: `NotFound` → 404, `InvalidPath` → 400,
  `CircuitOpen`/`Unavailable` → 503, `Timeout` → 504, `Other` → 500.
  `MockStorage` test helper updated to the new trait signature. Full
  handler rework belongs to Unit 4.
- **`tests/config_test.rs`** — existing tests updated to read
  `cfg.s3.*` instead of flat `cfg.s3_*`. Nine new tests cover
  `S3Settings` defaults, full overrides, per-field bounds rejection
  (`max_connections`, `timeout_ms`, `max_retries`, `cb_threshold`,
  `local_timeout_ms`), HTTPS enforcement, and the
  `allow_insecure_endpoint` escape hatch.
- **`tests/api_integration.rs`** — `make_server` is now `async` to
  await the new `build_app`; all six call sites updated to
  `make_server().await`.
- **`tests/e2e.rs`** — same change: `setup` is `async`; 12 call sites
  updated.
- **`.github/workflows/ci.yml`** — added a new
  `s3-integration-tests` job running
  `cargo test --test s3_integration -- --ignored --test-threads=1`
  on ubuntu-latest. LocalStack is spawned via the existing Docker
  daemon. The main `build-and-test` job is unchanged and stays fast
  because LocalStack tests are `#[ignore]`-gated.

## Test results (local, non-`--ignored`)

```text
test result: ok. 71 passed; 0 failed; 0 ignored  (lib)
test result: ok.  7 passed; 0 failed; 0 ignored  (api_integration)
test result: ok. 29 passed; 0 failed; 0 ignored  (config)
test result: ok. 12 passed; 0 failed; 0 ignored  (e2e)
test result: ok.  3 passed; 0 failed; 0 ignored  (circuit_breaker_proptest)
test result: ok.  0 passed; 0 failed; 0 ignored  (doc)
```

**Total: 122 tests passing** (up from the pre-Unit-2 baseline of ~70).

`cargo test --test s3_integration -- --ignored` is verified at compile
time; the runtime execution requires Docker and is covered by the new
CI job.

## Design decisions finalised during implementation

| Decision | Pre-determined? | Adjustment |
|---|---|---|
| Two-pass `envy` load for nested `S3Settings` | No — NFR Design pinned the shape; `#[serde(flatten)]` turned out to mishandle numeric types in flattened children. | Used `#[serde(skip)]` + a second `envy::prefixed("RENDITION_").from_env::<S3Settings>()` call. Documented in `src/config.rs` comments. ADR-0020 unaffected — the decision (nested config) stands; only the serde mechanism changed. |
| `new_for_test` gated on `#[cfg(test)]` vs public | NFR Design plan offered both. | Public (not cfg-gated). Rust's `tests/` directory compiles as an external crate and cannot see `#[cfg(test)]` items from the library. The name makes intent obvious; risk of production misuse is low. |
| `StorageMetrics` trait placement | Decided in NFR Design. | Lives in `src/storage/mod.rs` as `pub trait StorageMetrics: Send + Sync + 'static`. `NoopMetrics` instantiated inside `S3Storage::new` rather than passed in from `build_app` — Unit 7 will update both. |

## Known deferrals (per plan)

- **Unit 4 — CDN handler full rework.** The `storage_error_response`
  helper in `src/api/mod.rs` is a minimal bridge to keep the build
  green after the trait return-type change. Unit 4 will own the
  per-variant HTTP response policy, `Range` header parsing, and the
  `Content-Range` / `Accept-Ranges` response headers.
- **Unit 7 — Prometheus metrics.** `StorageMetrics` is currently wired
  to `NoopMetrics`. Unit 7 introduces `PrometheusMetrics`
  implementing the same trait and a `build_app`-time injection so
  `S3Storage::new` accepts a metrics arg instead of hardcoding the
  no-op.
- **Unit 7 — `/health/ready` endpoint.** `S3Storage::is_healthy()` is
  implemented and returns in O(1), but no HTTP route consumes it yet.
- **Infrastructure provisioning.** The bucket/IAM/VPC-endpoint JSON
  lives in `infrastructure-design.md` as a target spec — Terraform/CDK
  authoring is out of Unit 2 scope.

## Related ADRs

- **ADR-0004 (revised)** — Pluggable Storage via Trait Abstraction.
  Updated to reflect the `Result<T, StorageError>` return type and
  the addition of `get_range`.
- **ADR-0019 (new)** — Hand-rolled circuit breaker. Documents the
  consecutive-failures trigger, single-probe half-open, SDK retrier
  disablement, and crate alternatives rejected.
- **ADR-0020 (new)** — Nested configuration groups in `AppConfig`.
  Documents the refactor from flat `s3_*` fields to
  `AppConfig.s3: S3Settings`.

## Net diff scope

~**1600 lines added**, mostly new files (`circuit_breaker.rs`,
`s3.rs`, `s3_integration.rs`, `circuit_breaker_proptest.rs`) plus
Cargo.lock growth from the AWS SDK transitive deps.

# Unit 2 — S3 Storage Backend: Code Generation Plan

**Unit:** S3 Storage Backend (Unit 2 of 7)
**Stage:** Code Generation (Part 1 — Planning)
**Project type:** Brownfield (existing Rust workspace)
**Workspace root:** `/Users/ivan/code/rendition` (tsupa)
**Story map:** `aidlc-docs/inception/application-design/unit-of-work-story-map.md`
**Upstream stages:** Functional Design, NFR Requirements, NFR Design,
Infrastructure Design (all complete)

**This plan is the single source of truth for Code Generation.** Steps
are numbered and executed in order; each has a `- [ ]` checkbox to mark
when complete.

---

## Unit context

### Stories implemented by this unit

| Requirement | Component | Test type |
|---|---|---|
| FR-01: `S3Storage::get` fetches bytes from S3 | `S3Storage` | Integration (LocalStack) |
| FR-01: `S3Storage::exists` uses `HeadObject` | `S3Storage` | Integration |
| FR-01: S3 endpoint configurable for MinIO/R2 | `S3Settings::endpoint` | Integration |
| FR-01: No AWS SDK types outside `src/storage/s3.rs` | Module boundary | Compile-time |
| FR-10: Replace `todo!()` panics with typed errors | `S3Storage`, `StorageError` | Unit |
| QA-04: Circuit breaker opens after threshold errors | `CircuitBreaker` | Unit + proptest |
| QA-04: Circuit breaker auto-closes after cooldown | `CircuitBreaker` | Unit + proptest |

### Dependencies on other units

- **Unit 1 (Config)** — complete. `AppConfig` will be refactored to nest
  `S3Settings`; existing flat field access rewrites to `cfg.s3.*`.
- **Unit 4 (CDN handler)** — not yet built. This unit's trait return-type
  change forces a minimal error mapping in the existing `src/api/mod.rs`
  stub so the build stays green. Full `StorageError` → HTTP status mapping
  is Unit 4's responsibility.
- **Unit 7 (Observability)** — not yet built. `StorageMetrics` trait is
  stubbed with `NoopMetrics` here; Unit 7 drops in `PrometheusMetrics`.

### Expected interfaces and contracts

`StorageBackend` trait (evolved per ADR-0004 revision):

```rust
pub trait StorageBackend: Send + Sync {
    async fn get(&self, path: &str) -> Result<Asset, StorageError>;
    async fn exists(&self, path: &str) -> Result<bool, StorageError>;
    async fn get_range(
        &self,
        path: &str,
        range: Range<u64>,
    ) -> Result<Asset, StorageError>;  // default: full fetch + slice
}
```

### Unit boundaries (reminder)

- No `aws-sdk-s3::*` type appears in any `pub` signature outside
  `src/storage/s3.rs` (R-08, SECURITY / supply-chain boundary).
- `CircuitBreaker` is reusable — has no S3 knowledge.
- `StorageError` is the only error type crossing the `storage` module
  boundary (ADR-0004 revised).

---

## Code location

| Artifact class | Location |
|---|---|
| Rust source | `src/storage/`, `src/config.rs`, `src/lib.rs`, `src/main.rs`, `src/api/mod.rs` |
| Rust tests | `tests/config_test.rs`, `tests/api_integration.rs`, `tests/circuit_breaker_proptest.rs`, `tests/s3_integration.rs`, `tests/helpers/localstack.rs` |
| Build config | `Cargo.toml`, `Cargo.lock` |
| CI workflow | `.github/workflows/ci.yml` (add integration job) |
| Code summaries (markdown only) | `aidlc-docs/construction/s3-storage/code/` |

All code goes to workspace root per brownfield rules. `aidlc-docs/` holds
only markdown documentation — never `.rs` files.

---

## Generation steps

### Phase 1 — Dependencies & config shape

- [x] **Step 1 — Add crate dependencies to `Cargo.toml`**
  - Add `aws-config = "1"` with `rustls` feature
  - Add `aws-sdk-s3 = "1"` with `rt-tokio`, `rustls` features
  - Add `aws-smithy-runtime = "1"` with `client-hyper`, `connector-rustls` features
  - Add `aws-smithy-types = "1"` (used for `ByteStream`)
  - Add `rand = "0.9"` (runtime dep — for retry jitter)
  - Add dev-dep `testcontainers-modules = "0.x"` with `localstack` feature
  - Add dev-dep `uuid = "1"` with `v4` feature (fresh-bucket naming)
  - Verify `tokio` has `test-util` feature for `start_paused` tests
  - Run `cargo generate-lockfile` to refresh `Cargo.lock`

- [x] **Step 2 — Refactor `AppConfig` to nest `S3Settings` (ADR-0020)**
  - Modify `src/config.rs`:
    - Introduce `pub struct S3Settings` with all S3 fields (bucket, region,
      endpoint, prefix, max_connections, timeout_ms, cb_threshold,
      cb_cooldown_secs, max_retries, retry_base_ms, allow_insecure_endpoint)
    - Add `local_timeout_ms: u64` as a top-level field on `AppConfig`
    - Replace the flat `s3_*` fields with a single `pub s3: S3Settings`
    - Serde/envy: use `#[serde(flatten)]` with `#[serde(rename = "s3_*")]`
      on `S3Settings` fields **OR** pick another attribute shape that
      preserves the `RENDITION_S3_*` env-var contract
    - Update `AppConfig::validate()`: cross-field check for S3 bucket+region
      moves to read from `self.s3.*`; add new validations per
      `domain-entities.md` E4 (max_connections ≥ 1, timeout_ms ≥ 100, etc.)
    - Add `S3Settings::validate()` for per-field bounds; call it from
      `AppConfig::validate()`
    - Update custom `Debug` impl to nest `s3`
  - Modify `src/lib.rs`: any reference to `cfg.s3_bucket`, `cfg.s3_region`
    etc. becomes `cfg.s3.bucket`, `cfg.s3.region`
  - **Do NOT** change env var names — `RENDITION_S3_*` remains the
    operator-facing contract

- [x] **Step 3 — Update config tests**
  - Modify `tests/config_test.rs`:
    - All existing tests that read flat `s3_*` fields now read `cfg.s3.*`
    - Add tests for each new `S3Settings` field: default value, type
      coercion, bounds rejection
    - Add a test that `RENDITION_STORAGE_BACKEND=s3` + missing
      `RENDITION_S3_BUCKET` still fails `validate()` with a clear message
    - Add a test that `RENDITION_S3_ALLOW_INSECURE_ENDPOINT=true` permits
      `http://` endpoints, default (false) rejects them
    - Add a test for `RENDITION_LOCAL_TIMEOUT_MS` default and override
    - Property test: `S3Settings::validate()` rejects all zero and out-of-bound values

### Phase 2 — Storage module restructure

- [x] **Step 4 — Add `StorageError`, `StorageMetrics`, `Outcome` to `src/storage/mod.rs`**
  - Introduce `pub enum StorageError` (thiserror::Error derive) with
    variants: `NotFound`, `InvalidPath { reason }`, `CircuitOpen`,
    `Timeout { op }`, `Unavailable { source: Box<dyn Error + Send + Sync> }`,
    `Other { source: Box<dyn Error + Send + Sync> }`
  - Add `pub enum Outcome { Success, NotFound, Unavailable, Timeout, CircuitOpen, InvalidPath, Other }`
  - Add `pub trait StorageMetrics: Send + Sync + 'static` with methods
    `record(&self, op: &str, outcome: Outcome, duration: Duration)` and
    `set_circuit_open(&self, open: bool)`
  - Add `pub struct NoopMetrics` implementing `StorageMetrics` with empty bodies
  - Update the existing `StorageBackend` trait — `get` returns
    `Result<Asset, StorageError>`; `exists` returns
    `Result<bool, StorageError>`; add `get_range` with default impl that
    calls `get` and slices
  - Add `pub(crate) fn compose_key(prefix: &str, path: &str) -> Result<String, StorageError>`
    implementing R-07 / E5 rules (strip leading `/`, append `/` to prefix if
    non-empty, reject empty path and NUL bytes)
  - Keep `content_type_from_ext` helper in `mod.rs` (already there, shared)

- [x] **Step 5 — Extract `LocalStorage` into `src/storage/local.rs`**
  - Move the existing `LocalStorage` struct and its `StorageBackend` impl
    from `src/storage/mod.rs` into `src/storage/local.rs`
  - Update `LocalStorage::get` / `exists` to return `StorageError` instead
    of `anyhow::Result<T>` — map IO errors via:
    - `ErrorKind::NotFound` → `StorageError::NotFound`
    - `ErrorKind::InvalidInput` / traversal rejection → `StorageError::InvalidPath`
    - Other IO errors → `StorageError::Other { source }`
  - Wrap filesystem reads in `tokio::time::timeout(local_timeout_ms, ...)`;
    elapsed → `StorageError::Timeout { op: "get" }` etc.
  - Accept `local_timeout_ms: u64` as a constructor parameter (new)
  - Provide a default `get_range` (via the trait's default impl) — no
    override needed; local reads are cheap
  - Add `pub use local::LocalStorage;` re-export in `src/storage/mod.rs`

- [x] **Step 6 — Create `src/storage/circuit_breaker.rs`**
  - New file with:
    - `pub struct CircuitBreaker` — fields: `state: Mutex<State>`,
      `threshold: u32`, `cooldown: Duration`,
      `metrics: Arc<dyn StorageMetrics>`
    - Private `enum State` with variants `Closed { consecutive_failures: u32 }`,
      `Open { opened_at: tokio::time::Instant }`,
      `HalfOpen { probe_in_flight: bool }`
    - `impl CircuitBreaker::new(threshold, cooldown, metrics) -> Self`
    - `pub async fn call<F, T>(&self, f: F) -> Result<T, StorageError>
      where F: Future<Output = Result<T, StorageError>>` — implements
      Flow 4 of business-logic-model.md with `std::sync::Mutex` (critical
      section never crosses `.await`)
    - `pub fn is_open(&self) -> bool`
    - Failure-counting rule per R-06: only `Unavailable` and `Timeout`
      count; `NotFound`, `InvalidPath`, `Other`, `CircuitOpen` are treated
      as success
    - Emit `self.metrics.set_circuit_open(true/false)` on state transitions
  - Unit tests in an inline `#[cfg(test)] mod tests` module:
    - Opens after `threshold` consecutive failures
    - Stays open during cooldown
    - Enters half-open after cooldown
    - Closes on successful probe
    - Re-opens with fresh cooldown on failed probe
    - Concurrent call during half-open probe gets `CircuitOpen`
    - `NotFound` does not count as failure
  - Use `#[tokio::test(start_paused = true)]` + `tokio::time::advance`
    for all timing tests (Q4=D from NFR Design)

- [x] **Step 7 — Create `src/storage/s3.rs`**
  - New file containing all `aws-sdk-s3` imports — the **only** place
    AWS SDK types appear
  - `pub struct S3Storage` — fields: `client: Arc<aws_sdk_s3::Client>`,
    `bucket: String`, `prefix: String`, `max_retries: u32`,
    `retry_base_ms: u64`, `timeout: Duration`,
    `circuit_breaker: Arc<CircuitBreaker>`,
    `metrics: Arc<dyn StorageMetrics>`
  - `pub async fn new(settings: &S3Settings) -> Result<Self, StorageError>`
    — constructs the SDK client:
    - `aws_config::defaults(BehaviorVersion::latest())`
    - `.region(Region::new(settings.region.clone().unwrap_or_default()))`
    - `.endpoint_url(settings.endpoint.clone())` if set
    - HTTP client: explicit `HyperClientBuilder::new()` with
      `pool_idle_timeout`, `max_connections` from settings
    - `RetryConfig::disabled()` (NFR Req Q2 part D — our retry loop replaces it)
    - Build `aws_sdk_s3::Client::new(&sdk_config)`
    - Instantiate `CircuitBreaker::new(cb_threshold, cb_cooldown_secs, metrics.clone())`
  - Test-only constructor:
    `pub async fn new_for_test(endpoint, access_key, secret_key, bucket)
    -> Result<Self, StorageError>` gated behind `#[cfg(test)]` or a
    `test-util` feature. Explicit static credentials; no default chain call.
    Used by LocalStack integration tests.
  - `impl StorageBackend for S3Storage`:
    - `get` calls private `with_retries` wrapping
      `self.circuit_breaker.call(self.fetch_get(key)).await` (Flow 1)
    - `exists` — same shape; uses `HeadObject` (Flow 2)
    - `get_range` — builds `bytes=start-(end-1)` header; passes to
      `GetObject`; verifies returned bytes length matches range width;
      returns `StorageError::Other` on mismatch (Flow 3 post-fetch check)
  - Private helpers:
    - `fn classify<E>(err: SdkError<E>) -> StorageError` — implements R-01
      classification (404/403 → NotFound; 400 → Other; 5xx/throttling/
      timeout/connection → Unavailable or Timeout based on shape)
    - `async fn with_retries<F, T>(f: F) -> Result<T, StorageError>` —
      implements R-02 full-jitter retry with `max_retries` and `base_ms`
      from settings, hard cap 500 ms, using `rand::rngs::ThreadRng`
    - `fn range_header(range: Range<u64>) -> String` — `format!("bytes={}-{}", range.start, range.end - 1)`
    - `fn resolve_content_type(header_ct: Option<&str>, path: &str) -> String` — implements R-05 fallback chain
    - `fn body_to_bytes(stream: ByteStream) -> Result<Vec<u8>, StorageError>` — collects body
  - `pub fn is_healthy(&self) -> bool { !self.circuit_breaker.is_open() }`
  - Tracing: `#[tracing::instrument(skip(self), fields(backend = "s3", op, key))]` on the three public methods
  - Metrics hooks: call `self.metrics.record(...)` at outcome sites
  - Unit tests in inline `#[cfg(test)] mod tests` — limited to pure
    helpers (`classify`, `range_header`, `resolve_content_type`,
    `compose_key` called via `S3Storage::get` with an empty stub).
    Full-stack tests live in `tests/s3_integration.rs`.
  - Add `pub use s3::S3Storage;` re-export in `src/storage/mod.rs`

- [x] **Step 8 — Wire `build_app` in `src/lib.rs`**
  - At startup, pick backend based on `cfg.storage_backend`:
    - `StorageBackendKind::Local` → `Arc::new(LocalStorage::new(&cfg.assets_path, cfg.local_timeout_ms))`
    - `StorageBackendKind::S3` → `Arc::new(S3Storage::new(&cfg.s3).await?)`
  - Wire `NoopMetrics` as the `StorageMetrics` implementation (Unit 2)
  - Change `build_app`'s signature as needed so `main.rs` still compiles
  - `main.rs` may need to `.await` the config-to-app chain because
    `S3Storage::new` is async; keep it minimal

- [x] **Step 9 — Minimal `src/api/mod.rs` error mapping**
  - The existing `serve_asset` handler currently returns `anyhow`-wrapped
    results. Update the CDN handler to match the new `Result<Asset, StorageError>` return type so the project compiles.
  - Map `StorageError` variants to HTTP responses:
    - `NotFound` → `404`
    - `InvalidPath { .. }` → `400`
    - `CircuitOpen` | `Unavailable { .. }` → `503`
    - `Timeout { .. }` → `504`
    - `Other { .. }` → `500`
  - Keep the full request-handling refactor for Unit 4; this step is the
    minimum diff to keep `cargo build` / existing tests green.
  - Server-side log the full error via `tracing::error!("{err:#}")` but
    only return the generic status text to callers (SECURITY-09).

### Phase 3 — Tests

- [x] **Step 10 — Create `tests/helpers/localstack.rs`**
  - `static CONTAINER: OnceLock<LocalStackContainer>` from `testcontainers-modules`
  - `pub async fn endpoint() -> String` — returns `http://localhost:{port}`
    for the LocalStack container
  - `pub async fn fresh_bucket() -> String` — creates a uniquely named
    bucket via `uuid::Uuid::new_v4().simple()`; returns bucket name
  - `pub async fn put_fixture(bucket: &str, key: &str, bytes: &[u8], content_type: &str)`
    — uploads a test object
  - Initialize-on-first-use semantics (Q8=B from NFR Design)
  - Uses test-only `S3Storage::new_for_test`

- [x] **Step 11 — Create `tests/s3_integration.rs`**
  - **Every test carries `#[ignore]`** (Q5=A from NFR Design) with a
    reason string pointing developers at `cargo test -- --ignored`
  - Tests:
    - `get_fetches_existing_png` — put fixture, call `get`, verify bytes match
    - `get_returns_not_found_for_missing_key`
    - `get_range_returns_only_requested_bytes` — put 1000-byte fixture,
      request `10..50`, assert response length == 40
    - `get_range_fails_on_invalid_range` — `start >= end`
    - `exists_true_for_present_object`
    - `exists_false_for_missing_object` — via `HeadObject` only (no body download)
    - `exists_propagates_503_on_service_error` — harder; skip or use a
      fault-injected endpoint if LocalStack lacks error injection
    - `content_type_from_upload_metadata_is_preferred`
    - `content_type_falls_back_to_extension_inference`
    - `circuit_breaker_opens_after_threshold` — requires a fault mode;
      may be easier to leave to the unit test in `circuit_breaker.rs`
    - `prefix_composition_matches_r07` — create a bucket, put objects at
      both `assets/` and `no-prefix/`, verify `compose_key` produces
      the right S3 key

- [x] **Step 12 — Create `tests/circuit_breaker_proptest.rs`**
  - Proptest harness that generates arbitrary `Vec<Event>` where
    `Event = Success | Failure | AdvanceMs(u32)`
  - For each generated sequence, exercise the breaker and assert
    invariants after each step:
    - `consecutive_failures <= threshold` whenever state is `Closed`
    - `Open` state is only entered by crossing the threshold in `Closed`
      or by a failed probe in `HalfOpen`
    - `HalfOpen` state exists for at most one call at a time
    - `is_open()` returns `true` iff state matches `Open { .. }`
  - Uses `#[tokio::test(start_paused = true)]` for deterministic time
  - Shrinks minimal counterexamples on failure

- [x] **Step 13 — Update existing test files**
  - `tests/api_integration.rs`: whatever changes from the trait-return-type
    update (may need to match on new `StorageError` variants)
  - `tests/config_test.rs`: migration already captured in Step 3 — verify
    it's complete and passes
  - Existing `tests/e2e.rs`: likely no changes if it hits only
    `LocalStorage`, but run it to confirm

### Phase 4 — CI

- [x] **Step 14 — Add a LocalStack integration job to `.github/workflows/ci.yml`**
  - New job `s3-integration-tests` that:
    - Depends on the existing `build-and-test` job (needs its cache)
    - Installs Docker (present by default on `ubuntu-latest`)
    - Runs `cargo test --test s3_integration -- --ignored`
    - Triggered on push to `main` and on PRs that touch `src/storage/**`
      or `tests/s3_integration.rs`
  - Do not add LocalStack tests to the main `build-and-test` job — keeps
    the fast loop fast (NFR Design Q5=A)

### Phase 5 — Documentation

- [x] **Step 15 — Write code summaries in `aidlc-docs/construction/s3-storage/code/`**
  - `code-summary.md` — one-page summary of what was generated,
    mirroring the format of `aidlc-docs/construction/config/code/code-summary.md`
  - List of modified files vs created files (brownfield distinction)
  - Key decisions taken during implementation that weren't pre-pinned
  - Link to the three ADRs touched in Unit 2 (0004, 0019, 0020)
  - Known deferrals (what Unit 4, Unit 7 still owe)

- [x] **Step 16 — Update `README.md`**
  - Add the new `RENDITION_*` env vars to the configuration section table
  - Add a short "Storage backends" subsection mentioning LocalStorage,
    S3, and the S3-compatible fallback list (R2, MinIO, etc.)
  - Note the multi-cloud posture briefly

### Phase 6 — Verification

- [x] **Step 17 — Build and test locally**
  - Run `cargo fmt --all`
  - Run `cargo clippy --all-targets --all-features -- -D warnings`
  - Run `cargo build`
  - Run `DYLD_LIBRARY_PATH=/opt/homebrew/lib cargo test` (full non-ignored suite)
  - Run `DYLD_LIBRARY_PATH=/opt/homebrew/lib cargo test --test circuit_breaker_proptest`
  - Verify all tests pass
  - LocalStack tests are **not** run here — they're gated behind `--ignored`
    and belong to the dedicated CI job. Manual verification via
    `cargo test -- --ignored` if desired.

- [x] **Step 18 — Record approval and close stage**
  - Present stage-completion message with the Request Changes / Continue options
  - On approval, log to `audit.md` and mark this stage complete in
    `aidlc-state.md`

---

## Story traceability

| Story | Implemented by step(s) |
|---|---|
| S3Storage fetches bytes from S3 | 7, 11 |
| S3Storage uses HeadObject for `exists` | 7, 11 |
| Custom endpoint for MinIO/R2 | 7, config in step 2 |
| No AWS SDK types outside s3.rs | 7 (module boundary enforcement) |
| Replace `todo!()` panics with typed errors | 4, 5, 7 |
| CircuitBreaker opens after threshold errors | 6, 12 |
| CircuitBreaker auto-closes after cooldown | 6, 12 |

## Estimated scope

- **18 numbered steps** across six phases
- **~6 new files**: `src/storage/local.rs`, `src/storage/circuit_breaker.rs`,
  `src/storage/s3.rs`, `tests/helpers/localstack.rs`,
  `tests/circuit_breaker_proptest.rs`, `tests/s3_integration.rs`,
  plus code summary doc
- **~7 modified files**: `src/storage/mod.rs`, `src/config.rs`, `src/lib.rs`,
  `src/main.rs`, `src/api/mod.rs`, `tests/config_test.rs`, `Cargo.toml`,
  `.github/workflows/ci.yml`, `README.md`
- **Net code delta** — estimated 1200–1500 lines including tests and
  `Cargo.lock` growth

---

## Plan approval

Per the AI-DLC rules, this plan must be explicitly approved before Part 2
(Generation) begins. On approval, Part 2 executes the steps in order,
marking each `- [ ]` → `- [x]` immediately after completion.

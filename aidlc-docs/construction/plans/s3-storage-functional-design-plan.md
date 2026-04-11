# Unit 2 — S3 Storage Backend: Functional Design Plan

**Unit**: S3 Storage Backend (Unit 2 of 7)
**Stage**: Functional Design (Part 1 — Planning)
**Depth**: Standard

---

## Context Loaded

- `aidlc-docs/inception/application-design/unit-of-work.md` — Unit 2 definition
- `aidlc-docs/inception/application-design/components.md` — C-02 Storage
- `aidlc-docs/inception/application-design/component-methods.md` — `StorageBackend`, `S3Storage`, `CircuitBreaker` method signatures
- `aidlc-docs/inception/application-design/unit-of-work-story-map.md` — Unit 2 story map
- `aidlc-docs/inception/requirements/requirements.md` — FR-01, FR-10, QA-02 (reliability), QA-04
- `src/config.rs` — existing `AppConfig` with `storage_backend`, `s3_bucket`, `s3_region`, `s3_endpoint`, `s3_prefix`
- `src/storage/mod.rs` — current `StorageBackend` trait, `LocalStorage`, `S3Storage` stub

## Scope

Unit 2 replaces the `todo!()` stub in `S3Storage` with a production implementation and introduces a `CircuitBreaker` for fault isolation.

**In scope:**

- `src/storage/s3.rs` — `S3Storage` with `get`, `exists`, `get_range` via `aws-sdk-s3`
- `src/storage/circuit_breaker.rs` — internal fault tracker
- `StorageBackend` trait extension: add `get_range(path, range) -> Result<Asset>`
- Additional `RENDITION_S3_*` and `RENDITION_LOCAL_TIMEOUT_MS` config fields for timeouts, connection pool, and circuit-breaker thresholds
- `testcontainers-rs` + LocalStack S3 integration tests

**Out of scope (other units):**

- HTTP `206 Partial Content` range handling at the request layer (Unit 4)
- Cache / embargo / preset wiring (Units 3, 5, 6)
- `/health/ready` endpoint itself (Unit 7 — Observability)
- IAM policy authoring (Infrastructure Design stage)

## Deliverables (Part 2 output — not produced yet)

Once this plan and the embedded questions are approved:

- `aidlc-docs/construction/s3-storage/functional-design/business-logic-model.md`
- `aidlc-docs/construction/s3-storage/functional-design/business-rules.md`
- `aidlc-docs/construction/s3-storage/functional-design/domain-entities.md`

## Plan Checklist

- [ ] Confirm unit scope (this document) with user
- [ ] Collect answers to all `[Answer]:` questions below
- [ ] Resolve any ambiguities in the answers with follow-ups
- [ ] Generate `business-logic-model.md` — `S3Storage.get` / `exists` / `get_range` control flow, `CircuitBreaker.call` state machine, error classification (retriable vs terminal)
- [ ] Generate `business-rules.md` — circuit-breaker state transitions, retry policy, error-to-`StorageError` mapping, key composition rules
- [ ] Generate `domain-entities.md` — `Asset`, `StorageError` enum, `CircuitBreaker` state (`Closed` / `Open` / `HalfOpen`), `S3Config` consumed fields
- [ ] Present completion message with 2 options (Request Changes / Continue)
- [ ] Record approval in `audit.md` and mark Functional Design complete in `aidlc-state.md`

---

## Clarification Questions

**Instructions:** Please fill in each `[Answer]:` line. Leave it as-is if you want me to take the default (marked ⭐). Write your own text if none of the options fit.

### Q1 — Error classification granularity

The `StorageBackend` trait currently returns `anyhow::Result<Asset>`. For S3, callers (Unit 7 health check, Unit 4 range handler) will want to distinguish "asset not found" from "S3 unreachable" from "circuit open" to map to correct HTTP status (404 vs 503).

A. ⭐ Introduce a typed `StorageError` enum in `src/storage/mod.rs` with variants `NotFound`, `Unavailable { source }`, `CircuitOpen`, `InvalidPath`, `Timeout`, `Other(anyhow::Error)` — break the `anyhow::Result<Asset>` signature to `Result<Asset, StorageError>`. Update `LocalStorage` too.
B. Keep `anyhow::Result<Asset>` and classify with `downcast_ref` at call sites using sentinel error types.
C. Keep `anyhow::Result<Asset>` for now, introduce `StorageError` in a later unit.

[Answer]: A

A### Q2 — Circuit breaker scope

Circuit breaker state is per-instance. Should the breaker be:

A. ⭐ Global to the `S3Storage` — one breaker for all keys (simplest, fault-isolates the whole S3 dependency).
B. Per-operation — separate breakers for `get` / `exists` / `get_range`.
C. Per-prefix — hash the key prefix into a breaker pool (most granular, most complex).

[Answer]: A

### Q3 — Retry policy for transient S3 errors

QA-02 says retries "MUST use exponential backoff (jitter applied)". For `get_range`/`get` calls inside the circuit breaker:

A. ⭐ 3 retries max, base delay 50 ms, cap 500 ms, full jitter — retry only on `5xx`, `Throttling*`, `RequestTimeout`, connection errors. Non-retriable: `404 NoSuchKey`, `403`, `400`.
B. Configurable (`RENDITION_S3_MAX_RETRIES`, `RENDITION_S3_RETRY_BASE_MS`) with the same defaults.
C. Delegate entirely to `aws-sdk-s3`'s built-in retry config (no custom retry loop).

[Answer]: B

### Q4 — Circuit breaker half-open behavior

When the cooldown expires after opening:

A. ⭐ Classic half-open: let one probe request through. If it succeeds → `Closed`; if it fails → back to `Open` with a fresh cooldown.
B. Count-based: let `N` probes through in parallel; if majority succeed → `Closed`.
C. Time-window recovery: after cooldown, immediately return to `Closed` and retest fresh failures.

[Answer]: A

### Q5 — `get_range` in trait vs `S3Storage` only

Component design specifies `get_range` on the `StorageBackend` trait with a default impl that fetches the full asset and slices it. `S3Storage` overrides to pass the native `Range` header (ADR-0018).

A. ⭐ Add `get_range` to the trait with the default (full-fetch-and-slice) impl, override in `S3Storage`. `LocalStorage` uses the default — acceptable since local reads are cheap.
B. Add `get_range` to the trait and require every backend to implement it explicitly (no default).
C. Keep `get_range` only on `S3Storage` — Unit 4 (range handler) only calls it when backend is S3.

[Answer]: A

### Q6 — Key composition rules

S3 object keys are composed as `{s3_prefix}{logical_path}`. Rules:

A. ⭐ `s3_prefix` is concatenated verbatim (user is responsible for trailing slash). `logical_path` is passed through after path-traversal check (same rule as `LocalStorage::safe_join`). Empty `s3_prefix` → bare `logical_path`.
B. Always normalise: strip leading `/` from `logical_path`, ensure `s3_prefix` ends with `/` if non-empty.
C. Reject any `logical_path` containing `..` or starting with `/` at the `S3Storage::get` entry point (defence in depth even though S3 has no filesystem semantics).

[Answer]: B

### Q7 — Content-Type source

`LocalStorage` infers `Asset.content_type` from the file extension via `content_type_from_ext`. S3 objects can carry a real `Content-Type` header from upload metadata.

A. ⭐ Prefer the S3-returned `Content-Type` header; fall back to `content_type_from_ext` if missing or `application/octet-stream`.
B. Always use `content_type_from_ext` (ignore S3 header) — guarantees identical behaviour across backends.
C. Always trust S3's `Content-Type` as-is, even if `application/octet-stream`.

[Answer]: A

### Q8 — `exists` semantics on `HeadObject` errors

`HeadObject` can return `404` (not found), `403` (forbidden — looks like not-found for unauthorised ARNs), or `5xx` (backend error).

A. ⭐ `exists` returns `bool` — `true` on 200, `false` on 404/403, **panics or logs-and-returns-false** on 5xx? Split into three cases: 200 → true, 404/403 → false, 5xx → propagate error (but `exists` current signature returns `bool`, so we need to change the return type or swallow).
B. Change trait signature: `exists(&self, path: &str) -> Result<bool, StorageError>` — propagate errors. This is a breaking change to `LocalStorage` too.
C. Keep `-> bool`, log-and-return-false on 5xx (loses the 5xx signal — bad for health checks).

[Answer]: B

### Q9 — New config fields to add

QA-02 mentions these envs that are NOT yet in `AppConfig`. Which should land in Unit 2 vs be deferred?

- `RENDITION_S3_MAX_CONNECTIONS` (default 100) — for `aws-sdk-s3` HTTP client
- `RENDITION_S3_TIMEOUT_MS` (default 5000)
- `RENDITION_S3_CB_THRESHOLD` (default 5)
- `RENDITION_S3_CB_COOLDOWN_SECONDS` (default 30)
- `RENDITION_LOCAL_TIMEOUT_MS` (default 2000) — LocalStorage read timeout
- `RENDITION_S3_MAX_RETRIES` (default 3) — if Q3=B

A. ⭐ Add all of the above in Unit 2 (extends the Unit 1 `AppConfig` struct). This keeps related configuration together and avoids revisiting `config.rs` in Unit 7.
B. Add only S3-specific fields. Defer `RENDITION_LOCAL_TIMEOUT_MS` to a later unit.
C. Add nothing to `AppConfig`; hardcode defaults in `S3Storage::new` (easier to rip out later).

[Answer]: A

### Q10 — Integration test scope for this unit

LocalStack tests will run via `testcontainers-rs`. CI needs Docker available.

A. ⭐ Write LocalStack integration tests (feature-gated `#[cfg(feature = "localstack-tests")]` or a `#[ignore]` attribute), run them locally + in a dedicated CI job with Docker enabled. Keep the main CI `cargo test` job fast by skipping them.
B. Run LocalStack tests in the main CI job (Docker is available on GitHub Actions ubuntu-latest runners). Accept the extra CI time.
C. Skip LocalStack tests entirely in Unit 2; rely on unit tests with a mocked `S3Client` trait. Add integration tests in a later unit.

[Answer]: A

### Q11 — MinIO / Cloudflare R2 compatibility

The spec supports `RENDITION_S3_ENDPOINT` for S3-compatible stores. Should we explicitly test against one non-AWS backend?

A. ⭐ Test only against LocalStack in Unit 2. Document that MinIO / R2 are supported by configuration, but don't block the unit on proving it.
B. Run the same integration test matrix against LocalStack *and* MinIO.
C. Defer this to a separate "compatibility" test suite in a later unit.

[Answer]: A

---

## Acceptance (from Unit 2 definition, for reference)

- `S3Storage::get` fetches correct bytes from a real LocalStack bucket
- `S3Storage::exists` uses `HeadObject` (no body download)
- `S3Storage::get_range` fetches only the requested byte slice (verified by checking response size against requested range width)
- Circuit breaker opens after `threshold` consecutive errors; auto-closes after `cooldown`; `is_healthy()` reflects state
- No `aws-sdk-s3` types visible outside `src/storage/s3.rs`

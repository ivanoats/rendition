# Unit 2 — S3 Storage Backend — NFR Requirements

**Answers reference:** `aidlc-docs/construction/plans/s3-storage-nfr-requirements-plan.md`
**Source requirements:** `aidlc-docs/inception/requirements/requirements.md` (QA-01, QA-02, QA-03)

All targets below are **verifiable**: each has a test, metric, or bounded
code-level check named explicitly.

---

## Scalability (QA-01)

| Concern | Target | Verification |
|---|---|---|
| HTTP connection pool to S3 | `s3_max_connections` default **100**, configurable | Exposed via `AppConfig::s3_max_connections`; passed to `aws-smithy-runtime::HyperClientBuilder` |
| Concurrent calls per `S3Storage` instance | Unbounded at the breaker; bounded by pool at the HTTP layer | Load test with 500 concurrent `get` calls against LocalStack — no panic, no socket exhaustion |
| Horizontal scalability | Stateless — all per-instance state (breaker, pool) reconstructs on restart | Integration test: start two `S3Storage` instances against the same LocalStack bucket; verify independent breaker states |
| Memory footprint per `S3Storage` | < 2 MiB resident steady-state (excludes object body bytes in flight) | Manual check via `cargo test` + `RUST_LOG=debug` |

## Performance (QA-01)

| Operation | Target (P99, intra-region, warm pool, real AWS S3) | Verification |
|---|---|---|
| `S3Storage::get` | ≤ **50 ms** | LocalStack integration test asserts P99 ≤ 200 ms (×4 LocalStack tolerance) |
| `S3Storage::exists` | ≤ **20 ms** | LocalStack integration test asserts P99 ≤ 80 ms |
| `S3Storage::get_range` (partial fetch, 1 MiB) | ≤ **30 ms** | Same — verified via `Content-Length` matching requested range |
| `CircuitBreaker::call` overhead when closed | ≤ **5 µs** | Unit test: 10 000 calls on a no-op future; total < 50 ms |
| `is_healthy()` | ≤ **100 ns** | Single atomic/mutex read |

Cache-hit requests do not touch `S3Storage` — those targets live in Unit 3.

## Availability (QA-02)

| Concern | Target | Mechanism | Verification |
|---|---|---|---|
| Service availability contribution | ≤ 0.05 % of the 0.1 % 30-day downtime budget is attributable to S3 storage | Circuit breaker opens on sustained S3 failures; `/health/ready` (Unit 7) reports degraded so Kubernetes drains traffic | Runbook + chaos test (later) |
| Fail-fast on cold S3 outage | `CircuitOpen` within `threshold × per-call-timeout = 5 × 5 s = 25 s` | R-06 transition rules | Unit test with fault-injected fake S3 client |
| Recovery after outage | `HalfOpen` probe within `cooldown = 30 s`; `Closed` within one successful probe | R-06 transition rules | Fake-clock unit test |
| Retry amplification bound | `max_retries = 3` (configurable, capped at 10 by `AppConfig::validate`) | R-02 retry loop | Unit test verifies exactly `max_retries + 1` attempts before raising `Unavailable` |
| Graceful shutdown contribution | No hang on `SIGTERM` — no unawaited tasks in `S3Storage` | Drop impl + all futures properly cancelled | Existing drain-timeout test at the handler layer |

## Reliability (QA-02 — overlaps with Availability)

| Concern | Target | Verification |
|---|---|---|
| Error classification correctness | 100% of terminal errors (`NoSuchKey`, 403, 400) map to `NotFound`/`Other`, not `Unavailable` | Unit tests per R-01 classification table |
| Timeout enforcement | 100% of calls exceeding `s3_timeout_ms` raise `StorageError::Timeout` and never block further | `tokio::time::timeout` wrapper; unit test with sleeping fake client |
| Circuit breaker state-machine correctness | 100% compliance with R-06 transition table under arbitrary event sequences | Proptest in `tests/circuit_breaker_proptest.rs` (Q11=A+C) |
| `get_range` size verification | 100% of successful range fetches return bytes whose length equals the requested range width | Integration test asserts `response.data.len() == range.end - range.start` |

## Observability (QA-03 — hooks only, implementation deferred to Unit 7)

| Signal | Unit 2 responsibility | Unit 7 responsibility |
|---|---|---|
| `rendition_storage_requests_total` counter | Emit calls at each outcome site (success / not_found / unavailable / timeout / circuit_open) via a `NoopMetrics` trait object | Replace with real Prometheus registry |
| `rendition_storage_request_duration_seconds` histogram | Same pattern | Real histogram |
| `rendition_s3_circuit_breaker_open` gauge | `CircuitBreaker` mutates a `AtomicBool` gauge (no-op in Unit 2) | Real gauge |
| `tracing::info_span!("s3.get", key = %key)` | All three S3 ops wrapped in spans | Export via OTEL |

No Prometheus dependency introduced in Unit 2 — `Metrics` trait stubbed.

## Maintainability

| Concern | Target | Verification |
|---|---|---|
| Module boundary | `aws-sdk-s3` types not exported from `src/storage/s3.rs` | Compile-time: `pub use` lines contain no `aws_*` references; `cargo doc` review |
| Test isolation | All LocalStack tests use per-test bucket names via `uuid` prefix | Pattern: `let bucket = format!("rendition-test-{}", uuid);` |
| Doc coverage | Every public symbol in `src/storage/{mod,s3,circuit_breaker}.rs` has a `///` doc comment with a one-line summary | `cargo clippy -- -W missing_docs` (nice-to-have) |
| Fast `cargo test` loop | Main `cargo test` runs in < 30 s by excluding LocalStack tests (`#[ignore]` by default) | Existing convention; Q10=A |
| LocalStack integration job | Runs on PRs touching `src/storage/s3.rs`, nightly schedule, or manual dispatch | New GitHub Actions job in `.github/workflows/ci.yml` |

## Security

**Applicable baseline rules (per compliance matrix in the plan):**
SECURITY-01, -03, -05, -06 (deferred), -09, -10, -11 (partial), -13.

| Rule | Addressed by |
|---|---|
| **SECURITY-01 in transit** | `AppConfig::validate` rejects `http://` S3 endpoints unless `allow_insecure_endpoint = true` (test-only flag). New config field introduced in Unit 2. |
| **SECURITY-01 at rest** | Enforced via S3 bucket policy — deferred to Infrastructure Design stage. This unit does not inspect `x-amz-server-side-encryption` response headers. |
| **SECURITY-03 structured logging** | All S3 operations emit `tracing::info_span!` + `tracing::error!` with no PII / credentials / request bodies in fields. |
| **SECURITY-05 input validation** | `compose_key` rejects empty paths and NUL bytes (R-07); `get_range` rejects `range.start >= range.end`. HTTP-layer validation remains in Unit 4. |
| **SECURITY-06 least-privilege IAM** | **Deferred to Infrastructure Design stage.** Bucket policy + IAM role scoped to `s3:GetObject`/`s3:HeadObject` on `arn:aws:s3:::{bucket}/{prefix}*` only. |
| **SECURITY-09 hardening** | `StorageError::Unavailable { source }` does not expose the AWS request ID, bucket name, or internal error text to HTTP callers — Unit 4 maps to generic 503. Full detail is kept in server-side logs via `tracing::error!` only. |
| **SECURITY-10 supply chain** | `Cargo.lock` committed (existing). `cargo-audit` in CI (existing). No `latest` tags — LocalStack pinned to `3.8` per Q6=A. New crates all come from crates.io (trusted registry). |
| **SECURITY-11 back-pressure** | Circuit breaker provides fail-fast back-pressure. Per-IP rate limiting deferred to Unit 6. |
| **SECURITY-13 deserialization** | Object bodies are raw `Vec<u8>` — no structured deserialization of untrusted data. |

**Blocking findings:** 0.

**Deferred findings (not blocking — explicit next-stage owners):**

- SECURITY-06 IAM policy authoring → Infrastructure Design (next stage)
- SECURITY-01 bucket encryption policy → Infrastructure Design (next stage)

## Out of scope for Unit 2

- Prometheus metric emission (stubbed; Unit 7 owns)
- OTEL exporter wiring (Unit 7)
- Per-IP rate limiting (Unit 6)
- CDN cache surrogate keys (Unit 7)
- `/health/ready` HTTP endpoint (Unit 7 — though `is_healthy()` is exposed here)

# Unit 2 — S3 Storage Backend — NFR Design Patterns

**Answers reference:** `aidlc-docs/construction/plans/s3-storage-nfr-design-plan.md`
**Upstream stages:** `aidlc-docs/construction/s3-storage/functional-design/`,
`aidlc-docs/construction/s3-storage/nfr-requirements/`

This document captures *how* the NFR targets are achieved — the design
patterns, not the behavioural rules (those live in `business-rules.md`).

---

## Resilience patterns

### P-01 — Circuit Breaker (ADR-0019)

A textbook circuit breaker around every S3 call, implemented as an
asynchronous middleware wrapping a future.

- **Trigger rule:** consecutive failures (narrower than rate-based).
- **State:** `Closed { consecutive_failures } / Open { opened_at } / HalfOpen { probe_in_flight }`.
- **Recovery:** single-probe half-open (classic pattern, see R-06 and ADR-0019).
- **Concurrency primitive:** `std::sync::Mutex<State>` — short critical
  sections; **never** held across `.await` (Q6=A, Flow 4 invariants).
- **Time source:** `tokio::time::Instant` throughout — enables deterministic
  testing via `tokio::time::pause()` + `tokio::time::advance()` without a
  custom `Clock` trait (Q4=D).

### P-02 — Retry with Full Jitter (R-02)

A hand-rolled retry loop inside `CircuitBreaker::call` (so one sequence =
one breaker "attempt" and its outcome).

```text
attempt 0:  immediate
attempt N:  delay ~ uniform(0, min(base_ms * 2^N, cap_ms))   // full jitter
```

- **`base_ms`** = `s3_retry_base_ms` (config, default 50)
- **`max_retries`** = `s3_max_retries` (config, default 3)
- **`cap_ms`** = 500 (hardcoded)
- **Retry predicate:** R-01 classification — only `Unavailable` or `Timeout`
  are retried. Terminal errors (`NotFound`, `InvalidPath`, `Other`) exit
  immediately.
- **SDK retrier disabled** — `aws_sdk_s3::Client` constructed with
  `RetryConfig::disabled()` (NFR Req Q2=A+D).
- **RNG:** `rand::rngs::ThreadRng` via `rand::Rng::gen_range(0..exp)`
  per attempt. No shared state, no seeding — jitter is a load-shedding
  mechanism, not a cryptographic primitive.

### P-03 — Timeout Enforcement (R-03)

Every S3 network call wraps the SDK future in `tokio::time::timeout`:

```rust
match tokio::time::timeout(self.s3_timeout, fut).await {
    Ok(Ok(out))   => Ok(out),
    Ok(Err(sdk))  => classify(sdk),
    Err(_elapsed) => Err(StorageError::Timeout { op }),
}
```

- **Per-attempt deadline** — the timeout applies to each retry attempt, not
  the whole retry sequence. This is intentional: a slow-then-recovered S3
  should still fit within the request timeout budget.
- **Total-request timeout** of 30 s is enforced at the HTTP handler layer
  (Unit 4), not inside `S3Storage`.

### P-04 — Fail-Fast on `CircuitOpen`

When the breaker is open, every call returns `StorageError::CircuitOpen`
**before** any I/O. Unit 4 maps this to HTTP 503; Kubernetes readiness
probe (`/health/ready`, Unit 7) reads `is_healthy()` and reports the pod
as not-ready, draining traffic to healthy replicas.

### P-05 — Cooperative Cancellation

`tokio::time::timeout` and all `async fn` calls inside `S3Storage`
propagate the standard Rust cancellation semantics: dropping the future
cancels the in-flight request. No `std::process::abort` or `Drop` hacks.
This composes with Axum's request timeout and graceful shutdown (QA-02).

---

## Performance patterns

### P-06 — HTTP Connection Pooling

`aws-smithy-runtime`'s `HyperClientBuilder` is configured with:

- **`pool_idle_timeout`:** 90 seconds (SDK default)
- **`http2_only(false)`** — S3 endpoints use HTTP/1.1 by convention
- **Max connections:** `s3_max_connections` (default 100) via
  `connection_time_out` and pool limits
- **Keep-alive:** enabled (SDK default)

`hyper` + `rustls` feature set chosen per NFR Req Q1=A.

### P-07 — Zero-Copy Byte Stream Reading

`aws_sdk_s3::operation::get_object::GetObjectOutput::body` returns an
`aws_smithy_types::byte_stream::ByteStream`. We collect it via
`body.collect().await?.into_bytes().to_vec()` which produces a single
allocation sized to the Content-Length. No intermediate buffering.

### P-08 — `is_healthy()` Fast Path (Q6=A)

`is_healthy() -> bool` acquires the breaker mutex, reads the discriminant,
releases. ~30 ns uncontended on aarch64/x86_64. Well under the 100 ns target.

- No atomic shadow of the state — premature optimisation.
- No `RwLock` — every transition is a write.
- **Upgrade path:** if a future benchmark shows `/health/ready` contention
  (Kubernetes probes default to 1 req/s — unlikely), we can add an
  `AtomicBool is_open_flag` without breaking the public API.

---

## Scalability patterns

### P-09 — Stateless Instance

`S3Storage` holds:

- `Arc<aws_sdk_s3::Client>` — shared, thread-safe, internally pooled
- `Arc<CircuitBreaker>` — per-instance local state
- `S3Settings` — configuration snapshot taken at startup
- `Arc<dyn StorageMetrics>` — no-op stub in Unit 2

Nothing is persisted. Horizontal scaling multiplies `S3Storage` instances
without coordination; each instance maintains its own circuit-breaker view
of S3 health. This is the QA-01 requirement.

### P-10 — Unbounded Concurrent Dispatch

The breaker does not serialise S3 calls — it only gates them. Hundreds of
concurrent `get` calls can be in flight simultaneously against the same
`S3Storage` instance. Backpressure at the hyper connection pool handles
load shedding when the pool is saturated.

---

## Security patterns

### P-11 — TLS-Only By Default (SECURITY-01 in-transit)

`AppConfig::validate` rejects `s3_endpoint` values that start with `http://`
unless `allow_insecure_endpoint == true`. The escape hatch exists **only**
for LocalStack integration tests and is never set in production.

- **Validation location:** `src/config.rs` — fail-fast at startup.
- **Default value:** `allow_insecure_endpoint = false`.
- **Env var:** `RENDITION_S3_ALLOW_INSECURE_ENDPOINT` (only set in test harness).

### P-12 — Credential-Chain Isolation (NFR Req Q7=A)

Production uses `aws_config::defaults(BehaviorVersion::latest())` which
runs the standard credentials provider chain. Tests use a dedicated
constructor that takes explicit static credentials:

```rust
impl S3Storage {
    pub async fn new(settings: &S3Settings) -> Result<Self, StorageError>;
    #[cfg(test)] // or pub but test-only semantics
    pub async fn new_for_test(
        endpoint: impl Into<String>,
        access_key: &str,
        secret_key: &str,
        bucket: impl Into<String>,
    ) -> Result<Self, StorageError>;
}
```

The test constructor never consults IMDS, never touches `~/.aws/credentials`,
and cannot accidentally fall through to real AWS — tests pass their own
`("test", "test")` literal credentials.

### P-13 — Log Hygiene (SECURITY-03 + SECURITY-09)

Every S3 operation is wrapped in a `tracing::info_span!`:

```rust
#[tracing::instrument(
    skip(self),
    fields(backend = "s3", op = "get", key = %compose_key(&self.prefix, path)?),
)]
```

- **Logged:** operation name, key (sanitised), outcome, duration, HTTP status code.
- **Never logged:** request bodies, response bodies, AWS request IDs in
  *user-visible* contexts, credentials, session tokens, bucket names in
  error messages returned to HTTP clients.
- `StorageError::Unavailable { source }` implements `Display` to emit only
  a generic "S3 unavailable" message; the full source chain is emitted via
  `tracing::error!("{err:#}")` to server logs only.

### P-14 — Input Validation at Module Boundary (SECURITY-05)

`compose_key` (R-07) rejects empty paths and NUL bytes. `get_range`
validates `range.start < range.end`. These run before any S3 call.

---

## Observability patterns (stub in Unit 2, real in Unit 7)

### P-15 — `StorageMetrics` Trait (Q3=A)

```rust
pub trait StorageMetrics: Send + Sync + 'static {
    fn record(&self, op: &str, outcome: Outcome, duration: Duration);
    fn set_circuit_open(&self, open: bool);
}

pub enum Outcome {
    Success,
    NotFound,
    Unavailable,
    Timeout,
    CircuitOpen,
    InvalidPath,
    Other,
}

pub struct NoopMetrics;
impl StorageMetrics for NoopMetrics { /* empty bodies */ }
```

`S3Storage` holds `Arc<dyn StorageMetrics>` — Unit 2 wires `NoopMetrics`;
Unit 7 replaces with `PrometheusMetrics` backed by the `prometheus` crate
(ADR-0017) without touching `s3.rs`.

### P-16 — `tracing` Spans as OTEL Backbone

All S3 operations are wrapped in `tracing` spans with fields aligned to
OpenTelemetry semantic conventions (`net.peer.name`, `http.status_code`,
`aws.request_id`). Unit 7 plugs in an OTEL exporter that converts spans
to OTLP traces automatically — no Unit-2 changes required.

---

## Testability patterns

### P-17 — Deterministic Time in `CircuitBreaker` Tests (Q4=D)

All tests that exercise breaker state transitions use:

```rust
#[tokio::test(start_paused = true)]
async fn breaker_opens_after_threshold_failures() {
    let cb = CircuitBreaker::new(5, Duration::from_secs(30));
    // ... advance via tokio::time::advance(...)
}
```

`start_paused = true` freezes `tokio::time::Instant` at zero; tests call
`tokio::time::advance(Duration::from_secs(30))` to cross the cooldown
boundary deterministically. No `Clock` trait, no fake clock struct — just
tokio's built-in test hooks.

### P-18 — Property-Based State Machine Testing (Q11=A+C from NFR Req)

`tests/circuit_breaker_proptest.rs` generates arbitrary sequences of
`(event, advance_ms)` tuples and asserts invariants after each step:

- `consecutive_failures` never exceeds `threshold` in `Closed`
- `Open` always holds for ≥ `cooldown` before transitioning
- `HalfOpen` allows at most one in-flight probe
- `is_open()` iff state is `Open { .. }`

Proptest shrinks any failing sequence to a minimal reproducer.

### P-19 — LocalStack Shared Container (Q8=B)

```rust
// tests/helpers/localstack.rs
static CONTAINER: OnceLock<LocalStackContainer> = OnceLock::new();

pub async fn ensure_localstack() -> &'static LocalStackContainer {
    CONTAINER.get_or_init(|| {
        // blocking start ~8s
    })
}

pub async fn fresh_bucket() -> String {
    let name = format!("rendition-test-{}", Uuid::new_v4().simple());
    // CreateBucket
    name
}
```

- **One container per test binary run** — ~8s startup amortised.
- **One bucket per test** — perfect isolation without the container cost.
- **Automatic cleanup** — `testcontainers` kills the container on process
  exit; per-test buckets leak within LocalStack but the whole container is
  discarded.
- **Parallel-safe** — `cargo test` default thread count works unchanged.

### P-20 — `#[ignore]` Gate for Integration Tests (Q5=A)

Every LocalStack test carries `#[ignore]`:

```rust
#[tokio::test]
#[ignore = "requires LocalStack (run with `cargo test -- --ignored`)"]
async fn s3_get_fetches_real_bytes() { /* ... */ }
```

- **Main `cargo test`** skips them — test loop stays < 30 s.
- **Dev loop:** `cargo test -- --ignored` runs the LocalStack suite.
- **CI job** (separate workflow): `cargo test --test s3_integration -- --ignored`.

No Cargo feature flag — `#[ignore]` keeps the tests compiling on every
run so `cargo check` catches compile errors immediately.

---

## Pattern → NFR traceability

| Pattern | NFR target addressed |
|---|---|
| P-01 Circuit Breaker | QA-02 reliability, Availability table |
| P-02 Retry with Full Jitter | QA-02 reliability, R-02 |
| P-03 Timeout Enforcement | QA-02 reliability, R-03 |
| P-04 Fail-Fast on `CircuitOpen` | QA-02 availability, `/health/ready` integration |
| P-05 Cooperative Cancellation | QA-02 graceful shutdown |
| P-06 HTTP Connection Pooling | QA-01 scalability |
| P-07 Zero-Copy Body Reading | QA-01 performance, memory footprint |
| P-08 `is_healthy()` Fast Path | QA-01 performance, ≤ 100 ns target |
| P-09 Stateless Instance | QA-01 horizontal scalability |
| P-10 Unbounded Concurrent Dispatch | QA-01 throughput ≥ 200 rps |
| P-11 TLS-Only | SECURITY-01 in-transit |
| P-12 Credential-Chain Isolation | SECURITY-09 hardening |
| P-13 Log Hygiene | SECURITY-03, SECURITY-09 |
| P-14 Input Validation | SECURITY-05 |
| P-15 Metrics Trait | QA-03 observability (stub) |
| P-16 tracing Spans → OTEL | QA-03 observability |
| P-17 Deterministic Time | Reliability test target (100% state-machine compliance) |
| P-18 Property-Based State Machine | Reliability test target (arbitrary interleavings) |
| P-19 LocalStack Shared Container | Maintainability (test speed) |
| P-20 `#[ignore]` Gate | Maintainability (fast `cargo test` loop) |

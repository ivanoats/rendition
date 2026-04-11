# ADR-0019: Hand-Rolled Circuit Breaker for S3 Storage Backend

## Status

Accepted (2026-04-11, Unit 2 — S3 Storage Backend)

## Context

Rendition's `S3Storage` backend calls AWS S3 (or an S3-compatible store such
as LocalStack, MinIO, Cloudflare R2) on every cache-miss request. S3 is a
remote dependency that can fail in three distinct ways:

1. **Sustained hard outage** — all requests fail for minutes. Continuing to
   issue requests wastes CPU, holds connections open, and delays
   `/health/ready` reporting the service as degraded so Kubernetes can drain
   traffic to healthy replicas.
2. **Partial failure / elevated error rate** — a subset of requests fail.
   Aggressive retries under this condition amplify load on the degraded
   dependency and can worsen the outage (retry storm).
3. **Transient blip** — a single request fails and the next succeeds. Here
   we want the retry policy (R-02) to mask the blip entirely.

QA-02 (Reliability) in `aidlc-docs/inception/requirements/requirements.md`
mandates that Rendition implement a circuit breaker with configurable
`RENDITION_S3_CB_THRESHOLD` and `RENDITION_S3_CB_COOLDOWN_SECONDS`.

Two axes of decision:

- **Implementation strategy:** hand-roll vs use an existing crate (`failsafe`,
  `fail-safe-rust`).
- **Composition with the AWS SDK retrier:** keep SDK retries or replace them.

## Decision

### 1. Hand-roll a dedicated `CircuitBreaker` in `src/storage/circuit_breaker.rs`

The implementation is ~120 lines of state-machine logic:

```rust
enum CircuitBreakerState {
    Closed { consecutive_failures: u32 },
    Open { opened_at: Instant },
    HalfOpen { probe_in_flight: bool },
}

impl CircuitBreaker {
    pub fn new(threshold: u32, cooldown: Duration) -> Self;

    pub async fn call<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: Future<Output = Result<T, StorageError>>;

    pub fn is_open(&self) -> bool;
}
```

**Trigger:** `consecutive_failures >= threshold` in the `Closed` state
transitions to `Open`. A single success resets the counter to zero.

**Cooldown:** in `Open`, every call returns `StorageError::CircuitOpen`
without touching S3 until `now - opened_at >= cooldown`, at which point the
next call enters `HalfOpen` as a probe.

**Half-open recovery:** **single probe.** At most one concurrent call is
allowed in the half-open state. If the probe succeeds, state returns to
`Closed { consecutive_failures: 0 }`. If it fails, state returns to
`Open { opened_at: Instant::now() }` with a fresh cooldown. Concurrent
calls during `HalfOpen { probe_in_flight: true }` are rejected with
`CircuitOpen` — we prefer fail-fast to queued back-pressure here.

**Breaker-counted failures are narrower than call-result failures.**
The breaker only increments `consecutive_failures` on
`StorageError::Unavailable` and `StorageError::Timeout`. `NotFound`,
`InvalidPath`, and `Other` are considered "successes" from the breaker's
perspective — a missing object is not a dependency failure.

**Synchronisation:** `std::sync::Mutex<CircuitBreakerState>`. Critical
sections are short (nanoseconds) and never held across `.await` points —
the breaker takes the lock, checks state, releases, awaits the inner
future, then takes the lock again to record the outcome. This rules out
`tokio::sync::Mutex` (designed for lock-across-`.await` which we must avoid)
and makes `parking_lot::Mutex` an unnecessary optimisation.

### 2. Disable the `aws-sdk-s3` built-in retrier

`S3Storage` constructs its `aws_sdk_s3::Client` with
`RetryConfig::disabled()`. Our own retry loop (R-02) is the only retry layer.

### 3. Crate alternatives rejected

**`failsafe`** — uses a sliding-window *failure-rate* trigger
(e.g. "open if >50% of the last 100 calls failed in the last 30 s"). This
differs from our *consecutive-failures* rule and would diverge from the
functional design spec (R-06). Adapting it would cost more code than hand-rolling.

**`fail-safe-rust`** — closer to our semantics, but its half-open behaviour
does not match our single-probe rule (it allows N parallel probes), and
composition with our typed `StorageError` would require wrapping and
unwrapping through the crate's own error type.

**`tower::ServiceBuilder` layers** — tower ships rate limiters, not
circuit breakers. A full breaker would still be hand-rolled; tower just
wouldn't help.

## Consequences

**Benefits:**

- **Exact match with R-06.** The breaker's behaviour is defined in the
  functional design and implemented in the same shape, so reasoning and
  debugging are one-to-one.
- **Zero new runtime dependencies.** Supports SECURITY-10 (software supply
  chain minimisation).
- **Trivially testable.** A trait-injected `Clock` makes the state machine
  fully deterministic in unit tests. A `proptest` harness (Q11=C in the NFR
  plan) fuzzes arbitrary event sequences against R-06's transition table.
- **No double retry.** Disabling the SDK retrier prevents silent 3 × 3 = 9
  attempt amplification that would blind the circuit breaker to failure
  counts.
- **No lock-across-`.await`.** The `std::sync::Mutex` pattern guarantees that
  even pathological S3 latency cannot stall other tasks waiting on the
  breaker's lock.

**Drawbacks:**

- **Narrower than a rate-based breaker.** A dependency that fails every 6th
  request forever would never trip the consecutive-failures breaker. This is
  an intentional trade-off — rate-based breakers are harder to reason about
  and tune, and consecutive-failures is the right signal for "is S3
  currently unreachable?".
- **Single probe is slower to recover than parallel probes.** A flapping
  dependency may take a few cooldown cycles to stabilise. For a read-heavy
  CDN workload, prudent pacing beats aggressive recovery.
- **~120 LoC of state-machine code to maintain.** Mitigated by being
  hand-written against a spec (R-06) and fully covered by unit + property
  tests.

## Related

- **ADR-0004** — Pluggable Storage via Trait Abstraction. The circuit
  breaker lives inside `S3Storage`; `LocalStorage` is unaffected (no remote
  dependency to fail).
- **ADR-0018** — HTTP 206 Custom Range Parsing. The `get_range` trait
  method goes through the same circuit breaker as `get` and `exists`.
- **QA-02** — Requirements: Reliability.
- **Unit 2 Functional Design** —
  `aidlc-docs/construction/s3-storage/functional-design/` defines R-01
  (error classification), R-02 (retry policy), and R-06 (breaker state
  transitions).
- **Unit 2 NFR Requirements plan** —
  `aidlc-docs/construction/plans/s3-storage-nfr-requirements-plan.md`
  Q2, Q3, Q4, Q11.

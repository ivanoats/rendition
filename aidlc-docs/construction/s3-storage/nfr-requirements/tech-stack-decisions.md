# Unit 2 — S3 Storage Backend — Tech Stack Decisions

**Answers reference:** `aidlc-docs/construction/plans/s3-storage-nfr-requirements-plan.md`
**ADR updates:** ADR-0004 (updated), ADR-0019 (new — S3 circuit breaker)

---

## New crates added in Unit 2

### `aws-config = "1"`

**Purpose:** Resolves AWS region, endpoint, credentials, and retry configuration
for the SDK. Provides the default credentials-provider chain (env vars →
shared profile → IMDSv2 → ECS task role → SSO).

**Decision source:** Unit 2 definition — "replace `todo!()` with `aws-sdk-s3`".
**Alternative considered:** Building a minimal S3 REST client by hand. Rejected —
AWS signing (SigV4), retryable error classification, and the credential chain
represent tens of thousands of lines of battle-tested code we would
reimplement badly.

### `aws-sdk-s3 = "1"`

**Purpose:** Typed client for S3 `GetObject`, `HeadObject`, and range fetches.

**Decision source:** Unit 2 definition.
**Alternative considered:** `rusoto_s3`. Rejected — Rusoto is abandoned as of 2023.
`aws-sdk-rust` is the official AWS-maintained successor.

**Module boundary:** Every `aws_sdk_s3::*` import is confined to
`src/storage/s3.rs`. No AWS SDK type (`Client`, `GetObjectOutput`, `SdkError`,
`ByteStream`) is exported to the rest of the codebase. This is a **compile-time
invariant** enforced by the `pub use` lines in `src/storage/mod.rs`.

### `aws-smithy-types = "1"`

**Purpose:** Provides `ByteStream` for reading response bodies efficiently into
`Vec<u8>`. Imported only in `src/storage/s3.rs`.

**Decision source:** Required transitively by `aws-sdk-s3` but re-exported for
explicit use in the body-reading path.

### HTTP/TLS stack: `hyper` + `rustls` (Q1=A)

**Purpose:** Underlying HTTP/1.1 client for the AWS SDK. `rustls` provides
TLS termination using a pure-Rust implementation.

**Decision source:** NFR Plan Q1. Chosen over `native-tls` (OpenSSL) to:

1. Avoid the `libssl-dev` system dependency in CI and Docker images.
2. Keep the build self-contained and `musl`-friendly for potential static
   binaries.
3. Match the ecosystem default (`reqwest`, `sqlx`, `rustls` by default now).

**Feature flags:**

- `aws-smithy-runtime` with `client-hyper`, `connector-rustls`, `rustls` features
- `aws-config` with `rustls` feature

**Alternative rejected:** `native-tls`. It's smaller and uses the system cert
store automatically, but dragging OpenSSL into the build breaks `musl`
cross-compilation and requires extra apt packages in the CI image.

### `rand = "0.9"`

**Purpose:** Uniform random number generation for the retry loop's full-jitter
backoff (R-02).

**Decision source:** NFR Plan Q2 (hand-rolled retry loop). We need
`rand::Rng::gen_range(0..exp_backoff)` per attempt to compute a jittered delay.

**Alternative considered:** `fastrand` — marginally smaller and faster, but
lacks the same ubiquity and documentation. `rand` is already transitively
present in the dependency graph via many crates, so adding it as a direct
dependency has effectively zero incremental cost.

### `tokio` features — `time`, `sync` (already present)

**Purpose:** `tokio::time::timeout` for the per-call deadline (R-03),
`tokio::time::sleep` for the retry backoff delay.

No new crate — only ensures the existing `tokio = { version = "1", features = ["full"] }`
continues to include these. `full` already does.

---

## New dev dependencies

### `testcontainers-modules = "0.x"` with `localstack` feature (Q5=B)

**Purpose:** Spawn a LocalStack Docker container in integration tests via
`testcontainers-modules::localstack::LocalStack`, which handles:

- `SERVICES=s3` environment variable
- LocalStack-specific endpoint discovery
- Health probe polling until ready
- Container cleanup on test process exit

**Decision source:** NFR Plan Q5.
**Alternative considered:** Raw `testcontainers::GenericImage`. Rejected —
requires hand-wiring every LocalStack env var and maintaining it as LocalStack
evolves. The preset in `testcontainers-modules` absorbs that churn for us.

**Pinning (Q6=A):** The LocalStack image tag is pinned to **`3.8`** (current
stable major). The pin is enforced in the test setup code, not just in the
crate metadata, so a dependency upgrade cannot silently change the image.

**Scope:** This crate is a `dev-dependency` only — it never appears in release
builds.

### `proptest` (existing — reused for circuit breaker testing)

**Purpose:** Property-based testing for the `CircuitBreaker` state machine
under arbitrary event sequences (Q11=A+C).

**Location:** New test file `tests/circuit_breaker_proptest.rs`. No Cargo.toml
change required — `proptest` is already a dev-dep from Unit 1.

---

## Design decisions that do *not* add crates

These were considered and deliberately rejected in favour of hand-rolling.

### Retry loop implementation (Q2=A+D)

**Rejected crates:** `backoff`, `tokio-retry`.

**Rationale:** Both crates implement a `Result<T, E>`-based retry loop whose
"failure" concept is broader than ours. R-01 distinguishes retriable
(`Unavailable`/`Timeout`) from terminal (`NotFound`/`Other`) errors, and the
circuit breaker counts only the former. Fitting either crate's API around our
narrower classification costs more code than hand-rolling the 30-line loop.

**Q2 part D:** we also explicitly **disable** the `aws-sdk-s3` built-in
retrier by constructing the `Client` with
`RetryConfig::disabled()`. Keeping both the SDK retrier and our own active
would silently multiply real failures (3 × 3 = 9 attempts per call) and hide
failure counts from our circuit breaker.

### Circuit breaker implementation (Q3=A)

**Rejected crates:** `failsafe`, `fail-safe-rust`.

**Rationale:** `failsafe` uses a sliding-window rate-based trigger, which
differs from our `consecutive_failures`-based rule (R-06). Adapting it would
be more code and more divergence from the functional design than writing the
~120-line state machine ourselves. The hand-rolled version is also trivially
testable via a trait-injected clock (Q11=A).

### Synchronisation primitive for `CircuitBreaker` (Q4=A)

**Chosen:** `std::sync::Mutex<State>`.

**Rejected:** `tokio::sync::Mutex` (wrong tool — its lock-across-`.await`
capability is exactly what we must avoid, per Flow 4), `parking_lot::Mutex`
(faster but not justified when the critical section is a handful of enum
branches), `std::sync::RwLock` (every call mutates, so no reader benefit).

---

## ADR impact

| ADR | Change |
|---|---|
| **0004** Pluggable Storage | **Updated.** Trait signature evolved to `Result<T, StorageError>`, `get_range` added (cross-ref ADR-0018). Revision note added at the top. |
| **0018** HTTP 206 Range | No change — already specifies `get_range` on the trait with default full-fetch-and-slice impl. Unit 2 implements the S3 override. |
| **0019** S3 Circuit Breaker (new) | **New ADR.** Documents the consecutive-failures trigger, single-probe half-open, hand-rolled vs crate trade-off, and the decision to disable the SDK retrier. |

Other ADRs (0001, 0005, 0014) are unaffected.

---

## Summary table — all new dependencies

| Crate | Runtime/dev | Version | Purpose |
|---|---|---|---|
| `aws-config` | runtime | `1` | Credentials chain + region loading |
| `aws-sdk-s3` | runtime | `1` | S3 `GetObject`/`HeadObject` client |
| `aws-smithy-types` | runtime | `1` | `ByteStream` body reading |
| `rand` | runtime | `0.9` | Full-jitter RNG |
| `testcontainers-modules` (`localstack` feature) | dev | `0.x` | LocalStack in integration tests |

No new dependencies are introduced for the circuit breaker or the retry loop.

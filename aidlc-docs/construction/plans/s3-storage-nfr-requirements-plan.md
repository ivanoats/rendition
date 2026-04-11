# Unit 2 — S3 Storage Backend: NFR Requirements Plan

**Unit:** S3 Storage Backend (Unit 2 of 7)
**Stage:** NFR Requirements (Part 1 — Planning)
**Depth:** Standard
**Security extension:** Enabled (per `aidlc-state.md`)

---

## Context Loaded

- `aidlc-docs/construction/s3-storage/functional-design/*` — domain entities, rules, flows
- `aidlc-docs/inception/requirements/requirements.md` — QA-01 (Scalability), QA-02 (Reliability), QA-03 (Observability)
- `.aidlc-rule-details/extensions/security/baseline/security-baseline.md` — SECURITY-01..13
- `aidlc-docs/construction/config/nfr-requirements/*` — Unit 1 NFR format reference

## Scope

This stage pins tech-stack decisions and non-functional targets for Unit 2.
Several NFRs are already fixed by requirements.md and do not need re-asking:

**Fixed by QA-01 / QA-02 / QA-03 (no question):**

- Stateless service (QA-01)
- Throughput ≥ 1000 rps cache-hit, ≥ 200 rps cached-transform per 4-core instance (QA-01)
- P99 latency ≤ 200 ms for cache-hit (QA-01)
- Availability 99.9% 30-day rolling (QA-02)
- Graceful shutdown, 30 s drain (QA-02)
- Retries with full jitter, circuit breaker (QA-02 + Functional Design R-02/R-06)
- S3 timeout 5 s default, request timeout 30 s (QA-02)
- Structured JSON logging to stdout (QA-03)
- Prometheus metrics (QA-03)
- OTEL tracing (QA-03)

**Open questions for this stage** — decisions the functional design does not
pin but that directly affect tech-stack choices and code shape in Unit 2.

## Deliverables (Part 2 output — not produced yet)

- `aidlc-docs/construction/s3-storage/nfr-requirements/nfr-requirements.md`
- `aidlc-docs/construction/s3-storage/nfr-requirements/tech-stack-decisions.md`

## Plan Checklist

- [ ] Confirm scope (this document) with user
- [ ] Collect answers to all `[Answer]:` questions below
- [ ] Resolve any ambiguities with follow-ups
- [ ] Run the security-baseline compliance pass (SECURITY-01, -03, -06, -09, -10)
- [ ] Generate `nfr-requirements.md` — per-NFR targets and verification criteria for Unit 2
- [ ] Generate `tech-stack-decisions.md` — new crates added in Unit 2 with rationale
- [ ] Present stage-completion message with security-findings section
- [ ] Record approval in `audit.md` and mark NFR Requirements complete

---

## Clarification Questions

### Q1 — AWS SDK HTTP client & TLS backend

`aws-sdk-s3` lets you pick the underlying HTTP/TLS stack. This choice ripples
into build time, cross-compile compatibility, and CI container images.

| Option | Pros | Cons |
|---|---|---|
| A. ⭐ `aws-smithy-runtime` default (`hyper` + `rustls`) | Pure Rust. Fast `cargo build` from scratch. No OpenSSL system dep. Easy `musl` cross-compile. | Slightly larger binary (ring + webpki). |
| B. `hyper` + `native-tls` (OpenSSL) | Smaller binary. Uses system cert store automatically. | Requires `libssl-dev` on Ubuntu CI. Breaks `musl` without `vendored` feature. |
| C. Custom connector (explicit `HyperClientBuilder` tuning) | Fine-grained control over connect/read timeouts, pool idle, keep-alive. | More code to maintain. Tunes are rarely needed below 10k rps. |

**Recommended: A.** The pure-Rust `rustls` path is already used elsewhere in
the Rust ecosystem (`reqwest` default, `sqlx` default). It keeps the Dockerfile
minimal (no `libssl-dev`), avoids `musl` headaches if we later build a static
binary, and removes one category of CI surprises. The minor binary size cost
is irrelevant at CDN scale.

[Answer]: A (take recommendation)

---

### Q2 — Retry implementation: hand-roll vs crate

Functional Design R-02 specifies the retry policy (full jitter, configurable
`max_retries` + `base_ms`, cap 500 ms). The question is how to *implement* it.

| Option | LoC in `src/storage/s3.rs` | Flexibility | New dep |
|---|---|---|---|
| A. ⭐ Hand-rolled loop (~30 LoC) with `rand::Rng::gen_range` | Small — all logic visible in one file | Total control over classify + sleep | `rand` (tiny, ubiquitous) |
| B. `backoff` crate (`ExponentialBackoffBuilder`) | ~15 LoC | Configurable but opinionated — no native "full jitter" mode | `backoff = "0.4"` |
| C. `tokio-retry` crate (`Retry::spawn`) | ~10 LoC | `ExponentialBackoff` + `jitter` helpers | `tokio-retry = "0.3"` |
| D. Disable SDK retry and use `aws-smithy-types::retry::RetryConfig::disabled()` — we built the loop ourselves anyway | N/A | N/A | N/A (removes a behavior, not a crate) |

**Recommended: A + D.** Hand-rolling gives us the **exact** classification from
R-01 (what counts as "retry failure" for the breaker is narrower than what the
SDK or `backoff` crate would count). Crates B and C fight our circuit-breaker
semantics — both assume a bare `Result<T, E>` loop with no side-channel for
"this failure counts toward a separate state machine". D is essential: if we
keep the SDK's default retrier *and* add our own, every real failure
multiplies into 3×3 = 9 attempts silently.

[Answer]: A + D (take recommendation)

---

### Q3 — Circuit breaker implementation: hand-roll vs crate

R-06 specifies the state machine. How to implement the state transitions?

| Option | Matches R-06 out of the box? | Dep weight |
|---|---|---|
| A. ⭐ Hand-rolled `CircuitBreaker` struct in `src/storage/circuit_breaker.rs` (~120 LoC) | Yes — we defined R-06 to match what we'll write | Zero new deps |
| B. `failsafe` crate (a Rust port of Netflix Hystrix patterns) | Close — uses a sliding-window failure rate instead of consecutive failures | `failsafe = "1"` + its deps |
| C. `tower::limit::RateLimitLayer` or `tower::ServiceBuilder` circuit-breaker middleware | No — tower's built-ins don't include a full breaker; they're rate limiters | `tower-async` variant or custom layer |
| D. `fail-safe-rust` | Partial — doesn't model HalfOpen with single-probe the same way | New dep |

**Recommended: A.** R-06 is a ~100-line state machine when expressed cleanly;
the hand-rolled version is easier to unit-test (just a `Mutex<State>` and
methods, no async machinery) than adapting a crate whose opinions differ from
ours. Zero new deps also benefits SECURITY-10 (supply chain minimisation).

[Answer]: A (take recommendation)

---

### Q4 — Synchronisation primitive for `CircuitBreaker` state

The breaker state is shared across concurrent Tokio tasks. Flow 4 explicitly
says we must not hold the lock across `.await` points (otherwise blocked tasks
pile up).

| Option | Holds across `.await`? | Fairness / perf | Stdlib? |
|---|---|---|---|
| A. ⭐ `std::sync::Mutex<State>` — take the lock before `op().await` only to read/mutate state | No (we release before awaiting) | Standard OS mutex | Yes — no new dep |
| B. `parking_lot::Mutex<State>` | No | Faster uncontended, smaller | Adds `parking_lot` |
| C. `tokio::sync::Mutex<State>` | Can | Async-aware; higher overhead per lock | Already a dep (tokio) |
| D. `std::sync::RwLock<State>` | No | Reader-heavy workloads benefit | Stdlib |

**Recommended: A.** The breaker protocol reads and mutates state in short
critical sections (nanoseconds); `std::sync::Mutex` is the textbook answer.
`parking_lot` would be faster but the lock isn't hot enough to justify the
dep. `tokio::sync::Mutex` is the wrong tool here — its whole point is
allowing lock holds across `.await`, which we've explicitly designed to avoid.
`RwLock` doesn't help because every call mutates.

[Answer]: A (take recommendation)

---

### Q5 — LocalStack testcontainer library choice

The unit definition says "`testcontainers-rs` + LocalStack". The ecosystem
has two active crates.

| Option | API quality | LocalStack support | Active maintenance |
|---|---|---|---|
| A. `testcontainers = "0.x"` + raw `GenericImage::new("localstack/localstack", tag)` | Lower-level; manual env var setup | Works, but you wire up everything | Yes |
| B. ⭐ `testcontainers-modules = "0.x"` with the `localstack` feature (`testcontainers_modules::localstack`) | Higher-level; preset wrappers | First-class | Yes — actively maintained |

**Recommended: B.** The `testcontainers-modules` crate exposes a preset
`LocalStack` type that wires `SERVICES=s3`, `DEBUG=1`, the right healthcheck,
and the AWS SDK endpoint override. Less boilerplate, less drift when the
LocalStack image changes its env var contract.

[Answer]: A (take recommendation)

---

### Q6 — LocalStack image pinning

SECURITY-10 ("no `latest` tags in production Dockerfiles or CI configurations")
applies here for the test container. We can still use `latest` if we document
the exception, or pin.

| Option | Reproducibility | CI drift risk | Upgrade cadence |
|---|---|---|---|
| A. ⭐ Pin `localstack/localstack:3.8` (current stable major) | High | Zero (until we upgrade) | Manual |
| B. Pin by digest `localstack/localstack@sha256:…` | Highest | Zero | Manual + requires digest update |
| C. `localstack/localstack:latest` | Lowest | High — tests can break silently on any LocalStack release | Automatic |

**Recommended: A.** Tag-pinning to a major version is the ecosystem norm for
test containers. Digest-pinning (B) is stricter but forces a chore whenever
LocalStack publishes a patch we want. `latest` (C) violates SECURITY-10 and
invites flaky CI.

[Answer]: A (take recommendation)

---

### Q7 — AWS credentials handling in tests vs production

`aws-sdk-s3`'s default credentials provider chain checks env → profile → IMDS
→ ECS. LocalStack accepts any credentials. Tests must not touch real AWS.

| Option | Prod credentials | Test safety | Code path |
|---|---|---|---|
| A. ⭐ Prod uses `DefaultCredentialsChain`; tests pass explicit `Credentials::new("test", "test", None, None, "static")` to a test-only `S3Storage` constructor | Yes — follows AWS best practice | High — explicit static creds, no IMDS call | Two constructors |
| B. Same, but tests set env vars `AWS_ACCESS_KEY_ID=test` etc. before spawning LocalStack | Yes | Medium — env leakage across parallel tests | One constructor |
| C. Prod and tests both take an explicit `Credentials` arg — no default chain | No — production would need env var wiring that the chain already handles for free | High | Single path but more config |

**Recommended: A.** Dedicated test constructor (`S3Storage::new_for_test(endpoint, creds)`)
keeps the production `new(&S3Config)` path clean and lets it use the SDK's
default chain (which handles EC2 IRSA, ECS task roles, and local `~/.aws/credentials`
for free). Test code is explicit about mock creds; no env var race conditions.

[Answer]: A (take recommendation)

---

### Q8 — HTTPS enforcement (SECURITY-01 — encryption in transit)

SECURITY-01 is a **blocking** baseline rule. We must enforce TLS for S3
traffic in production.

| Option | Compliance | LocalStack compatibility |
|---|---|---|
| A. ⭐ Production `AppConfig::validate()` rejects `s3_endpoint` values starting with `http://` unless `RENDITION_ALLOW_INSECURE_ENDPOINT=true` (for LocalStack only). Tests set the override; prod never does. | Compliant — documented exception | Works |
| B. Production validates the scheme is `https://`; LocalStack tests use a separate code path that bypasses validation | Compliant | Works (split path) |
| C. Allow `http://` endpoints unconditionally, document that operators must configure HTTPS | **Non-compliant with SECURITY-01** — a bug could leak real traffic over HTTP | Works |

**Recommended: A.** One config field (`allow_insecure_endpoint: bool`, default
false) gates the escape hatch. Documented in the README as a test-only toggle.
SECURITY-01's "documented exception" escape applies because LocalStack
specifically requires HTTP for its in-container S3 endpoint. Production
misconfiguration is impossible without explicitly setting the flag.

[Answer]: A (take recommendation)

---

### Q9 — Server-side encryption (SECURITY-01 — encryption at rest)

S3 can enforce SSE at the bucket level (bucket policy). Rendition's read path
has two possible postures:

| Option | Policy | Code in Unit 2 |
|---|---|---|
| A. ⭐ Trust the bucket policy to enforce SSE; document the required bucket setting in the Infrastructure Design stage. Rendition reads whatever S3 returns and does not inspect `x-amz-server-side-encryption`. | Policy enforced out-of-band | None |
| B. Rendition inspects the `x-amz-server-side-encryption` response header and fails the request if absent, to defend against misconfigured buckets. | Policy enforced in-band | ~10 LoC in `fetch_get` |
| C. Rendition requires `RENDITION_S3_REQUIRE_SSE=true` to enable B. | Opt-in in-band | ~15 LoC |

**Recommended: A.** SECURITY-01's verification item is "Object storage enforces
encryption at rest and rejects non-TLS requests via policy" — *via policy*,
not via application-level checks. The bucket's `BucketEncryption` setting is
the enforcement point; Infrastructure Design stage will specify it. Inspecting
headers in the hot path (B/C) adds complexity for a check that a single
`aws s3api put-bucket-encryption` command prevents entirely.

[Answer]: A (take recommendation)

---

### Q10 — Throughput and latency targets specifically for S3 `get`

QA-01 gives the service-wide target (1000 rps cache-hit, 200 rps cached-transform).
The storage layer's contribution:

| Measurement | Cache-hit (not called) | Cache-miss (hot path) |
|---|---|---|
| Calls per 4-core instance | 0 | ≤ 200 rps target |
| P99 `S3Storage::get` latency target (intra-region, warm pool) | N/A | **≤ 50 ms** |
| P99 `S3Storage::exists` latency | N/A | **≤ 20 ms** |

| Option | Targets | How we verify |
|---|---|---|
| A. ⭐ Adopt the table above. Verify via integration test that measures LocalStack round trip under light load and asserts P99 < target × 4 (LocalStack is slower than real S3). | Concrete | Integration test + manual prod benchmarking later |
| B. No unit-level latency target — only the service-wide QA-01 target applies | Vague | N/A |
| C. Require a throughput bench in Unit 2 via `criterion` | Strong | More work now |

**Recommended: A.** Giving the unit its own budget lets Unit 4 (request handler)
know what it can expect. LocalStack isn't production-speed so we assert a
looser multiple (×4); a real-S3 benchmark can happen in Unit 7 when we wire
the observability pipeline.

[Answer]: A (take recommendation)

---

### Q11 — Circuit breaker availability target

The breaker itself is an availability-protection mechanism; what's its own
correctness target?

| Option | Target | Verification |
|---|---|---|
| A. ⭐ 100% of opens happen within `threshold + 1` consecutive failures and 100% of closes happen within `cooldown + tolerance` seconds (±1s). Verified by unit tests with a fake clock. | Deterministic | Unit test with a trait-injected clock |
| B. Best-effort; accept that wall-clock drift can cause ±5s variance | Loose | None |
| C. Use a property test (proptest) to hammer the state machine with random event sequences | Rigorous | New proptest in `tests/circuit_breaker_proptest.rs` |

**Recommended: A + C.** A defines the contract; C proves the contract holds
under arbitrary interleavings. Both are cheap — the breaker is pure state
machine code. Property testing (C) aligns with your existing enabled
extension (property-based testing per `aidlc-state.md`).

[Answer]: A + C (take recommendation)

---

### Q12 — New crate additions for Unit 2 — approval gate

Consolidated list of new dependencies implied by the recommendations above:

| Crate | Purpose | Size / quality | Alternative considered |
|---|---|---|---|
| `aws-config = "1"` | AWS SDK credentials chain + region loading | First-party, pinned with SDK | None |
| `aws-sdk-s3 = "1"` | The S3 client | First-party | `rusoto` (abandoned) |
| `aws-smithy-types = "1"` | Byte stream types needed to read S3 object body | First-party | N/A |
| `rand = "0.9"` | Full-jitter backoff | Ubiquitous | `fastrand` — slightly smaller, no cryptographic guarantees (fine for jitter) |
| `tokio` | Already a dep — reuse its `time::timeout` and `spawn_blocking` | — | — |
| `tempfile` (dev) | Already a dep | — | — |
| `testcontainers-modules = "0.x"` (dev) | LocalStack test container | Active | Hand-roll with `testcontainers` |
| `proptest` (dev) | Already a dep — for Q11=C | — | — |

| Option | |
|---|---|
| A. ⭐ Approve the full list above | |
| B. Substitute `fastrand` for `rand` | — marginal binary size save, loses the ecosystem standard |
| C. Defer `testcontainers-modules` until Infrastructure Design stage | — forces a second review pass |
| D. Propose different crates in your answer | |

**Recommended: A.** The four AWS crates are non-negotiable (they *are* the
S3 client). `rand` is already transitively present via `axum`/`tower` stacks
and doesn't add to the effective dependency graph. `testcontainers-modules`
is the single new test-only dep and is well justified by Q5=B.

[Answer]: A (take recommendation)

---

## Security Baseline Compliance Checkpoint

Before presenting stage-completion, the generated `nfr-requirements.md` will
include a per-rule compliance summary:

| Rule | Applies to Unit 2? | Addressed by |
|---|---|---|
| SECURITY-01 Encryption at rest + in transit | **Yes** | Q8 (TLS enforcement) + Q9 (SSE via bucket policy) |
| SECURITY-02 Access logging on network intermediaries | N/A | No LB / API gateway in this unit |
| SECURITY-03 Application-level structured logging | **Yes** | Existing `tracing_subscriber` from Unit 1; S3 calls get `tracing::info_span!` |
| SECURITY-04 HTTP security headers | N/A | No HTML endpoint in this unit |
| SECURITY-05 Input validation | **Yes** | `compose_key` validates path; `get_range` validates range invariants (R-07, E6) |
| SECURITY-06 Least privilege (IAM) | **Yes** | **Deferred to Infrastructure Design stage** — scoped IAM policy |
| SECURITY-07 Restrictive network | N/A | No VPC/SG in this unit; Infrastructure Design if we document bucket policy |
| SECURITY-08 App-level access control | N/A | Public CDN reads; auth lives in Unit 5 (admin) |
| SECURITY-09 Hardening: no default creds, no internal leaks in errors | **Yes** | `StorageError::Unavailable { source }` must not log the full AWS request ID in user-visible form (Unit 4 maps to generic 503) |
| SECURITY-10 Supply chain | **Yes** | Dep list frozen in Q12; `Cargo.lock` committed; `cargo-audit` in CI (existing) |
| SECURITY-11 Secure design + rate limiting | **Partial** | Circuit breaker is a form of back-pressure; full rate limiting lives in Unit 6 |
| SECURITY-12 Authentication | N/A | No auth in this unit |
| SECURITY-13 Deserialization safety | **Yes** | Raw `Vec<u8>` pass-through — no structured deserialization of S3 bodies |

Non-compliant rules = **zero blocking findings** expected if the recommended
answers are adopted. Infrastructure Design stage will pick up SECURITY-06 and
the bucket-policy half of SECURITY-01.

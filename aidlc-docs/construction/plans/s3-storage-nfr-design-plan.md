# Unit 2 — S3 Storage Backend: NFR Design Plan

**Unit:** S3 Storage Backend (Unit 2 of 7)
**Stage:** NFR Design (Part 1 — Planning)
**Depth:** Standard

---

## Context Loaded

- `aidlc-docs/construction/s3-storage/functional-design/*` — domain, rules, flows
- `aidlc-docs/construction/s3-storage/nfr-requirements/nfr-requirements.md` — targets
- `aidlc-docs/construction/s3-storage/nfr-requirements/tech-stack-decisions.md` — crate picks
- `docs/adr/0004-pluggable-storage-backends.md` (revised), `docs/adr/0019-s3-circuit-breaker.md` (new)
- `src/storage/mod.rs` — current module with `StorageBackend` trait and `LocalStorage` in the same file
- `src/config.rs` — current `AppConfig` with flat S3 fields (`s3_bucket`, `s3_region`, etc.)

## Scope

This stage translates NFR requirements into concrete design patterns and
module/file layout. Many decisions were pre-decided by Functional Design and
NFR Requirements; this stage covers the **remaining design choices that affect
code shape but not behavior**.

**Already pinned (no questions):**

- Circuit breaker state-machine shape (R-06)
- Retry policy (R-02)
- Error taxonomy (`StorageError` variants)
- Tech-stack crates (NFR Req Q12=A)
- LocalStack pin to `3.8` (NFR Req Q6=A)

**Open for this stage:** module layout, testability hooks, config shape,
metrics abstraction, testcontainer lifecycle.

## Deliverables (Part 2 output)

- `aidlc-docs/construction/s3-storage/nfr-design/nfr-design-patterns.md`
- `aidlc-docs/construction/s3-storage/nfr-design/logical-components.md`

## Plan Checklist

- [ ] Confirm scope with user
- [ ] Collect answers to all `[Answer]:` questions below
- [ ] Resolve any ambiguities with follow-ups
- [ ] Generate `nfr-design-patterns.md`
- [ ] Generate `logical-components.md`
- [ ] Run markdownlint; fix issues
- [ ] Present completion message
- [ ] Record approval in `audit.md`

---

## Clarification Questions

### Q1 — Module file layout

`src/storage/mod.rs` currently holds the trait, `LocalStorage`, and the
`S3Storage` stub. Unit 2 adds significant code — where does it live?

| Option | Files | Total LoC estimate | Test-friendliness |
|---|---|---|---|
| A. ⭐ Extract into four files: `mod.rs` (trait + `StorageError` + content-type helper), `local.rs` (`LocalStorage`), `s3.rs` (`S3Storage` + AWS SDK), `circuit_breaker.rs` | 4 | `mod.rs` ~150, `local.rs` ~200, `s3.rs` ~400, `circuit_breaker.rs` ~150 | High — `circuit_breaker` is importable standalone for unit tests |
| B. Three files: `mod.rs` (trait + `LocalStorage` + error), `s3.rs`, `circuit_breaker.rs` | 3 | `mod.rs` ~350 | Medium |
| C. Keep everything in `mod.rs` | 1 | ~900 | Low — single file to navigate |
| D. Flat module with an `s3/` sub-directory for SDK pieces (`s3/mod.rs`, `s3/circuit_breaker.rs`, `s3/client.rs`) | 4+ | Similar | Very high — full encapsulation |

**Recommended: A.** Extracting `LocalStorage` into `local.rs` is low-risk
(the code already exists, just moves) and establishes the symmetric pattern
for `s3.rs`. Splitting `circuit_breaker` into its own file matters because it
has its own unit test module and a proptest file (`tests/circuit_breaker_proptest.rs`)
that needs a stable public path. Option D's sub-directory is overkill — the
SDK surface is one file.

[Answer]: A (take recommendation)

---

### Q2 — Config grouping: flat vs nested `S3Config`

The Unit 1 `AppConfig` currently has flat fields (`s3_bucket`, `s3_region`,
`s3_endpoint`, `s3_prefix`). Unit 2 adds 7 more S3-related fields. Keep
them flat or nest?

| Option | `AppConfig` change | Access pattern | `envy` compatibility |
|---|---|---|---|
| A. Flat — keep Unit 1's convention, add 7 more flat fields | Additive | `cfg.s3_max_connections` | Works as-is — `envy` prefixes all |
| B. ⭐ Nest into a new `pub struct S3Settings { bucket, region, endpoint, prefix, max_connections, timeout_ms, cb_threshold, cb_cooldown_secs, max_retries, retry_base_ms, allow_insecure_endpoint }` on `AppConfig` | One new field on `AppConfig`, 11 existing fields move into it | `cfg.s3.max_connections` | `envy` supports nested structs via `#[serde(flatten)]` or prefix scoping; requires `RENDITION_S3_*` prefix already present |
| C. Two tiers: keep Unit 1 fields flat (back-compat for anyone reading them), add a `pub struct S3RuntimeConfig` for the 7 new fields | Additive plus new nested struct | Mixed | Works |

**Recommended: B.** Nesting is the right long-term shape: `S3Settings`
becomes the natural constructor argument for `S3Storage::new(&cfg.s3)` instead
of passing the whole `AppConfig`. The Unit 1 migration cost is small (rename
accessors in 2 places) and all Unit 1 tests pass unchanged because the env
vars (`RENDITION_S3_*`) don't move — only the struct layout changes. Option A
will force a flat `AppConfig` to grow to ~40 fields by Unit 7; nesting now
avoids that cliff.

[Answer]: B (take recommendation)

---

### Q3 — Metrics abstraction shape

R-09 says metrics are stubbed in Unit 2 and implemented in Unit 7. What's
the stub shape?

| Option | Call site (`S3Storage`) | Unit 7 integration cost |
|---|---|---|
| A. ⭐ Trait `StorageMetrics { fn record(&self, op: &str, outcome: Outcome, duration: Duration); fn set_circuit_open(&self, open: bool); }` with a `NoopMetrics` implementation used in Unit 2 | `self.metrics.record("get", Outcome::Success, elapsed)` | Unit 7 writes `PrometheusMetrics` implementing the trait |
| B. Concrete `Metrics` struct holding `Arc<dyn CounterRegistry>` optionally set | `if let Some(m) = &self.metrics { m.increment(...) }` | More boilerplate in Unit 7 |
| C. Structured `tracing::event!` only — Unit 7 bridges tracing → metrics | `tracing::event!(target: "storage.metrics", …)` | Requires tracing-to-metrics adapter |
| D. No hooks at all in Unit 2 — Unit 7 adds them by editing `s3.rs` directly | Minimal in Unit 2 | High — requires touching every op in `s3.rs` in Unit 7 |

**Recommended: A.** The trait pattern lets Unit 7 drop in a real
implementation without touching `s3.rs` again — ideal for the per-unit
sequential integration plan. A no-op default also keeps Unit 2 testable
in isolation: no metrics assertion, no Prometheus dep. C is elegant but
tracing→metrics bridging adds Unit-7 complexity we don't need; D forces
Unit 7 to re-review the hot path.

[Answer]: A (take recommendation)

---

### Q4 — Clock injection for `CircuitBreaker` testability

Q11=A+C of NFR Requirements commits us to deterministic state-machine tests
with a fake clock. How is the clock abstracted?

| Option | Trait shape | Prod cost | Test ergonomics |
|---|---|---|---|
| A. ⭐ `trait Clock { fn now(&self) -> Instant; }` with `SystemClock` (prod) and `FakeClock` (test). `CircuitBreaker<C: Clock>` is generic. | Zero runtime cost (monomorphised) | Generic propagation into `S3Storage<C>` — but we can type-alias `pub type S3Storage = S3StorageInner<SystemClock>` for the prod path | Best — explicit clock advances |
| B. `trait Clock { … }` used via `Arc<dyn Clock>` — dynamic dispatch | One vtable indirection per `now()` | Minimal propagation | Same |
| C. Feature-gated module: `#[cfg(test)] static FAKE_NOW: AtomicU64 = …` with a helper `now()` that reads the fake in tests and real `Instant::now()` in prod | None in prod | Ugly thread-local hack | Works but brittle |
| D. Skip the abstraction — test with `tokio::time::pause()` + `tokio::time::advance()` | None | None | Requires `tokio::time::Instant` instead of `std::time::Instant` throughout |

**Recommended: D.** `tokio::time::pause()` is the idiomatic Rust async testing
pattern: `#[tokio::test(start_paused = true)]` freezes the clock,
`tokio::time::advance(Duration::from_secs(30))` moves it forward
deterministically. No new traits, no generic propagation, no
dynamic-dispatch cost. The only constraint is using `tokio::time::Instant`
instead of `std::time::Instant` inside `CircuitBreaker`, which is a trivial
edit. A (trait injection) is the canonical textbook answer but is more
machinery than tokio's built-in already provides.

[Answer]: D (take recommendation)

---

### Q5 — LocalStack test isolation: `#[ignore]` vs `cfg(feature)` gate

NFR Q10=A said "`#[cfg(feature = "localstack-tests")]` **or** `#[ignore]`" —
pick one.

| Option | Main `cargo test` runs them? | How devs run them locally | CI job invocation |
|---|---|---|---|
| A. ⭐ `#[ignore]` attribute on each test; no Cargo feature | No | `cargo test -- --ignored` | `cargo test --test s3_integration -- --ignored` in dedicated job |
| B. `#[cfg(feature = "localstack-tests")]` on each test; Cargo `[features] localstack-tests = []` | No (feature off by default) | `cargo test --features localstack-tests` | Feature enabled in dedicated job |
| C. Both — feature flag gates compilation, `#[ignore]` adds a second safety net | No | Either incantation works | Both flags |

**Recommended: A.** `#[ignore]` is the idiomatic Rust pattern for "slow
integration tests that should not run in the default loop". Feature flags
(B) force recompilation when toggling, which disrupts cached builds and
confuses `rust-analyzer`. The single-word CLI flag `-- --ignored` is
standard, documented in the Rust Book, and zero friction. C is paranoid
without added safety.

[Answer]: A (take recommendation)

---

### Q6 — `is_healthy()` performance pattern

R-06 says `is_healthy()` returns `!is_open()` and is called from
`/health/ready` in Unit 7. NFR target: ≤ 100 ns. How to achieve that?

| Option | Read cost | Consistency with canonical state | Complexity |
|---|---|---|---|
| A. ⭐ Hold state in `Mutex<State>`; `is_open()` takes the lock, reads, releases | ~30 ns mutex acquire + enum match | Perfect — always reads canonical state | Minimal |
| B. Shadow state in `AtomicBool is_open_flag`; mutate alongside `Mutex<State>` writes; `is_open()` reads the atomic | ~1 ns atomic load | Good — atomic is updated inside the mutex so it can never lie for long | Slight bookkeeping overhead |
| C. Store the whole state in a single `AtomicU64` (pack enum discriminant + fields) | ~1 ns | Perfect — no mutex at all | Highest complexity; loses clarity of R-06 transitions |
| D. `Arc<RwLock<State>>`; `is_open()` takes a read lock | ~20 ns | Perfect | Reader-writer pattern is overkill for one-writer scenario |

**Recommended: A.** A std mutex acquire is ~30 ns uncontended on
aarch64/x86_64 — comfortably under the 100 ns target. The atomic shadow
(B) and packed state (C) are premature optimisations that complicate the
state-transition code we hand-wrote carefully in R-06. If a later
benchmark shows `/health/ready` contention is real (unlikely — it's a
once-per-second Kubernetes probe), we can move to B without changing the
public API.

[Answer]: A (take recommendation)

---

### Q7 — `StorageError::Unavailable { source }` — inner source type

The error enum variants `Unavailable { source }` and `Other { source }` wrap
an underlying cause. What's the concrete type?

| Option | Impl | Pros | Cons |
|---|---|---|---|
| A. ⭐ `Box<dyn std::error::Error + Send + Sync + 'static>` | `Unavailable { source: Box<dyn …> }` | Stdlib only; composes with `?` from any `std::error::Error` | No rich context; no `downcast` without knowing the concrete type |
| B. `anyhow::Error` | `Unavailable { source: anyhow::Error }` | Rich `context()` chain; ubiquitous | Adds `anyhow` to the public API surface of a library-style module |
| C. Concrete `AwsSdkError(Box<SdkError<…>>)` enum variant | One variant per SDK error type | Type-safe matching on the specific AWS error | Leaks AWS SDK types through the public API — **violates R-08 module boundary rule** |
| D. `String` — store only the display message | Simplest | No inner error at all | Loses the cause chain for logs |

**Recommended: A.** `Box<dyn Error + Send + Sync>` is the stdlib-only answer
and composes cleanly with `thiserror::Error` via `#[source]`. It preserves
the cause chain for `tracing::error!("{err:#}")` logging, doesn't leak AWS
SDK types (critical for R-08), and doesn't add `anyhow` to `src/storage/mod.rs`'s
public surface. `anyhow::Error` is fine for application error propagation
but we're defining a library-style typed-error module; stdlib is cleaner.

[Answer]: A (take recommendation)

---

### Q8 — LocalStack test harness lifecycle

Integration tests spin up a LocalStack container. Each container takes
~5–10 s to start. Share or isolate?

| Option | Startup cost | Test isolation | Parallel safety |
|---|---|---|---|
| A. Per-test container — each `#[tokio::test]` starts its own LocalStack | `N tests × ~8s` ≈ 40 s for 5 tests | Perfect | Perfect |
| B. ⭐ Shared container — a `static` `OnceLock<LocalStackContainer>` initialised on first use; each test creates its own uniquely-named bucket | One 8 s startup for the whole test binary | High — per-bucket isolation | High — bucket names use `uuid::Uuid::new_v4()` |
| C. Module-level `#[ctor]` initialiser | Same as B | High | Requires `ctor` crate |
| D. Parallel-unsafe — tests run serially with a shared container + shared bucket | One 8 s startup | Weak — tests step on each other | Unsafe — must use `--test-threads=1` |

**Recommended: B.** A shared container with per-test buckets is the
cost-performance sweet spot: one startup cost, full isolation via UUID
bucket names, parallel-safe. `OnceLock<LocalStackContainer>` (stdlib,
Rust 1.70+) gives us lazy initialisation without the `ctor` crate (C).
A is clean but the ×5–10 startup multiplier is too slow for dev loops.
D sacrifices parallelism and isolation for no gain.

[Answer]: B (take recommendation)

---

## Summary of what does NOT need a question

These decisions are locked by earlier stages and restated here for traceability:

| Topic | Locked by | Decision |
|---|---|---|
| Circuit breaker state machine | Functional Design R-06 | `Closed { consecutive_failures } / Open { opened_at } / HalfOpen { probe_in_flight }` |
| Retry policy (full jitter, cap 500 ms) | Functional Design R-02 | Configurable `max_retries` + `base_ms`, cap hardcoded |
| SDK retrier disablement | NFR Req Q2=A+D | `RetryConfig::disabled()` on `aws_sdk_s3::Client` |
| Crate choices | NFR Req Q12=A | `aws-config`, `aws-sdk-s3`, `aws-smithy-types`, `rand`, `testcontainers-modules` |
| TLS stack | NFR Req Q1=A | `hyper` + `rustls` |
| LocalStack image pin | NFR Req Q6=A | `localstack/localstack:3.8` |
| Credentials in tests | NFR Req Q7=A | Explicit `new_for_test(endpoint, creds)` constructor |
| Trait signature | ADR-0004 (revised) | `Result<T, StorageError>`; `get_range` with full-fetch default |
| IAM least privilege | NFR Req SECURITY-06 deferral | Infrastructure Design stage owns |
| S3 bucket encryption | NFR Req SECURITY-01 at-rest deferral | Infrastructure Design stage owns |

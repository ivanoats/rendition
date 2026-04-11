# Unit 2 — S3 Storage Backend — Logical Components

**Answers reference:** `aidlc-docs/construction/plans/s3-storage-nfr-design-plan.md`

This document pins the module/file layout, public interfaces, and
inter-component dependencies for Unit 2. Nothing here is new behaviour —
it's the concrete shape that implements the patterns in
`nfr-design-patterns.md`.

---

## Module layout (Q1=A)

```text
src/
├── config.rs                 (existing, refactored in Q2=B — see below)
├── storage/
│   ├── mod.rs                (trait + error + content-type helper)
│   ├── local.rs              (new — LocalStorage extracted from mod.rs)
│   ├── s3.rs                 (new — S3Storage + AWS SDK wiring)
│   └── circuit_breaker.rs    (new — CircuitBreaker + tokio-time state machine)
tests/
├── config_test.rs            (existing, updated for nested S3Settings)
├── circuit_breaker_proptest.rs  (new — proptest for R-06 invariants)
└── s3_integration.rs         (new — LocalStack tests, #[ignore])
```

### File responsibilities

**`src/storage/mod.rs`** — public module surface. Contains:

- `pub trait StorageBackend` — the port (re-exported at `crate::storage::StorageBackend`)
- `pub struct Asset` — the DTO
- `pub enum StorageError` — the typed error (`thiserror::Error`)
- `pub enum Outcome` — metrics outcome discriminant
- `pub trait StorageMetrics` — the metrics port
- `pub struct NoopMetrics` — default no-op implementation
- `pub(crate) fn compose_key(prefix, path)` — shared key composition helper (R-07)
- `pub(crate) fn content_type_from_ext(path)` — shared MIME inference
- Re-exports: `pub use local::LocalStorage;` and `pub use s3::S3Storage;`

**`src/storage/local.rs`** — moved from `mod.rs` unchanged:

- `pub struct LocalStorage { root: PathBuf, timeout_ms: u64 }`
- `impl StorageBackend for LocalStorage`
- Safe-join helper is moved here (it's filesystem-specific).
- `local_timeout_ms` is now honoured via `tokio::time::timeout`.

**`src/storage/s3.rs`** — all AWS SDK usage:

- `use aws_sdk_s3::*;` and related imports are confined here.
- `pub struct S3Storage { client, bucket, prefix, circuit_breaker, metrics, settings }`
- `impl StorageBackend for S3Storage`
- `impl S3Storage { pub async fn new(&S3Settings) -> Result<Self, StorageError> }`
- `#[cfg(test)] impl S3Storage { pub async fn new_for_test(...) -> Result<Self, StorageError> }`
- `pub fn is_healthy(&self) -> bool { !self.circuit_breaker.is_open() }`
- Private helpers: `classify(sdk_err) -> StorageError`,
  `with_retries<F>(f) -> Result<T, StorageError>`,
  `range_header(range) -> String`,
  `resolve_content_type(headers, path) -> String`.
- **No `aws_sdk_s3::*` type appears in any `pub fn` signature.**
  Compile-time check via `cargo doc` + review. This is the R-08 boundary.

**`src/storage/circuit_breaker.rs`** — standalone resilience primitive:

- `pub struct CircuitBreaker` — fields:
  `state: Mutex<State>`, `threshold: u32`, `cooldown: Duration`,
  `metrics: Arc<dyn StorageMetrics>`
- `pub async fn call<F, T>(&self, f: F) -> Result<T, StorageError>`
  where `F: Future<Output = Result<T, StorageError>>`
- `pub fn is_open(&self) -> bool`
- `#[cfg(test)] pub(crate) fn state_debug(&self) -> &'static str` for proptest assertions
- Private: `enum State { Closed { f: u32 }, Open { opened_at: Instant }, HalfOpen { probe_in_flight: bool } }`
- Uses `tokio::time::Instant` (not `std::time::Instant`) so tests can pause/advance time.
- Reusable — the breaker has no S3 knowledge. Future units (embargo store,
  preset store) can reuse it.

---

## Configuration refactor (Q2=B)

### Before (Unit 1 flat shape)

```rust
pub struct AppConfig {
    pub bind_addr: SocketAddr,
    pub assets_path: PathBuf,
    pub storage_backend: StorageBackendKind,
    pub s3_bucket: Option<String>,
    pub s3_region: Option<String>,
    pub s3_endpoint: Option<String>,
    pub s3_prefix: String,
    // ... more flat fields
}
```

### After (Unit 2 nested shape)

```rust
pub struct AppConfig {
    pub bind_addr: SocketAddr,
    pub assets_path: PathBuf,
    pub storage_backend: StorageBackendKind,
    pub s3: S3Settings,
    pub local_timeout_ms: u64,
    // ... other top-level fields
}

pub struct S3Settings {
    pub bucket: Option<String>,            // was s3_bucket
    pub region: Option<String>,            // was s3_region
    pub endpoint: Option<String>,          // was s3_endpoint
    pub prefix: String,                    // was s3_prefix
    pub max_connections: u32,              // NEW
    pub timeout_ms: u64,                   // NEW
    pub cb_threshold: u32,                 // NEW
    pub cb_cooldown_secs: u64,             // NEW
    pub max_retries: u32,                  // NEW
    pub retry_base_ms: u64,                // NEW
    pub allow_insecure_endpoint: bool,     // NEW (P-11 escape hatch)
}
```

### Migration impact on Unit 1 code

- **Env var contract unchanged** — `RENDITION_S3_BUCKET` → `S3Settings::bucket`
  via `envy`'s nested-struct support with `#[serde(rename = "s3_bucket")]` on
  field or via a `#[serde(flatten)]` approach. The exact `serde` attribute
  pattern is decided at Code Generation time.
- **Accessors** — existing code referencing `cfg.s3_bucket` becomes `cfg.s3.bucket`.
  Two call sites in `src/lib.rs` and tests.
- **`validate()`** — the cross-field validation for "if backend == S3, bucket
  and region required" stays in `AppConfig::validate`; each `S3Settings`
  field can also have its own per-field validation.
- **`ConfigError`** — no changes needed; the error variants are shape-agnostic.

### Why not just add 7 flat fields?

See NFR Design Q2 recommendation. Short version: nesting now saves nesting
later when Unit 7 would otherwise push `AppConfig` to ~40 flat fields.

---

## Component dependency graph

```text
┌──────────────┐
│  AppConfig   │  (src/config.rs)
│  .s3:        │
│  S3Settings  │
└──────┬───────┘
       │
       ▼
┌──────────────┐      ┌─────────────────────┐
│  S3Storage   │─────▶│   CircuitBreaker    │
│  (s3.rs)     │      │ (circuit_breaker.rs)│
└──────┬───────┘      └─────────┬───────────┘
       │                        │
       ▼                        ▼
┌──────────────┐      ┌─────────────────────┐
│ aws_sdk_s3   │      │   StorageMetrics    │◀── NoopMetrics
│   Client     │      │  (trait in mod.rs)  │    (default)
└──────────────┘      └─────────────────────┘
                                ▲
                                │
                                │  (Unit 7)
                         ┌──────┴────────┐
                         │PrometheusMetrics│
                         └───────────────┘

┌──────────────┐
│ LocalStorage │  (local.rs) — no breaker, no metrics (for now),
│              │  just fs reads + timeout wrapper
└──────────────┘

┌──────────────┐
│ StorageBackend│  (trait, mod.rs)
│     trait    │
└──────┬───────┘
       │ implemented by
       ├─► LocalStorage
       └─► S3Storage
```

### Explicit dependency list

- **`storage::mod`** — `tokio`, `thiserror`, `tracing`, stdlib.
- **`storage::local`** — `tokio::fs`, `tokio::time`;
  `crate::storage::{StorageBackend, StorageError, Asset,
  compose_key, content_type_from_ext}`.
- **`storage::s3`** — `aws-config`, `aws-sdk-s3`, `aws-smithy-runtime`,
  `aws-smithy-types`, `rand`, `tracing`;
  `crate::storage::{StorageBackend, StorageError, Asset, StorageMetrics,
  compose_key, content_type_from_ext, circuit_breaker::CircuitBreaker}`.
- **`storage::circuit_breaker`** — `tokio::time::{Instant, sleep}`,
  `tracing`, `std::sync::Mutex`;
  `crate::storage::{StorageError, StorageMetrics, Outcome}`.

No circular dependencies. `mod.rs` is the "hub" with the trait and error
types; `local`, `s3`, and `circuit_breaker` are spokes.

---

## Logical components — public API summary

| Component | Public interface | Consumers (now and future) |
|---|---|---|
| `StorageBackend` trait | `async fn get(&self, &str) -> Result<Asset, StorageError>`, `async fn exists(&self, &str) -> Result<bool, StorageError>`, `async fn get_range(&self, &str, Range<u64>) -> Result<Asset, StorageError>` | Unit 4 (CDN handler), Unit 5 (embargo check), Unit 7 (health probe) |
| `StorageError` enum | Variants `NotFound / InvalidPath / CircuitOpen / Timeout { op } / Unavailable { source } / Other { source }` | Unit 4 HTTP status mapping |
| `Asset` struct | `data: Vec<u8>`, `content_type: String`, `size: usize` | Unit 4 handler |
| `LocalStorage` | `::new(root, timeout_ms) -> Self` | `build_app` in `lib.rs` |
| `S3Storage` | `::new(&S3Settings) -> Result<Self>`, `new_for_test(endpoint, key, secret, bucket) -> Result<Self>` (test-only), `is_healthy() -> bool` | `build_app`, Unit 7 `/health/ready` |
| `CircuitBreaker` | `::new(threshold, cooldown, metrics) -> Self`, `call<F>(&self, F) -> Result<T, StorageError>`, `is_open() -> bool` | `S3Storage`; reusable by future units |
| `StorageMetrics` trait | `record(&self, op, outcome, duration)`, `set_circuit_open(&self, bool)` | `S3Storage`, `CircuitBreaker`; Unit 7 provides `PrometheusMetrics` |
| `NoopMetrics` struct | `::new() -> Self` | Default metrics wiring in Unit 2 tests and `build_app` |

---

## Test component layout

### `tests/circuit_breaker_proptest.rs` (new)

Self-contained property test for R-06. Does not need LocalStack. Runs in
the default `cargo test` loop. Uses `#[tokio::test(start_paused = true)]`
so `tokio::time::advance` moves the fake clock.

### `tests/s3_integration.rs` (new)

All `#[ignore]`-gated. Uses the shared `OnceLock<LocalStackContainer>`
helper in `tests/helpers/localstack.rs` (new). Creates a fresh bucket per
test via `uuid`.

```rust
#[tokio::test]
#[ignore = "requires LocalStack (run with `cargo test -- --ignored`)"]
async fn s3_get_fetches_real_bytes() { /* ... */ }
```

### `tests/helpers/localstack.rs` (new)

Shared test infrastructure:

- `static CONTAINER: OnceLock<LocalStackContainer>` — lazy, one per test binary
- `pub async fn localstack_endpoint() -> String`
- `pub async fn fresh_bucket() -> String` — creates a uniquely named bucket
- `pub async fn put_fixture(bucket, key, bytes)` — test helper for uploading

---

## What this stage does NOT decide

- **Exact `serde` / `envy` field attribute syntax** for the nested
  `S3Settings` struct — decided at Code Generation. The contract (env var
  names unchanged) is locked here.
- **Retry loop RNG seeding policy** — `rand::thread_rng()` is fine.
- **Bucket name validation regex** — standard S3 naming rules enforced by
  AWS anyway; we don't duplicate them.
- **How `build_app` wires `NoopMetrics`** — this is a one-line instantiation
  in `src/lib.rs`, decided at Code Generation.

These go into the Code Generation stage plan.

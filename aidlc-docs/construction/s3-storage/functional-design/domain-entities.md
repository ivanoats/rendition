# Unit 2 — S3 Storage Backend: Domain Entities

**Unit:** S3 Storage Backend
**Stage:** Functional Design
**Answers reference:** `aidlc-docs/construction/plans/s3-storage-functional-design-plan.md`

Technology-agnostic domain model for Unit 2. Types below describe *what*
the storage subsystem reasons about, not *how* it calls AWS.

---

## E1 — `Asset` (existing — unchanged)

Already defined in `src/storage/mod.rs`. Reproduced here for completeness
because `StorageError::NotFound` is meaningful only in contrast to it.

| Field | Type | Meaning |
|---|---|---|
| `data` | `Vec<u8>` | Raw bytes as fetched from the backend. May be the full object or a byte range. |
| `content_type` | `String` | MIME type. For S3: header if present, else inferred from path extension (Q7=A). |
| `size` | `usize` | Length of `data`. For a range fetch this is the range width, not the full object size. |

---

## E2 — `StorageError` (new)

Replaces `anyhow::Result<Asset>` on the trait. Introduced in `src/storage/mod.rs`
so both `LocalStorage` and `S3Storage` return typed errors. (Q1=A, Q8=B)

```text
enum StorageError {
    NotFound,                     // asset absent (LocalStorage: ENOENT; S3: 404/403 ambiguity — see R-04)
    InvalidPath { reason: String },// path-traversal attempt, empty path, non-UTF8
    CircuitOpen,                   // S3Storage only — breaker is open, call skipped
    Timeout { op: String },        // I/O deadline exceeded (local or S3)
    Unavailable { source: Error }, // transient backend failure, may retry (S3 5xx, network error)
    Other { source: Error },       // unclassified — propagate as 500
}
```

### Variant mapping to HTTP (consumed by Unit 4 request handler)

| Variant | HTTP status | Retry at caller? |
|---|---|---|
| `NotFound` | 404 | No |
| `InvalidPath` | 400 | No |
| `CircuitOpen` | 503 | No (fail fast) |
| `Timeout` | 504 | Caller's choice |
| `Unavailable` | 503 | No (the S3Storage already retried internally) |
| `Other` | 500 | No |

### Properties

- Implements `std::error::Error` and `Display` so it composes with `anyhow`
  and `thiserror` at call sites outside the storage module.
- Implements `PartialEq` for the cheap variants (`NotFound`, `CircuitOpen`,
  `InvalidPath`, `Timeout`) — enables clean test assertions like
  `assert_eq!(err, StorageError::NotFound)`.
- `Unavailable` and `Other` wrap a boxed inner error (`Box<dyn Error + Send + Sync>`
  or `anyhow::Error`) and do **not** implement `PartialEq`; tests compare via
  `matches!(err, StorageError::Unavailable { .. })`.

---

## E3 — `CircuitBreakerState` (new, internal)

State machine stored inside `CircuitBreaker`. Purely domain — has no knowledge
of S3, of `anyhow`, or of async runtime. (Q2=A, Q4=A)

```text
enum CircuitBreakerState {
    Closed { consecutive_failures: u32 },
    Open { opened_at: Instant },
    HalfOpen { probe_in_flight: bool },
}
```

### Transitions

| From | Event | To |
|---|---|---|
| `Closed { f }` | success | `Closed { 0 }` |
| `Closed { f }` | failure, `f + 1 < threshold` | `Closed { f + 1 }` |
| `Closed { f }` | failure, `f + 1 >= threshold` | `Open { now }` |
| `Open { opened_at }` | any call, `now - opened_at < cooldown` | stay `Open` (reject with `StorageError::CircuitOpen`) |
| `Open { opened_at }` | any call, `now - opened_at >= cooldown` | `HalfOpen { false }` — call proceeds as probe |
| `HalfOpen { false }` | probe call starts | `HalfOpen { true }` |
| `HalfOpen { true }` | second concurrent call arrives | reject with `CircuitOpen` (only one probe at a time) |
| `HalfOpen { true }` | probe success | `Closed { 0 }` |
| `HalfOpen { true }` | probe failure | `Open { now }` |

### Invariants

- `consecutive_failures` is reset on any success in `Closed` state.
- A failure while in `HalfOpen { true }` always reopens the circuit with a
  **fresh** `opened_at` — not the original one. This is how "cooldown" is
  re-earned.
- `is_open() -> bool` returns `true` iff the current state is `Open { .. }`.
  Used by `S3Storage::is_healthy()` for `/health/ready` in Unit 7.
- `probe_in_flight = true` guarantees at most one probe per half-open window.

---

## E4 — `S3Config` (extended from Unit 1)

Unit 1 already defines `s3_bucket`, `s3_region`, `s3_endpoint`, `s3_prefix`.
Unit 2 extends `AppConfig` with the following fields (Q9=A). Fields are
documented here as domain concepts; actual `AppConfig` layout is a code
concern and lives in `code-generation` stage output.

| Field | Default | Used by | Notes |
|---|---|---|---|
| `s3_max_connections` | 100 | `S3Storage` HTTP pool | QA-01 scalability |
| `s3_timeout_ms` | 5000 | Every S3 call | QA-02 reliability |
| `s3_cb_threshold` | 5 | `CircuitBreaker::new` | QA-04 |
| `s3_cb_cooldown_secs` | 30 | `CircuitBreaker::new` | QA-04 |
| `s3_max_retries` | 3 | Retry loop (Q3=B) | QA-02 |
| `s3_retry_base_ms` | 50 | Retry loop base delay | QA-02 |
| `local_timeout_ms` | 2000 | `LocalStorage` read timeout | QA-02 (kept in scope per Q9=A) |

### Validation (extends `AppConfig::validate`)

- `s3_max_connections >= 1`
- `s3_timeout_ms >= 100` (below 100 ms is almost certainly a typo)
- `s3_cb_threshold >= 1`
- `s3_cb_cooldown_secs >= 1`
- `s3_max_retries <= 10` (unbounded retries defeat the circuit breaker)
- `s3_retry_base_ms >= 1`
- `local_timeout_ms >= 100`

---

## E5 — `StorageKey` (logical, not a Rust type)

Not a distinct type — it's the composition of `(s3_prefix, logical_path)`
that yields the final S3 object key. Documented here because the rules
below are the *only* place `logical_path → object key` mapping is
defined. (Q6=B — normalise.)

### Composition rule

```text
fn compose(prefix: &str, path: &str) -> Result<String, StorageError::InvalidPath>
```

1. Reject `path` if empty, contains a NUL byte, or is not valid UTF-8.
2. Strip any leading `/` from `path`.
3. If `prefix` is empty, return `path`.
4. If `prefix` does not end with `/`, append one.
5. Return `prefix + path`.

### Examples

| `s3_prefix` | `path` | Result |
|---|---|---|
| `""` | `products/shoe.jpg` | `products/shoe.jpg` |
| `"assets"` | `products/shoe.jpg` | `assets/products/shoe.jpg` |
| `"assets/"` | `products/shoe.jpg` | `assets/products/shoe.jpg` |
| `"assets/"` | `/products/shoe.jpg` | `assets/products/shoe.jpg` |
| `""` | `` | Error — `InvalidPath { reason: "empty" }` |

### Why not `..` rejection (Q6=B vs Q6=C)

S3 has no filesystem — `foo/../bar.jpg` is just an object named literally
`foo/../bar.jpg`. Rejecting `..` at `S3Storage` would diverge from S3's
own semantics with no security benefit, because path validation for
user-supplied input happens earlier at the HTTP entry point (Unit 4/6),
before `S3Storage::get` is ever called.

---

## E6 — `ByteRange` (logical, Rust `Range<u64>`)

Consumed by `get_range`. Invariants enforced at the call site, not inside
`compose`:

- `range.start < range.end` — empty ranges are caller errors.
- `range.end - range.start <= max_payload_bytes` — see Unit 4.

`S3Storage::get_range` passes the range verbatim to S3's `Range: bytes=…` header.
`LocalStorage::get_range` uses the trait's default full-fetch-and-slice
implementation (Q5=A).

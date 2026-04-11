# Unit 2 — S3 Storage Backend: Business Rules

**Unit:** S3 Storage Backend
**Stage:** Functional Design
**Answers reference:** `aidlc-docs/construction/plans/s3-storage-functional-design-plan.md`

Declarative rules that govern behavior. Referenced by the business logic
model and enforced by the code generated in the Code Generation stage.

---

## R-01 — Error classification for retries

Every failed S3 call is classified into exactly one of three categories:

| Category | Retriable | Counts toward circuit breaker `consecutive_failures` | Maps to |
|---|---|---|---|
| **Terminal / client** | No | **No** | `NotFound` or `Other` |
| **Transient** | Yes | Yes | `Unavailable` (after retries exhausted) |
| **Timeout** | Yes (via retry loop) | Yes | `Timeout` (after retries exhausted) |

**Terminal / client errors** (do not retry, do not trip the breaker):

- S3 `NoSuchKey` / HTTP 404 → `StorageError::NotFound`
- HTTP 403 Forbidden → `StorageError::NotFound` (indistinguishable from 404
  for unauthorised ARNs; surfacing "forbidden" would leak information)
- HTTP 400 Bad Request → `StorageError::Other`

**Transient errors** (retry per R-02, then propagate):

- HTTP 5xx (any)
- `ServiceUnavailable`, `SlowDown`, `Throttling*` (all S3 throttling codes)
- `RequestTimeout` at the S3 protocol layer
- Connection errors / TCP reset / TLS failures / DNS errors

**Why 403 → `NotFound` and not `Other`:** clients must not be able to
probe bucket contents through error-code disambiguation (timing and status
codes). Unit 4 will render both as 404.

---

## R-02 — Retry policy (Q3=B — configurable)

Applied inside `S3Storage` around every call that can fail transiently
(`GetObject`, `HeadObject`, `GetObject` with Range header). Implemented as
a thin wrapper around the AWS SDK call — **not** by relying on the SDK's
built-in retrier (explicit policy so the circuit breaker sees the attempt
counts).

```text
attempt 0:                base_ms
attempt 1:  delay ≈ random_between(0, base_ms * 2)       // "full jitter"
attempt 2:  delay ≈ random_between(0, base_ms * 4)
attempt N:  delay ≈ random_between(0, min(base_ms * 2^N, cap))
```

Rules:

- `max_retries = s3_max_retries` (default 3 → at most 4 total attempts).
- `base = s3_retry_base_ms` (default 50).
- `cap = 500 ms` — hard upper bound on any single delay, regardless of config.
- **Full jitter** — delay is uniformly random in `[0, exp_backoff]`, not
  `exp_backoff + random`. This is the AWS-recommended pattern; avoids
  retry storms on correlated failures.
- Retries stop immediately on a **terminal / client** error per R-01.
- Each attempt resets the per-call timeout (R-03) — the timeout is
  per-attempt, not per-retry-loop.

**Interaction with the circuit breaker:**

- The retry loop is *inside* `CircuitBreaker::call`. One breaker call =
  one "attempt sequence", not one network round trip.
- A sequence that exhausts retries and still fails counts as **one**
  `consecutive_failures` increment, not `max_retries + 1`. Otherwise a
  single bad request would trip the breaker.

---

## R-03 — Timeout enforcement

Every S3 network call is wrapped in `tokio::time::timeout(s3_timeout_ms, …)`.
Two concerns:

- **Per-attempt deadline** — the retry loop applies this deadline to each
  individual `GetObject` / `HeadObject` call. Hitting it raises
  `StorageError::Timeout { op: "get" | "exists" | "get_range" }`.
- **The total wall-clock time** of a retry sequence can exceed
  `s3_timeout_ms` due to back-off between attempts. The total request
  timeout (`RENDITION_REQUEST_TIMEOUT_MS`, default 30000) is enforced at
  the HTTP handler layer (Unit 4), not here.

`LocalStorage` applies `local_timeout_ms` to its `fs::read` via the same
pattern.

---

## R-04 — `exists` semantics (Q7=B + Q8=B)

`exists` returns `Result<bool, StorageError>`:

| `HeadObject` outcome | Return |
|---|---|
| 200 OK | `Ok(true)` |
| 404 NoSuchKey | `Ok(false)` |
| 403 Forbidden | `Ok(false)` (see R-01 rationale) |
| Transient error, retries exhausted | `Err(StorageError::Unavailable { .. })` |
| Timeout per R-03 | `Err(StorageError::Timeout { op: "exists" })` |
| Circuit open | `Err(StorageError::CircuitOpen)` |

`LocalStorage::exists` mirrors this shape: `Ok(true)` / `Ok(false)` from
`fs::metadata`; `Err(Timeout)` if `local_timeout_ms` elapsed; no
`CircuitOpen` variant (local has no breaker).

---

## R-05 — Content-Type resolution (Q7=A)

`S3Storage::get` populates `Asset.content_type` using this fallback chain:

1. The `Content-Type` header on the S3 response, **iff** it is non-empty
   and **not** exactly `application/octet-stream`.
2. Otherwise, `content_type_from_ext(path)` — the same helper
   `LocalStorage` uses.
3. Otherwise, `application/octet-stream` as the final fallback.

**Rationale:** production uploads set correct MIME types that extension
inference cannot reproduce (e.g. HEIC, `video/mp4` vs `video/quicktime`).
But a bare `application/octet-stream` from S3 means "uploader didn't set
it" — falling through to extension inference is better than serving
downloads to browsers.

---

## R-06 — Circuit breaker state transitions

(Duplicated compactly from `domain-entities.md` E3; see there for the full
transition table.)

**Opening:** exactly `s3_cb_threshold` (default 5) consecutive failures
in the `Closed` state transition to `Open`. Single successes reset the
counter to 0.

**Cooling:** `Open` rejects every call with `StorageError::CircuitOpen`
until `now - opened_at >= s3_cb_cooldown_secs`. The first call after the
cooldown enters `HalfOpen` and proceeds as a probe.

**Half-open:** at most **one** probe at a time. Success → `Closed`.
Failure → `Open` with a **fresh** `opened_at` (fresh cooldown). Concurrent
calls during half-open are rejected with `CircuitOpen` rather than
queued — we prefer fail-fast to backpressure here.

**`is_open()` definition:** returns `true` iff current state is `Open { .. }`.
`HalfOpen` counts as "not open" for health-check purposes because the
dependency is recovering — `/health/ready` should report the service as
ready the instant the breaker is willing to try S3 again.

---

## R-07 — Path composition and validation

See E5 for the full table. Rules restated as business invariants:

- Empty `path` → `StorageError::InvalidPath { reason: "empty" }`.
- `path` containing a NUL byte → `InvalidPath { reason: "null byte" }`.
- All other strings are accepted verbatim after leading-`/` stripping and
  `s3_prefix` normalisation.

Note: `S3Storage` does **not** reject `..`-containing paths. S3 has no
filesystem semantics and such objects are legal S3 keys. Path-traversal
defence lives in the HTTP layer (Units 4, 6).

---

## R-08 — Module boundary (from unit definition)

- No `aws-sdk-s3` type (`Client`, `GetObjectOutput`, `SdkError`, etc.) is
  publicly exposed from `src/storage/s3.rs`. Only standard Rust types
  (`Asset`, `StorageError`) cross the module boundary.
- The `CircuitBreaker` struct is `pub(crate)` at most, so the rest of the
  codebase cannot depend on its internals.
- `S3Storage::is_healthy() -> bool` is the one back-channel exposed for
  Unit 7 health checks.

---

## R-09 — Metrics hooks (deferred to Unit 7, sketched here)

The following counters and gauges will be incremented by `S3Storage` and
`CircuitBreaker` in Unit 7 (observability unit). Documented here so the
code scaffolding allocates the right call sites.

- `rendition_storage_requests_total{backend="s3",op="get",outcome="success|not_found|error|timeout|circuit_open"}` — counter
- `rendition_storage_request_duration_seconds{backend,op}` — histogram
- `rendition_s3_circuit_breaker_open{backend="s3"}` — gauge (0 or 1)
- `rendition_s3_retries_total{op}` — counter (incremented per retry attempt beyond the first)

In Unit 2 these are **no-ops** — the struct fields exist but point to a
no-op `Metrics` trait object. Unit 7 replaces the implementation.

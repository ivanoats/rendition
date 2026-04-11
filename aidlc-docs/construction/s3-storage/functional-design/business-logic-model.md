# Unit 2 — S3 Storage Backend: Business Logic Model

**Unit:** S3 Storage Backend
**Stage:** Functional Design
**References:** `domain-entities.md` (E1–E6), `business-rules.md` (R-01–R-09)

Describes the *flows* each operation executes. Technology-agnostic —
no AWS SDK symbols, no Rust syntax beyond pseudocode. Domain types come
from `domain-entities.md`; rules from `business-rules.md`.

---

## Actors

- **Caller** — HTTP handler (Unit 4) or health check (Unit 7). Holds an
  `Arc<dyn StorageBackend>` and calls `get`, `exists`, or `get_range`.
- **`StorageBackend` port** — the trait defined in `src/storage/mod.rs`.
- **`S3Storage` adapter** — concrete implementation wrapping
  `aws-sdk-s3::Client` and a `CircuitBreaker`.
- **`CircuitBreaker`** — tracks failure rate; short-circuits calls when
  open.
- **AWS S3 (or LocalStack)** — remote object store.

---

## Flow 1 — `S3Storage::get(path)`

**Purpose:** Fetch the full object for `path`, populated with a correct
`Content-Type` and `size`.

```text
1.  path → compose_key(s3_prefix, path)     [E5 / R-07]
    ├─ on InvalidPath → return Err(StorageError::InvalidPath)
    └─ success → key: String

2.  enter CircuitBreaker::call(|| fetch_get(key))
    ├─ breaker Open → return Err(CircuitOpen)                [R-06]
    └─ breaker Closed or HalfOpen → proceed to step 3

3.  fetch_get(key):
    attempt = 0
    loop:
        result = with_timeout(s3_timeout_ms, s3_client.get_object(bucket, key))   [R-03]
        classify(result) per R-01:
          ├─ Ok(response)    → return Ok(response)
          ├─ Terminal(404/403) → return Err(NotFound)
          ├─ Terminal(other) → return Err(Other)
          └─ Transient or Timeout →
              if attempt < s3_max_retries:
                  attempt += 1
                  sleep(full_jitter(attempt, base, cap))      [R-02]
                  continue
              else:
                  return Err(Unavailable | Timeout)

4.  CircuitBreaker observes the Result from step 3:
    ├─ Ok(_) or Err(NotFound | Other | InvalidPath)   → "success" for breaker purposes
    └─ Err(Unavailable | Timeout)                     → "failure" — increment consecutive_failures
                                                         (may transition to Open per R-06)

5.  On Ok(response):
    content_type = resolve_content_type(response.headers, path)   [R-05]
    asset = Asset {
        data: response.body_bytes,
        content_type,
        size: response.body_bytes.len(),
    }
    return Ok(asset)
```

### Notes (get)

- Step 2 wraps step 3. One breaker "call" spans the entire retry loop
  (R-02) — breaker counts *sequences*, not retries.
- Step 4's classification: `NotFound` is a success for breaker purposes.
  A missing object is normal; 503 is not.
- `resolve_content_type` is R-05's fallback chain.

---

## Flow 2 — `S3Storage::exists(path)`

**Purpose:** Tell the caller whether an object exists, without downloading
its body.

```text
1.  path → compose_key                                        [E5 / R-07]

2.  CircuitBreaker::call(|| fetch_head(key))
    ├─ Open → Err(CircuitOpen)
    └─ Closed | HalfOpen → proceed

3.  fetch_head(key):
    attempt = 0
    loop:
        result = with_timeout(s3_timeout_ms, s3_client.head_object(bucket, key))
        classify per R-01:
          ├─ Ok(_)                        → return Ok(true)
          ├─ Terminal(404/403)            → return Ok(false)        [R-04]
          ├─ Terminal(other)              → return Err(Other)
          └─ Transient or Timeout →
              if attempt < s3_max_retries: retry with backoff (R-02)
              else: return Err(Unavailable | Timeout)

4.  Breaker outcome: Ok(_) and Err(NotFound|Other|InvalidPath) = success;
    Err(Unavailable|Timeout) = failure.
```

### Notes (exists)

- `exists` **never** downloads the body — S3 `HeadObject` is required by
  acceptance criterion 2 of the unit.
- The 403→`false` mapping (R-01) is applied at classify, not later.
- Consumed by `/health/ready` in Unit 7 via `is_healthy()`, which reads
  the circuit breaker directly without calling `exists`.

---

## Flow 3 — `S3Storage::get_range(path, range)`

**Purpose:** Fetch only a byte range of the object, using S3's native
`Range: bytes=…` header (ADR-0018). Unit 4's `206 Partial Content` handler
is the caller.

```text
1.  path → compose_key                                        [E5 / R-07]
    Validate range: require range.start < range.end
    └─ on invalid → return Err(InvalidPath { reason: "empty range" })

2.  CircuitBreaker::call(|| fetch_range(key, range))
    ├─ Open → Err(CircuitOpen)
    └─ Closed | HalfOpen → proceed

3.  fetch_range(key, range):
    attempt = 0
    loop:
        result = with_timeout(
            s3_timeout_ms,
            s3_client.get_object(bucket, key, range_header("bytes=start-end_inclusive"))
        )
        classify per R-01 (same as get / exists)

4.  On Ok(response):
    asset = Asset {
        data: response.body_bytes,
        content_type: resolve_content_type(response.headers, path),
        size: response.body_bytes.len(),  // == (range.end - range.start) if S3 honoured the range
    }
    Verification hook:
      if asset.size != range.end - range.start:
          // S3 didn't honour the range — treat as Other (not a retriable transient)
          return Err(Other)
    return Ok(asset)
```

### Notes (get_range)

- `range.end` is **exclusive** in Rust's `Range<u64>`; the `bytes=` header
  uses an **inclusive** end. The adapter converts: `bytes=start-(end-1)`.
- The post-fetch size check is cheap insurance: if a future S3 config
  (e.g. client-side encryption wrapper) silently ignores the Range header,
  we want to fail fast, not serve the wrong bytes.
- `LocalStorage::get_range` uses the trait's default implementation (Q5=A):
  call `get` then slice the `Vec<u8>`. That is not shown here — see the
  trait definition in `domain-entities.md`.

---

## Flow 4 — `CircuitBreaker::call<F>(op)`

**Purpose:** Short-circuit an async operation when the circuit is open;
record its outcome to drive the state machine.

```text
1.  match current state:
      Closed { f }:
          result = op().await
          if result.is_failure_per_breaker:   // Unavailable|Timeout per R-01
              new_f = f + 1
              if new_f >= threshold:
                  state = Open { opened_at: now }
              else:
                  state = Closed { consecutive_failures: new_f }
          else:
              state = Closed { consecutive_failures: 0 }
          return result

      Open { opened_at }:
          if now - opened_at < cooldown:
              return Err(CircuitOpen)                    // fail fast
          else:
              // transition to half-open and fall through to probe handling
              state = HalfOpen { probe_in_flight: false }
              // re-enter the outer match

      HalfOpen { probe_in_flight: false }:
          state = HalfOpen { probe_in_flight: true }
          result = op().await
          if result.is_failure_per_breaker:
              state = Open { opened_at: now }            // fresh cooldown
          else:
              state = Closed { consecutive_failures: 0 }
          return result

      HalfOpen { probe_in_flight: true }:
          return Err(CircuitOpen)                        // only one probe at a time
```

### Invariants

- State transitions are atomic. Implementation will use a `Mutex` or
  `std::sync::atomic` + `std::sync::Mutex` split to avoid holding the
  lock across `op().await`.
- `CircuitBreaker::call` is generic over any future returning
  `Result<T, StorageError>`. It does not know or care that the `op` is an
  S3 request — the breaker is reusable.
- The breaker's concept of "failure" is narrower than the underlying
  call's error set: only `Unavailable` and `Timeout` count. `NotFound`,
  `InvalidPath`, `Other`, and `CircuitOpen` do not.

---

## Flow 5 — `S3Storage::is_healthy()`

**Purpose:** Cheap synchronous check for `/health/ready` (Unit 7).

```text
return !self.circuit_breaker.is_open()
```

- **O(1)**, no I/O. Just reads the breaker's current state.
- Returns `true` when state is `Closed` or `HalfOpen`, `false` when `Open`.
- Health checks must never call `exists` — that would load every readiness
  probe onto S3, and probes during a cooldown would just extend the
  outage. `is_healthy` exposes the breaker's view directly.

---

## Composition with callers

| Caller (unit) | Uses | Expected errors handled |
|---|---|---|
| Unit 4 — `serve_asset` handler | `get` or `get_range` | `NotFound`→404, `CircuitOpen`→503, `Unavailable`→503, `Timeout`→504, `InvalidPath`→400, `Other`→500 |
| Unit 7 — `/health/ready` | `is_healthy()` | none — just a bool |
| Unit 5 — embargo enforcement | `exists` | `NotFound`→404 reuse, error variants surfaced verbatim |

---

## Out-of-scope for Unit 2 (cross-references)

- HTTP response mapping of `StorageError` — Unit 4.
- Prometheus metric emission from S3Storage / CircuitBreaker — Unit 7
  (stubbed no-op in Unit 2 per R-09).
- Presigned URLs / upload paths — not in Rendition's scope; ECM handles
  authoring.
- Server-side encryption key management — defer to Infrastructure
  Design stage.

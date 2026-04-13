# Unit 3 — Transform Cache: Code Summary

**Status**: COMPLETE  
**Tests**: 105 passing (67 lib + 7 api_integration + 19 config + 12 e2e)  
**Clippy**: clean (`-D warnings`)

---

## Files Created

| File | Description |
|---|---|
| `src/cache.rs` | `TransformCache` trait, `MokaTransformCache`, `compute_cache_key` |
| `src/metrics.rs` | `Metrics` struct with atomic cache hit/miss counters |

## Files Modified

| File | Change |
|---|---|
| `Cargo.toml` | Added `moka = "0.12"` (sync feature) and `sha2 = "0.10"` |
| `src/transform/mod.rs` | Added `TransformParams::canonical_bytes()` |
| `src/lib.rs` | Added `pub mod cache; pub mod metrics;`; updated `build_app` to wire cache + metrics |
| `src/api/mod.rs` | Added `cache` + `metrics` to `AppState`; cache integration in `serve_asset`; new tests |

---

## Key Design Decisions

### CacheKey
`[u8; 32]` — SHA-256 of `path ∥ NUL ∥ canonical_params_bytes`.

### canonical_bytes
Fixed-field-order JSON via a private `CanonicalParams` struct (fields declared alphabetically).
Ensures that `?wid=800&fmt=webp` and `?fmt=webp&wid=800` produce the same cache key.

### put signature deviation
`put(key, path, response)` — includes `path: &str` (not in original component spec).
Required for `invalidate_by_path` to maintain a secondary path→keys index.

### Metrics (Unit 3 scope)
`AtomicU64` counters only. Prometheus registration is deferred to Unit 7 (Observability).

---

## Acceptance Criteria Status

| Criterion | Status |
|---|---|
| Second request returns from cache; libvips bypassed | ✅ `second_request_hits_cache` |
| Cache bounded by `RENDITION_CACHE_MAX_ENTRIES` | ✅ `cache_does_not_exceed_max_capacity` |
| Entries expire after `RENDITION_CACHE_TTL_SECONDS` | ✅ `entries_expire_after_ttl` |
| Cache key identical regardless of param order | ✅ `prop_cache_key_is_deterministic` |
| `rendition_cache_hits_total` increments on hit | ✅ `cache_hit_increments_metric` |

## PBT Compliance (PBT-03 / NFR-02)

| Rule | Status |
|---|---|
| PBT-01: Properties identified | ✅ Documented in code-generation-plan.md |
| PBT-02: Round-trip | N/A — no invertible operation in this unit |
| PBT-03: Invariant (key determinism, key collision resistance) | ✅ 3 proptest cases |
| PBT-04: Idempotence | N/A — cache writes are not documented as idempotent (overwrite semantics) |
| PBT-05: Oracle | N/A — no reference implementation available |
| PBT-06: Stateful PBT | N/A — `MokaTransformCache` delegated to moka which has its own test suite |
| PBT-07: Generator quality | ✅ Domain-constrained arb_params / arb_path generators |
| PBT-08: Shrinking | ✅ proptest default shrinking enabled |
| PBT-09: Framework | ✅ `proptest = "1"` already in dev-dependencies |
| PBT-10: Complementary | ✅ Each PBT has companion example-based tests |

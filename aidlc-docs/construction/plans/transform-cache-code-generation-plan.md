# Unit 3 — Transform Cache: Code Generation Plan

**Unit**: Transform Cache (Unit 3 of 7)
**Stage**: Code Generation (Parts 1 & 2)
**Depth**: Standard
**Entry condition**: Unit 1 (Config) complete ✅

---

## Context

- **Unit definition**: `aidlc-docs/inception/application-design/unit-of-work.md` §Unit 3
- **Story map**: `aidlc-docs/inception/application-design/unit-of-work-story-map.md` §Unit 3
- **Component spec**: `aidlc-docs/inception/application-design/component-methods.md` §C-04 Transform Cache
- **AppState target**: `aidlc-docs/inception/application-design/component-methods.md` §C-07 CDN API
- **Extensions active**: Security Baseline (yes), Property-Based Testing (yes)

## Stories Implemented

| Requirement | Component | Test type |
|---|---|---|
| FR-03: Cache transformed outputs to avoid redundant libvips work | `MokaTransformCache` | Integration |
| FR-03: Cache key includes path + full transform params | `compute_cache_key` | Unit + proptest |
| FR-03: Cache bounded by `RENDITION_CACHE_MAX_ENTRIES` (LRU eviction) | `MokaTransformCache` | Unit |
| FR-03: Cache entries expire after `RENDITION_CACHE_TTL_SECONDS` | `MokaTransformCache` | Unit |
| FR-03: Cache safe for concurrent access | `MokaTransformCache` | Unit (threaded) |
| FR-03: Cache hits bypass libvips | `serve_asset` wire-up | Integration |
| FR-03: Cache hit/miss metrics emitted | `Metrics` counters | Unit |
| FR-14: Embargoed responses must not enter cache | `serve_asset` | Integration |
| NFR-02 (PBT): `compute_cache_key` deterministic for same inputs | `compute_cache_key` | Proptest |
| NFR-02 (PBT): Distinct keys for differing params | `compute_cache_key` | Proptest |

## PBT Properties Identified (PBT-01)

| Component | Property category | Property |
|---|---|---|
| `compute_cache_key` | Invariant | Identical inputs always produce identical output |
| `compute_cache_key` | Invariant | Distinct (path, params) pairs produce distinct keys |
| `MokaTransformCache` | Idempotence | `put(k, v); put(k, v)` leaves the same observable state as `put(k, v)` |
| `TransformParams::canonical_bytes` | Invariant | Field order in URL does not affect canonical bytes |

---

## Step Checklist

### Step 1 — Dependencies (Cargo.toml)
- [x] Add `moka = { version = "0.12", features = ["sync"] }` under `[dependencies]`
- [x] Add `sha2 = "0.10"` under `[dependencies]`

### Step 2 — TransformParams::canonical_bytes (src/transform/mod.rs)
- [x] Add `pub fn canonical_bytes(&self) -> Vec<u8>` to `TransformParams`
- [x] Fields serialised alphabetically via a private `CanonicalParams` struct
- [x] Uses `serde_json` (already a transitive dep via axum)

### Step 3 — src/cache.rs (new file)
- [x] `CacheKey = [u8; 32]` type alias
- [x] `CachedResponse { data: Vec<u8>, content_type: &'static str }` (Clone)
- [x] `TransformCache` trait — `get`, `put` (with path), `invalidate`, `invalidate_by_path`, `entry_count`
- [x] `MokaTransformCache` — `moka::sync::Cache` + `Mutex<HashMap<String, Vec<CacheKey>>>` path index
- [x] `compute_cache_key(path, params) -> CacheKey` — SHA-256
- [x] Unit tests: get/miss, put/get, invalidate, invalidate_by_path, bounded eviction, TTL expiry, concurrent access
- [x] Proptest: deterministic keys, distinct paths → distinct keys, distinct params → distinct keys

### Step 4 — src/metrics.rs (new file)
- [x] `Metrics` struct with `AtomicU64` fields: `cache_hits`, `cache_misses`
- [x] `record_cache_hit()`, `record_cache_miss()`
- [x] `cache_hits_total() -> u64`, `cache_misses_total() -> u64`

### Step 5 — src/lib.rs
- [x] Add `pub mod cache;`
- [x] Add `pub mod metrics;`
- [x] Update `build_app` to construct `MokaTransformCache` and `Metrics`, wire into `AppState`

### Step 6 — src/api/mod.rs
- [x] Add `cache: Arc<dyn TransformCache>` and `metrics: Arc<Metrics>` to `AppState`
- [x] Import `crate::cache` and `crate::metrics`
- [x] Update `serve_asset`: cache lookup (hit → return + record_cache_hit) before storage; cache store after successful transform
- [x] Update `make_server` test helper to provide `MokaTransformCache` + `Metrics`
- [x] Add integration test: two identical requests — second returns from cache (metrics verify hit)
- [x] Add test: cache hit increments `cache_hits_total`; cache miss increments `cache_misses_total`

### Step 7 — Verify + fix
- [x] `cargo test` passes
- [x] `cargo clippy -- -D warnings` clean

---

## Acceptance Criteria Traceability

| Criterion | Covered by |
|---|---|
| Second identical request returns bytes from cache; libvips not invoked | `api::tests::second_request_hits_cache` |
| Cache bounded — inserting beyond `max_capacity` evicts LRU entry | `cache::tests::cache_evicts_entries_beyond_max_capacity` |
| Cache entries expire after `ttl` seconds | `cache::tests::entries_expire_after_ttl` |
| Cache key identical regardless of URL query parameter order | `cache::tests::cache_key_is_deterministic` (proptest) |
| `rendition_cache_hits_total` increments on cache hit | `api::tests::cache_hit_increments_metric` |

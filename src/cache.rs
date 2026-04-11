//! In-process transform cache.
//!
//! Caches fully-transformed image responses so that identical requests bypass
//! the libvips pipeline entirely.  The cache is keyed on a SHA-256 digest of
//! the asset path combined with the canonical serialisation of the transform
//! parameters, so two requests that differ only in URL query-parameter order
//! are treated as identical.
//!
//! # Capacity and expiry
//!
//! [`MokaTransformCache`] is backed by [`moka`]'s synchronous bounded cache.
//! Capacity and TTL are configured from [`crate::config::AppConfig`] fields
//! `cache_max_entries` and `cache_ttl_seconds`.
//!
//! # Path-based invalidation
//!
//! A secondary `path_index` maps each logical asset path to the set of cache
//! keys that were derived from it.  This allows all cached variants of an
//! asset (different widths, formats, etc.) to be invalidated atomically when
//! the asset is embargoed or purged (Unit 5).

use moka::sync::Cache as MokaCache;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::transform::TransformParams;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// SHA-256 digest used as the cache key.
///
/// 32 bytes = 256 bits — collision-resistant for this workload.
pub type CacheKey = [u8; 32];

/// A fully-transformed image response ready to serve directly to a client.
#[derive(Clone)]
pub struct CachedResponse {
    /// Encoded image bytes (e.g. a WebP or AVIF buffer).
    pub data: Vec<u8>,
    /// MIME type of the encoded output (e.g. `"image/webp"`).
    /// Uses `&'static str` so the HTTP header value can be built without
    /// allocation — matches what `transform::apply` returns.
    pub content_type: &'static str,
}

// ---------------------------------------------------------------------------
// TransformCache trait
// ---------------------------------------------------------------------------

/// Shared in-process transform cache.
///
/// All implementations must be `Send + Sync + 'static` so they can be placed
/// in Axum's [`State`](axum::extract::State) extractor.
pub trait TransformCache: Send + Sync + 'static {
    /// Look up a cached response by key.  Returns `None` on a miss or when the
    /// entry has expired.
    fn get(&self, key: &CacheKey) -> Option<CachedResponse>;

    /// Insert (or replace) an entry.
    ///
    /// `path` is the logical asset path (e.g. `"products/shoe.jpg"`).  It is
    /// stored in a secondary index to enable [`invalidate_by_path`].
    fn put(&self, key: CacheKey, path: &str, response: CachedResponse);

    /// Remove a single entry by key.  No-op if the key is not present.
    fn invalidate(&self, key: &CacheKey);

    /// Remove **all** entries whose path matches `path`.
    ///
    /// Used by the embargo system (Unit 5) when an asset is embargoed or
    /// released.  If no entries exist for the path this is a no-op.
    fn invalidate_by_path(&self, path: &str);

    /// Current number of entries in the cache (approximate; subject to
    /// background eviction scheduling in moka).
    fn entry_count(&self) -> u64;
}

// ---------------------------------------------------------------------------
// MokaTransformCache
// ---------------------------------------------------------------------------

/// [`moka`]-backed implementation of [`TransformCache`].
///
/// Uses a synchronous, bounded cache with per-entry time-to-live.
pub struct MokaTransformCache {
    inner: MokaCache<CacheKey, CachedResponse>,
    /// Secondary index mapping logical asset path → Vec of cache keys derived
    /// from that path.  Allows all variants of an asset to be invalidated
    /// together without scanning the entire cache.
    path_index: Mutex<HashMap<String, Vec<CacheKey>>>,
}

impl MokaTransformCache {
    /// Create a new cache bounded to `max_capacity` entries, where each entry
    /// expires `ttl` after insertion.
    pub fn new(max_capacity: u64, ttl: Duration) -> Self {
        Self {
            inner: MokaCache::builder()
                .max_capacity(max_capacity)
                .time_to_live(ttl)
                .build(),
            path_index: Mutex::new(HashMap::new()),
        }
    }

    /// Run pending background eviction tasks.
    ///
    /// Moka processes evictions asynchronously to amortise the cost.  In
    /// production this happens automatically; in tests we call this explicitly
    /// to assert capacity-bound behaviour without sleeping.
    #[cfg(test)]
    pub(crate) fn run_pending_tasks(&self) {
        self.inner.run_pending_tasks();
    }
}

impl TransformCache for MokaTransformCache {
    fn get(&self, key: &CacheKey) -> Option<CachedResponse> {
        self.inner.get(key)
    }

    fn put(&self, key: CacheKey, path: &str, response: CachedResponse) {
        self.inner.insert(key, response);
        if let Ok(mut index) = self.path_index.lock() {
            index.entry(path.to_owned()).or_default().push(key);
        }
    }

    fn invalidate(&self, key: &CacheKey) {
        self.inner.invalidate(key);
    }

    fn invalidate_by_path(&self, path: &str) {
        if let Ok(mut index) = self.path_index.lock() {
            if let Some(keys) = index.remove(path) {
                for key in keys {
                    self.inner.invalidate(&key);
                }
            }
        }
    }

    fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }
}

// ---------------------------------------------------------------------------
// Cache key computation
// ---------------------------------------------------------------------------

/// Compute a [`CacheKey`] as `SHA-256(path ∥ NUL ∥ canonical_params_bytes)`.
///
/// The NUL byte separates the path from the params to prevent a path like
/// `"a"` with params `"b=1"` colliding with path `"ab"` and params `"=1"`.
///
/// The key is stable regardless of the order in which query parameters were
/// supplied — [`TransformParams::canonical_bytes`] serialises all fields in
/// alphabetical order.
pub fn compute_cache_key(path: &str, params: &TransformParams) -> CacheKey {
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    hasher.update(b"\x00"); // NUL separator
    hasher.update(params.canonical_bytes());
    hasher.finalize().into()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::sync::Arc;
    use std::thread;

    // -- Test helpers --------------------------------------------------------

    fn make_cache() -> MokaTransformCache {
        MokaTransformCache::new(100, Duration::from_secs(3600))
    }

    fn params_with_fmt(fmt: &str) -> TransformParams {
        TransformParams {
            fmt: Some(fmt.to_owned()),
            ..Default::default()
        }
    }

    fn params_with_wid(wid: u32) -> TransformParams {
        TransformParams {
            wid: Some(wid),
            ..Default::default()
        }
    }

    fn sample_response() -> CachedResponse {
        CachedResponse {
            data: b"transformed image bytes".to_vec(),
            content_type: "image/jpeg",
        }
    }

    // -- Basic get/put -------------------------------------------------------

    #[test]
    fn get_returns_none_on_empty_cache() {
        let cache = make_cache();
        let key = compute_cache_key("photo.jpg", &TransformParams::default());
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn put_then_get_returns_correct_response() {
        let cache = make_cache();
        let key = compute_cache_key("photo.jpg", &TransformParams::default());
        cache.put(key, "photo.jpg", sample_response());

        let hit = cache.get(&key).expect("expected cache hit");
        assert_eq!(hit.data, b"transformed image bytes");
        assert_eq!(hit.content_type, "image/jpeg");
    }

    #[test]
    fn put_overwrites_existing_entry() {
        let cache = make_cache();
        let key = compute_cache_key("photo.jpg", &TransformParams::default());

        cache.put(key, "photo.jpg", CachedResponse {
            data: b"v1".to_vec(),
            content_type: "image/jpeg",
        });
        cache.put(key, "photo.jpg", CachedResponse {
            data: b"v2".to_vec(),
            content_type: "image/webp",
        });

        let hit = cache.get(&key).expect("expected cache hit");
        assert_eq!(hit.data, b"v2");
    }

    // -- Invalidation --------------------------------------------------------

    #[test]
    fn invalidate_removes_entry() {
        let cache = make_cache();
        let key = compute_cache_key("photo.jpg", &TransformParams::default());
        cache.put(key, "photo.jpg", sample_response());
        cache.invalidate(&key);
        assert!(cache.get(&key).is_none(), "entry should be gone after invalidate");
    }

    #[test]
    fn invalidate_nonexistent_key_is_noop() {
        let cache = make_cache();
        let key = compute_cache_key("ghost.jpg", &TransformParams::default());
        // Should not panic.
        cache.invalidate(&key);
    }

    #[test]
    fn invalidate_by_path_removes_all_variants_for_that_path() {
        let cache = make_cache();

        let key_default = compute_cache_key("photo.jpg", &TransformParams::default());
        let key_webp = compute_cache_key("photo.jpg", &params_with_fmt("webp"));
        let key_other = compute_cache_key("banner.png", &TransformParams::default());

        cache.put(key_default, "photo.jpg", sample_response());
        cache.put(key_webp, "photo.jpg", sample_response());
        cache.put(key_other, "banner.png", sample_response());

        cache.invalidate_by_path("photo.jpg");

        assert!(cache.get(&key_default).is_none(), "default variant should be evicted");
        assert!(cache.get(&key_webp).is_none(), "webp variant should be evicted");
        assert!(
            cache.get(&key_other).is_some(),
            "unrelated path must not be evicted"
        );
    }

    #[test]
    fn invalidate_by_path_on_unknown_path_is_noop() {
        let cache = make_cache();
        // Should not panic.
        cache.invalidate_by_path("does-not-exist.jpg");
    }

    // -- Bounded capacity (eviction) ----------------------------------------

    #[test]
    fn cache_does_not_exceed_max_capacity() {
        let max = 10u64;
        let cache = MokaTransformCache::new(max, Duration::from_secs(3600));

        // Insert 3× max entries with distinct params.
        for i in 0..max * 3 {
            let key = compute_cache_key("photo.jpg", &params_with_wid(i as u32));
            cache.put(key, "photo.jpg", CachedResponse {
                data: vec![i as u8],
                content_type: "image/jpeg",
            });
        }

        // Force moka's background eviction to run synchronously.
        cache.run_pending_tasks();

        assert!(
            cache.entry_count() <= max + 2,
            "entry count {} should not exceed max capacity {} (±2 margin for moka's batching)",
            cache.entry_count(),
            max
        );
    }

    // -- TTL expiration ------------------------------------------------------

    #[test]
    fn entries_expire_after_ttl() {
        let cache = MokaTransformCache::new(100, Duration::from_millis(50));
        let key = compute_cache_key("photo.jpg", &TransformParams::default());

        cache.put(key, "photo.jpg", sample_response());
        assert!(cache.get(&key).is_some(), "entry should be present immediately");

        // Wait past the TTL.
        thread::sleep(Duration::from_millis(150));

        // moka checks TTL on each get() call.
        assert!(
            cache.get(&key).is_none(),
            "entry should have expired after TTL elapsed"
        );
    }

    // -- Concurrent access ---------------------------------------------------

    #[test]
    fn concurrent_puts_and_gets_do_not_panic() {
        let cache = Arc::new(MokaTransformCache::new(50, Duration::from_secs(3600)));
        let mut handles = Vec::new();

        for i in 0u32..16 {
            let c = Arc::clone(&cache);
            let h = thread::spawn(move || {
                let key = compute_cache_key("photo.jpg", &params_with_wid(i));
                c.put(key, "photo.jpg", CachedResponse {
                    data: vec![i as u8],
                    content_type: "image/jpeg",
                });
                // Interleave reads with writes.
                let _ = c.get(&key);
            });
            handles.push(h);
        }

        for h in handles {
            h.join().expect("thread panicked during concurrent cache access");
        }
    }

    // -- Cache key properties ------------------------------------------------

    #[test]
    fn same_inputs_produce_same_key() {
        let p = params_with_fmt("webp");
        assert_eq!(
            compute_cache_key("photo.jpg", &p),
            compute_cache_key("photo.jpg", &p),
        );
    }

    #[test]
    fn different_paths_produce_different_keys() {
        let p = TransformParams::default();
        assert_ne!(
            compute_cache_key("photo.jpg", &p),
            compute_cache_key("banner.png", &p),
        );
    }

    #[test]
    fn different_fmt_params_produce_different_keys() {
        let path = "photo.jpg";
        assert_ne!(
            compute_cache_key(path, &params_with_fmt("jpeg")),
            compute_cache_key(path, &params_with_fmt("webp")),
        );
    }

    #[test]
    fn different_wid_params_produce_different_keys() {
        let path = "photo.jpg";
        assert_ne!(
            compute_cache_key(path, &params_with_wid(800)),
            compute_cache_key(path, &params_with_wid(400)),
        );
    }

    // -- Property-based tests (PBT-03 / NFR-02) ------------------------------

    /// Generates constrained but realistic `TransformParams`.
    fn arb_params() -> impl Strategy<Value = TransformParams> {
        (
            proptest::option::of(1u32..=4096u32),
            proptest::option::of(1u32..=4096u32),
            proptest::option::of(proptest::sample::select(vec![
                "crop".to_owned(),
                "constrain".to_owned(),
                "fill".to_owned(),
                "stretch".to_owned(),
            ])),
            proptest::option::of(proptest::sample::select(vec![
                "jpeg".to_owned(),
                "webp".to_owned(),
                "png".to_owned(),
                "avif".to_owned(),
            ])),
            proptest::option::of(1u8..=100u8),
        )
        .prop_map(|(wid, hei, fit, fmt, qlt)| TransformParams {
            wid,
            hei,
            fit,
            fmt,
            qlt,
            ..Default::default()
        })
    }

    /// Generates valid relative asset paths.
    fn arb_path() -> impl Strategy<Value = String> {
        proptest::string::string_regex("[a-z]{2,8}/[a-z]{2,8}\\.(jpg|png|webp)")
            .expect("valid path regex")
    }

    proptest! {
        /// NFR-02 / PBT-03: compute_cache_key is deterministic for identical inputs.
        #[test]
        fn prop_cache_key_is_deterministic(
            path in arb_path(),
            params in arb_params(),
        ) {
            let key1 = compute_cache_key(&path, &params);
            let key2 = compute_cache_key(&path, &params);
            prop_assert_eq!(key1, key2, "cache key must be deterministic for identical inputs");
        }

        /// NFR-02 / PBT-03: distinct paths with same params produce distinct keys.
        #[test]
        fn prop_distinct_paths_produce_distinct_keys(
            path1 in arb_path(),
            path2 in arb_path(),
            params in arb_params(),
        ) {
            prop_assume!(path1 != path2);
            let key1 = compute_cache_key(&path1, &params);
            let key2 = compute_cache_key(&path2, &params);
            prop_assert_ne!(key1, key2, "distinct paths must produce distinct keys");
        }

        /// NFR-02 / PBT-03: distinct params with same path produce distinct keys.
        #[test]
        fn prop_distinct_params_produce_distinct_keys(
            path in arb_path(),
            params1 in arb_params(),
            params2 in arb_params(),
        ) {
            // Only test pairs where the canonical bytes actually differ.
            prop_assume!(params1.canonical_bytes() != params2.canonical_bytes());
            let key1 = compute_cache_key(&path, &params1);
            let key2 = compute_cache_key(&path, &params2);
            prop_assert_ne!(key1, key2, "distinct params must produce distinct keys");
        }
    }
}

//! Application metrics.
//!
//! Tracks operational counters using atomic integers.  In Unit 7
//! (Observability & Ops) these counters will be wired into a Prometheus
//! registry and exposed on `GET /metrics`.  For Units 3–6 they provide
//! observable state for unit tests without requiring a metrics server.
//!
//! All methods use `Ordering::Relaxed` because the counters are not used to
//! synchronise memory between threads — they are purely informational.

use std::sync::atomic::{AtomicU64, Ordering};

/// In-process application metrics.
///
/// Wrap in `Arc<Metrics>` and clone the `Arc` across handlers; the atomics
/// are safe to share.
pub struct Metrics {
    /// Total number of transform cache hits since process start.
    /// Prometheus name: `rendition_cache_hits_total`.
    cache_hits: AtomicU64,

    /// Total number of transform cache misses since process start.
    /// Prometheus name: `rendition_cache_misses_total`.
    cache_misses: AtomicU64,
}

impl Metrics {
    /// Create a new `Metrics` instance with all counters at zero.
    pub fn new() -> Self {
        Self {
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
        }
    }

    /// Increment `rendition_cache_hits_total` by one.
    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment `rendition_cache_misses_total` by one.
    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Read the current value of `rendition_cache_hits_total`.
    pub fn cache_hits_total(&self) -> u64 {
        self.cache_hits.load(Ordering::Relaxed)
    }

    /// Read the current value of `rendition_cache_misses_total`.
    pub fn cache_misses_total(&self) -> u64 {
        self.cache_misses.load(Ordering::Relaxed)
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

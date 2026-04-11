//! Consecutive-failures circuit breaker used by [`super::S3Storage`].
//!
//! See ADR-0019 and Functional Design R-06 for the full spec. Summary:
//!
//! - **Closed** state counts consecutive failures; crossing the
//!   configured threshold transitions to **Open**.
//! - **Open** state rejects every call with [`StorageError::CircuitOpen`]
//!   for `cooldown` seconds, then lets the next call proceed as a
//!   half-open **probe**.
//! - **HalfOpen** state permits at most one in-flight probe. Success →
//!   Closed; failure → back to Open with a fresh cooldown.
//!
//! The breaker is cloud- and backend-agnostic — it only knows how to
//! wrap an async future returning `Result<T, StorageError>`.
//!
//! **Synchronisation:** `std::sync::Mutex<State>` with critical sections
//! that never cross `.await`. Do not replace with `tokio::sync::Mutex` —
//! that primitive exists to hold the lock across `.await`, which is
//! exactly what we must avoid.
//!
//! **Time source:** `tokio::time::Instant`, which lets tests use
//! `#[tokio::test(start_paused = true)]` and `tokio::time::advance` for
//! deterministic state-transition verification.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use super::{StorageError, StorageMetrics};

/// Internal state machine. Not part of the public API.
#[derive(Debug)]
enum State {
    Closed { consecutive_failures: u32 },
    Open { opened_at: Instant },
    HalfOpen { probe_in_flight: bool },
}

/// Consecutive-failures circuit breaker.
///
/// Construct once per backend instance; wrap each backend call in
/// [`CircuitBreaker::call`].
pub struct CircuitBreaker {
    state: Mutex<State>,
    threshold: u32,
    cooldown: Duration,
    metrics: Arc<dyn StorageMetrics>,
}

impl CircuitBreaker {
    /// Create a new breaker with the given failure threshold and
    /// cooldown. `metrics` is typically [`super::NoopMetrics`] in Unit 2
    /// and a real Prometheus sink in Unit 7.
    pub fn new(threshold: u32, cooldown: Duration, metrics: Arc<dyn StorageMetrics>) -> Self {
        Self {
            state: Mutex::new(State::Closed {
                consecutive_failures: 0,
            }),
            threshold,
            cooldown,
            metrics,
        }
    }

    /// Return `true` iff the breaker is currently in the [`State::Open`]
    /// state. `HalfOpen` reports as **not** open — the dependency is
    /// recovering and `/health/ready` should say the service is ready.
    pub fn is_open(&self) -> bool {
        let state = self.state.lock().expect("breaker state mutex poisoned");
        matches!(&*state, State::Open { .. })
    }

    /// Run `fut` through the breaker.
    ///
    /// Per Flow 4 of the functional design, the breaker counts only
    /// `Unavailable` and `Timeout` as failures. `NotFound`, `InvalidPath`,
    /// `Other`, and `CircuitOpen` are successes from the breaker's
    /// perspective — a missing object is not a dependency failure.
    pub async fn call<F, T>(&self, fut: F) -> Result<T, StorageError>
    where
        F: Future<Output = Result<T, StorageError>>,
    {
        // Pre-call: check state, decide whether to proceed.
        let mode = self.begin_call();
        match mode {
            BeginCall::Reject => Err(StorageError::CircuitOpen),
            BeginCall::Proceed { half_open } => {
                // Await the real work outside the mutex.
                let result = fut.await;
                // Post-call: update state based on outcome.
                self.end_call(&result, half_open);
                result
            }
        }
    }

    // Pre-call transitions. Returns whether to proceed and, if so,
    // whether the call is a half-open probe.
    fn begin_call(&self) -> BeginCall {
        let mut state = self.state.lock().expect("breaker state mutex poisoned");
        match &mut *state {
            State::Closed { .. } => BeginCall::Proceed { half_open: false },
            State::Open { opened_at } => {
                let elapsed = Instant::now().saturating_duration_since(*opened_at);
                if elapsed >= self.cooldown {
                    // Cooldown expired — enter half-open and let this
                    // call through as the probe.
                    *state = State::HalfOpen {
                        probe_in_flight: true,
                    };
                    self.metrics.set_circuit_open(false);
                    BeginCall::Proceed { half_open: true }
                } else {
                    BeginCall::Reject
                }
            }
            State::HalfOpen { probe_in_flight } => {
                if *probe_in_flight {
                    // Another probe already running — reject.
                    BeginCall::Reject
                } else {
                    *probe_in_flight = true;
                    BeginCall::Proceed { half_open: true }
                }
            }
        }
    }

    // Post-call transitions based on the observed outcome.
    fn end_call<T>(&self, result: &Result<T, StorageError>, half_open: bool) {
        let failed = matches!(
            result,
            Err(StorageError::Unavailable { .. }) | Err(StorageError::Timeout { .. })
        );

        let mut state = self.state.lock().expect("breaker state mutex poisoned");
        match &mut *state {
            State::Closed {
                consecutive_failures,
            } if failed => {
                *consecutive_failures += 1;
                if *consecutive_failures >= self.threshold {
                    *state = State::Open {
                        opened_at: Instant::now(),
                    };
                    self.metrics.set_circuit_open(true);
                }
            }
            State::Closed {
                consecutive_failures,
            } => {
                *consecutive_failures = 0;
            }
            State::HalfOpen { .. } if half_open => {
                if failed {
                    *state = State::Open {
                        opened_at: Instant::now(),
                    };
                    self.metrics.set_circuit_open(true);
                } else {
                    *state = State::Closed {
                        consecutive_failures: 0,
                    };
                    self.metrics.set_circuit_open(false);
                }
            }
            // A non-probe call somehow ended while state is HalfOpen.
            // This should not happen under normal flow (we reject
            // non-probe calls during HalfOpen), but if it does, leave
            // the state untouched so the actual probe can still resolve.
            State::HalfOpen { .. } => {}
            // A call finished in Open state (e.g. a probe that was
            // rejected before the state changed). No state change.
            State::Open { .. } => {}
        }
    }
}

enum BeginCall {
    Proceed { half_open: bool },
    Reject,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::NoopMetrics;

    fn breaker(threshold: u32, cooldown_ms: u64) -> CircuitBreaker {
        CircuitBreaker::new(
            threshold,
            Duration::from_millis(cooldown_ms),
            Arc::new(NoopMetrics),
        )
    }

    fn unavailable() -> StorageError {
        StorageError::Unavailable {
            source: "backend down".into(),
        }
    }

    #[tokio::test]
    async fn closed_by_default() {
        let cb = breaker(5, 1000);
        assert!(!cb.is_open());
    }

    #[tokio::test]
    async fn ok_calls_dont_trip_the_breaker() {
        let cb = breaker(3, 1000);
        for _ in 0..10 {
            let result: Result<i32, StorageError> = cb.call(async { Ok(42) }).await;
            assert_eq!(result.unwrap(), 42);
        }
        assert!(!cb.is_open());
    }

    #[tokio::test]
    async fn opens_after_threshold_consecutive_unavailable() {
        let cb = breaker(3, 1000);
        for _ in 0..3 {
            let _: Result<i32, _> = cb.call(async { Err(unavailable()) }).await;
        }
        assert!(cb.is_open());
    }

    #[tokio::test]
    async fn not_found_does_not_count_as_failure() {
        let cb = breaker(2, 1000);
        for _ in 0..10 {
            let _: Result<i32, _> = cb
                .call(async { Err::<i32, _>(StorageError::NotFound) })
                .await;
        }
        assert!(!cb.is_open());
    }

    #[tokio::test]
    async fn timeout_counts_as_failure() {
        let cb = breaker(2, 1000);
        for _ in 0..2 {
            let _: Result<i32, _> = cb
                .call(async { Err::<i32, _>(StorageError::Timeout { op: "get" }) })
                .await;
        }
        assert!(cb.is_open());
    }

    #[tokio::test]
    async fn success_resets_consecutive_failures() {
        let cb = breaker(3, 1000);
        let _: Result<i32, _> = cb.call(async { Err(unavailable()) }).await;
        let _: Result<i32, _> = cb.call(async { Err(unavailable()) }).await;
        let _: Result<i32, _> = cb.call(async { Ok(1) }).await;
        // Counter reset — two more failures should NOT trip the breaker.
        let _: Result<i32, _> = cb.call(async { Err(unavailable()) }).await;
        let _: Result<i32, _> = cb.call(async { Err(unavailable()) }).await;
        assert!(!cb.is_open());
    }

    #[tokio::test(start_paused = true)]
    async fn open_rejects_calls_during_cooldown() {
        let cb = breaker(1, 1000);
        let _: Result<i32, _> = cb.call(async { Err(unavailable()) }).await;
        assert!(cb.is_open());
        let result: Result<i32, _> = cb.call(async { Ok(1) }).await;
        assert_eq!(result.unwrap_err(), StorageError::CircuitOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn half_open_probe_closes_on_success() {
        let cb = breaker(1, 1000);
        let _: Result<i32, _> = cb.call(async { Err(unavailable()) }).await;
        assert!(cb.is_open());
        tokio::time::advance(Duration::from_millis(1100)).await;
        // First call after cooldown enters half-open as the probe.
        let result: Result<i32, _> = cb.call(async { Ok(42) }).await;
        assert_eq!(result.unwrap(), 42);
        assert!(!cb.is_open());
    }

    #[tokio::test(start_paused = true)]
    async fn half_open_probe_failure_reopens_with_fresh_cooldown() {
        let cb = breaker(1, 1000);
        let _: Result<i32, _> = cb.call(async { Err(unavailable()) }).await;
        tokio::time::advance(Duration::from_millis(1100)).await;
        // Probe fails — breaker returns to Open with a fresh cooldown.
        let _: Result<i32, _> = cb.call(async { Err(unavailable()) }).await;
        assert!(cb.is_open());
        // A half-cooldown advance should NOT let another probe through.
        tokio::time::advance(Duration::from_millis(500)).await;
        let result: Result<i32, _> = cb.call(async { Ok(1) }).await;
        assert_eq!(result.unwrap_err(), StorageError::CircuitOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_half_open_probes_get_circuit_open() {
        // Setup: trip the breaker, wait for cooldown.
        let cb = Arc::new(breaker(1, 1000));
        let _: Result<i32, _> = cb.call(async { Err(unavailable()) }).await;
        tokio::time::advance(Duration::from_millis(1100)).await;

        // Start the probe with a delay that we can control.
        let cb1 = cb.clone();
        let probe = tokio::spawn(async move {
            cb1.call(async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok::<i32, StorageError>(42)
            })
            .await
        });

        // Yield so the probe starts and takes the half-open slot.
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;

        // Second concurrent call should be rejected.
        let second: Result<i32, _> = cb.call(async { Ok(1) }).await;
        assert_eq!(second.unwrap_err(), StorageError::CircuitOpen);

        // Finish the probe.
        tokio::time::advance(Duration::from_millis(300)).await;
        let probe_result = probe.await.unwrap().unwrap();
        assert_eq!(probe_result, 42);
        assert!(!cb.is_open());
    }
}

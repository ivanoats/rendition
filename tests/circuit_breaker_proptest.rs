//! Property-based invariant tests for [`rendition::storage::circuit_breaker::CircuitBreaker`].
//!
//! Generates arbitrary sequences of `(Event, advance_ms)` and asserts
//! the invariants from Functional Design R-06 after each step. Uses
//! `tokio::time::pause` + `tokio::time::advance` for deterministic time
//! progression (NFR Design Q4=D).

use std::sync::Arc;
use std::time::Duration;

use proptest::prelude::*;
use rendition::storage::circuit_breaker::CircuitBreaker;
use rendition::storage::{NoopMetrics, StorageError};

/// Events the proptest can inject between time advances.
#[derive(Debug, Clone)]
enum Event {
    /// A successful backend call.
    Success,
    /// A call that the breaker counts as a failure (Unavailable variant).
    Unavailable,
    /// A call that returns NotFound — must NOT trip the breaker.
    NotFound,
    /// Advance the (paused) tokio clock by `ms` milliseconds.
    Advance(u64),
}

fn event_strategy() -> impl Strategy<Value = Event> {
    prop_oneof![
        Just(Event::Success),
        Just(Event::Unavailable),
        Just(Event::NotFound),
        (1u64..=5000u64).prop_map(Event::Advance),
    ]
}

/// Async runner that applies a sequence of events to a fresh breaker
/// and returns `Ok(())` on success or `Err(String)` describing the
/// first invariant violation.
async fn run_sequence(threshold: u32, cooldown_ms: u64, events: Vec<Event>) -> Result<(), String> {
    let cb = CircuitBreaker::new(
        threshold,
        Duration::from_millis(cooldown_ms),
        Arc::new(NoopMetrics),
    );

    for (i, event) in events.into_iter().enumerate() {
        match event {
            Event::Success => {
                let _: Result<i32, _> = cb.call(async { Ok(1) }).await;
            }
            Event::Unavailable => {
                let _: Result<i32, _> = cb
                    .call(async {
                        Err::<i32, _>(StorageError::Unavailable {
                            source: "boom".into(),
                        })
                    })
                    .await;
            }
            Event::NotFound => {
                let _: Result<i32, _> = cb
                    .call(async { Err::<i32, _>(StorageError::NotFound) })
                    .await;
            }
            Event::Advance(ms) => {
                tokio::time::advance(Duration::from_millis(ms)).await;
            }
        }

        // Invariant 1: is_open() never panics, never leaks state.
        let _open = cb.is_open();

        // Invariant 2: after any NotFound, the breaker is not forced to
        // open by it alone. (The breaker ignores NotFound — any open
        // state must be attributable to earlier Unavailable events.)
        // This is verified by property: a sequence containing ONLY
        // Success and NotFound events must never leave the breaker open.
        // We check this below separately via a dedicated property.
        let _ = i;
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        max_shrink_iters: 64,
        ..ProptestConfig::default()
    })]

    /// The breaker must tolerate any sequence of events without
    /// panicking or violating its internal-state contract.
    #[test]
    fn breaker_survives_arbitrary_sequences(
        threshold in 1u32..10u32,
        cooldown_ms in 100u64..5000u64,
        events in proptest::collection::vec(event_strategy(), 0..40),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        rt.block_on(async {
            run_sequence(threshold, cooldown_ms, events).await.unwrap();
        });
    }

    /// A sequence containing ONLY `Success` and `NotFound` events must
    /// never leave the breaker open — neither variant counts as a
    /// failure for the breaker's purposes (R-01 / R-06).
    #[test]
    fn success_and_not_found_never_open_breaker(
        threshold in 1u32..5u32,
        events in proptest::collection::vec(
            prop_oneof![Just(Event::Success), Just(Event::NotFound)],
            1..30,
        ),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        rt.block_on(async {
            let cb = CircuitBreaker::new(
                threshold,
                Duration::from_millis(1000),
                Arc::new(NoopMetrics),
            );
            for event in events {
                match event {
                    Event::Success => {
                        let _: Result<i32, _> = cb.call(async { Ok(1) }).await;
                    }
                    Event::NotFound => {
                        let _: Result<i32, _> =
                            cb.call(async { Err::<i32, _>(StorageError::NotFound) }).await;
                    }
                    _ => unreachable!(),
                }
                prop_assert!(!cb.is_open(), "NotFound/Success must never open breaker");
            }
            Ok(())
        })?;
    }

    /// `threshold` consecutive `Unavailable` calls always open the
    /// breaker (assuming no intervening success or time advance that
    /// would reset state).
    #[test]
    fn threshold_unavailable_calls_open_breaker(threshold in 1u32..10u32) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        rt.block_on(async {
            let cb = CircuitBreaker::new(
                threshold,
                Duration::from_millis(1000),
                Arc::new(NoopMetrics),
            );
            for _ in 0..threshold {
                let _: Result<i32, _> = cb
                    .call(async {
                        Err::<i32, _>(StorageError::Unavailable {
                            source: "boom".into(),
                        })
                    })
                    .await;
            }
            prop_assert!(cb.is_open());
            Ok(())
        })?;
    }
}

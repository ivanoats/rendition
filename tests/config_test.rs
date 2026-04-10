//! Integration tests for `rendition::config::AppConfig`.
//!
//! These tests manipulate the process environment, so they MUST run
//! single-threaded to avoid interfering with each other. The `ENV_LOCK`
//! `Mutex` serialises access; `with_env` is the only sanctioned helper
//! for setting `RENDITION_*` vars during a test.

use proptest::prelude::*;
use rendition::config::{AppConfig, ConfigError, RateLimitKey, StorageBackendKind};
use std::sync::Mutex;

/// Global serialisation point for env-var manipulation. All tests acquire
/// this before touching `std::env`.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with the given `RENDITION_*` env vars set, then unset them.
/// All RENDITION_ vars are cleared before each test runs to give a clean
/// slate.
fn with_env<F, R>(vars: &[(&str, &str)], f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Snapshot and clear all RENDITION_* vars so previous test residue
    // can't influence the current test.
    let snapshot: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k.starts_with("RENDITION_"))
        .collect();
    for (k, _) in &snapshot {
        std::env::remove_var(k);
    }

    for (k, v) in vars {
        std::env::set_var(k, v);
    }

    let result = f();

    for (k, _) in vars {
        std::env::remove_var(k);
    }
    // Restore the snapshot so concurrent test runners (different processes)
    // don't lose state.
    for (k, v) in snapshot {
        std::env::set_var(k, v);
    }

    result
}

// ---- Happy path ------------------------------------------------------------

#[test]
fn loads_with_only_required_defaults() {
    with_env(&[], || {
        let cfg = AppConfig::load().expect("default config should load");
        assert_eq!(cfg.bind_addr.to_string(), "0.0.0.0:3000");
        assert_eq!(cfg.admin_bind_addr.to_string(), "127.0.0.1:3001");
        assert_eq!(cfg.storage_backend, StorageBackendKind::Local);
        assert_eq!(cfg.assets_path.to_str(), Some("./assets"));
        assert_eq!(cfg.cache_max_entries, 1000);
        assert_eq!(cfg.cache_ttl_seconds, 3600);
        assert_eq!(cfg.max_payload_bytes, 50 * 1024 * 1024);
        assert_eq!(cfg.rate_limit_rps, 100);
        assert_eq!(cfg.rate_limit_burst, 200);
        assert_eq!(cfg.rate_limit_key, RateLimitKey::PeerIp);
        assert_eq!(cfg.embargo_cache_ttl_seconds, 30);
        assert!(!cfg.oidc.is_configured());
        assert!(cfg.admin_api_keys.is_empty());
    });
}

#[test]
fn loads_with_full_s3_config() {
    with_env(
        &[
            ("RENDITION_STORAGE_BACKEND", "s3"),
            ("RENDITION_S3_BUCKET", "my-assets"),
            ("RENDITION_S3_REGION", "us-west-2"),
            ("RENDITION_S3_ENDPOINT", "https://minio.example.com"),
            ("RENDITION_S3_PREFIX", "prod/"),
        ],
        || {
            let cfg = AppConfig::load().expect("S3 config should load");
            assert_eq!(cfg.storage_backend, StorageBackendKind::S3);
            assert_eq!(cfg.s3_bucket.as_deref(), Some("my-assets"));
            assert_eq!(cfg.s3_region.as_deref(), Some("us-west-2"));
            assert_eq!(
                cfg.s3_endpoint.as_deref(),
                Some("https://minio.example.com")
            );
            assert_eq!(cfg.s3_prefix, "prod/");
        },
    );
}

#[test]
fn loads_with_oidc_config() {
    with_env(
        &[
            (
                "RENDITION_OIDC_ISSUER",
                "https://company.okta.com/oauth2/default",
            ),
            ("RENDITION_OIDC_AUDIENCE", "rendition-admin"),
            ("RENDITION_OIDC_ADMIN_GROUP", "rendition-admins"),
        ],
        || {
            let cfg = AppConfig::load().expect("OIDC config should load");
            assert!(cfg.oidc.is_configured());
            assert_eq!(cfg.oidc.oidc_audience.as_deref(), Some("rendition-admin"));
            assert_eq!(
                cfg.oidc.oidc_admin_group.as_deref(),
                Some("rendition-admins")
            );
        },
    );
}

#[test]
fn loads_with_redis_url() {
    with_env(&[("RENDITION_REDIS_URL", "redis://localhost:6379")], || {
        let cfg = AppConfig::load().expect("redis url should load");
        assert_eq!(cfg.redis_url.as_deref(), Some("redis://localhost:6379"));
    });
}

#[test]
fn loads_admin_api_keys_as_comma_separated_list() {
    with_env(
        &[("RENDITION_ADMIN_API_KEYS", "abc123,def456,ghi789")],
        || {
            let cfg = AppConfig::load().expect("api keys should parse");
            assert_eq!(cfg.admin_api_keys.len(), 3);
            assert_eq!(cfg.admin_api_keys[0], "abc123");
            assert_eq!(cfg.admin_api_keys[2], "ghi789");
        },
    );
}

// ---- Validation failures ---------------------------------------------------

#[test]
fn s3_backend_without_bucket_fails() {
    with_env(
        &[
            ("RENDITION_STORAGE_BACKEND", "s3"),
            ("RENDITION_S3_REGION", "us-west-2"),
        ],
        || {
            let err = AppConfig::load().unwrap_err();
            assert!(matches!(err, ConfigError::Validation(_)));
            assert!(err.to_string().contains("S3_BUCKET"));
        },
    );
}

#[test]
fn s3_backend_without_region_fails() {
    with_env(
        &[
            ("RENDITION_STORAGE_BACKEND", "s3"),
            ("RENDITION_S3_BUCKET", "my-assets"),
        ],
        || {
            let err = AppConfig::load().unwrap_err();
            assert!(matches!(err, ConfigError::Validation(_)));
            assert!(err.to_string().contains("S3_REGION"));
        },
    );
}

#[test]
fn oidc_issuer_without_audience_fails() {
    with_env(
        &[(
            "RENDITION_OIDC_ISSUER",
            "https://company.okta.com/oauth2/default",
        )],
        || {
            let err = AppConfig::load().unwrap_err();
            assert!(matches!(err, ConfigError::Validation(_)));
            assert!(err.to_string().contains("OIDC"));
        },
    );
}

#[test]
fn rate_limit_burst_below_rps_fails() {
    with_env(
        &[
            ("RENDITION_RATE_LIMIT_RPS", "100"),
            ("RENDITION_RATE_LIMIT_BURST", "50"),
        ],
        || {
            let err = AppConfig::load().unwrap_err();
            assert!(matches!(err, ConfigError::Validation(_)));
            assert!(err.to_string().contains("BURST"));
        },
    );
}

#[test]
fn zero_cache_max_entries_fails() {
    with_env(&[("RENDITION_CACHE_MAX_ENTRIES", "0")], || {
        let err = AppConfig::load().unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    });
}

#[test]
fn payload_below_1kib_fails() {
    with_env(&[("RENDITION_MAX_PAYLOAD_BYTES", "512")], || {
        let err = AppConfig::load().unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
        assert!(err.to_string().contains("MAX_PAYLOAD_BYTES"));
    });
}

#[test]
fn invalid_bind_addr_fails() {
    with_env(&[("RENDITION_BIND_ADDR", "not-a-socket-addr")], || {
        let err = AppConfig::load().unwrap_err();
        assert!(matches!(err, ConfigError::EnvVar(_)));
    });
}

#[test]
fn invalid_storage_backend_fails() {
    with_env(&[("RENDITION_STORAGE_BACKEND", "azure")], || {
        let err = AppConfig::load().unwrap_err();
        assert!(matches!(err, ConfigError::EnvVar(_)));
    });
}

#[test]
fn invalid_redis_url_fails() {
    with_env(&[("RENDITION_REDIS_URL", "not a url")], || {
        let err = AppConfig::load().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidUrl { .. }));
    });
}

// ---- Security: Debug redacts secrets ---------------------------------------

#[test]
fn debug_output_redacts_admin_api_keys() {
    with_env(
        &[(
            "RENDITION_ADMIN_API_KEYS",
            "supersecretkey1,supersecretkey2",
        )],
        || {
            let cfg = AppConfig::load().expect("config should load");
            let dbg = format!("{cfg:?}");
            assert!(
                !dbg.contains("supersecretkey1"),
                "Debug output must redact api keys; got: {dbg}"
            );
            assert!(
                !dbg.contains("supersecretkey2"),
                "Debug output must redact api keys; got: {dbg}"
            );
            assert!(
                dbg.contains("redacted"),
                "Debug output should mention redaction; got: {dbg}"
            );
        },
    );
}

#[test]
fn debug_output_redacts_redis_url() {
    with_env(
        &[(
            "RENDITION_REDIS_URL",
            "redis://user:password@localhost:6379",
        )],
        || {
            let cfg = AppConfig::load().expect("config should load");
            let dbg = format!("{cfg:?}");
            assert!(
                !dbg.contains("password"),
                "Debug output must not contain redis password; got: {dbg}"
            );
        },
    );
}

// ---- Property-based tests --------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        .. ProptestConfig::default()
    })]

    /// For any combination of valid numeric configuration values, loading
    /// should succeed and the loaded values should round-trip exactly.
    #[test]
    fn valid_numeric_env_round_trips(
        cache_max in 1u64..10_000u64,
        cache_ttl in 1u64..100_000u64,
        payload in 1024u64..(100 * 1024 * 1024),
        rps in 1u32..10_000u32,
        burst_extra in 0u32..10_000u32,
    ) {
        let burst = rps + burst_extra;
        let result = with_env(
            &[
                ("RENDITION_CACHE_MAX_ENTRIES", &cache_max.to_string()),
                ("RENDITION_CACHE_TTL_SECONDS", &cache_ttl.to_string()),
                ("RENDITION_MAX_PAYLOAD_BYTES", &payload.to_string()),
                ("RENDITION_RATE_LIMIT_RPS", &rps.to_string()),
                ("RENDITION_RATE_LIMIT_BURST", &burst.to_string()),
            ],
            AppConfig::load,
        );
        let cfg = result.expect("valid numeric env should load");
        prop_assert_eq!(cfg.cache_max_entries, cache_max);
        prop_assert_eq!(cfg.cache_ttl_seconds, cache_ttl);
        prop_assert_eq!(cfg.max_payload_bytes, payload);
        prop_assert_eq!(cfg.rate_limit_rps, rps);
        prop_assert_eq!(cfg.rate_limit_burst, burst);
    }

    /// For any rate-limit pair where burst < rps, validation must reject.
    #[test]
    fn burst_less_than_rps_always_fails(
        rps in 2u32..10_000u32,
        burst_deficit in 1u32..1_000u32,
    ) {
        let burst = rps.saturating_sub(burst_deficit);
        prop_assume!(burst < rps);
        let err = with_env(
            &[
                ("RENDITION_RATE_LIMIT_RPS", &rps.to_string()),
                ("RENDITION_RATE_LIMIT_BURST", &burst.to_string()),
            ],
            AppConfig::load,
        )
        .unwrap_err();
        prop_assert!(matches!(err, ConfigError::Validation(_)));
    }

    /// `validate()` is a pure function: calling it twice on the same value
    /// always returns the same result.
    #[test]
    fn validate_is_deterministic(
        cache_max in 0u64..10_000u64,
        rps in 0u32..1_000u32,
        burst in 0u32..1_000u32,
    ) {
        let result1 = with_env(
            &[
                ("RENDITION_CACHE_MAX_ENTRIES", &cache_max.to_string()),
                ("RENDITION_RATE_LIMIT_RPS", &rps.to_string()),
                ("RENDITION_RATE_LIMIT_BURST", &burst.to_string()),
            ],
            AppConfig::load,
        );
        let result2 = with_env(
            &[
                ("RENDITION_CACHE_MAX_ENTRIES", &cache_max.to_string()),
                ("RENDITION_RATE_LIMIT_RPS", &rps.to_string()),
                ("RENDITION_RATE_LIMIT_BURST", &burst.to_string()),
            ],
            AppConfig::load,
        );
        prop_assert_eq!(result1.is_ok(), result2.is_ok());
    }
}

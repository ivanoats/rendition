# Unit 1 — Config — Code Summary

## Status

✅ Complete. All 8 plan steps marked done. CI green locally.

## Files

### Created

- `src/config.rs` — `AppConfig`, `S3Config` (flattened into AppConfig),
  `OidcConfig`, `StorageBackendKind`, `RateLimitKey`, `ConfigError`,
  `AppConfig::load()`, `AppConfig::validate()`, redacting `Debug` impls,
  `Default` impl for tests
- `tests/config_test.rs` — 19 tests: 16 unit/scenario + 3 proptest
  invariants. Single-threaded `ENV_LOCK` `Mutex` serialises env-var
  manipulation
- `aidlc-docs/construction/config/code/code-summary.md` — this file

### Modified

- `Cargo.toml` — added `envy = "0.4"`, `thiserror = "1"`,
  `url = { version = "2", features = ["serde"] }`, dev `proptest = "1"`
- `src/lib.rs` — added `pub mod config;`; `build_app` signature changed
  from `&str` to `&AppConfig`
- `src/main.rs` — replaced direct `std::env::var` with `AppConfig::load()`,
  fail-fast exit on error, sanitised log of loaded config, returns
  `ExitCode` for clean error reporting
- `tests/api_integration.rs` — constructs `AppConfig { assets_path, ..Default::default() }`
- `tests/e2e.rs` — same pattern

## Public API surface

```rust
// rendition::config
pub struct AppConfig { ...22 fields... }
pub struct OidcConfig { ...3 fields... }
pub enum StorageBackendKind { Local, S3 }
pub enum RateLimitKey { PeerIp, XForwardedFor }
pub enum ConfigError { EnvVar, Validation, InvalidUrl }

impl AppConfig {
    pub fn load() -> Result<AppConfig, ConfigError>;
    pub fn validate(&self) -> Result<(), ConfigError>;
    pub fn cache_ttl(&self) -> Duration;
    pub fn embargo_cache_ttl(&self) -> Duration;
}

impl OidcConfig {
    pub fn is_configured(&self) -> bool;
}

impl Default for AppConfig { ... }
```

## Configuration variables supported

All 22 `RENDITION_*` variables documented in `README.md`:

| Required when | Variable |
|---|---|
| always (default `0.0.0.0:3000`) | `RENDITION_BIND_ADDR` |
| always (default `127.0.0.1:3001`) | `RENDITION_ADMIN_BIND_ADDR` |
| always (default `local`) | `RENDITION_STORAGE_BACKEND` |
| always (default `./assets`) | `RENDITION_ASSETS_PATH` |
| `STORAGE_BACKEND=s3` | `RENDITION_S3_BUCKET` |
| `STORAGE_BACKEND=s3` | `RENDITION_S3_REGION` |
| optional | `RENDITION_S3_ENDPOINT` |
| optional | `RENDITION_S3_PREFIX` |
| always (default 1000) | `RENDITION_CACHE_MAX_ENTRIES` |
| always (default 3600) | `RENDITION_CACHE_TTL_SECONDS` |
| always (default 50 MiB) | `RENDITION_MAX_PAYLOAD_BYTES` |
| always (default 100) | `RENDITION_RATE_LIMIT_RPS` |
| always (default 200) | `RENDITION_RATE_LIMIT_BURST` |
| always (default `peer_ip`) | `RENDITION_RATE_LIMIT_KEY` |
| always (default sane HTTP) | `RENDITION_CACHE_CONTROL_PUBLIC` |
| optional | `RENDITION_PUBLIC_BASE_URL` |
| optional | `RENDITION_REDIS_URL` |
| always (default 30) | `RENDITION_EMBARGO_CACHE_TTL_SECONDS` |
| OIDC mode | `RENDITION_OIDC_ISSUER` |
| OIDC mode | `RENDITION_OIDC_AUDIENCE` |
| OIDC mode | `RENDITION_OIDC_ADMIN_GROUP` |
| API key mode | `RENDITION_ADMIN_API_KEYS` (comma-separated) |
| optional | `RENDITION_OTEL_ENDPOINT` |

## Cross-field validation rules

`AppConfig::validate()` enforces:

1. `S3` backend requires `s3_bucket` and `s3_region`
2. `oidc_issuer` and `oidc_audience` must come together
3. `cache_max_entries >= 1`
4. `cache_ttl_seconds >= 1`
5. `max_payload_bytes >= 1024`
6. `rate_limit_rps >= 1`
7. `rate_limit_burst >= rate_limit_rps`
8. `redis_url` parses as a URL if present

## Security posture

- Custom `Debug` impl on `AppConfig` redacts `admin_api_keys` (rendered
  as `[N entries: <redacted>]`) and `redis_url` (rendered as
  `<redacted>` since the URL may contain credentials in `user:pass@host`)
- Custom `Debug` impl on `OidcConfig` does NOT redact `oidc_issuer`,
  `oidc_audience`, or `oidc_admin_group` — these are not secrets,
  they're public IdP metadata
- The `Display` impl is intentionally not provided; use `Debug` for
  logging
- Verified by two redaction tests: `debug_output_redacts_admin_api_keys`
  and `debug_output_redacts_redis_url`

## Test coverage summary

| Category | Tests |
|---|---|
| Happy path (default + each variant) | 5 |
| Validation failures | 9 |
| Security (redaction) | 2 |
| Property-based invariants | 3 |
| **Total** | **19** |

All 3 proptest invariants pass with default `cases: 64`:

1. `valid_numeric_env_round_trips` — any valid numeric env set produces a
   loadable config with exact field values
2. `burst_less_than_rps_always_fails` — `burst < rps` always rejected
3. `validate_is_deterministic` — same input always produces same outcome

## Acceptance criteria check

From `unit-of-work.md` Unit 1:

- [x] `AppConfig::load()` returns `Ok` for any valid env var set
- [x] `AppConfig::load()` returns `Err` with a human-readable message for
  any invalid set
- [x] Process exits at startup — no panics at request time due to config
  issues
- [x] All `RENDITION_*` variables documented (in README.md and this file)

## Known follow-ups for later units

These are intentionally deferred:

- **Wire `bind_addr` into `main`** for the actual listener — done
- **Wire `admin_bind_addr` into a second listener** — Unit 5 (Embargo +
  Admin API)
- **Wire `s3_*` fields into `S3Storage::new`** — Unit 2 (S3 Storage Backend)
- **Wire `cache_*` fields into `MokaTransformCache::new`** — Unit 3
  (Transform Cache)
- **Wire `redis_url` into `RedisEmbargoStore::new`** — Unit 5
- **Wire `oidc_*` fields into `JwksCache::new`** — Unit 5
- **Wire `rate_limit_*` fields into `tower-governor`** — Unit 6
  (Middleware)
- **Wire `otel_endpoint` into `init_otel`** — Unit 7 (Observability & Ops)

The `_` warnings for unused fields are silenced by the fact that all
fields are read in either `validate()` or the `Debug` impl.

## Build & test result

```text
$ cargo test --all -- --test-threads=1
test result: ok. 44 passed; 0 failed   (lib unit tests)
test result: ok.  7 passed; 0 failed   (api_integration)
test result: ok. 19 passed; 0 failed   (config_test)
test result: ok. 12 passed; 0 failed   (e2e)
                ──────────────────
total           82 passed; 0 failed

$ cargo clippy --all-targets --all-features -- -D warnings
Finished — clean

$ cargo fmt --all -- --check
Finished — clean
```

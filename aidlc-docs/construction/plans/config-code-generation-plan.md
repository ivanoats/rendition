# Unit 1 — Config — Code Generation Plan

## Unit Context

- **Unit:** 1 — Config
- **Goal:** Establish the typed configuration foundation every other unit depends on
- **Dependencies:** None — Unit 1 is the foundation
- **Stories covered:** FR-02, NFR-01 (coverage), NFR-02 (PBT), NFR-03 (security), NFR-06 (portability)
- **Project type:** Brownfield Rust crate at `/Users/ivan/dev/rendition`
- **Application code location:**
  - `src/config.rs` (new)
  - `Cargo.toml` (modify)
  - `src/main.rs` (modify)
  - `src/lib.rs` (modify)
  - `tests/config_test.rs` (new)
- **Documentation location:** `aidlc-docs/construction/config/code/`

## Steps

### Step 1 — Cargo.toml dependency updates

- [x] Add to `[dependencies]`:
  - `envy = "0.4"`
  - `thiserror = "1"`
  - `url = { version = "2", features = ["serde"] }`
- [x] Add to `[dev-dependencies]`:
  - `proptest = "1"`
- [x] Verify `cargo check` succeeds

### Step 2 — Create `src/config.rs`

- [x] Define `AppConfig` struct with `#[derive(Debug, Clone, Deserialize)]`
- [x] Define `StorageBackendKind` enum (`Local` | `S3`)
- [x] Define nested `S3Config` struct
- [x] Define nested `OidcConfig` struct
- [x] Define `RateLimitKey` enum (`PeerIp` | `XForwardedFor`)
- [x] Define `ConfigError` enum using `thiserror::Error`
- [x] All `RENDITION_*` fields from the README configuration table
- [x] `serde(default = "...")` attributes for fields with defaults
- [x] Doc comment on every field
- [x] Custom `Debug` impl on `S3Config` redacting `secret_access_key` if present
- [x] Custom `Debug` impl on `AppConfig` redacting `admin_api_keys`

### Step 3 — Implement `AppConfig::load()` and `validate()`

- [x] `pub fn load() -> Result<AppConfig, ConfigError>`
  - Calls `envy::prefixed("RENDITION_").from_env::<AppConfig>()`
  - Maps `envy::Error` to `ConfigError::EnvVar`
  - Calls `validate()` after successful deserialisation
- [x] `pub fn validate(&self) -> Result<(), ConfigError>` cross-field rules:
  - If `storage_backend == S3`: `s3.bucket` and `s3.region` MUST be set
  - If `oidc.issuer` is set: `oidc.audience` MUST also be set
  - `cache_max_entries` ≥ 1
  - `cache_ttl_seconds` ≥ 1
  - `max_payload_bytes` ≥ 1024 (1 KiB minimum)
  - `rate_limit_rps` ≥ 1
  - `rate_limit_burst` ≥ `rate_limit_rps`
  - If `redis_url` is set: parse with `url::Url::parse` to validate
- [x] `pub fn s3(&self) -> Result<&S3Config, ConfigError>` accessor
- [x] `pub fn oidc(&self) -> Option<&OidcConfig>` accessor

### Step 4 — Create `tests/config_test.rs`

- [x] Helper: `with_env<F>(vars: &[(&str, &str)], f: F)` that sets env vars,
  runs the closure, and unsets them (use `std::sync::Mutex` to serialise
  parallel test access)
- [x] Test: minimal valid config (only required fields) loads successfully
- [x] Test: missing required field returns `ConfigError::EnvVar`
- [x] Test: invalid type returns `ConfigError::EnvVar`
- [x] Test: `storage_backend=s3` without `s3_bucket` returns `ConfigError::Validation`
- [x] Test: `oidc_issuer` without `oidc_audience` returns `ConfigError::Validation`
- [x] Test: `rate_limit_burst < rate_limit_rps` returns `ConfigError::Validation`
- [x] Test: `Debug` output for `AppConfig` does not contain raw API key strings
- [x] Proptest: any valid env var set produces a valid `AppConfig`
- [x] Proptest: `validate()` is deterministic (same input → same output)

### Step 5 — Wire `src/lib.rs` to expose config

- [x] Add `pub mod config;` to the module list in `src/lib.rs`
- [x] Update `build_app` signature: `pub fn build_app(config: &AppConfig) -> Router`
- [x] Read `assets_path` from `config.assets_path`
- [x] Note: full wiring of S3Storage, cache, embargo etc. is deferred to later units;
  Unit 1 only changes the function signature and reads `assets_path`

### Step 6 — Update `src/main.rs`

- [x] Replace `std::env::var("RENDITION_ASSETS_PATH")` with `AppConfig::load()`
- [x] On `Err`: log the error and exit with status `1`
- [x] On `Ok`: log a sanitised summary of the loaded config (redacted)
- [x] Bind address from `config.bind_addr` (was hardcoded `0.0.0.0:3000`)
- [x] Pass `&config` to `rendition::build_app(&config)`

### Step 7 — Verify build and tests

- [x] `cargo build` succeeds
- [x] `cargo test --lib config` passes (unit tests in `src/config.rs`)
- [x] `cargo test --test config_test` passes (integration tests)
- [x] `cargo clippy --all-targets -- -D warnings` clean
- [x] `cargo fmt --check` clean
- [x] Existing `tests/e2e.rs` still passes (regression check — the lib.rs
  signature changed, but tests pass `LocalStorage` directly which is unaffected)

### Step 8 — Generate code summary documentation

- [x] Write `aidlc-docs/construction/config/code/code-summary.md` with:
  - Files created
  - Files modified
  - Public API surface
  - Test coverage summary
  - Known follow-ups for later units

## Story Traceability

| Story | Step(s) | Deliverable |
|---|---|---|
| FR-02 (typed env var config) | 2, 3 | `AppConfig`, `load()`, `validate()` |
| FR-02 (fail-fast at startup) | 3, 6 | `validate()` + `main()` exit on error |
| FR-02 (S3 fields required when backend=s3) | 3 | `validate()` cross-field rule |
| NFR-01 (≥ 80% coverage) | 4 | Comprehensive unit tests |
| NFR-02 (PBT) | 4 | Proptest invariants |
| NFR-03 / SECURITY-03 (no secrets in logs) | 2 | Custom `Debug` impl |
| NFR-06 (hexagonal) | 5, 6 | Config injected into `build_app`, no global state |

## Acceptance Criteria

- All checkboxes above marked `[x]`
- All Unit 1 acceptance criteria from `unit-of-work.md` met
- `cargo test` passes including new tests
- `cargo clippy` clean
- Code summary documented in `aidlc-docs/construction/config/code/code-summary.md`

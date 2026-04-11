//! Application configuration.
//!
//! All Rendition operational parameters are read from `RENDITION_*`
//! environment variables at startup, parsed into a typed [`AppConfig`]
//! struct, and validated. Any error fails the process before it binds
//! its listeners — there is no graceful degradation for misconfiguration.
//!
//! Loading is performed by [`AppConfig::load`], which uses the
//! [`envy`] crate to deserialise the env vars and then runs cross-field
//! validation in [`AppConfig::validate`].
//!
//! Sensitive fields (S3 secret access key, hashed admin API keys) are
//! redacted in the [`Debug`] output to prevent accidental logging.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;
use url::Url;

/// Top-level typed configuration loaded from `RENDITION_*` environment
/// variables.
///
/// Use [`AppConfig::load`] to construct an instance from the process
/// environment. The returned value is intended to be wrapped in
/// `Arc<AppConfig>` and treated as immutable for the lifetime of the
/// process.
#[derive(Clone, Deserialize)]
pub struct AppConfig {
    /// Public CDN listener address. Default: `0.0.0.0:3000`.
    #[serde(default = "default_bind_addr")]
    pub bind_addr: SocketAddr,

    /// Internal admin API listener address. Default: `127.0.0.1:3001`.
    /// MUST NOT be publicly routable — see ADR-0013.
    #[serde(default = "default_admin_bind_addr")]
    pub admin_bind_addr: SocketAddr,

    /// Storage backend selector.
    #[serde(default)]
    pub storage_backend: StorageBackendKind,

    /// Local filesystem root for `LocalStorage`. Default: `./assets`.
    /// Required when `storage_backend == Local`.
    #[serde(default = "default_assets_path")]
    pub assets_path: PathBuf,

    /// S3 backend configuration group. All `RENDITION_S3_*` environment
    /// variables deserialise into this nested struct (see ADR-0020).
    ///
    /// `#[serde(skip)]` here because `envy`'s `#[serde(flatten)]` support
    /// does not coerce numeric env-var strings into numeric struct fields
    /// through a flattened child — numeric fields in a flattened struct are
    /// handed to serde as already-deserialised strings. The two-pass load
    /// in [`AppConfig::load`] reads the `S3Settings` fields via a second
    /// `envy::prefixed("RENDITION_").from_env::<S3Settings>()` call.
    #[serde(skip)]
    pub s3: S3Settings,

    /// Local filesystem read timeout in milliseconds. Default: `2000`.
    #[serde(default = "default_local_timeout_ms")]
    pub local_timeout_ms: u64,

    /// Maximum number of entries in the in-process transform cache.
    #[serde(default = "default_cache_max_entries")]
    pub cache_max_entries: u64,

    /// TTL for transform cache entries, in seconds.
    #[serde(default = "default_cache_ttl_seconds")]
    pub cache_ttl_seconds: u64,

    /// Maximum allowed request/asset payload size, in bytes. Default: 50 MiB.
    #[serde(default = "default_max_payload_bytes")]
    pub max_payload_bytes: u64,

    /// Per-IP rate limit (requests per second).
    #[serde(default = "default_rate_limit_rps")]
    pub rate_limit_rps: u32,

    /// Per-IP rate limit burst capacity.
    #[serde(default = "default_rate_limit_burst")]
    pub rate_limit_burst: u32,

    /// Strategy for extracting the rate-limit key from a request.
    #[serde(default)]
    pub rate_limit_key: RateLimitKey,

    /// `Cache-Control` header value emitted on CDN asset responses.
    #[serde(default = "default_cache_control_public")]
    pub cache_control_public: String,

    /// Optional public base URL for canonical asset URLs in API responses.
    pub public_base_url: Option<String>,

    /// Redis connection URL for the embargo and preset stores.
    /// Required when admin features are in use; format `redis://host:port`.
    pub redis_url: Option<String>,

    /// In-process embargo cache TTL, in seconds.
    #[serde(default = "default_embargo_cache_ttl_seconds")]
    pub embargo_cache_ttl_seconds: u64,

    /// OIDC configuration. `None` if no OIDC env vars are set.
    #[serde(flatten)]
    pub oidc: OidcConfig,

    /// SHA-256-hashed admin API keys, comma-separated in the env var.
    /// Each entry is the hex-encoded SHA-256 of a raw key.
    #[serde(default)]
    pub admin_api_keys: Vec<String>,

    /// Optional OTLP endpoint for OpenTelemetry trace export.
    pub otel_endpoint: Option<Url>,
}

/// Storage backend selector.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackendKind {
    /// Local filesystem; default for development.
    #[default]
    Local,
    /// Amazon S3 or S3-compatible object store.
    S3,
}

/// S3 backend configuration group (ADR-0020).
///
/// All fields deserialise from `RENDITION_S3_*` environment variables via
/// `#[serde(flatten)]` on the enclosing [`AppConfig`]. Names in this struct
/// retain the `s3_` prefix so envy's deserialisation matches the env var
/// contract unchanged.
#[derive(Clone, Deserialize)]
pub struct S3Settings {
    /// Bucket name. Required when `storage_backend == S3`.
    pub s3_bucket: Option<String>,

    /// AWS region for the bucket. Required when `storage_backend == S3`.
    pub s3_region: Option<String>,

    /// Custom endpoint URL for S3-compatible stores (MinIO, R2, LocalStack).
    /// When unset, the SDK resolves the standard AWS regional endpoint from
    /// `s3_region`.
    pub s3_endpoint: Option<String>,

    /// Key prefix within the bucket. Default: empty string.
    #[serde(default)]
    pub s3_prefix: String,

    /// Maximum concurrent HTTP connections to S3. Default: `100`.
    #[serde(default = "default_s3_max_connections")]
    pub s3_max_connections: u32,

    /// Per-attempt S3 call timeout in milliseconds. Default: `5000`.
    #[serde(default = "default_s3_timeout_ms")]
    pub s3_timeout_ms: u64,

    /// Consecutive failure threshold at which the circuit breaker opens.
    /// Default: `5`.
    #[serde(default = "default_s3_cb_threshold")]
    pub s3_cb_threshold: u32,

    /// Circuit breaker cooldown in seconds before a half-open probe.
    /// Default: `30`.
    #[serde(default = "default_s3_cb_cooldown_secs")]
    pub s3_cb_cooldown_secs: u64,

    /// Maximum retry attempts per S3 call (in addition to the initial try).
    /// Default: `3`.
    #[serde(default = "default_s3_max_retries")]
    pub s3_max_retries: u32,

    /// Base delay for the full-jitter retry backoff in milliseconds.
    /// Default: `50`.
    #[serde(default = "default_s3_retry_base_ms")]
    pub s3_retry_base_ms: u64,

    /// Escape hatch permitting `http://` S3 endpoints. Required for
    /// LocalStack integration tests. MUST remain `false` in production —
    /// `AppConfig::validate` enforces this.
    #[serde(default)]
    pub s3_allow_insecure_endpoint: bool,
}

impl Default for S3Settings {
    fn default() -> Self {
        Self {
            s3_bucket: None,
            s3_region: None,
            s3_endpoint: None,
            s3_prefix: String::new(),
            s3_max_connections: default_s3_max_connections(),
            s3_timeout_ms: default_s3_timeout_ms(),
            s3_cb_threshold: default_s3_cb_threshold(),
            s3_cb_cooldown_secs: default_s3_cb_cooldown_secs(),
            s3_max_retries: default_s3_max_retries(),
            s3_retry_base_ms: default_s3_retry_base_ms(),
            s3_allow_insecure_endpoint: false,
        }
    }
}

impl S3Settings {
    /// Validate per-field bounds. Called from [`AppConfig::validate`]
    /// after cross-field checks.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.s3_max_connections < 1 {
            return Err(ConfigError::Validation(
                "RENDITION_S3_MAX_CONNECTIONS must be >= 1".into(),
            ));
        }
        if self.s3_timeout_ms < 100 {
            return Err(ConfigError::Validation(
                "RENDITION_S3_TIMEOUT_MS must be >= 100".into(),
            ));
        }
        if self.s3_cb_threshold < 1 {
            return Err(ConfigError::Validation(
                "RENDITION_S3_CB_THRESHOLD must be >= 1".into(),
            ));
        }
        if self.s3_cb_cooldown_secs < 1 {
            return Err(ConfigError::Validation(
                "RENDITION_S3_CB_COOLDOWN_SECS must be >= 1".into(),
            ));
        }
        if self.s3_max_retries > 10 {
            return Err(ConfigError::Validation(
                "RENDITION_S3_MAX_RETRIES must be <= 10 (unbounded retries defeat the circuit breaker)".into(),
            ));
        }
        if self.s3_retry_base_ms < 1 {
            return Err(ConfigError::Validation(
                "RENDITION_S3_RETRY_BASE_MS must be >= 1".into(),
            ));
        }
        // Endpoint scheme enforcement (SECURITY-01 in-transit, NFR Req Q8=A).
        if let Some(endpoint) = self.s3_endpoint.as_deref() {
            let lower = endpoint.to_ascii_lowercase();
            if !lower.starts_with("https://") && !self.s3_allow_insecure_endpoint {
                return Err(ConfigError::Validation(
                    "RENDITION_S3_ENDPOINT must use https:// \
                     (set RENDITION_S3_ALLOW_INSECURE_ENDPOINT=true only for LocalStack tests)"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

/// Strategy for extracting the per-request rate-limit key.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitKey {
    /// Use the TCP peer address (default; correct when not behind a proxy).
    #[default]
    PeerIp,
    /// Use the `X-Forwarded-For` header (correct behind a CDN/reverse proxy).
    XForwardedFor,
}

/// OIDC identity provider configuration.
///
/// All fields are `Option`; `is_configured()` returns true when at least
/// `issuer` and `audience` are present.
#[derive(Clone, Default, Deserialize)]
pub struct OidcConfig {
    /// OIDC issuer URL (e.g. `https://company.okta.com/oauth2/default`).
    pub oidc_issuer: Option<Url>,

    /// Expected `aud` claim value (e.g. `rendition-admin`).
    pub oidc_audience: Option<String>,

    /// Required group claim membership for admin access (e.g. `rendition-admins`).
    pub oidc_admin_group: Option<String>,
}

impl OidcConfig {
    /// Returns true if the minimum required OIDC fields are configured.
    pub fn is_configured(&self) -> bool {
        self.oidc_issuer.is_some() && self.oidc_audience.is_some()
    }
}

// ---- Default impl ----------------------------------------------------------

impl Default for AppConfig {
    /// Returns an `AppConfig` populated with the same defaults as
    /// [`AppConfig::load`] would produce from an empty environment.
    /// Intended for tests and integration helpers — production callers
    /// should always go through `load` so misconfiguration fails fast.
    fn default() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            admin_bind_addr: default_admin_bind_addr(),
            storage_backend: StorageBackendKind::default(),
            assets_path: default_assets_path(),
            s3: S3Settings::default(),
            local_timeout_ms: default_local_timeout_ms(),
            cache_max_entries: default_cache_max_entries(),
            cache_ttl_seconds: default_cache_ttl_seconds(),
            max_payload_bytes: default_max_payload_bytes(),
            rate_limit_rps: default_rate_limit_rps(),
            rate_limit_burst: default_rate_limit_burst(),
            rate_limit_key: RateLimitKey::default(),
            cache_control_public: default_cache_control_public(),
            public_base_url: None,
            redis_url: None,
            embargo_cache_ttl_seconds: default_embargo_cache_ttl_seconds(),
            oidc: OidcConfig::default(),
            admin_api_keys: Vec::new(),
            otel_endpoint: None,
        }
    }
}

// ---- Defaults --------------------------------------------------------------

fn default_bind_addr() -> SocketAddr {
    "0.0.0.0:3000".parse().expect("valid default bind addr")
}

fn default_admin_bind_addr() -> SocketAddr {
    "127.0.0.1:3001"
        .parse()
        .expect("valid default admin bind addr")
}

fn default_assets_path() -> PathBuf {
    PathBuf::from("./assets")
}

fn default_cache_max_entries() -> u64 {
    1000
}

fn default_cache_ttl_seconds() -> u64 {
    3600
}

fn default_max_payload_bytes() -> u64 {
    50 * 1024 * 1024
}

fn default_rate_limit_rps() -> u32 {
    100
}

fn default_rate_limit_burst() -> u32 {
    200
}

fn default_cache_control_public() -> String {
    "public, max-age=31536000, immutable".to_string()
}

fn default_embargo_cache_ttl_seconds() -> u64 {
    30
}

fn default_local_timeout_ms() -> u64 {
    2000
}

fn default_s3_max_connections() -> u32 {
    100
}

fn default_s3_timeout_ms() -> u64 {
    5000
}

fn default_s3_cb_threshold() -> u32 {
    5
}

fn default_s3_cb_cooldown_secs() -> u64 {
    30
}

fn default_s3_max_retries() -> u32 {
    3
}

fn default_s3_retry_base_ms() -> u64 {
    50
}

// ---- Errors ----------------------------------------------------------------

/// Errors returned from [`AppConfig::load`] and [`AppConfig::validate`].
#[derive(Debug, Error)]
pub enum ConfigError {
    /// `envy` failed to deserialise an env var (missing, wrong type, etc.).
    #[error("environment variable error: {0}")]
    EnvVar(#[from] envy::Error),

    /// Cross-field validation failed.
    #[error("invalid configuration: {0}")]
    Validation(String),

    /// A URL field could not be parsed.
    #[error("invalid URL in {field}: {source}")]
    InvalidUrl {
        field: &'static str,
        #[source]
        source: url::ParseError,
    },
}

// ---- Loading ---------------------------------------------------------------

impl AppConfig {
    /// Load and validate `RENDITION_*` environment variables into an
    /// `AppConfig`. This is the only public way to construct an `AppConfig`
    /// in normal operation.
    ///
    /// # Errors
    /// Returns [`ConfigError`] if any required field is missing, any field
    /// fails type coercion, or any cross-field invariant is violated.
    pub fn load() -> Result<AppConfig, ConfigError> {
        // First pass: top-level fields (everything except S3).
        let mut cfg: AppConfig = envy::prefixed("RENDITION_").from_env()?;
        // Second pass: S3 group. envy ignores env vars that don't match
        // struct fields, so reading the same namespace twice is safe.
        cfg.s3 = envy::prefixed("RENDITION_").from_env::<S3Settings>()?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Run cross-field validation on a constructed `AppConfig`.
    ///
    /// Called automatically by [`AppConfig::load`]; exposed publicly so
    /// tests and integration code can build an `AppConfig` programmatically
    /// and verify it.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // S3 backend requires bucket and region.
        if self.storage_backend == StorageBackendKind::S3 {
            if self.s3.s3_bucket.as_deref().unwrap_or("").is_empty() {
                return Err(ConfigError::Validation(
                    "RENDITION_S3_BUCKET is required when RENDITION_STORAGE_BACKEND=s3".into(),
                ));
            }
            if self.s3.s3_region.as_deref().unwrap_or("").is_empty() {
                return Err(ConfigError::Validation(
                    "RENDITION_S3_REGION is required when RENDITION_STORAGE_BACKEND=s3".into(),
                ));
            }
        }

        // Per-field S3 bounds and endpoint scheme.
        self.s3.validate()?;

        // Local storage read timeout.
        if self.local_timeout_ms < 100 {
            return Err(ConfigError::Validation(
                "RENDITION_LOCAL_TIMEOUT_MS must be >= 100".into(),
            ));
        }

        // OIDC issuer and audience must come together.
        if self.oidc.oidc_issuer.is_some() != self.oidc.oidc_audience.is_some() {
            return Err(ConfigError::Validation(
                "RENDITION_OIDC_ISSUER and RENDITION_OIDC_AUDIENCE must be set together".into(),
            ));
        }

        // Cache must allow at least one entry.
        if self.cache_max_entries == 0 {
            return Err(ConfigError::Validation(
                "RENDITION_CACHE_MAX_ENTRIES must be >= 1".into(),
            ));
        }
        if self.cache_ttl_seconds == 0 {
            return Err(ConfigError::Validation(
                "RENDITION_CACHE_TTL_SECONDS must be >= 1".into(),
            ));
        }

        // Payload limit must be at least 1 KiB to allow a real request body.
        if self.max_payload_bytes < 1024 {
            return Err(ConfigError::Validation(
                "RENDITION_MAX_PAYLOAD_BYTES must be >= 1024".into(),
            ));
        }

        // Rate-limit values must be positive and burst >= rps.
        if self.rate_limit_rps == 0 {
            return Err(ConfigError::Validation(
                "RENDITION_RATE_LIMIT_RPS must be >= 1".into(),
            ));
        }
        if self.rate_limit_burst < self.rate_limit_rps {
            return Err(ConfigError::Validation(format!(
                "RENDITION_RATE_LIMIT_BURST ({}) must be >= RENDITION_RATE_LIMIT_RPS ({})",
                self.rate_limit_burst, self.rate_limit_rps
            )));
        }

        // Validate redis_url format if present.
        if let Some(redis_url) = &self.redis_url {
            Url::parse(redis_url).map_err(|e| ConfigError::InvalidUrl {
                field: "RENDITION_REDIS_URL",
                source: e,
            })?;
        }

        Ok(())
    }

    /// Convenience accessor for the cache TTL as a `Duration`.
    pub fn cache_ttl(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_seconds)
    }

    /// Convenience accessor for the embargo cache TTL as a `Duration`.
    pub fn embargo_cache_ttl(&self) -> Duration {
        Duration::from_secs(self.embargo_cache_ttl_seconds)
    }
}

// ---- Debug impls (redacting) -----------------------------------------------

const REDACTED: &str = "<redacted>";

/// Redacting `Debug` impl. Sensitive fields render as `<redacted>` so
/// startup-time logs of `&config` cannot leak credentials.
impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("bind_addr", &self.bind_addr)
            .field("admin_bind_addr", &self.admin_bind_addr)
            .field("storage_backend", &self.storage_backend)
            .field("assets_path", &self.assets_path)
            .field("s3", &self.s3)
            .field("local_timeout_ms", &self.local_timeout_ms)
            .field("cache_max_entries", &self.cache_max_entries)
            .field("cache_ttl_seconds", &self.cache_ttl_seconds)
            .field("max_payload_bytes", &self.max_payload_bytes)
            .field("rate_limit_rps", &self.rate_limit_rps)
            .field("rate_limit_burst", &self.rate_limit_burst)
            .field("rate_limit_key", &self.rate_limit_key)
            .field("cache_control_public", &self.cache_control_public)
            .field("public_base_url", &self.public_base_url)
            .field(
                "redis_url",
                &self.redis_url.as_ref().map(|_| REDACTED), // hostname may carry creds
            )
            .field("embargo_cache_ttl_seconds", &self.embargo_cache_ttl_seconds)
            .field("oidc", &self.oidc)
            .field(
                "admin_api_keys",
                &format!("[{} entries: <redacted>]", self.admin_api_keys.len()),
            )
            .field("otel_endpoint", &self.otel_endpoint)
            .finish()
    }
}

impl fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcConfig")
            .field("oidc_issuer", &self.oidc_issuer)
            .field("oidc_audience", &self.oidc_audience)
            .field("oidc_admin_group", &self.oidc_admin_group)
            .finish()
    }
}

impl fmt::Debug for S3Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Settings")
            .field("s3_bucket", &self.s3_bucket)
            .field("s3_region", &self.s3_region)
            .field("s3_endpoint", &self.s3_endpoint)
            .field("s3_prefix", &self.s3_prefix)
            .field("s3_max_connections", &self.s3_max_connections)
            .field("s3_timeout_ms", &self.s3_timeout_ms)
            .field("s3_cb_threshold", &self.s3_cb_threshold)
            .field("s3_cb_cooldown_secs", &self.s3_cb_cooldown_secs)
            .field("s3_max_retries", &self.s3_max_retries)
            .field("s3_retry_base_ms", &self.s3_retry_base_ms)
            .field(
                "s3_allow_insecure_endpoint",
                &self.s3_allow_insecure_endpoint,
            )
            .finish()
    }
}

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

    /// S3 bucket name. Required when `storage_backend == S3`.
    pub s3_bucket: Option<String>,

    /// AWS region for the S3 bucket. Required when `storage_backend == S3`.
    pub s3_region: Option<String>,

    /// Custom endpoint URL for S3-compatible stores (MinIO, R2). Optional.
    pub s3_endpoint: Option<String>,

    /// Key prefix within the bucket. Default: empty string.
    #[serde(default)]
    pub s3_prefix: String,

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
            s3_bucket: None,
            s3_region: None,
            s3_endpoint: None,
            s3_prefix: String::new(),
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
        let cfg: AppConfig = envy::prefixed("RENDITION_").from_env()?;
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
            if self.s3_bucket.as_deref().unwrap_or("").is_empty() {
                return Err(ConfigError::Validation(
                    "RENDITION_S3_BUCKET is required when RENDITION_STORAGE_BACKEND=s3".into(),
                ));
            }
            if self.s3_region.as_deref().unwrap_or("").is_empty() {
                return Err(ConfigError::Validation(
                    "RENDITION_S3_REGION is required when RENDITION_STORAGE_BACKEND=s3".into(),
                ));
            }
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
            .field("s3_bucket", &self.s3_bucket)
            .field("s3_region", &self.s3_region)
            .field("s3_endpoint", &self.s3_endpoint)
            .field("s3_prefix", &self.s3_prefix)
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

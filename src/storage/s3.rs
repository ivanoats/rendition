//! AWS S3 / S3-compatible object storage backend.
//!
//! This module contains **all** `aws-sdk-s3` type usage — no AWS SDK
//! type appears in any `pub fn` signature (R-08 module boundary). See
//! ADR-0004 (revised), ADR-0019 (circuit breaker), and the functional
//! design artifacts at
//! `aidlc-docs/construction/s3-storage/functional-design/` for the full
//! design.
//!
//! ## Flow summary
//!
//! 1. Compose the S3 key via [`super::compose_key`] (R-07).
//! 2. Route the call through [`CircuitBreaker::call`] so sustained
//!    outages short-circuit (ADR-0019).
//! 3. Inside the breaker, run a full-jitter retry loop against the SDK,
//!    classifying each outcome via [`classify_sdk_error`] (R-01).
//! 4. Each attempt wraps the SDK future in `tokio::time::timeout` (R-03).
//! 5. On success, populate [`Asset`] with a resolved content type (R-05).
//!
//! The AWS SDK's built-in retrier is **disabled** — we build our own so
//! the circuit breaker sees accurate failure counts (NFR Req Q2 part D).

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::config::retry::RetryConfig;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use rand::Rng;
use tracing::instrument;

use super::circuit_breaker::CircuitBreaker;
use super::{
    compose_key, content_type_from_ext, Asset, NoopMetrics, Outcome, StorageBackend, StorageError,
    StorageMetrics,
};
use crate::config::S3Settings;

const RETRY_CAP_MS: u64 = 500;

/// AWS S3 / S3-compatible storage adapter.
///
/// Wraps an `aws-sdk-s3::Client` with a hand-rolled retry loop and a
/// dedicated [`CircuitBreaker`]. Public methods return only
/// [`StorageError`] — no AWS SDK type crosses the module boundary.
#[derive(Clone)]
pub struct S3Storage {
    client: Client,
    bucket: String,
    prefix: String,
    max_retries: u32,
    retry_base: Duration,
    call_timeout: Duration,
    circuit_breaker: Arc<CircuitBreaker>,
    metrics: Arc<dyn StorageMetrics>,
}

impl S3Storage {
    /// Construct an `S3Storage` from validated [`S3Settings`].
    ///
    /// Uses the SDK's default credentials provider chain: env vars →
    /// shared profile → IMDS → ECS task role → EKS IRSA. Production
    /// deployments rely on IRSA short-lived credentials (see
    /// `aidlc-docs/construction/s3-storage/infrastructure-design/`).
    pub async fn new(settings: &S3Settings) -> Result<Self, StorageError> {
        let bucket = settings
            .s3_bucket
            .clone()
            .ok_or_else(|| StorageError::Other {
                source: "S3Settings::s3_bucket is required".into(),
            })?;
        let region = settings
            .s3_region
            .clone()
            .ok_or_else(|| StorageError::Other {
                source: "S3Settings::s3_region is required".into(),
            })?;

        let mut loader = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region))
            // Disable the SDK's built-in retrier — we implement our own so
            // the circuit breaker sees accurate failure counts (NFR Q2 D).
            .retry_config(RetryConfig::disabled());
        if let Some(endpoint) = settings.s3_endpoint.clone() {
            loader = loader.endpoint_url(endpoint);
        }
        let sdk_config = loader.load().await;
        let client = Client::new(&sdk_config);

        let metrics: Arc<dyn StorageMetrics> = Arc::new(NoopMetrics);
        let circuit_breaker = Arc::new(CircuitBreaker::new(
            settings.s3_cb_threshold,
            Duration::from_secs(settings.s3_cb_cooldown_secs),
            metrics.clone(),
        ));

        Ok(Self {
            client,
            bucket,
            prefix: settings.s3_prefix.clone(),
            max_retries: settings.s3_max_retries,
            retry_base: Duration::from_millis(settings.s3_retry_base_ms),
            call_timeout: Duration::from_millis(settings.s3_timeout_ms),
            circuit_breaker,
            metrics,
        })
    }

    /// Test-only constructor using static credentials — never calls the
    /// SDK default chain. Used by LocalStack integration tests.
    ///
    /// Public (not `#[cfg(test)]`) so integration tests in the `tests/`
    /// directory can reach it; the method name makes the intent obvious,
    /// and production code must never invoke it. See Unit 2 Code
    /// Generation plan Step 7 for the rationale.
    pub async fn new_for_test(
        endpoint: impl Into<String>,
        access_key: &str,
        secret_key: &str,
        bucket: impl Into<String>,
    ) -> Result<Self, StorageError> {
        let creds = Credentials::new(access_key, secret_key, None, None, "rendition-test");
        let sdk_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .endpoint_url(endpoint.into())
            .credentials_provider(creds)
            .retry_config(RetryConfig::disabled())
            .load()
            .await;
        let client = Client::new(&sdk_config);
        let metrics: Arc<dyn StorageMetrics> = Arc::new(NoopMetrics);
        let circuit_breaker = Arc::new(CircuitBreaker::new(
            5,
            Duration::from_secs(30),
            metrics.clone(),
        ));
        Ok(Self {
            client,
            bucket: bucket.into(),
            prefix: String::new(),
            max_retries: 3,
            retry_base: Duration::from_millis(50),
            call_timeout: Duration::from_millis(5000),
            circuit_breaker,
            metrics,
        })
    }

    /// Cheap health check used by `/health/ready` in Unit 7.
    ///
    /// Returns `true` whenever the circuit breaker is not in the `Open`
    /// state. No I/O — just a mutex read. Target ≤ 100 ns.
    pub fn is_healthy(&self) -> bool {
        !self.circuit_breaker.is_open()
    }

    // Internal: retry loop around a single-attempt SDK call. Runs inside
    // the circuit breaker.
    async fn with_retries<F, Fut, T>(
        &self,
        op_label: &'static str,
        mut make_fut: F,
    ) -> Result<T, StorageError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, StorageError>>,
    {
        let mut attempt: u32 = 0;
        loop {
            let fut = make_fut();
            let result = match tokio::time::timeout(self.call_timeout, fut).await {
                Ok(inner) => inner,
                Err(_) => Err(StorageError::Timeout { op: op_label }),
            };

            let retry = matches!(
                &result,
                Err(StorageError::Unavailable { .. }) | Err(StorageError::Timeout { .. })
            );

            if !retry || attempt >= self.max_retries {
                return result;
            }

            attempt += 1;
            let delay = full_jitter_delay(attempt, self.retry_base);
            tokio::time::sleep(delay).await;
        }
    }

    async fn fetch_get(&self, key: &str) -> Result<Asset, StorageError> {
        let out = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(classify_get_object_error)?;
        let returned_ct = out.content_type().map(|s| s.to_string());
        let bytes = collect_body(out.body).await?;
        let size = bytes.len();
        Ok(Asset {
            content_type: resolve_content_type(returned_ct.as_deref(), key),
            data: bytes,
            size,
        })
    }

    async fn fetch_head(&self, key: &str) -> Result<bool, StorageError> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(err) => match classify_head_object_error(err) {
                StorageError::NotFound => Ok(false),
                other => Err(other),
            },
        }
    }

    async fn fetch_get_range(
        &self,
        key: &str,
        range: std::ops::Range<u64>,
    ) -> Result<Asset, StorageError> {
        let header = format!("bytes={}-{}", range.start, range.end - 1);
        let out = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .range(header)
            .send()
            .await
            .map_err(classify_get_object_error)?;
        let returned_ct = out.content_type().map(|s| s.to_string());
        let bytes = collect_body(out.body).await?;
        let expected = (range.end - range.start) as usize;
        if bytes.len() != expected {
            return Err(StorageError::Other {
                source: format!(
                    "S3 returned {} bytes for requested range width {}",
                    bytes.len(),
                    expected
                )
                .into(),
            });
        }
        let size = bytes.len();
        Ok(Asset {
            content_type: resolve_content_type(returned_ct.as_deref(), key),
            data: bytes,
            size,
        })
    }

    fn record(&self, op: &str, outcome: Outcome, started: Instant) {
        self.metrics.record(op, outcome, started.elapsed());
    }
}

impl StorageBackend for S3Storage {
    #[instrument(skip(self), fields(backend = "s3", op = "get"))]
    async fn get(&self, path: &str) -> Result<Asset, StorageError> {
        let started = Instant::now();
        let key = match compose_key(&self.prefix, path) {
            Ok(k) => k,
            Err(err) => {
                self.record("get", Outcome::InvalidPath, started);
                return Err(err);
            }
        };
        let result = self
            .circuit_breaker
            .call(self.with_retries("get", || self.fetch_get(&key)))
            .await;
        self.record("get", outcome_of(&result), started);
        result
    }

    #[instrument(skip(self), fields(backend = "s3", op = "exists"))]
    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let started = Instant::now();
        let key = match compose_key(&self.prefix, path) {
            Ok(k) => k,
            Err(err) => {
                self.record("exists", Outcome::InvalidPath, started);
                return Err(err);
            }
        };
        let result = self
            .circuit_breaker
            .call(self.with_retries("exists", || self.fetch_head(&key)))
            .await;
        self.record("exists", outcome_of(&result), started);
        result
    }

    #[instrument(skip(self), fields(backend = "s3", op = "get_range"))]
    async fn get_range(
        &self,
        path: &str,
        range: std::ops::Range<u64>,
    ) -> Result<Asset, StorageError> {
        let started = Instant::now();
        if range.start >= range.end {
            self.record("get_range", Outcome::InvalidPath, started);
            return Err(StorageError::InvalidPath {
                reason: format!(
                    "empty or inverted range: start={} end={}",
                    range.start, range.end
                ),
            });
        }
        let key = match compose_key(&self.prefix, path) {
            Ok(k) => k,
            Err(err) => {
                self.record("get_range", Outcome::InvalidPath, started);
                return Err(err);
            }
        };
        let result = self
            .circuit_breaker
            .call(self.with_retries("get_range", || self.fetch_get_range(&key, range.clone())))
            .await;
        self.record("get_range", outcome_of(&result), started);
        result
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (kept private; unit-tested below)
// ---------------------------------------------------------------------------

fn outcome_of<T>(result: &Result<T, StorageError>) -> Outcome {
    match result {
        Ok(_) => Outcome::Success,
        Err(StorageError::NotFound) => Outcome::NotFound,
        Err(StorageError::Unavailable { .. }) => Outcome::Unavailable,
        Err(StorageError::Timeout { .. }) => Outcome::Timeout,
        Err(StorageError::CircuitOpen) => Outcome::CircuitOpen,
        Err(StorageError::InvalidPath { .. }) => Outcome::InvalidPath,
        Err(StorageError::Other { .. }) => Outcome::Other,
    }
}

fn full_jitter_delay(attempt: u32, base: Duration) -> Duration {
    let base_ms = base.as_millis() as u64;
    let exp = base_ms.saturating_mul(1u64 << attempt.min(10));
    let capped = exp.min(RETRY_CAP_MS);
    let jittered = rand::rng().random_range(0..=capped);
    Duration::from_millis(jittered)
}

/// Classify an `SdkError<GetObjectError>` into a [`StorageError`].
fn classify_get_object_error(err: SdkError<GetObjectError>) -> StorageError {
    // Missing object → NotFound (R-01 terminal).
    if let SdkError::ServiceError(svc) = &err {
        match svc.err() {
            GetObjectError::NoSuchKey(_) => return StorageError::NotFound,
            GetObjectError::InvalidObjectState(_) => {
                return StorageError::Other {
                    source: Box::new(err),
                };
            }
            _ => {}
        }
    }
    classify_sdk_shape(err)
}

/// Classify an `SdkError<HeadObjectError>` into a [`StorageError`].
fn classify_head_object_error(err: SdkError<HeadObjectError>) -> StorageError {
    if let SdkError::ServiceError(svc) = &err {
        if matches!(svc.err(), HeadObjectError::NotFound(_)) {
            return StorageError::NotFound;
        }
    }
    classify_sdk_shape(err)
}

/// Shape-based classification shared by both SDK error types.
///
/// Maps the transport/protocol layer into retriable (`Unavailable`) vs
/// terminal (`Other`) per R-01. The 403 → `NotFound` rule from R-01 is
/// implemented here at the HTTP-status level so unauthorised ARNs are
/// indistinguishable from missing objects to HTTP callers.
fn classify_sdk_shape<E>(err: SdkError<E>) -> StorageError
where
    E: std::error::Error + Send + Sync + 'static,
{
    match &err {
        SdkError::TimeoutError(_) => StorageError::Timeout { op: "s3" },
        SdkError::DispatchFailure(_)
        | SdkError::ResponseError(_)
        | SdkError::ConstructionFailure(_) => StorageError::Unavailable {
            source: Box::new(err),
        },
        SdkError::ServiceError(svc) => {
            let status = svc.raw().status().as_u16();
            if status == 403 || status == 404 {
                StorageError::NotFound
            } else if (500..600).contains(&status) || status == 429 {
                StorageError::Unavailable {
                    source: Box::new(err),
                }
            } else {
                StorageError::Other {
                    source: Box::new(err),
                }
            }
        }
        _ => StorageError::Other {
            source: Box::new(err),
        },
    }
}

async fn collect_body(stream: ByteStream) -> Result<Vec<u8>, StorageError> {
    let aggregated = stream.collect().await.map_err(|e| StorageError::Other {
        source: Box::new(e),
    })?;
    Ok(aggregated.into_bytes().to_vec())
}

/// Pick a content type per R-05 fallback chain:
///
/// 1. The S3-returned header, iff it is non-empty and not
///    `application/octet-stream`.
/// 2. Otherwise, [`content_type_from_ext`] on the logical path.
fn resolve_content_type(s3_header: Option<&str>, path: &str) -> String {
    if let Some(ct) = s3_header {
        if !ct.is_empty() && ct != "application/octet-stream" {
            return ct.to_string();
        }
    }
    content_type_from_ext(path).to_string()
}

// ---------------------------------------------------------------------------
// Tests — pure helpers only. Integration tests against LocalStack live
// in tests/s3_integration.rs and are `#[ignore]`-gated.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_content_type_prefers_s3_header() {
        let ct = resolve_content_type(Some("image/heic"), "photo.jpg");
        assert_eq!(ct, "image/heic");
    }

    #[test]
    fn resolve_content_type_falls_back_to_extension_when_header_missing() {
        let ct = resolve_content_type(None, "photo.jpg");
        assert_eq!(ct, "image/jpeg");
    }

    #[test]
    fn resolve_content_type_falls_back_when_header_is_octet_stream() {
        let ct = resolve_content_type(Some("application/octet-stream"), "photo.jpg");
        assert_eq!(ct, "image/jpeg");
    }

    #[test]
    fn resolve_content_type_falls_back_when_header_is_empty() {
        let ct = resolve_content_type(Some(""), "photo.jpg");
        assert_eq!(ct, "image/jpeg");
    }

    #[test]
    fn full_jitter_delay_never_exceeds_cap() {
        let base = Duration::from_millis(50);
        for attempt in 0..20 {
            for _ in 0..100 {
                let d = full_jitter_delay(attempt, base);
                assert!(d.as_millis() as u64 <= RETRY_CAP_MS);
            }
        }
    }

    #[test]
    fn outcome_of_maps_all_variants() {
        let _: Outcome = outcome_of::<i32>(&Ok(1));
        assert_eq!(outcome_of::<i32>(&Ok(1)), Outcome::Success);
        assert_eq!(
            outcome_of::<i32>(&Err(StorageError::NotFound)),
            Outcome::NotFound
        );
        assert_eq!(
            outcome_of::<i32>(&Err(StorageError::CircuitOpen)),
            Outcome::CircuitOpen
        );
        assert_eq!(
            outcome_of::<i32>(&Err(StorageError::Timeout { op: "get" })),
            Outcome::Timeout
        );
        assert_eq!(
            outcome_of::<i32>(&Err(StorageError::InvalidPath { reason: "".into() })),
            Outcome::InvalidPath
        );
    }
}

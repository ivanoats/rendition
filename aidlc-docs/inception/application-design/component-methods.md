# Component Methods

Method signatures for all components. Input/output types are described at the
Rust type level. Detailed business rules (validation logic, pipeline sequencing,
error handling branches) are defined in Functional Design (Construction phase).

---

## C-01 — Config

```rust
impl AppConfig {
    /// Load and validate all RENDITION_* env vars. Fails fast at startup.
    pub fn load() -> Result<AppConfig>;

    /// Cross-field validation (e.g. S3 fields required when backend = s3).
    pub fn validate(&self) -> Result<()>;

    /// Convenience accessor — returns S3Config or error if backend != S3.
    pub fn s3(&self) -> Result<&S3Config>;

    /// Convenience accessor — returns OidcConfig if OIDC is configured.
    pub fn oidc(&self) -> Option<&OidcConfig>;
}
```

---

## C-02 — Storage

```rust
pub trait StorageBackend: Send + Sync + 'static {
    /// Fetch the full asset. Returns Asset or error.
    async fn get(&self, path: &str) -> Result<Asset>;

    /// Check existence without downloading the body.
    async fn exists(&self, path: &str) -> bool;

    /// Fetch a byte range. Default impl fetches full asset and slices.
    /// S3Storage overrides to pass Range header to GetObject (ADR-0018).
    async fn get_range(&self, path: &str, range: Range<u64>) -> Result<Asset> {
        // default: full fetch + slice
    }
}

impl LocalStorage {
    pub fn new(root: PathBuf) -> Self;
}

impl S3Storage {
    pub fn new(cfg: &S3Config) -> Result<Self>;
    /// Exposed for /health/ready — true when circuit breaker is closed.
    pub fn is_healthy(&self) -> bool;
}

impl CircuitBreaker {
    pub fn new(threshold: u32, cooldown: Duration) -> Self;
    pub async fn call<F, T>(&self, f: F) -> Result<T>
    where
        F: Future<Output = Result<T>>;
    pub fn is_open(&self) -> bool;
}
```

---

## C-03 — Transform

```rust
/// Resolves fmt=auto and runs the pipeline on a blocking thread.
pub async fn apply(
    bytes: Vec<u8>,
    params: TransformParams,
    accept: Option<&HeaderValue>,
) -> Result<(Vec<u8>, &'static str)>;

/// Blocking pipeline: decode → crop → resize → sharpen → watermark
/// → rotate → flip → encode. Called via spawn_blocking.
pub(crate) fn apply_blocking(
    bytes: Vec<u8>,
    params: &TransformParams,
    format: ImageFormat,
) -> Result<(Vec<u8>, &'static str)>;

/// Parse Accept header q-values, return best format Rendition can produce.
/// Order: AVIF > WebP > PNG (alpha) > JPEG.
pub fn negotiate_format(
    accept: Option<&HeaderValue>,
    has_alpha: bool,
) -> ImageFormat;

impl TransformParams {
    /// Validate all fields against documented constraints (FR-09).
    /// Returns Err with a user-safe message on violation.
    pub fn validate(&self) -> Result<()>;

    /// Stable canonical JSON for cache key derivation — field order is fixed.
    pub fn canonical_bytes(&self) -> Vec<u8>;
}
```

---

## C-04 — Transform Cache

```rust
pub trait TransformCache: Send + Sync + 'static {
    fn get(&self, key: &CacheKey) -> Option<CachedResponse>;
    fn put(&self, key: CacheKey, response: CachedResponse);
    fn invalidate(&self, key: &CacheKey);
    fn invalidate_by_path(&self, path: &str);
    fn entry_count(&self) -> u64;
}

/// SHA-256(asset_path || canonical_params_bytes || format_byte).
/// Identical requests always produce the same key regardless of param order.
pub fn compute_cache_key(
    path: &str,
    params: &TransformParams,
    format: ImageFormat,
) -> CacheKey;

impl MokaTransformCache {
    pub fn new(max_capacity: u64, ttl: Duration) -> Self;
}
```

---

## C-05 — Embargo

```rust
pub trait EmbargoStore: Send + Sync + 'static {
    async fn get(&self, path: &str) -> Result<Option<EmbargoRecord>>;
    async fn put(&self, record: EmbargoRecord) -> Result<()>;
    async fn update(&self, path: &str, update: EmbargoUpdate) -> Result<EmbargoRecord>;
    async fn delete(&self, path: &str) -> Result<()>;
    async fn list_active(&self) -> Result<Vec<EmbargoRecord>>;
}

impl EmbargoEnforcer {
    pub fn new(store: Arc<dyn EmbargoStore>, cache_ttl: Duration) -> Self;

    /// Hot-path check. Returns Some if asset is currently under embargo.
    /// Uses in-process cache; falls back to EmbargoStore on miss.
    pub async fn check(&self, path: &str) -> Option<EmbargoRecord>;

    /// Called by admin handlers after a delete/update to purge the local cache.
    pub fn invalidate(&self, path: &str);

    /// For /health/ready — returns false if the store is unreachable.
    pub async fn is_healthy(&self) -> bool;
}

impl RedisEmbargoStore {
    pub fn new(redis_url: &str, key_prefix: &str) -> Result<Self>;
}
```

---

## C-06 — Preset

```rust
pub trait PresetStore: Send + Sync + 'static {
    async fn get(&self, name: &str) -> Result<Option<NamedPreset>>;
    async fn put(&self, preset: NamedPreset) -> Result<()>;
    async fn update(&self, name: &str, params: TransformParams) -> Result<NamedPreset>;
    async fn delete(&self, name: &str) -> Result<()>;
    async fn list(&self) -> Result<Vec<NamedPreset>>;
}

/// Merge preset params with explicit URL override params.
/// Explicit params always win over preset defaults.
pub fn resolve_params(
    preset: Option<NamedPreset>,
    overrides: TransformParams,
) -> TransformParams;

impl RedisPresetStore {
    /// Reuses the Redis connection pool established by RedisEmbargoStore.
    pub fn new(pool: Arc<RedisPool>) -> Self;
}
```

---

## C-07 — CDN API

```rust
/// Shared state injected into all CDN handlers via Axum State extractor.
pub struct AppState<S: StorageBackend> {
    pub storage: Arc<S>,
    pub cache: Arc<dyn TransformCache>,
    pub embargo: Arc<EmbargoEnforcer>,
    pub presets: Arc<dyn PresetStore>,
    pub config: Arc<AppConfig>,
    pub metrics: Arc<Metrics>,
}

/// GET /cdn/*asset_path
/// Pipeline: preset resolve → embargo check → cache hit → storage → transform
/// → cache store → set headers → respond.
pub async fn serve_asset<S: StorageBackend>(
    State(state): State<AppState<S>>,
    Path(asset_path): Path<String>,
    Query(raw_params): Query<RawTransformParams>,
    TypedHeader(accept): Option<TypedHeader<Accept>>,
    TypedHeader(range): Option<TypedHeader<Range>>,
) -> impl IntoResponse;

/// Build the CDN router with middleware stack applied.
pub fn cdn_router<S: StorageBackend + Clone>(state: AppState<S>) -> Router;
```

---

## C-08 — Admin API

```rust
impl JwksCache {
    pub fn new(jwks_url: String, audience: String, issuer: String) -> Self;

    /// Validates JWT. Force-refreshes JWKS keys once on InvalidSignature
    /// before returning 401, to handle key rotation.
    pub async fn validate(&self, token: &str) -> Result<AdminClaims>;
}

impl AuthLayer {
    /// Tower Service: extracts Bearer token or X-Api-Key, validates,
    /// injects AdminIdentity into request extensions.
    pub fn new(jwks: Option<Arc<JwksCache>>, api_key_hashes: Vec<[u8; 32]>) -> Self;
}

// Embargo handlers
pub async fn create_embargo(
    State(state): State<AdminState>,
    Extension(identity): Extension<AdminIdentity>,
    Json(body): Json<CreateEmbargoRequest>,
) -> impl IntoResponse;  // 201 | 400 | 409

pub async fn list_embargoes(State(state): State<AdminState>) -> impl IntoResponse;  // 200

pub async fn get_embargo(
    State(state): State<AdminState>,
    Path(path): Path<String>,
) -> impl IntoResponse;  // 200 | 404

pub async fn update_embargo(
    State(state): State<AdminState>,
    Extension(identity): Extension<AdminIdentity>,
    Path(path): Path<String>,
    Json(body): Json<UpdateEmbargoRequest>,
) -> impl IntoResponse;  // 200 | 400 | 404

pub async fn delete_embargo(
    State(state): State<AdminState>,
    Extension(identity): Extension<AdminIdentity>,
    Path(path): Path<String>,
) -> impl IntoResponse;  // 204 | 404

// Preset handlers (analogous CRUD — omitted for brevity, same pattern)

// Cache purge
pub async fn purge_cache(
    State(state): State<AdminState>,
    Json(body): Json<PurgeRequest>,
) -> impl IntoResponse;  // 200 with { purged_count }

/// Build the admin router. AuthLayer is applied to all routes.
pub fn admin_router(state: AdminState) -> Router;
```

---

## C-09 — Middleware

```rust
/// Returns the fully stacked Tower layer for the CDN listener.
/// Order: RequestId → Trace → RateLimit → SecurityHeaders → Compression.
pub fn cdn_middleware_stack(config: &AppConfig) -> impl Layer<Router> + Clone;

/// Returns the security headers layer applied to all listeners.
pub fn security_headers_layer() -> SetResponseHeadersLayer<...>;

/// Build tower-governor RateLimitLayer from config.
/// KeyExtractor reads from X-Forwarded-For or peer IP per RENDITION_RATE_LIMIT_KEY.
pub fn rate_limit_layer(config: &AppConfig) -> GovernorLayer<...>;
```

---

## C-10 — Observability

```rust
impl Metrics {
    pub fn new() -> Result<Self>;  // registers all metrics with global registry

    pub fn record_cache_hit(&self);
    pub fn record_cache_miss(&self);
    pub fn record_embargo_rejection(&self);
    pub fn record_storage_error(&self, backend: &str);
    pub fn record_transform_duration(&self, duration: Duration, format: &str);
    pub fn record_storage_fetch_duration(&self, duration: Duration, backend: &str);
    pub fn set_cache_entries(&self, count: u64);
    pub fn set_circuit_breaker_open(&self, backend: &str, open: bool);
}

/// GET /metrics — Prometheus text exposition format.
pub async fn metrics_handler() -> impl IntoResponse;

/// GET /health/live — always 200 OK.
pub async fn liveness_handler() -> impl IntoResponse;

/// GET /health/ready — 200 if storage + embargo store reachable and circuit
/// breaker closed. 503 with detail JSON if any dependency is unhealthy.
pub async fn readiness_handler<S: StorageBackend>(
    State(state): State<AppState<S>>,
) -> impl IntoResponse;

/// Initialise OTEL OTLP exporter. Returns guard — drop on shutdown to flush.
pub fn init_otel(config: &AppConfig) -> Result<OtelGuard>;
```

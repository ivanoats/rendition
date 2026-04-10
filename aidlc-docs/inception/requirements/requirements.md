# Requirements

## Intent Analysis Summary

- **User Request**: Continue the development of Rendition as a robust, enterprise-scale
  open-source replacement for Adobe Scene7, implementing the S3 storage backend,
  configuration management, performance/caching improvements, and resolving existing
  technical debt — all within a hexagonal architecture targeting AWS with provider
  portability.
- **Request Type**: Multi-initiative enhancement (new features + technical debt
  resolution + NFR uplift)
- **Scope Estimate**: System-wide — touches all existing modules and introduces new ones
- **Complexity Estimate**: Complex — multiple interdependent concerns, enterprise-grade
  NFRs, security baseline, and property-based testing enforcement

---

## Functional Requirements

### FR-01: S3 Storage Backend

- The system MUST provide a fully functional `S3Storage` implementation of the
  `StorageBackend` trait.
- `S3Storage::get` MUST fetch asset bytes from an S3-compatible bucket and return an
  `Asset` with correct `content_type` and `size`.
- `S3Storage::exists` MUST check for key existence without downloading the object body
  (e.g., using `HeadObject`).
- The implementation MUST NOT couple to AWS SDK types at the API or storage-trait
  boundary — AWS SDK usage MUST be encapsulated within the adapter.
- The backend MUST be selectable at startup via configuration (not compile-time
  conditional).
- The S3 endpoint MUST be configurable to support S3-compatible stores (MinIO,
  Cloudflare R2, etc.).

### FR-02: Configuration Management

- The system MUST expose all operational parameters via environment variables.
- Required configuration parameters:
  - `RENDITION_BIND_ADDR` — bind address (default: `0.0.0.0:3000`)
  - `RENDITION_ASSETS_PATH` — local asset root (existing; default: `./assets`)
  - `RENDITION_STORAGE_BACKEND` — storage type (`local` | `s3`; default: `local`)
  - `RENDITION_S3_BUCKET` — S3 bucket name (required when backend is `s3`)
  - `RENDITION_S3_REGION` — AWS region (required when backend is `s3`)
  - `RENDITION_S3_ENDPOINT` — custom endpoint URL (optional; for S3-compatible stores)
  - `RENDITION_S3_PREFIX` — key prefix within the bucket (optional; default: `""`)
  - `RENDITION_CACHE_MAX_ENTRIES` — max transformed-image cache entries (default: `1000`)
  - `RENDITION_CACHE_TTL_SECONDS` — cache entry TTL in seconds (default: `3600`)
  - `RENDITION_MAX_PAYLOAD_BYTES` — maximum request/asset size in bytes (default: `50MB`)
- The system MUST fail fast at startup with a clear error if required configuration is
  missing or invalid.
- Configuration MUST be parsed and validated into a typed `Config` struct before the
  server starts.

### FR-03: In-Memory Transform Cache

- The system MUST cache transformed image outputs to avoid redundant libvips processing
  for repeated identical requests.
- The cache key MUST incorporate the asset path and the full set of transform parameters.
- The cache MUST be bounded (LRU eviction) with a configurable maximum entry count
  (FR-02: `RENDITION_CACHE_MAX_ENTRIES`).
- Cache entries MUST have a configurable TTL (FR-02: `RENDITION_CACHE_TTL_SECONDS`).
- Cache MUST be safe for concurrent access.
- Cache hits MUST bypass libvips entirely and serve bytes directly.
- A cache metrics endpoint or structured log on each hit/miss MUST be emitted.

### FR-04: Request Size Limiting

- The system MUST enforce a configurable maximum payload size on all inbound requests
  (FR-02: `RENDITION_MAX_PAYLOAD_BYTES`).
- Requests exceeding the limit MUST be rejected with HTTP `413 Payload Too Large`.

### FR-05: Rate Limiting

- Public-facing CDN endpoints (`/cdn/*`) MUST be protected by per-IP rate limiting.
- Rate limit parameters (requests per second, burst) MUST be configurable.
- Clients exceeding the rate limit MUST receive HTTP `429 Too Many Requests` with a
  `Retry-After` header.

### FR-06: HTTP Security Headers

- All responses MUST include the following security headers:
  - `Strict-Transport-Security: max-age=31536000; includeSubDomains`
  - `X-Content-Type-Options: nosniff`
  - `X-Frame-Options: DENY`
  - `Referrer-Policy: strict-origin-when-cross-origin`
  - `Content-Security-Policy: default-src 'none'` (media CDN — no scripts served)
- Headers MUST be applied via Tower middleware, not per-handler.

### FR-07: Structured Request Logging

- Every HTTP request MUST be logged with: timestamp, method, path, status code,
  latency, and a correlation/request ID.
- The request ID MUST be generated per request and included in both the log entry and
  the `X-Request-Id` response header.
- Sensitive data (credentials, PII) MUST NOT appear in log output.

### FR-08: Error Response Hardening

- Error responses in all environments MUST return generic user-facing messages.
- Internal details (stack traces, file paths, libvips error text) MUST NOT be included
  in HTTP response bodies.
- Internal details MUST be logged server-side (with request ID correlation) but never
  returned to clients.

### FR-09: Input Validation

- All query parameters MUST be validated before processing:
  - `wid`, `hei`: MUST be positive integers ≤ configurable max dimension
    (default: 8192 px).
  - `qlt`: MUST be in range 1–100.
  - `rotate`: MUST be one of `0`, `90`, `180`, `270`.
  - `fit`: MUST be one of the documented values; unknown values fallback to `constrain`
    (existing behavior — document this explicitly).
  - `fmt`: MUST be one of the documented values; unknown values fallback to `jpeg`
    (existing behavior — document this explicitly).
  - `crop`: MUST be exactly 4 comma-separated integers; all MUST be non-negative;
    `w` and `h` MUST be positive.
- Validation violations MUST return HTTP `400 Bad Request` with a non-internal error
  message.

### FR-11: Embargo Data Model

- An **embargo** is a record associating an asset path with a future UTC datetime before
  which the asset MUST NOT be served to unauthenticated clients.
- Each embargo record MUST contain:
  - `asset_path` — the logical asset path (exact match, e.g. `campaigns/fall24/hero.jpg`)
  - `embargo_until` — an RFC 3339 UTC datetime after which the asset is freely served
  - `created_by` — the admin identity that set the embargo
  - `created_at` — the UTC datetime the embargo was created
  - `note` (optional) — a human-readable reason for the embargo
- Embargo records MUST be persisted in durable storage that survives process restarts.
  The storage backend for embargo state is an **open architectural decision** to be
  resolved in Application Design (candidates: Redis with TTL, DynamoDB, PostgreSQL).
- Embargo state MUST be reachable with ≤ 5 ms latency at P99 (hot-path check on every
  CDN request), making in-process caching of embargo state a mandatory optimisation.
- A local in-memory read-through cache of embargo records MUST be maintained, with a
  configurable TTL (`RENDITION_EMBARGO_CACHE_TTL_SECONDS`, default: `30`) to bound
  staleness after an admin updates or lifts an embargo.

### FR-12: Embargo Management API

- A new admin sub-router MUST be mounted at `/admin/embargoes`.
- All admin endpoints MUST require authentication (FR-13).
- Required endpoints:

  | Method   | Path                          | Description                              |
  |----------|-------------------------------|------------------------------------------|
  | `POST`   | `/admin/embargoes`            | Create an embargo on an asset path       |
  | `GET`    | `/admin/embargoes`            | List all active (not yet expired) embargoes |
  | `GET`    | `/admin/embargoes/{path}`     | Get embargo status for a specific path   |
  | `PUT`    | `/admin/embargoes/{path}`     | Update the `embargo_until` date or note  |
  | `DELETE` | `/admin/embargoes/{path}`     | Lift (remove) an embargo immediately     |

- `POST /admin/embargoes` request body:

  ```json
  {
    "asset_path": "campaigns/fall24/hero.jpg",
    "embargo_until": "2026-09-15T00:00:00Z",
    "note": "Fall 2026 campaign — do not release before launch date"
  }
  ```

- Creating an embargo on a path that already has one MUST return `409 Conflict`.
- `embargo_until` MUST be in the future at creation time; a past datetime MUST return
  `400 Bad Request`.
- Lifting an embargo MUST immediately invalidate the local embargo cache for that path.
- All embargo mutations (create, update, delete) MUST be written to the audit trail
  (FR-14).

### FR-13: Admin Authentication and Authorization

- All `/admin/*` endpoints MUST require authentication.
- **Two authentication modes** are supported, both via `Authorization: Bearer <token>`:

  **Mode A — SSO / OIDC (human admins):**
  - Rendition MUST validate OIDC ID tokens / access tokens issued by a configured
    Identity Provider (IdP): Okta, Azure AD (Entra ID), Google Workspace, or any
    standards-compliant OIDC provider.
  - The IdP issuer URL and audience MUST be configurable:
    `RENDITION_OIDC_ISSUER` (e.g. `https://company.okta.com/oauth2/default`)
    `RENDITION_OIDC_AUDIENCE` (e.g. `rendition-admin`)
  - Token validation MUST verify: signature (via IdP JWKS endpoint), expiry, issuer,
    and audience. The JWKS endpoint MUST be fetched and cached with periodic refresh.
  - Group/role claims from the IdP token MUST be used for authorization:
    `RENDITION_OIDC_ADMIN_GROUP` (e.g. `rendition-admins`) — only members of this
    group may access `/admin/*`.
  - The SSO login flow (browser redirect, authorization code exchange) is handled by
    the IdP and any API gateway / BFF in front of Rendition. Rendition itself validates
    tokens — it does not implement OAuth2 redirect flows.

  **Mode B — API key (machine/service clients):**
  - For CI/CD automation or service-to-service calls, API key auth MUST remain
    supported alongside OIDC.
  - API keys MUST be stored as `SHA-256`-hashed values in configuration
    (`RENDITION_ADMIN_API_KEYS`, comma-separated).
  - Raw keys MUST exist only at provisioning time and in a secrets manager — never
    in source code or plaintext config.

- If both modes are configured, either is accepted. If only one is configured, only
  that mode is active.
- Unauthenticated or invalid-token requests to `/admin/*` MUST return `401 Unauthorized`.
- There is no role hierarchy in v1 — all authenticated admins have full access.
- Admin endpoints MUST NOT be exposed on the public-facing port. Separate internal
  listener: `RENDITION_ADMIN_BIND_ADDR` (default: `127.0.0.1:3001`).

### FR-14: Embargo Enforcement in the CDN Serve Path

- Before fetching or transforming any asset, `serve_asset` MUST check the embargo store
  for the requested `asset_path`.
- If an active embargo exists (current UTC time is before `embargo_until`):
  - The response MUST be `HTTP 451 Unavailable For Legal Reasons` with a generic body
    (`"asset unavailable"`). **Rationale**: `451` is semantically correct for
    legally/commercially withheld content; `403` leaks that the asset exists; `404`
    is deceptive and complicates debugging for admins.
  - The `embargo_until` datetime MUST NOT be included in the response body (avoid
    leaking launch dates to end users).
  - The request MUST be logged server-side with the embargo details (for admin
    auditability) but MUST NOT expose those details to the caller.
- If the embargo has expired (current UTC time ≥ `embargo_until`):
  - The embargo record MAY be lazily deleted or ignored; the asset is served normally.
  - A background cleanup job SHOULD remove expired records periodically.
- The embargo check MUST use the local in-memory cache (FR-11) to avoid a round-trip
  to the embargo store on every request.
- Embargoed responses MUST NOT be stored in the transform cache (FR-03).

### FR-15: Automatic Format Negotiation (`fmt=auto`)

- The CDN MUST support a `fmt=auto` value that selects the smallest-size format the
  requesting client can accept, based on the `Accept` request header.
- Preference order: AVIF → WebP → JPEG (PNG only for assets with transparency).
- When `fmt=auto` is used, the `Vary: Accept` response header MUST be set so CDN edge
  caches and browsers do not serve the wrong format to subsequent clients.
- This is Rendition's primary gap-closer vs Imgix/ImageKit's automatic format selection.

### FR-16: Image Sharpening

- The transform pipeline MUST support an `unsharp` (unsharp mask) parameter:
  `unsharp=radius,sigma,amount,threshold` — values matching libvips `sharpen` op.
- A simplified `sharp=1` convenience alias MUST apply a sensible default sharpening
  preset (equivalent to Scene7's `op_sharpen=1`).
- Sharpening MUST be applied as the final step before encoding (after resize/crop).

### FR-17: Watermarking and Compositing

- The API MUST support overlaying a watermark image onto the source asset:
  `layer=path/to/watermark.png&layer_pos=center|topleft|topright|bottomleft|bottomright`
  `&layer_opacity=0–100`
- Watermark assets MUST be resolved from the same storage backend as source assets.
- This closes the Scene7 watermarking / basic compositing capability.
- Full multi-layer compositing (image templates with text/data-driven personalization)
  is **deferred to a future major version** and noted as a known gap vs Scene7.

### FR-18: Image Sets and Named Presets

- Admins MUST be able to define **named transform presets** stored in the embargo/config
  store (reusing the same durable backend from FR-11):
  `GET /cdn/product.jpg?preset=thumbnail` expands to the stored parameter set.
- Presets MUST be manageable via the admin API:
  `POST/GET/PUT/DELETE /admin/presets`
- This replaces Scene7's Image Set / viewer preset concept for pure URL-based delivery
  and reduces per-URL parameter verbosity (addressing the DX gap).

### FR-19: Content-Aware Smart Crop (AI-Assisted Focal Point)

- The API MUST support a `fit=smart` mode that uses saliency detection to identify the
  focal point of an image and crop around it, rather than always center-cropping.
- Implementation: libvips `smartcrop` operation (available in libvips ≥ 8.5).
- `fit=smart` MUST be explicitly documented as distinct from `fit=crop` (center crop).
- This closes Scene7 Smart Crop / Cloudinary content-aware crop.

### FR-20: Custom Domain Support

- Rendition MUST serve assets under operator-configured custom domains (e.g.
  `images.lululemon.com`) with no dependency on a vendor-controlled subdomain.
- Custom domain configuration is handled at the infrastructure/reverse-proxy layer
  (Nginx, CloudFront, Kubernetes Ingress) — Rendition itself MUST NOT hardcode any
  domain names.
- The `build_app` configuration MUST accept an optional `RENDITION_PUBLIC_BASE_URL`
  for generating canonical asset URLs in API responses (e.g. preset listing).
- This closes Scene7's `scene7.com` subdomain lock-in.

### FR-21: Multi-CDN / BYO-CDN Readiness

- Rendition MUST be CDN-agnostic — deployable behind any CDN (CloudFront, Fastly,
  Akamai, Cloudflare, Azure CDN) with no vendor-specific behaviour in the application
  layer.
- Cache-control headers MUST be configurable: `RENDITION_CACHE_CONTROL_PUBLIC`
  (default: `public, max-age=31536000, immutable` for versioned URLs).
- The `Vary` header MUST be set correctly for `fmt=auto` responses (FR-15) to prevent
  CDN cache poisoning across format-negotiated variants.
- Surrogate-Key / Cache-Tag headers MUST be emitted per asset path to enable
  targeted CDN cache purging:
  `Surrogate-Key: asset:{asset_path}` (Fastly / Varnish compatible).
- This directly addresses Scene7's CDN lock-in and ImageKit's multi-CDN strength.

### FR-22: Video Passthrough Delivery

- Rendition MUST support passthrough delivery of video assets (`mp4`, `webm`, `mov`)
  with correct MIME types and `Content-Range` / partial content (`HTTP 206`) support
  for byte-range requests (HTML5 `<video>` seek support).
- Video transcoding and adaptive bitrate streaming (HLS/DASH) are **out of scope for
  this version** and noted as a known gap vs Scene7's full video capability.
  A future `src/video/` module is anticipated.
- This covers the minimum viable video delivery use case without the full Scene7 video
  suite complexity.

### FR-10: Technical Debt Resolution

- Replace `S3Storage::get/exists` `todo!()` panics with proper typed errors that
  propagate to the caller (resolved by FR-01).
- Replace `webp_save_buffer` suffix-string workaround with a clean implementation once
  the minimum libvips version is documented and enforced.
- Document the minimum required libvips version in `README.md` and `Cargo.toml`
  metadata.

---

## Non-Functional Requirements

### NFR-01: Test Coverage

- **Target**: 80% line/branch coverage across unit, integration, and end-to-end tests.
- Coverage MUST be measured with `cargo-llvm-cov` or equivalent.
- Critical paths (transform pipeline, S3 adapter, config parsing, cache) MUST have
  near-100% coverage.
- A coverage report MUST be generated as part of the CI pipeline.

### NFR-02: Property-Based Testing

- The `proptest` crate MUST be added as a dev-dependency.
- PBT MUST be applied to (at minimum):
  - `TransformParams` deserialization round-trips (PBT-02)
  - Transform pipeline invariants: output dimensions match fit-mode semantics for all
    valid inputs (PBT-03)
  - Cache key determinism: same params always produce the same cache key (PBT-03)
  - Config parsing: valid env-var sets always produce a valid `Config`; invalid sets
    always fail (PBT-03)
- PBT seed MUST be logged on failure (PBT-08).
- See `extensions/testing/property-based/property-based-testing.md` for full rule set.

### NFR-03: Security

- All 15 rules in `extensions/security/baseline/security-baseline.md` apply as
  blocking constraints.
- Key items applicable to this project:
  - **SECURITY-01**: S3 buckets MUST enforce encryption at rest; all S3 calls MUST use
    HTTPS (TLS enforced by AWS SDK).
  - **SECURITY-03**: Structured logging with no secrets in logs (FR-07).
  - **SECURITY-04**: HTTP security headers on all responses (FR-06).
  - **SECURITY-05**: Input validation on all API parameters (FR-09).
  - **SECURITY-06**: S3 IAM role MUST use least-privilege (only `s3:GetObject`,
    `s3:HeadObject` on the specific bucket — no wildcards).
  - **SECURITY-08**: Rate limiting on public endpoints (FR-05); no auth required for
    CDN (public asset delivery), but CORS MUST be configured explicitly.
  - **SECURITY-09**: Error responses MUST NOT expose internal details (FR-08).
  - **SECURITY-10**: `Cargo.lock` MUST be committed; dependency vulnerability scanning
    (e.g., `cargo-audit`) MUST be added to CI.
  - **SECURITY-11**: Rate limiting and input validation as layered controls (FR-05,
    FR-09).
  - **SECURITY-15**: All external calls (`tokio::fs`, AWS SDK, libvips) MUST have
    explicit error handling with fail-closed semantics (FR-08).

### NFR-04: Performance

- Transform cache (FR-03) MUST reduce repeated-request latency by ≥ 90% for cached
  assets (benchmark target).
- libvips `spawn_blocking` usage MUST be maintained to avoid blocking the Tokio reactor.
- Large asset streaming: assets above a configurable threshold SHOULD be streamed rather
  than fully buffered (future consideration — document as a known gap for v2).

### NFR-05: Observability

- Structured JSON logging MUST be configurable via `RUST_LOG` (existing).
- A `GET /metrics` endpoint (Prometheus-compatible) SHOULD expose: request count,
  error count, cache hit/miss ratio, transform latency histogram.
- This is a SHOULD for the current iteration — logged as a near-term follow-on.

### NFR-06: Portability (Hexagonal Architecture)

- AWS SDK usage MUST be confined to a dedicated adapter module (e.g.,
  `src/storage/s3.rs`) behind the `StorageBackend` trait.
- No AWS SDK types MUST leak into `api`, `transform`, or `lib.rs`.
- Adding a new storage backend in the future MUST require only: a new adapter module
  implementing `StorageBackend` + a config variant — no changes to API or transform
  layers.

### NFR-07: Supply Chain Security

- `Cargo.lock` MUST be committed to version control.
- `cargo-audit` MUST be added to the CI pipeline for dependency vulnerability scanning.
- All dependencies MUST be sourced from `crates.io` (the official Rust registry).

---

## User Scenarios

### Scenario 1: Serve a Cached Transform

1. Client requests `GET /cdn/products/shoe.jpg?wid=400&fmt=webp`.
2. Embargo check passes (no embargo for this path).
3. Cache is checked — entry exists (hit).
4. Cached bytes returned immediately with correct `Content-Type: image/webp`.
5. Log entry records cache hit, latency < 5 ms.

### Scenario 2: First-Time S3 Asset Transform

1. System starts with `RENDITION_STORAGE_BACKEND=s3`.
2. Client requests `GET /cdn/hero.jpg?hei=300&fit=crop`.
3. Embargo check passes.
4. `S3Storage::exists` → `HeadObject` → exists.
5. `S3Storage::get` → `GetObject` → bytes.
6. Transform pipeline applies crop + height constraint via libvips.
7. Result stored in cache.
8. Response returned to client.

### Scenario 3: Automatic Format Negotiation

1. Client sends `GET /cdn/product.jpg?wid=800&fmt=auto` with
   `Accept: image/avif,image/webp,*/*`.
2. Rendition selects AVIF (highest priority accepted next-gen format).
3. Response: `200 Content-Type: image/avif`, `Vary: Accept`.
4. A second client with `Accept: */*` (no AVIF support) receives JPEG instead.
5. CDN edge caches both variants separately due to `Vary: Accept`.

### Scenario 4: Smart Crop for Mobile Thumbnail

1. Merchandiser uploads a runway photo with the model off-centre.
2. Client requests `GET /cdn/runway/look-12.jpg?wid=200&hei=200&fit=smart`.
3. libvips saliency detection identifies the face/focal point.
4. Image is cropped around the focal point, not the geometric centre.
5. Response: `200` with a well-composed 200×200 thumbnail.

### Scenario 5: Watermarked Asset Delivery

1. Client requests `GET /cdn/products/jacket.jpg?wid=1200&layer=brand/watermark.png&layer_pos=bottomright&layer_opacity=60`.
2. Rendition fetches both `products/jacket.jpg` and `brand/watermark.png` from storage.
3. Watermark is composited at bottom-right at 60% opacity.
4. Response: `200` with the watermarked image.

### Scenario 6: Named Preset Usage

1. Admin has created a preset `thumbnail` = `{wid:300, hei:300, fit:smart, fmt:auto}`.
2. Client requests `GET /cdn/products/shoe.jpg?preset=thumbnail`.
3. Rendition expands the preset to its stored parameters.
4. Transform applied; response returned.
5. The URL is short and readable — no verbose parameter string.

### Scenario 7: Embargoed Asset Request

1. Admin has embargoed `campaigns/aw26/hero.jpg` until `2026-09-01T00:00:00Z`.
2. Client requests `GET /cdn/campaigns/aw26/hero.jpg` before that date.
3. Embargo check fires — active embargo found in cache.
4. Response: `451 Unavailable For Legal Reasons` with body `"asset unavailable"`.
5. `embargo_until` is NOT included in the response.
6. Request is logged server-side with embargo details and request ID.

### Scenario 8: Admin Sets an Embargo via SSO

1. Merchandising admin authenticates via Okta SSO; receives OIDC access token.
2. Admin's tooling calls `POST /admin/embargoes` with `Authorization: Bearer <token>`.
3. Rendition validates the OIDC token: signature ✓, expiry ✓, audience ✓,
   group membership (`rendition-admins`) ✓.
4. Embargo record created; local cache invalidated for that path.
5. Immediately, all CDN requests for that path return `451`.
6. Audit log records: `created_by` = admin's `sub` claim, timestamp, path, date.

### Scenario 9: Video Passthrough with Byte-Range

1. Client's HTML5 `<video>` player requests
   `GET /cdn/videos/catwalk-ss26.mp4` with `Range: bytes=0-1048575`.
2. Rendition detects `.mp4` — no transform applied.
3. `S3Storage::get` streams the requested byte range.
4. Response: `206 Partial Content`, `Content-Type: video/mp4`,
   `Content-Range: bytes 0-1048575/48234567`.
5. Player seeks without re-downloading the full file.

### Scenario 10: Invalid Transform Parameters

1. Client requests `GET /cdn/image.jpg?wid=99999&qlt=200`.
2. Input validation rejects `wid > 8192` and `qlt > 100`.
3. Response: `400 Bad Request` with generic message.
4. No libvips or S3 call is made.

### Scenario 11: Rate Limit Exceeded

1. Client sends > N requests per second from the same IP.
2. Server responds `429 Too Many Requests` with `Retry-After` header.
3. No asset processing occurs for the rejected requests.

### Scenario 12: S3 Circuit Breaker Open

1. S3 becomes unreachable; 5 consecutive `GetObject` calls time out.
2. Circuit breaker opens; all subsequent storage calls fail fast.
3. All CDN requests return `503 Service Unavailable`.
4. `/health/ready` returns `503` — Kubernetes stops routing traffic to this pod.
5. After 30 s cooldown, circuit enters half-open; a probe request tests S3.
6. On success, circuit closes; `/health/ready` returns `200`; traffic resumes.

### Scenario 13: Startup Config Validation Failure

1. `RENDITION_STORAGE_BACKEND=s3` is set but `RENDITION_S3_BUCKET` is missing.
2. Process exits immediately with a clear error identifying the missing variable.
3. No server socket is opened.

---

## Quality Attributes

These define the quality attributes the system must exhibit at enterprise scale for a
high-volume retailer (baseline: lululemon-class traffic — peak tens of thousands of
concurrent CDN requests, seasonal spikes of 5–10× normal load).

### QA-01: Scalability

- The service MUST be stateless — no in-process state that cannot be reconstructed
  prevents horizontal scaling. The transform cache (FR-03) is node-local and acceptable
  as a performance optimisation, not a correctness dependency.
- All configuration MUST be injectable via environment variables (FR-02) so the service
  runs identically in any number of replicas without coordination.
- The S3 client MUST use a configurable connection pool (`RENDITION_S3_MAX_CONNECTIONS`,
  default: `100`) to prevent socket exhaustion under load.
- The number of concurrent libvips transform tasks MUST be bounded by a configurable
  semaphore (`RENDITION_TRANSFORM_CONCURRENCY`, default: number of logical CPUs × 2) to
  prevent memory exhaustion from unbounded `spawn_blocking` queuing.
- Throughput target: ≥ 1 000 requests/second per instance at P99 latency ≤ 200 ms for
  cache-hit requests (no transform); ≥ 200 requests/second per instance for uncached,
  full-transform requests on a standard 4-core instance.

### QA-02: Reliability

- **Availability target**: 99.9% measured over a rolling 30-day window (≤ 43.8 min
  downtime/month).
- **Graceful shutdown**: On `SIGTERM`, the server MUST stop accepting new connections,
  drain in-flight requests (up to a configurable `RENDITION_DRAIN_TIMEOUT_SECONDS`,
  default: `30`), then exit cleanly. Kubernetes rolling deploys depend on this.
- **Retry with backoff**: Transient S3 errors (5xx, throttling, network timeouts) MUST
  be retried with exponential backoff (max 3 attempts, initial delay 100 ms, jitter
  applied). Non-retriable errors (4xx) MUST NOT be retried.
- **Circuit breaker**: The S3 storage adapter MUST implement a circuit breaker. After
  a configurable number of consecutive failures (`RENDITION_S3_CB_THRESHOLD`, default:
  `5`), the circuit opens and requests fail fast with `503 Service Unavailable` for a
  configurable cooldown (`RENDITION_S3_CB_COOLDOWN_SECONDS`, default: `30`) before the
  half-open probe.
- **Timeout enforcement**: All I/O operations MUST have explicit timeouts:
  - S3 `GetObject` / `HeadObject`: `RENDITION_S3_TIMEOUT_MS` (default: `5000`)
  - Filesystem reads (`LocalStorage`): `RENDITION_LOCAL_TIMEOUT_MS` (default: `2000`)
  - Total request timeout (including transform): `RENDITION_REQUEST_TIMEOUT_MS`
    (default: `30000`)
- **Health probes**: The `/health` endpoint MUST be split into:
  - `GET /health/live` — liveness: always `200` if the process is running.
  - `GET /health/ready` — readiness: `200` only if the storage backend is reachable and
    the service can accept requests; `503` otherwise. Kubernetes uses readiness to gate
    traffic during startup and circuit-open periods.

### QA-03: Observability

- **Structured logging**: All logs MUST be emitted as JSON to stdout (FR-07) for
  ingestion by Datadog, CloudWatch, Splunk, or equivalent. Log level configurable via
  `RUST_LOG`.
- **Distributed tracing**: The service MUST emit OpenTelemetry (OTLP) traces for every
  inbound request, with child spans for: storage fetch, transform execution, cache
  lookup, and cache write. The OTLP exporter endpoint MUST be configurable via the
  standard `OTEL_EXPORTER_OTLP_ENDPOINT` env var.
- **Metrics (Prometheus)**: A `GET /metrics` endpoint MUST expose the following Prometheus
  metrics:
  - `rendition_requests_total{method, path_pattern, status}` — request counter
  - `rendition_request_duration_seconds{path_pattern, status}` — request latency
    histogram (P50, P95, P99 buckets)
  - `rendition_transform_duration_seconds{fmt, fit}` — transform latency histogram
  - `rendition_cache_hits_total` / `rendition_cache_misses_total` — cache counters
  - `rendition_cache_size_entries` — current LRU cache occupancy gauge
  - `rendition_storage_errors_total{backend, operation, error_type}` — storage error
    counter
  - `rendition_circuit_breaker_state{backend}` — gauge: `0` = closed, `1` = open,
    `2` = half-open
- **Request ID propagation**: `X-Request-Id` MUST be forwarded if present in the
  inbound request, or generated if absent, and included in all downstream log/trace
  spans (FR-07).
- **SLO instrumentation**: The metrics above MUST be sufficient to define and monitor
  SLOs (e.g., "99% of CDN requests complete in < 500 ms over a 5-minute window").

### QA-04: Performance

- **Latency targets** (per-instance, single-core baseline, measured at P99):
  - Cache hit (no transform): ≤ 10 ms
  - Uncached JPEG → JPEG passthrough: ≤ 100 ms for assets ≤ 2 MB
  - Uncached resize/convert (typical product image, 2–5 MB source → WebP 800 px):
    ≤ 500 ms
- **Memory budget**: Peak RSS per instance MUST NOT exceed 1 GB under sustained load.
  libvips transform concurrency limit (QA-01) MUST be tuned to enforce this.
- **Cache efficiency**: Target ≥ 80% cache hit ratio under steady-state retail traffic
  (product image requests are highly repetitive).
- **Asset size limit**: Reject assets > `RENDITION_MAX_PAYLOAD_BYTES` (default 50 MB)
  before initiating a transform (FR-04). This prevents runaway memory from oversized
  source images.

### QA-05: Deployability

- **Container-ready**: The binary MUST run as a non-root user inside a minimal Docker
  image (distroless or `debian:slim`). A production `Dockerfile` MUST be provided.
- **Kubernetes-ready**: The service MUST support Kubernetes deployment out of the box:
  - Liveness (`/health/live`) and readiness (`/health/ready`) probe endpoints (QA-02).
  - Graceful shutdown on `SIGTERM` (QA-02).
  - All config via env vars (FR-02).
  - A reference `k8s/` directory MUST be provided with `Deployment`, `Service`, and
    `HorizontalPodAutoscaler` manifests.
- **Zero-downtime rolling deploys**: Graceful shutdown + readiness gate enable
  zero-downtime updates in Kubernetes.
- **Image tagging**: Docker images MUST be tagged with the application semantic version;
  `latest` MUST NOT be used in production manifests (SECURITY-10).

### QA-06: Resilience and Fault Tolerance

- **Bulkhead isolation**: libvips CPU work runs in `spawn_blocking` and is bounded by
  the transform concurrency semaphore (QA-01). This ensures image-processing overload
  does not starve the Tokio I/O reactor or health-probe handlers.
- **Graceful degradation**: If the transform pipeline fails for a non-fatal reason
  (e.g., unsupported source format), the system SHOULD return the original unmodified
  asset bytes rather than a `500`, if the original asset is available. This behaviour
  MUST be configurable (`RENDITION_PASSTHROUGH_ON_TRANSFORM_ERROR`, default: `false`).
- **Backpressure**: When the transform semaphore is exhausted (all workers busy), new
  requests for uncached transforms MUST receive `503 Service Unavailable` with a
  `Retry-After` header rather than queuing indefinitely. The queue depth limit MUST be
  configurable (`RENDITION_TRANSFORM_QUEUE_DEPTH`, default: `50`).

### QA-07: Maintainability

- **Semantic versioning**: The application MUST follow SemVer. The version MUST be
  embedded in the binary and exposed in the `GET /health/ready` response body.
- **API stability**: The `/cdn/*` URL parameter contract is considered the public API.
  Breaking changes to parameter names or semantics MUST increment the major version.
- **Changelog**: A `CHANGELOG.md` MUST be maintained with entries for every release.
- **Dependency freshness**: Dependency updates MUST be reviewed at least quarterly.
  `cargo-outdated` output MUST be reviewed before each release.
- **Documentation**: `README.md` MUST document: all env-var configuration knobs, the
  full URL parameter reference, deployment prerequisites (libvips version), and a
  quickstart guide.

### QA-08: Testability

- **80% coverage target** (NFR-01) applies to all new code introduced for the above
  operational characteristics — circuit breaker logic, retry, rate limiting, graceful
  shutdown, and metrics must all have automated tests.
- **Load testing**: A `k6` or `wrk` load-test script MUST be provided in `load-tests/`
  that can validate P99 latency targets (QA-04) against a running instance.
- **Chaos/fault injection**: The S3 adapter design MUST allow injecting a mock that
  simulates transient failures, throttling, and timeouts, enabling circuit breaker and
  retry tests without real AWS calls.

### QA-09: Security — Embargo Feature

The embargo feature introduces the first **authenticated surface** in Rendition and
triggers additional security requirements beyond the baseline:

- Admin API MUST be network-isolated (separate bind address, internal only) — no
  public internet exposure (SECURITY-07).
- API key material MUST be stored hashed; raw keys MUST exist only in a secrets manager
  at rest (SECURITY-12).
- Embargo check on the CDN hot path MUST fail closed — if the embargo store is
  unreachable, the system MUST treat all paths with *known* embargoes as embargoed
  (deny-by-default), and log the degraded state. Paths with no embargo record MAY be
  served (circuit breaker on the embargo store is open → cache-only mode).
- `451` status code MUST be used for embargoed assets (not `403` or `404`) — leaking
  a `403` reveals existence; `404` is deceptive and impedes admin debugging.
- All embargo mutation operations MUST be captured in an immutable audit log (FR-14,
  SECURITY-13).

## Constraints and Assumptions

- **Language**: Rust (no language change).
- **Async runtime**: Tokio (no change).
- **HTTP framework**: Axum (no change).
- **Image processing**: libvips (no change); minimum version to be documented.
- **Primary cloud target**: AWS S3 for initial S3 adapter.
- **End-user authentication**: Rendition is a public media CDN; end-user auth at the
  edge/gateway layer is out of scope. Admin authentication (FR-13) is in scope.
- **Embargo storage backend**: The durable store for embargo records is an open
  architectural decision (Redis, DynamoDB, or PostgreSQL). To be resolved in
  Application Design.
- **No persistent state**: All server state is either ephemeral (cache) or external
  (S3/filesystem). No database is introduced.
- **Streaming for large assets**: Deferred to a future iteration; buffered I/O is
  acceptable for v1.
- **Video transcoding / HLS / DASH**: Out of scope. Passthrough delivery only (FR-22).
  Full video pipeline is a future major version.
- **Rich media viewers** (spin sets, eCatalogs, shoppable video, interactive video):
  Out of scope. Rendition is a delivery and transformation engine; viewer UI components
  are a separate concern.
- **Multi-layer compositing / image templates / data-driven personalization**: Out of
  scope for v1. FR-17 covers single watermark overlay only.
- **AI tagging, background removal, generative fill**: Out of scope for v1. FR-19
  covers libvips saliency-based smart crop only.
- **Media Portal / brand portal / DAM integration**: Out of scope. Rendition is a
  headless delivery layer; DAM integrations connect at the storage backend level.
- **Adobe Analytics / viewer tracking**: Out of scope. Observability (QA-03) covers
  infrastructure metrics; product analytics integrations are the responsibility of the
  embedding application.

# Services

Services are logical orchestration boundaries — they describe how components
collaborate to fulfil a business capability. In Rendition's architecture these
are not separate structs but named flows wired in `src/lib.rs` and the handler
functions.

---

## SVC-01 — Asset Delivery Service

**Orchestrated by:** `serve_asset<S>` in `src/api/mod.rs`

**Purpose:** Deliver a (possibly transformed) media asset in response to a CDN
request. Enforces embargoes, uses cached transforms, and sets all required
response headers.

**Flow:**

```text
1. Validate & sanitise asset_path (strip leading slash, path traversal check)
2. Parse & validate TransformParams (FR-09) → 400 on violation
3. Resolve preset (if ?preset= present) via PresetStore
4. Merge preset params with explicit URL params (explicit wins)
5. Negotiate format (fmt=auto → negotiate_format(Accept header))
6. EmbargoEnforcer.check(path) → 451 if active embargo found
7. Compute CacheKey = compute_cache_key(path, params, format)
8. TransformCache.get(key) → if hit: set headers, return 200/206
9. StorageBackend.exists(path) → 404 if missing
10. StorageBackend.get(path) or get_range(path, range) for byte-range requests
11. transform::apply(bytes, params, format) via spawn_blocking
12. TransformCache.put(key, response)  [skip if asset was embargoed — never reached]
13. Set response headers:
    - Content-Type from transform result
    - Cache-Control from config
    - Surrogate-Key: asset:{path}
    - Vary: Accept  (if fmt=auto)
    - Accept-Ranges: bytes
    - X-Request-Id from request extension
14. Return 200 OK or 206 Partial Content
```

**Components involved:** C-01, C-02, C-03, C-04, C-05, C-06, C-09, C-10

**Error responses:**

| Condition | Status |
|---|---|
| Active embargo | 451 Unavailable For Legal Reasons |
| Asset not found | 404 Not Found |
| Invalid params | 400 Bad Request |
| Transform failure | 500 Internal Server Error (generic body) |
| Storage error | 500 Internal Server Error (circuit open → 503) |
| Rate limit exceeded | 429 Too Many Requests + Retry-After |
| Payload too large | 413 Payload Too Large |

---

## SVC-02 — Embargo Management Service

**Orchestrated by:** Admin handlers in `src/admin/embargo_handlers.rs`

**Purpose:** Allow authenticated admins to create, read, update, and delete
embargo records. Maintain consistency between the durable store and the
in-process enforcer cache.

**Flow — Create embargo:**

```text
1. AuthLayer validates identity → 401/403 if invalid
2. Validate request body (embargo_until in future, valid path) → 400
3. EmbargoStore.get(path) → 409 if already embargoed
4. EmbargoStore.put(record) with audit fields (created_by, created_at)
5. Write to audit log (FR-14)
6. Return 201 Created with EmbargoRecord + Surrogate-Key value for CDN purge
```

**Flow — Delete (lift) embargo:**

```text
1. AuthLayer validates identity
2. EmbargoStore.get(path) → 404 if not found
3. EmbargoStore.delete(path)
4. EmbargoEnforcer.invalidate(path)  ← immediate local cache flush
5. Write to audit log
6. Return 204 No Content + Surrogate-Key header for operator CDN purge call
```

**Components involved:** C-05, C-08, C-10

---

## SVC-03 — Preset Management Service

**Orchestrated by:** Admin handlers in `src/admin/preset_handlers.rs`

**Purpose:** Allow admins to define named transform presets that simplify CDN
URLs and encapsulate approved transform configurations.

**Flow — Create preset:**

```text
1. AuthLayer validates identity
2. Validate preset name (alphanumeric + hyphens, max 64 chars)
3. Validate TransformParams in body (same rules as FR-09)
4. PresetStore.get(name) → 409 if already exists
5. PresetStore.put(NamedPreset { name, params, created_by, created_at })
6. Return 201 Created
```

**Preset resolution in serve_asset (SVC-01 step 3):**

```text
1. If ?preset=<name> present: PresetStore.get(name) → 404 if not found
2. resolve_params(preset, explicit_url_params)
3. Continue pipeline with merged params
```

**Components involved:** C-06, C-08

---

## SVC-04 — Admin Authentication Service

**Orchestrated by:** `AuthLayer` Tower middleware in `src/admin/auth.rs`

**Purpose:** Validate every request to `/admin/*` before it reaches a handler.
Support two modes: OIDC JWT and API key.

**Flow — OIDC mode:**

```text
1. Extract Authorization: Bearer <token> header → 401 if absent
2. JwksCache.validate(token):
   a. Parse JWT header to find key ID (kid)
   b. Look up key in cached JWKS
   c. Verify signature with RS256/ES256 key
   d. Verify expiry, issuer, audience claims
   e. On InvalidSignature: force-refresh JWKS once, retry
3. Extract groups claim, check RENDITION_OIDC_ADMIN_GROUP → 403 if not member
4. Inject AdminIdentity { sub, email, groups } into request extensions
5. Call next handler
```

**Flow — API key mode:**

```text
1. Extract Authorization: Bearer <key> or X-Api-Key: <key> header
2. Compute SHA-256(key)
3. Constant-time compare against RENDITION_ADMIN_API_KEYS hashes → 401 if no match
4. Inject AdminIdentity { sub: "api-key:<hash_prefix>" } into request extensions
5. Call next handler
```

**Components involved:** C-08

---

## SVC-05 — Application Bootstrap Service

**Orchestrated by:** `build_app()` in `src/lib.rs` and `main()` in
`src/main.rs`

**Purpose:** Initialise all components, wire dependencies, and start both TCP
listeners. Ensure fail-fast startup — any misconfiguration or dependency
failure prevents the server from starting.

**Flow:**

```text
1. AppConfig::load() → exit(1) on failure with clear error
2. init_otel(&config) → OtelGuard (flush on Drop)
3. Metrics::new() → register Prometheus metrics
4. Select storage backend:
   - config.storage_backend == Local  → LocalStorage::new(config.assets_path)
   - config.storage_backend == S3    → S3Storage::new(&config.s3()?)
5. RedisEmbargoStore::new(config.redis_url) → shared Redis pool
6. EmbargoEnforcer::new(store, config.embargo_cache_ttl)
7. RedisPresetStore::new(redis_pool)
8. MokaTransformCache::new(config.cache_max_entries, config.cache_ttl)
9. AppState { storage, cache, embargo, presets, config, metrics }
10. Build CDN router with cdn_middleware_stack
11. Build admin router with AuthLayer
12. Bind RENDITION_BIND_ADDR → cdn_listener
13. Bind RENDITION_ADMIN_BIND_ADDR → admin_listener
14. tokio::spawn(axum::serve(cdn_listener, cdn_router))
15. tokio::spawn(axum::serve(admin_listener, admin_router))
16. Await CancellationToken (SIGTERM/SIGINT) → graceful shutdown both servers
```

**Components involved:** C-01 through C-10 (all)

---

## SVC-06 — Cache Purge Service

**Orchestrated by:** `POST /admin/purge` handler in `src/admin/purge_handlers.rs`

**Purpose:** Allow admins to invalidate in-process transform cache entries by
asset path, typically after uploading a new version of an asset to storage.

**Flow:**

```text
1. AuthLayer validates identity
2. Parse PurgeRequest { paths: Vec<String> }
3. For each path: TransformCache.invalidate_by_path(path)
4. Optionally: emit Surrogate-Key value for upstream CDN purge integration
5. Return 200 { purged_count }
```

**Note:** This purges only the in-process cache on the receiving pod. In a
multi-pod deployment, the operator must also call the CDN edge purge API using
the `Surrogate-Key` value returned in asset responses. A future enhancement
could broadcast purge events via Redis pub/sub to all pods.

**Components involved:** C-04, C-08

# Application Design Plan

## Plan Status

- [x] Step 1: Resolve open architectural questions (user input required)
- [x] Step 2: Generate `components.md`
- [x] Step 3: Generate `component-methods.md`
- [x] Step 4: Generate `services.md`
- [x] Step 5: Generate `component-dependency.md`
- [x] Step 6: Generate `application-design.md` (consolidated)
- [x] Step 7: Validate and present for approval

---

## Open Architectural Questions

Each question includes a recommendation and a trade-off table. Accept the
recommendation or override it using the `[Answer]: <your answer>` tag. Reply "done" when all
answers are filled in.

---

### Q1 — Embargo Store Backend (ADR-0010 — open decision)

FR-11 requires durable embargo storage consistent across all Kubernetes pods.

| Criterion | Redis (ElastiCache) | DynamoDB | PostgreSQL (RDS) |
|---|---|---|---|
| Read latency | < 1 ms | 1–5 ms | 5–10 ms |
| TTL support | Native (`EXPIREAT`) | Native (TTL attribute) | Background job / pg_cron |
| Operational overhead | Moderate (Sentinel / Cluster for HA) | Low (fully serverless) | Moderate (RDS) |
| Cloud portability | High (portable) | Low (AWS-only) | High (portable) |
| Rich queries | No | Limited | Full SQL |
| v1 feature fit | Good | Good | Overkill for v1 |

#### Recommendation: Redis (ElastiCache)

Redis is the best fit for Rendition's access pattern: high-frequency reads,
infrequent writes, and a natural TTL per embargo record. The in-process
read-through cache absorbs most CDN-path reads; Redis only sees cold-cache
misses. ElastiCache is standard in AWS deployments. Both embargo records and
named presets (FR-18) map cleanly to Redis hashes with `EXPIREAT`. If the
deployment environment already includes DynamoDB and explicitly avoids Redis,
DynamoDB is a sound alternative. PostgreSQL is not recommended for v1 — its
query power is not needed and TTL management adds operational complexity.

[Answer]: Accepted — Redis (ElastiCache). See ADR-0010.
---

### Q2 — Admin API Listener Strategy

FR-13 requires admin endpoints to not be exposed on the public-facing port.

| Criterion | Two `TcpListener` binds (Option A) | Single listener, path split (Option B) |
|---|---|---|
| FR-13 compliance | Strong — port boundary is OS-enforced | Weak — relies on Nginx / NetworkPolicy |
| Operational clarity | Port `:3001` is always internal-only | Requires external config to block `/admin/*` |
| Code complexity | Two `tokio::net::TcpListener` + two `axum::serve` tasks | Single `serve`; auth middleware on `/admin/*` |
| K8s `Service` config | Two `Service` objects or one with two ports | One `Service` |
| Defense in depth | Higher | Lower |

#### Recommendation: Two `TcpListener` binds (Option A)

Binding the admin router to `127.0.0.1:3001` provides a hard network-layer
boundary — no Nginx rule or `NetworkPolicy` misconfiguration can accidentally
expose admin endpoints to CDN traffic. The code cost is low: one extra
`TcpListener` and one extra `tokio::spawn`. This also makes the K8s
`Service` manifest explicit about which port is external and which is
internal-only.

[Answer]: Accepted — Two TcpListener binds. See ADR-0013.
---

### Q3 — Configuration Parsing Library

FR-02 requires all `RENDITION_*` env vars parsed into a typed `Config` struct
with fail-fast validation at startup.

| Criterion | `envy` | `config` crate | `std::env::var` manual |
|---|---|---|---|
| Lines of code | ~5 (derive + one call) | ~20 (builder chain) | ~60–100 (one var per field) |
| Type coercion | Automatic via serde | Automatic | Manual per field |
| Layered config (env + file) | No | Yes | No |
| Extra dependencies | 1 (`envy`) | 1 (`config`) | 0 |
| Maintenance status | Active | Active | N/A |
| v1 need for file-based config | No | Over-engineered | N/A |

#### Recommendation: `envy`

`envy` deserialises environment variables directly into a `#[derive(Deserialize)]`
struct in one call. For Rendition's use case (env-var-only config, Kubernetes
`ConfigMap` / `Secret` injection, no TOML files), `envy` gives the most value
for the least code. Custom validation (e.g. confirming `RENDITION_S3_BUCKET`
is set when `RENDITION_STORAGE_BACKEND=s3`) is added as a `validate()` method
on `AppConfig` after `envy::from_env()` succeeds.

[Answer]: Accepted — envy crate. See ADR-0014.
---

### Q4 — Per-IP Rate Limiting Strategy

FR-05 requires per-IP rate limiting on `/cdn/*` with configurable RPS/burst
and `429 Too Many Requests` + `Retry-After` on breach.

| Criterion | `tower-governor` | Custom `DashMap` + `governor` |
|---|---|---|
| Lines of implementation code | ~10 (middleware config) | ~80–120 (custom layer) |
| IP extraction | Handled (with `PeerIpKeyExtractor`) | Manual (from `ConnectInfo` or header) |
| X-Forwarded-For / behind-proxy support | Via custom `KeyExtractor` | Manual parsing |
| Algorithm | GCRA (governor) | GCRA (governor) |
| Extra dependencies | `tower-governor`, `governor` | `governor`, `dashmap` |
| Maintenance risk | Moderate (smaller crate) | Low (you own the code) |

#### Recommendation: `tower-governor`

`tower-governor` wraps `governor` with a Tower middleware interface that
already handles IP extraction, per-key rate-limiter maps, and `429` responses.
The GCRA algorithm provides smooth rate limiting without thundering-herd spikes.
The only non-trivial configuration is `PeerIpKeyExtractor` vs a custom extractor
for deployments behind a CDN/proxy (where the real client IP is in
`X-Forwarded-For`) — this is a one-function implementation regardless of which
option is chosen.

[Answer]: Accepted — tower-governor. See ADR-0015.
---

### Q5 — OIDC Token Validation Library

FR-13 requires JWT validation against an OIDC provider's JWKS endpoint
(signature, expiry, issuer, audience, group claim check).

| Criterion | `jsonwebtoken` + manual JWKS | `openidconnect` crate |
|---|---|---|
| Scope | JWT decode + validate | Full OIDC client (discovery, PKCE, token, userinfo) |
| JWKS fetch/cache | Manual (`reqwest` + `RwLock`) | Handled internally |
| Code volume | ~100 lines | ~50 lines |
| Dependency weight | Light (`jsonwebtoken`, `reqwest`) | Heavy (pulls in many OIDC RFCs) |
| v1 need (token validation only) | Exact fit | Over-engineered |
| Future OIDC flows (device flow etc.) | Would require migration | Already supported |

#### Recommendation: `jsonwebtoken` + manual JWKS cache

Rendition validates tokens — it does not participate in OAuth2/OIDC flows
(those are handled by the IdP and any BFF in front of Rendition). A
`JwksCache` struct wrapping `reqwest` + `tokio::sync::RwLock` with a 1-hour
refresh is ~60 lines of straightforward code. `jsonwebtoken` handles RS256/ES256
signature verification and all standard claims. This keeps the dependency tree
lean and the implementation auditable.

[Answer]: Accepted — jsonwebtoken + manual JWKS cache. See ADR-0016.
---

### Q6 — Prometheus Metrics Library

NFR-05 requires a `GET /metrics` endpoint in Prometheus text format.

| Criterion | `prometheus` crate | `metrics` + `metrics-exporter-prometheus` |
|---|---|---|
| Lines to register a counter | ~5 (lazy_static macro) | ~3 (metrics::counter!) |
| Backend swappability | No (Prometheus-specific) | Yes (swap exporter) |
| Histogram bucket config | Manual (`Opts` + `register`) | Declarative |
| Maturity | Very high (official client) | High |
| Extra dependencies | 1 | 2 |
| v1 need for multiple backends | No | Premature |

#### Recommendation: `prometheus` crate

Prometheus is the only target metrics backend for Rendition v1. The `prometheus`
crate is the official Rust client, battle-tested, and directly supported by the
Prometheus community. The `metrics` facade abstraction adds an indirection layer
that provides no value until a second backend is needed. If Rendition ever needs
to emit to StatsD or OpenTelemetry metrics (beyond traces), the migration from
`prometheus` to the `metrics` facade is mechanical.

[Answer]: Accepted — prometheus crate. See ADR-0017.
---

### Q7 — Video Passthrough HTTP 206 Implementation

FR-22 requires byte-range (`HTTP 206`) support for video assets so HTML5
`<video>` seeking works across both `LocalStorage` and `S3Storage`.

| Criterion | `tower-http::ServeFile` | Custom `Range` header parsing |
|---|---|---|
| Works with `LocalStorage` | Yes (file-path based) | Yes |
| Works with `S3Storage` | No (needs file path, not `Vec<u8>`) | Yes |
| S3 native range fetch (`GetObject` with `Range`) | Not applicable | Yes — pass `Range` header to S3 API |
| Code volume | Minimal for local; inapplicable for S3 | ~80 lines (parser + response builder) |
| Architecture consistency | Breaks hexagonal abstraction | Consistent with `StorageBackend` trait |

#### Recommendation: Custom `Range` header parsing

`tower-http::ServeFile` cannot be used with `S3Storage` because it requires a
filesystem path. Custom range parsing keeps both adapters behind the
`StorageBackend` trait and enables an important optimisation for S3: pass the
`Range` header through to `GetObject` so only the requested byte range is
downloaded from S3 (avoiding full-file fetches for mid-video seeks). This is
the correct hexagonal architecture choice.

[Answer]: Accepted — custom Range header parsing. See ADR-0018.
---

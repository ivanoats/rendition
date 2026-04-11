# Rendition

![Rendition — open source enterprise media CDN](https://github.com/user-attachments/assets/76328afc-bfcc-4e60-a1cb-4be9b577209e)

[![Build](https://github.com/ivanoats/rendition/actions/workflows/ci.yml/badge.svg)](https://github.com/ivanoats/rendition/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.78+](https://img.shields.io/badge/rust-1.78%2B-orange.svg)](https://www.rust-lang.org/)

**Open-source, self-hosted media CDN. Scene7-compatible. Built in Rust.**

---

## The Problem

Adobe Scene7 / Dynamic Media powers the image pipelines of many of the world's
largest retailers. It is also a six-figure annual contract, a proprietary URL
scheme, and a deployment model that sits entirely outside the cloud-native
infrastructure you have spent years building.

Modern SaaS alternatives — Cloudinary, Imgix, ImageKit — are genuinely excellent.
But at 100 million+ monthly image requests, usage-based pricing scales with your
success in the wrong direction. And every one of them is still someone else's
infrastructure, someone else's uptime, someone else's data agreement.

## The Solution

Rendition is a self-hosted media CDN origin server. You deploy it on your own
infrastructure, behind any CDN edge (CloudFront, Fastly, Cloudflare). It reads
from your existing S3 bucket. Your Scene7 URLs keep working — Rendition uses the
same parameter names. The vendor contract goes away. The CDN bill reflects only
what you actually cache.

```text
https://images.example.com/cdn/products/shoe.jpg?wid=800&fmt=auto&qlt=85
```

That is a real Rendition URL. If it looks like a Scene7 URL, that is the point.

---

## Features

### Image transformation

- Resize (`wid`, `hei`) with five fit modes: `constrain`, `crop`, `stretch`,
  `fill`, `smart` (content-aware focal-point crop via libvips)
- Format conversion: `webp`, `avif`, `jpeg`, `png`, and `auto` (picks the best
  format the client accepts via the `Accept` header)
- Quality tuning (`qlt`), pre-resize crop (`crop`), rotation (`rotate`),
  flip (`flip`)
- Unsharp mask (`unsharp`) and one-shot sharpening (`sharp=1`)
- Watermark / compositing (`layer`, `layer_pos`, `layer_opacity`)
- Named transform presets (`?preset=thumbnail`) — stored in Redis, managed via
  the admin API

### Delivery

- Scene7-compatible URL parameter names — migrate with a DNS change
- Automatic format negotiation (`fmt=auto`) with `Vary: Accept` so CDN edges
  cache format variants correctly
- `Surrogate-Key: asset:{path}` on every response for targeted CDN cache purge
- Video passthrough with byte-range support (`HTTP 206`) for HTML5 `<video>`
  seeking
- Per-IP GCRA rate limiting with `429 Too Many Requests` + `Retry-After`
- Full HTTP security header set (HSTS, CSP, `X-Content-Type-Options`,
  `X-Frame-Options`)

### Operations

- Pluggable storage backends: Amazon S3 (with circuit breaker) or local
  filesystem for development. S3-compatible stores (MinIO, Cloudflare R2,
  Wasabi, Backblaze B2, DigitalOcean Spaces) work via
  `RENDITION_S3_ENDPOINT` — no code changes needed
- In-process LRU transform cache (`moka`) — configurable capacity and TTL;
  SHA-256 cache keys
- Embargoed assets: hold back campaign images until a launch date; returns
  `HTTP 451 Unavailable For Legal Reasons` (RFC 7725) — not `403` or `404`
- Admin API on an isolated internal port (`127.0.0.1:3001`) — OIDC SSO
  (Okta, Azure AD, Google Workspace) or SHA-256 API key authentication
- Prometheus `/metrics` endpoint and OpenTelemetry OTLP traces
- Split health probes: `/health/live` (always 200) and `/health/ready`
  (checks S3 circuit breaker and Redis reachability)
- Kubernetes-ready: Dockerfile, HPA on CPU + cache-miss rate, graceful shutdown

---

## Quick Start

### Prerequisites

- Rust 1.78+ — `rustup update stable`
- libvips 8.x — `brew install vips` (macOS) or `apt install libvips-dev` (Linux)
- An `assets/` directory with some images, or an S3 bucket

### Run locally

```bash
git clone https://github.com/ivanoats/rendition
cd rendition
mkdir -p assets
cp /path/to/some/image.jpg assets/
RUST_LOG=info cargo run
```

The CDN server starts on <http://localhost:3000>.

```bash
# Health check
curl http://localhost:3000/health/live
# {"status":"ok"}

# Serve and transform an image
curl -o out.webp "http://localhost:3000/cdn/image.jpg?wid=400&fmt=webp&qlt=80"
```

### Run with Docker

```bash
docker build -t rendition .
docker run -p 3000:3000 -v $(pwd)/assets:/assets \
  -e RENDITION_ASSETS_PATH=/assets \
  rendition
```

### Run tests

```bash
cargo test
# With coverage
cargo llvm-cov --open
```

---

## URL Transform API

```text
GET /cdn/{asset_path}?param=value&…
```

All parameters are optional. Unknown values fall back to documented defaults.

### Resize

| Parameter | Values | Default | Description |
|---|---|---|---|
| `wid` | 1–8192 | original | Output width in pixels |
| `hei` | 1–8192 | original | Output height in pixels |
| `fit` | `constrain` `crop` `stretch` `fill` `smart` | `constrain` | How to fit into the requested dimensions |

`fit=smart` uses libvips `smartcrop` for content-aware focal-point cropping.
`fit=crop` center-crops. `fit=constrain` preserves aspect ratio without
cropping (default).

### Format and quality

| Parameter | Values | Default | Description |
|---|---|---|---|
| `fmt` | `jpeg` `webp` `avif` `png` `auto` | `jpeg` | Output format. `auto` picks AVIF → WebP → JPEG based on `Accept` header. |
| `qlt` | 1–100 | `85` | Quality for lossy formats |

When `fmt=auto` is used, the response includes `Vary: Accept` so CDN edges
cache format variants separately.

### Crop, rotate, flip

| Parameter | Values | Default | Description |
|---|---|---|---|
| `crop` | `x,y,w,h` | — | Pre-resize crop rectangle (non-negative integers, `w`/`h` > 0) |
| `rotate` | `0` `90` `180` `270` | `0` | Clockwise rotation |
| `flip` | `h` `v` `hv` | — | Horizontal, vertical, or both |

### Sharpening

| Parameter | Values | Description |
|---|---|---|
| `sharp` | `1` | Apply default unsharp mask (equivalent to Scene7 `op_sharpen=1`) |
| `unsharp` | `radius,sigma,amount,threshold` | Full unsharp mask control |

### Watermark / compositing

| Parameter | Values | Description |
|---|---|---|
| `layer` | asset path | Path to overlay image (resolved from same storage backend) |
| `layer_pos` | `center` `topleft` `topright` `bottomleft` `bottomright` | Overlay position |
| `layer_opacity` | 0–100 | Overlay opacity percentage |

### Presets

| Parameter | Values | Description |
|---|---|---|
| `preset` | preset name | Expand a named transform preset stored in Redis. Explicit URL params override preset defaults. |

**Example — product thumbnail with watermark:**

```text
/cdn/products/boot.jpg?preset=thumbnail&layer=assets/logo.png&layer_pos=bottomright&layer_opacity=60
```

---

## Admin API

The admin API runs on a separate internal listener (`127.0.0.1:3001` by default)
and is never exposed via the public CDN port. All endpoints require
authentication.

**Authentication:** `Authorization: Bearer <token>`

- **OIDC / SSO** — JWT from Okta, Azure AD, or Google Workspace. Rendition
  validates the signature via the IdP's JWKS endpoint, checks expiry, audience,
  and group membership (`RENDITION_OIDC_ADMIN_GROUP`).
- **API key** — for CI/CD and service accounts. Keys are stored as SHA-256
  hashes (`RENDITION_ADMIN_API_KEYS`).

### Embargo endpoints

```text
POST   /admin/embargoes              Create an embargo
GET    /admin/embargoes              List active embargoes
GET    /admin/embargoes/{path}       Get embargo for a path
PUT    /admin/embargoes/{path}       Update embargo date or note
DELETE /admin/embargoes/{path}       Lift an embargo immediately
```

Embargoed assets return `HTTP 451 Unavailable For Legal Reasons` on the CDN
path. The response body never includes the release date. The `Surrogate-Key`
value returned by the create/delete endpoints can be used to issue a CDN purge.

### Preset endpoints

```text
POST   /admin/presets                Create a named preset
GET    /admin/presets                List all presets
GET    /admin/presets/{name}         Get a preset
PUT    /admin/presets/{name}         Update a preset
DELETE /admin/presets/{name}         Delete a preset
```

### Cache purge

```text
POST   /admin/purge                  Invalidate in-process cache by path list
```

---

## Configuration

All configuration is via environment variables. The server fails fast at startup
with a clear error if required variables are missing.

| Variable | Default | Description |
|---|---|---|
| `RENDITION_BIND_ADDR` | `0.0.0.0:3000` | CDN listener address |
| `RENDITION_ADMIN_BIND_ADDR` | `127.0.0.1:3001` | Admin API listener address |
| `RENDITION_STORAGE_BACKEND` | `local` | `local` or `s3` |
| `RENDITION_ASSETS_PATH` | `./assets` | Root path for local storage |
| `RENDITION_S3_BUCKET` | — | S3 bucket name (required when backend is `s3`) |
| `RENDITION_S3_REGION` | — | AWS region |
| `RENDITION_S3_ENDPOINT` | — | Custom endpoint for S3-compatible stores (MinIO, R2) — must be `https://` unless the insecure flag is set |
| `RENDITION_S3_PREFIX` | `""` | Key prefix within the bucket |
| `RENDITION_S3_MAX_CONNECTIONS` | `100` | HTTP connection pool size to S3 |
| `RENDITION_S3_TIMEOUT_MS` | `5000` | Per-attempt S3 call timeout |
| `RENDITION_S3_MAX_RETRIES` | `3` | Max retry attempts for transient S3 failures |
| `RENDITION_S3_RETRY_BASE_MS` | `50` | Base delay for full-jitter retry backoff |
| `RENDITION_S3_CB_THRESHOLD` | `5` | Consecutive failures before the circuit breaker opens |
| `RENDITION_S3_CB_COOLDOWN_SECS` | `30` | Circuit breaker cooldown before a half-open probe |
| `RENDITION_S3_ALLOW_INSECURE_ENDPOINT` | `false` | Permit `http://` S3 endpoints — LocalStack tests only, **never set in production** |
| `RENDITION_LOCAL_TIMEOUT_MS` | `2000` | Local filesystem read timeout |
| `RENDITION_CACHE_MAX_ENTRIES` | `1000` | Max transform cache entries (LRU eviction) |
| `RENDITION_CACHE_TTL_SECONDS` | `3600` | Cache entry TTL |
| `RENDITION_MAX_PAYLOAD_BYTES` | `52428800` | Max request/asset size (50 MB) |
| `RENDITION_RATE_LIMIT_RPS` | `100` | Per-IP requests per second |
| `RENDITION_RATE_LIMIT_BURST` | `200` | Per-IP burst capacity |
| `RENDITION_RATE_LIMIT_KEY` | `peer_ip` | `peer_ip` or `x_forwarded_for` |
| `RENDITION_CACHE_CONTROL_PUBLIC` | `public, max-age=31536000, immutable` | CDN `Cache-Control` header value |
| `RENDITION_PUBLIC_BASE_URL` | — | Base URL for canonical asset URLs in API responses |
| `RENDITION_REDIS_URL` | — | Redis connection URL for embargo and preset store |
| `RENDITION_EMBARGO_CACHE_TTL_SECONDS` | `30` | In-process embargo cache TTL |
| `RENDITION_OIDC_ISSUER` | — | OIDC issuer URL (e.g. `https://company.okta.com/oauth2/default`) |
| `RENDITION_OIDC_AUDIENCE` | — | OIDC audience (e.g. `rendition-admin`) |
| `RENDITION_OIDC_ADMIN_GROUP` | — | Required group claim for admin access |
| `RENDITION_ADMIN_API_KEYS` | — | Comma-separated SHA-256-hashed API keys |
| `RENDITION_OTEL_ENDPOINT` | — | OTLP collector endpoint (e.g. `http://collector:4317`) |
| `RUST_LOG` | `info` | Log level (`error` `warn` `info` `debug` `trace`) |

---

## Architecture

Rendition is a single Rust binary with a hexagonal (ports and adapters)
architecture. AWS SDK, Redis, and libvips are each confined to their own adapter
module and never leak into the API or transform layers.

```text
src/
├── config.rs          — typed env-var configuration (envy)
├── storage/           — StorageBackend trait + LocalStorage + S3Storage
├── transform/         — libvips pipeline, format negotiation, spawn_blocking
├── cache.rs           — in-process LRU (moka), SHA-256 cache keys
├── embargo/           — EmbargoEnforcer, EmbargoStore trait, RedisEmbargoStore
├── preset/            — PresetStore trait, RedisPresetStore
├── api/               — CDN request handler, AppState
├── admin/             — admin router, AuthLayer, JwksCache, CRUD handlers
├── middleware/        — Tower layers: RequestId, Trace, RateLimit, SecurityHeaders
└── observability/     — Prometheus metrics, OTEL exporter, health probes
```

- [`docs/architecture.md`](docs/architecture.md) — C4 diagrams (context,
  container, component), sequence diagrams, middleware stack, deployment topology
- [`docs/adr/`](docs/adr/) — 18 Architecture Decision Records covering every
  significant technical choice

---

## Scene7 / Dynamic Media Migration

Rendition uses the same URL parameter names as Scene7 by design. For most
implementations, migration is:

1. Deploy Rendition pointing at your existing asset S3 bucket
2. Update your CDN origin from the Scene7 hostname to your Rendition hostname
3. Done — existing `wid`, `hei`, `fit`, `fmt`, `qlt`, `crop`, `rotate`, `flip`
   parameters work without changes

Scene7 features not yet in Rendition: full video transcoding / HLS / DASH,
image templates with data-driven text, spin sets, eCatalogs. These are noted
as known gaps in the architecture documentation.

---

## Contributing

Contributions are welcome. Rendition follows standard open-source conventions:

1. **Open an issue** before starting significant work — alignment saves time
2. **Fork, branch, PR** — branch names like `feat/smart-crop` or `fix/s3-retry`
3. **Tests required** — new behaviour needs unit or integration test coverage;
   property-based tests (`proptest`) for pure functions
4. **Security issues** — please disclose privately via GitHub Security Advisories,
   not public issues

### Development setup

```bash
# Install libvips
brew install vips          # macOS
apt install libvips-dev    # Ubuntu / Debian

# Run the test suite
cargo test

# Run with debug logging
RUST_LOG=debug RENDITION_ASSETS_PATH=./assets cargo run

# Lint and format
cargo clippy -- -D warnings
cargo fmt --check

# Check for dependency vulnerabilities
cargo audit
```

---

## License

MIT — see [LICENSE](LICENSE).

---

## Get Involved

If Rendition solves a problem you recognise, the most useful things you can do:

- **Star the repo** — it helps others find the project
- **Open an issue** describing your use case or a gap you hit
- **Submit a PR** — the [good first issue](https://github.com/ivanoats/rendition/issues?q=label%3A%22good+first+issue%22)
  label is a good starting point
- **Spread the word** — if your team is evaluating Scene7 alternatives,
  Rendition is worth a look

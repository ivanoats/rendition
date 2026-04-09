# Rendition

> Open source, enterprise-ready media CDN — a modern alternative to Adobe Scene7.

Rendition is a high-performance image and video delivery service built in Rust.
It accepts original media assets from any storage backend (S3, GCS, Azure Blob,
local disk) and serves them on-the-fly with URL-driven transformations: resize,
crop, format conversion, quality tuning, and more.

## Why Rust?

| Concern | Why Rust wins |
|---|---|
| **Throughput** | Zero-cost abstractions and async I/O (Tokio + Axum) mean a single node handles tens of thousands of concurrent requests. |
| **Memory safety** | No GC pauses, no null-pointer crashes, no buffer overflows — critical when processing untrusted media bytes. |
| **Binary size** | Single statically-linked binary; trivial to containerise and deploy. |
| **libvips integration** | The `libvips` C library is the fastest open-source image processor; Rust's FFI makes binding it straightforward and safe. |

## URL Transform API

Rendition uses Scene7-compatible URL parameters so existing integrations migrate
with minimal changes.

```
GET /cdn/{asset_path}?param=value&…
```

| Parameter | Description | Default |
|-----------|-------------|---------|
| `wid` | Output width (px) | original |
| `hei` | Output height (px) | original |
| `fit` | `crop` · `fit` · `stretch` · `constrain` | `constrain` |
| `fmt` | `webp` · `avif` · `jpeg` · `png` | original |
| `qlt` | Quality 1–100 (lossy formats only) | `85` |
| `crop` | Pre-resize crop `x,y,w,h` | — |

**Example**

```
https://media.example.com/cdn/products/shoe.jpg?wid=800&hei=600&fit=crop&fmt=webp&qlt=85
```

## Tech Stack

- **Runtime**: [Tokio](https://tokio.rs/) async runtime
- **HTTP**: [Axum](https://github.com/tokio-rs/axum) web framework
- **Middleware**: [Tower](https://github.com/tower-rs/tower) / [tower-http](https://github.com/tower-rs/tower-http) (CORS, tracing, static files)
- **Image processing**: [libvips](https://www.libvips.org/) (planned)
- **Serialisation**: [Serde](https://serde.rs/)
- **Observability**: [tracing](https://github.com/tokio-rs/tracing) + structured JSON logs

## Getting Started

### Prerequisites

- Rust 1.78+ (`rustup update stable`)
- `libvips` development headers (for image processing — optional during early development)

### Run locally

```bash
git clone https://github.com/ivanoats/rendition
cd rendition
RUST_LOG=debug cargo run
```

The server starts on **http://localhost:3000**.

```bash
curl http://localhost:3000/health
# {"status":"ok","service":"rendition"}
```

### Running tests

```bash
cargo test
```

## Project Structure

```
src/
├── main.rs          # Axum server entry point
├── api/mod.rs       # URL-based transform API router
├── transform/mod.rs # Image transformation pipeline
└── storage/mod.rs   # Storage backend trait + adapters
```

## Roadmap

- [ ] libvips integration for image transforms
- [ ] S3 storage adapter
- [ ] Response caching (in-memory + Redis)
- [ ] Signed URL support
- [ ] Video thumbnail extraction
- [ ] WebP / AVIF auto-negotiation via `Accept` header
- [ ] Prometheus metrics endpoint
- [ ] Helm chart for Kubernetes deployment

## License

MIT

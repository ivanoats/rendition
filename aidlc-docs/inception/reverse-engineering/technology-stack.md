# Technology Stack

## Programming Languages

- **Rust** — Edition 2021 — All application code.

## Frameworks

- **Axum 0.7** — Async HTTP web framework; routing, extractors, response types.
- **Tokio 1 (full)** — Async runtime; task scheduling, `fs`, `net`, `spawn_blocking`.
- **Tower / tower-http 0.5** — Middleware layer; `TraceLayer` for request tracing.

## Image Processing

- **libvips 1.7.3** — Native C library for high-performance image decode, transform,
  and encode. Linked dynamically at runtime.

## Serialization

- **serde 1 (derive)** — Derive macros for `Deserialize` / `Serialize`.
- **serde_json 1** — JSON encoding for health-check responses.

## Error Handling

- **anyhow 1** — Ergonomic error propagation with context chaining.

## Observability

- **tracing 0.1** — Structured, async-aware logging macros.
- **tracing-subscriber 0.3 (env-filter)** — Runtime log-level configuration via
  `RUST_LOG`.

## Build Tools

- **Cargo** — Rust package manager and build system.
- **build.rs** — Custom build script for libvips linker flags.
- `.cargo/config.toml` — Per-target `rustflags` for macOS aarch64.

## Testing Tools

- **axum-test 14** — In-process HTTP test client for Axum handlers.
- **tempfile 3** — Temporary filesystem directories for test fixtures.
- **libvips (via `ops::black_with_opts`)** — Test JPEG fixture generation.

## Infrastructure / Deployment

- No cloud infrastructure defined in this repository.
- Service exposes port `3000`; assets path configurable via `RENDITION_ASSETS_PATH`.

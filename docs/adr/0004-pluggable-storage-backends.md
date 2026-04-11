# ADR-0004: Pluggable Storage via Trait Abstraction

## Status

Accepted. **Revised 2026-04-11** during Unit 2 (S3 Storage Backend) NFR
Requirements stage. Two evolutions:

1. The trait now returns `Result<T, StorageError>` (typed error enum) instead
   of `anyhow::Result<T>`. Allows callers to distinguish `NotFound` (→ HTTP
   404) from `Unavailable` (→ HTTP 503) from `CircuitOpen` (→ fail-fast 503)
   without downcasting.
2. A `get_range` method was added to the trait for HTTP 206 byte-range
   support. See **ADR-0018** for the full rationale of that addition.

The hexagonal architecture and monomorphised dispatch described below are
unchanged.

## Context

Rendition must retrieve original media assets before transforming them. The
storage topology varies across deployments:

- **Local filesystem** during development and for on-prem deployments.
- **Amazon S3** (or S3-compatible: MinIO, Cloudflare R2) for cloud deployments.
- Potentially **GCS**, **Azure Blob Storage**, or a custom origin in the future.

Hardcoding any single backend into the HTTP handler would couple application
logic to infrastructure, making the service harder to test and harder to extend.

## Decision

Define a **`StorageBackend` trait** and make the HTTP handler generic over it.

```rust
pub trait StorageBackend: Send + Sync {
    async fn get(&self, path: &str) -> Result<Asset, StorageError>;
    async fn exists(&self, path: &str) -> Result<bool, StorageError>;
    async fn get_range(
        &self,
        path: &str,
        range: std::ops::Range<u64>,
    ) -> Result<Asset, StorageError> {
        // Default: fetch full asset and slice. `S3Storage` overrides to
        // pass the native `Range` header to GetObject (see ADR-0018).
        let asset = self.get(path).await?;
        // …slice `asset.data[range]`, clamping to bounds
    }
}

pub enum StorageError {
    NotFound,
    InvalidPath { reason: String },
    CircuitOpen,
    Timeout { op: String },
    Unavailable { source: BoxError },
    Other { source: BoxError },
}
```

The Unit 2 (S3 Storage Backend) functional design artifacts
(`aidlc-docs/construction/s3-storage/functional-design/`) specify the full
variant semantics and the HTTP-status mapping for each.

- `AppState<S: StorageBackend>` holds `Arc<S>`, injected into every handler.
- The handler `serve_asset<S: StorageBackend>` is generic — it never imports
  `LocalStorage` or `S3Storage` directly.
- `build_app()` in `lib.rs` is the single wiring point; it instantiates
  `LocalStorage` and wraps it in `AppState`.
- The trait uses **Return Position Impl Trait (RPIT)** for async methods,
  available in stable Rust 1.75+. No `async-trait` macro is required.

This is a **Hexagonal Architecture (Ports & Adapters)** pattern:

- The `StorageBackend` trait is the *port* (defined by the application core).
- `LocalStorage` and `S3Storage` are *adapters* (infrastructure implementations).
- The HTTP handler depends only on the port; swapping adapters requires no
  change to handler code.

### Test implications

In integration tests, a `MockStorage` struct implements `StorageBackend` with
an in-memory `HashMap`. Tests never touch the filesystem, making them fast,
hermetic, and parallelisable.

## Consequences

**Benefits:**

- Handler logic is fully decoupled from storage infrastructure.
- Adding a new backend (S3, GCS) requires implementing one trait — no changes
  to `api/mod.rs` or `lib.rs`.
- `MockStorage` in tests eliminates filesystem dependencies, improving test
  speed and reliability.
- `Arc<S>` sharing is zero-cost for the common case where `S = LocalStorage`:
  Rust monomorphises the generic, removing dynamic dispatch overhead.

**Drawbacks:**

- Trait generics propagate through the type system: `AppState<S>`, `router<S>`,
  `serve_asset<S>` all carry the `S` type parameter. This increases type
  signature complexity.
- Dynamic dispatch (`Arc<dyn StorageBackend>`) would simplify signatures but
  adds vtable indirection on every storage call. Static dispatch was preferred
  for performance.
- RPIT in traits requires Rust 1.75+. Contributors on older toolchains cannot
  build the project.

# Dependencies

## Internal Dependencies

```text
main.rs
  └── rendition (lib)
        ├── src/api/mod.rs
        │     ├── src/storage/mod.rs  (StorageBackend trait, Asset)
        │     └── src/transform/mod.rs (TransformParams, apply)
        ├── src/storage/mod.rs
        └── src/transform/mod.rs

tests/e2e.rs
  └── rendition (lib)  [build_app + LocalStorage + transform]
```

### `src/api/mod.rs` depends on

- `crate::storage::StorageBackend` — for `exists` and `get` calls.
- `crate::transform` — for `apply` and `TransformParams`.
- **Type**: Compile-time (same crate).

### `src/lib.rs` depends on

- `api::AppState`, `api::router` — for router assembly.
- `storage::LocalStorage` — for concrete backend in production.
- **Type**: Compile-time (same crate).

## External Dependencies

### axum 0.7

- **Purpose**: HTTP router and extractor framework.
- **License**: MIT.
- **Features used**: `multipart`.

### tokio 1

- **Purpose**: Async runtime.
- **License**: MIT.
- **Features used**: `full`.

### tower 0.4

- **Purpose**: Service/middleware abstraction layer.
- **License**: MIT.

### tower-http 0.5

- **Purpose**: HTTP-specific middleware (tracing, CORS, static file serving).
- **License**: MIT.
- **Features used**: `trace`, `cors`, `fs`.

### serde 1

- **Purpose**: Serialization/deserialization framework.
- **License**: MIT / Apache-2.0.
- **Features used**: `derive`.

### serde_json 1

- **Purpose**: JSON encoding for health-check responses.
- **License**: MIT / Apache-2.0.

### tracing 0.1

- **Purpose**: Structured logging instrumentation.
- **License**: MIT.

### tracing-subscriber 0.3

- **Purpose**: Log-level filtering and formatting subscriber.
- **License**: MIT.
- **Features used**: `env-filter`.

### anyhow 1

- **Purpose**: Error propagation with contextual messages.
- **License**: MIT / Apache-2.0.

### libvips 1.7.3

- **Purpose**: High-performance image processing (decode, transform, encode).
- **License**: LGPL-2.1 (native C library); Rust bindings are MIT.
- **Note**: Requires libvips C library installed on the system; linked dynamically.

### axum-test 14 (dev)

- **Purpose**: In-process HTTP testing client.
- **License**: MIT.

### tempfile 3 (dev)

- **Purpose**: Temporary directory management in tests.
- **License**: MIT / Apache-2.0.

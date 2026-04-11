# Code Quality Assessment

## Test Coverage

- **Overall**: Good — all three core modules have unit tests; an e2e integration test
  suite covers the full HTTP stack.
- **Unit Tests**:
  - `src/api/mod.rs` — 8 tests covering 404, 200 pass-through, format conversion
    (WebP, AVIF, PNG), resize, invalid crop, rotate, flip.
  - `src/storage/mod.rs` — 9 tests covering `content_type_from_ext` for all MIME types,
    `LocalStorage::exists`, `get` (happy path, error, size).
  - `src/transform/mod.rs` — 10 tests covering passthrough, resize (width-only, WebP
    with quality, crop fit, constrain, stretch), pre-crop + rotate, flip, AVIF encode.
- **Integration Tests**: `tests/e2e.rs` — 11 tests covering health endpoint, asset
  serving, all four format conversions, resize, quality, rotate, flip, and invalid crop
  error.
- **Coverage gaps**: S3Storage is a stub with no tests; port/host env-var configuration
  is untested; no tests for the `fill` fit mode alias.

## Code Quality Indicators

- **Linting**: Rust compiler warnings enforced by default; no explicit `#![deny(warnings)]`
  or Clippy configuration found, but the codebase is clean.
- **Code Style**: Consistent idiomatic Rust throughout — uses `?` operator, `anyhow`
  context chaining, `OnceLock` for singletons, `impl Trait` return types.
- **Documentation**: Good — all public functions and structs have doc comments; the
  `api/mod.rs` module header documents URL format and all query parameters in a table.
- **Error handling**: Consistent use of `anyhow::Result` and `.context()` for rich error
  messages throughout the transform and storage layers.

## Technical Debt

- `S3Storage` is a stub — both `get` and `exists` `todo!()` panic; marked `#[allow(dead_code)]`.
- Port `3000` and bind address `0.0.0.0` are hardcoded in `main.rs` with no env-var
  override.
- `build.rs` and `.cargo/config.toml` hardcode `/opt/homebrew/lib` for libvips on macOS
  aarch64 — won't work out of the box on Linux CI without `VIPS_LIB_DIR` set.
- `webp_save_buffer` works around a libvips version-skew issue with a suffix-encoded
  option string — this is a fragile approach that should be replaced when libvips 8.15+
  can be assumed.

## Patterns and Anti-patterns

- **Good Patterns**:
  - Trait-based storage abstraction enables testability without mocking frameworks.
  - Generic `AppState<S>` keeps the API layer decoupled from storage implementation.
  - `spawn_blocking` correctly offloads CPU-bound libvips work from the Tokio reactor.
  - Shared `build_app` between binary and integration tests avoids duplication.
  - Pipeline pattern in `apply_blocking` keeps each transform step independently testable.
- **Anti-patterns**:
  - `S3Storage::get/exists` use `todo!()` rather than returning a typed `Err` — will
    panic at runtime if accidentally wired in.
  - No structured configuration (no config file or full env-var coverage) — operational
    knobs are mostly hardcoded.

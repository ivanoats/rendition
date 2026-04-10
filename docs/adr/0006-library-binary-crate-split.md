# ADR-0006: Split into Library + Binary Crates

## Status

Accepted

## Context

Rust's integration test infrastructure (files under `tests/`) can only import
from a **library crate** (`src/lib.rs`). A pure binary crate (`src/main.rs`
only) cannot be imported by `tests/*.rs` files, which means end-to-end tests
that need to construct the application router have no way to access it.

Early in development, Rendition was a binary-only crate. All modules (`api`,
`storage`, `transform`) were declared with `mod` inside `main.rs`. This
structure prevented writing end-to-end tests that exercise the full routing
stack without spawning a real OS process.

## Decision

Split the crate into a **library target** (`src/lib.rs`) and a thin **binary
target** (`src/main.rs`).

- `src/lib.rs` declares `pub mod api; pub mod storage; pub mod transform;` and
  exposes `pub fn build_app(assets_path: &str) -> Router`.
- `src/main.rs` is reduced to: initialise logging, read `RENDITION_ASSETS_PATH`,
  call `rendition::build_app()`, bind the TCP listener.
- `tests/e2e.rs` imports `rendition::build_app()` and drives the full stack
  via `axum_test::TestServer` without a real network socket.

```
src/lib.rs        ← library crate (pub modules, build_app)
src/main.rs       ← binary crate (thin, delegates to lib)
tests/e2e.rs      ← integration tests (import from lib)
```

### Test structure enabled by this split

| Test type | Location | Uses |
|---|---|---|
| Unit | `#[cfg(test)] mod tests` in each `src/*.rs` | Module-private helpers, direct function calls |
| Integration | `src/api/mod.rs` `#[cfg(test)]` | `axum_test::TestServer`, `MockStorage` |
| E2E | `tests/e2e.rs` | `rendition::build_app()`, real `LocalStorage`, real libvips |

## Consequences

**Benefits:**
- End-to-end tests exercise the complete request pipeline (routing, storage,
  transform) without spawning an OS process or binding a real port.
- `build_app()` is a stable, documented entry point for embedders or future
  library consumers.
- The binary stays minimal (< 35 lines); all testable logic lives in the
  library.
- `cargo test` runs all three test layers in a single invocation.

**Drawbacks:**
- `src/main.rs` must reference the library via `rendition::build_app()`, which
  requires Rust to compile both targets. Build times increase marginally.
- Items that are `pub(crate)` in the library (e.g. `content_type_from_ext`,
  `ensure_vips`) are not accessible from `tests/` integration tests, since
  those tests are compiled as a separate crate. Only `pub` items cross the
  boundary.
- The split is a one-way door: reverting to a binary-only crate would break
  the e2e test infrastructure.

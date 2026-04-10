# ADR-0002: Axum as the HTTP Framework

## Status

Accepted

## Context

Rendition needs an HTTP framework that:

- Integrates natively with the **Tokio** async runtime.
- Supports **type-safe extractors** for path segments, query parameters, and
  shared state — all of which the CDN handler requires.
- Composes well with the **Tower** middleware ecosystem (tracing, CORS, static
  files) for observability and cross-cutting concerns.
- Has a **stable, actively maintained** API.

Alternatives considered: Actix-web, Warp, Rocket, Hyper (bare).

| Framework | Notes |
|---|---|
| Actix-web | Mature, very fast. Uses its own actor-based runtime atop Tokio; tighter coupling, different middleware model. |
| Warp | Tower-compatible, but combinator-based routing is verbose for simple CRUD. Smaller community. |
| Rocket | Ergonomic macros, but requires nightly Rust and has a different async model. |
| Hyper (bare) | Maximum control, but requires hand-rolling routing, extractors, and error responses. |

## Decision

Use **Axum 0.7** as the HTTP framework.

- Built by the Tokio team; first-class Tokio integration.
- **Extractor pattern** (`Path`, `Query`, `State`) cleanly separates parameter
  parsing from handler logic with zero boilerplate.
- **Tower-native**: middleware (e.g. `TraceLayer`, CORS) is applied via
  `.layer()`, keeping concerns separate from route logic.
- `Router` is composable: `api::router()` returns a sub-router that is
  `merge()`d into the top-level router in `lib.rs`.
- Generic handlers (`serve_asset<S: StorageBackend>`) monomorphise cleanly
  with Axum's `Handler` trait, enabling the pluggable storage pattern.

## Consequences

**Benefits:**
- Extractor-based handlers are unit-testable without spinning up a real server
  (via `axum-test::TestServer`).
- Tower middleware ecosystem is reusable across projects.
- Strong type safety: missing or malformed query parameters return 400 before
  the handler runs.
- Active upstream development from the Tokio project team.

**Drawbacks:**
- Axum's generic constraint on handlers (`S: Clone + Send + Sync + 'static`)
  can produce dense compiler error messages when constraints are not satisfied.
- Version 0.7 introduced breaking changes from 0.6; future major versions may
  require migration effort.

# Unit 1 — Config — Tech Stack Decisions

All tech stack choices for Unit 1 were made during Inception. This document
records them for traceability.

## Selected Crates

| Crate | Version | Purpose | Decision source |
|---|---|---|---|
| `envy` | `0.4` | Env var → struct deserialisation | ADR-0014 |
| `serde` | `1` (existing) | Derive `Deserialize` for `AppConfig` | Project default |
| `thiserror` | `1` | Typed error enums for `ConfigError` | New — see below |
| `url` | `2` | Parse and validate `RENDITION_REDIS_URL`, `RENDITION_OIDC_ISSUER` | New — see below |
| `proptest` | `1` (dev) | Property-based tests for validation | NFR-02 |

## New Dependencies Added in Unit 1

### `thiserror`

`thiserror` provides ergonomic typed error enums via derive macro. Used for
`ConfigError` so that `AppConfig::load()` and `validate()` return rich, typed
errors that can be matched on by `main()` for tailored exit codes if needed.

Alternative considered: `anyhow` (already in `Cargo.toml`). `anyhow::Error` is
fine for application-level error propagation but provides poor ergonomics for
matching on specific error variants. Both crates coexist commonly: `thiserror`
for library errors, `anyhow` for application errors.

### `url`

`url` parses and validates URL strings. Used for early validation of
`RENDITION_REDIS_URL` and `RENDITION_OIDC_ISSUER` so misconfigured URLs are
caught at startup, not at first connection attempt. Lightweight (no transitive
runtime deps).

## Existing Dependencies Used

- `serde` — already present for `TransformParams`
- `tracing` — for startup config-loaded log line

## No New Runtime Frameworks

Unit 1 introduces no new async runtimes, HTTP clients, or middleware. It is a
pure synchronous startup-time component.

## Tooling

- `cargo llvm-cov` — coverage measurement (project-wide, used in CI)
- `cargo clippy` — lints, deny warnings on Unit 1 code
- `cargo fmt` — formatting

## Versions

Pinned in `Cargo.toml` with caret ranges (`^0.4`, `^1`, etc.). `Cargo.lock` is
committed (NFR-07) to ensure reproducible builds.

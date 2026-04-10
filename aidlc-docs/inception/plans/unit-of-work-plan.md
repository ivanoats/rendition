# Unit of Work Plan

## Plan Status

- [x] Step 1: Resolve open decomposition questions (user input required)
- [x] Step 2: Generate `unit-of-work.md`
- [x] Step 3: Generate `unit-of-work-dependency.md`
- [x] Step 4: Generate `unit-of-work-story-map.md`
- [x] Step 5: Validate and present for approval

---

## Context

The execution plan already defines 7 units derived from the Application Design.
This plan formalises them as unit-of-work artifacts and resolves three open
decomposition questions. Fill each `[Answer]:` tag and reply "done".

---

## Proposed Unit Decomposition

| # | Unit | Key deliverables | Depends on |
|---|---|---|---|
| 1 | **Config** | `src/config.rs`, `AppConfig`, `envy` loading, validation, tests | — |
| 2 | **S3 Storage Backend** | `src/storage/s3.rs`, `CircuitBreaker`, retry, `S3Storage` impl | 1 |
| 3 | **Transform Cache** | `src/cache.rs`, `MokaTransformCache`, `compute_cache_key`, metrics | 1 |
| 4 | **Transform Pipeline Enhancements** | `fmt=auto`, smart crop, sharpening, watermark, HTTP 206, presets | 1, 3 |
| 5 | **Embargo + Presets** | `src/embargo/`, `src/preset/`, admin API, Redis store, `AuthLayer` | 1, 3 |
| 6 | **Middleware** | Rate limiting, security headers, request ID, input validation, error hardening | 1 |
| 7 | **Observability & Ops** | Prometheus `/metrics`, OTEL, health probes, Dockerfile, K8s manifests | all |

Critical path: **1 → {2, 3} → {4, 5} → 6 → 7**

Units 2 and 3 have no inter-dependency. Units 4 and 5 have no inter-dependency.

---

## Open Decomposition Questions

---

### Q1 — Development Branch Strategy for Independent Units

Units 2/3 and Units 4/5 are independent of each other but depend on the same
prior unit. In practice (single contributor, open source), two strategies:

**Option A — Sequential** (recommended)
Execute all 7 units one at a time on `main`. Simpler, no merge overhead,
always releasable. Each unit adds tests before the next begins.

**Option B — Parallel feature branches**
Units 2 and 3 developed on separate branches simultaneously, merged before
proceeding to 4/5. Useful if multiple contributors work in parallel.

#### Recommendation: Sequential (Option A)

For a single-contributor open source project, sequential development on `main`
minimises merge friction and keeps the repo in a continuously releasable state
after each unit. If contributors join and want to take a parallel unit, a
feature branch can be cut at that point without changing this plan.

[Answer]: Accepted — Sequential on main (Option A).
---

### Q2 — Integration Test Infrastructure

The embargo and preset stores (Unit 5) and S3 storage (Unit 2) need real
dependencies in tests. Two approaches:

**Option A — `testcontainers-rs`** (recommended)
Start real Docker containers (Redis, LocalStack for S3) programmatically inside
`cargo test`. Tests are self-contained and run in CI without a pre-existing
`docker-compose up`. Slightly slower startup (~2 s per test suite run).

**Option B — Docker Compose service + env var gate**
`docker-compose.yml` provides Redis and LocalStack. Integration tests are
gated behind a `#[cfg(feature = "integration")]` feature flag or
`RUN_INTEGRATION_TESTS=1` env var. Faster if already running; skipped otherwise.

#### Recommendation: `testcontainers-rs` (Option A)

`testcontainers-rs` makes integration tests unconditionally runnable with
`cargo test` in any environment with Docker — including GitHub Actions. This
is the right choice for an open source project where contributors should not
need to manually `docker-compose up` before running tests.

[Answer]: Accepted — testcontainers-rs (Option A).
---

### Q3 — Incremental Deployment: Feature Flags vs Always-On

As units are completed and merged, some features (embargo enforcement, OIDC
auth, rate limiting) are always-on once the code is present. Two options:

**Option A — Always-on, config-gated** (recommended)
Features activate when their configuration variables are set. No Redis URL →
embargo enforcement skips the store check and logs a warning (fail-open, for
dev only). No OIDC config → admin API only accepts API keys. Rate limiting
is always on but can be set to a very high limit effectively disabling it.
This is the 12-factor app approach.

**Option B — Compile-time feature flags**
`cargo build --features embargo,oidc` gates features at compile time. Smaller
binary when features are excluded; harder to reason about in production.

#### Recommendation: Config-gated, always-on code (Option A)

Compile-time feature flags fragment the test matrix and add maintenance burden.
Config-gated behaviour is easier to reason about, document, and test. The
`AppConfig::validate()` method enforces correct configuration for enabled
features — if `RENDITION_STORAGE_BACKEND=s3` is set without `RENDITION_S3_BUCKET`,
the process exits at startup rather than failing at runtime.

[Answer]: Accepted — Config-gated, always-on code (Option A).
---

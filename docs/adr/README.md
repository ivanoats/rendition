# Architecture Decision Records

This directory tracks significant architectural decisions made in the Rendition
project. Each ADR captures the context, the decision, and its consequences so
future contributors understand the reasoning behind the current design.

## Index

| # | Title | Status |
|---|-------|--------|
| [0001](0001-rust-as-runtime.md) | Rust as the primary runtime | Accepted |
| [0002](0002-axum-http-framework.md) | Axum as the HTTP framework | Accepted |
| [0003](0003-libvips-image-processing.md) | libvips for image processing | Accepted |
| [0004](0004-pluggable-storage-backends.md) | Pluggable storage via trait abstraction | Accepted |
| [0005](0005-scene7-url-compatibility.md) | Scene7-compatible URL parameter naming | Accepted |
| [0006](0006-library-binary-crate-split.md) | Split into library + binary crates | Accepted |
| [0007](0007-oidc-sso-admin-authentication.md) | OIDC / SSO for admin authentication | Accepted |
| [0008](0008-http-451-for-embargoed-assets.md) | HTTP 451 for embargoed asset responses | Accepted |
| [0009](0009-lru-transform-cache.md) | In-process LRU cache for transformed images | Accepted |
| [0010](0010-embargo-store-backend.md) | Embargo store backend selection | Accepted |
| [0013](0013-admin-api-dual-port-listener.md) | Admin API on a separate internal listener | Accepted |
| [0014](0014-envy-configuration-parsing.md) | `envy` crate for environment variable configuration | Accepted |
| [0015](0015-tower-governor-rate-limiting.md) | `tower-governor` for per-IP rate limiting | Accepted |
| [0016](0016-jsonwebtoken-oidc-validation.md) | `jsonwebtoken` for OIDC token validation | Accepted |
| [0017](0017-prometheus-metrics-crate.md) | `prometheus` crate for metrics | Accepted |
| [0018](0018-http-206-custom-range-parsing.md) | Custom `Range` header parsing for HTTP 206 video delivery | Accepted |
| [0011](0011-automatic-format-negotiation.md) | Automatic format negotiation via Accept header | Accepted |
| [0012](0012-surrogate-key-cdn-cache-invalidation.md) | Surrogate-Key headers for CDN cache invalidation | Accepted |

## Format

Each ADR follows this structure:

```markdown
# ADR-NNNN: Title

## Status
Proposed | Accepted | Deprecated | Superseded by ADR-XXXX

## Context
Why did this decision need to be made?

## Decision
What was decided and why?

## Consequences
What are the trade-offs, benefits, and known drawbacks?
```

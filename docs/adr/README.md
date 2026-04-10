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

## Format

Each ADR follows this structure:

```
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

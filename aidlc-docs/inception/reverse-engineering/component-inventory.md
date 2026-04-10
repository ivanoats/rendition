# Component Inventory

## Application Packages

- `rendition` (binary + library) — CDN HTTP service; API, storage, and transform modules.

## Infrastructure Packages

- None — no CDK, Terraform, or CloudFormation resources in this repository.

## Shared Packages

- None — single-crate project; all shared logic lives within the `rendition` library.

## Test Packages

- `tests/e2e.rs` — End-to-end integration test suite (HTTP-level, real libvips + real
  `LocalStorage` against a temp directory).
- `src/api/mod.rs#[cfg(test)]` — Unit tests for the API layer using `MockStorage`.
- `src/storage/mod.rs#[cfg(test)]` — Unit tests for `LocalStorage` and
  `content_type_from_ext`.
- `src/transform/mod.rs#[cfg(test)]` — Unit tests for all transform operations.

## Total Count

- **Total Packages**: 1 (Cargo crate)
- **Application**: 1
- **Infrastructure**: 0
- **Shared**: 0
- **Test**: 4 test modules/files (within the single crate)

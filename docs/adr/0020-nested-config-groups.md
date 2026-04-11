# ADR-0020: Nested Configuration Groups in `AppConfig`

## Status

Accepted (2026-04-11, Unit 2 — S3 Storage Backend, NFR Design stage)

## Context

Unit 1 landed a flat `AppConfig` struct (`src/config.rs`) with a single level
of fields:

```rust
pub struct AppConfig {
    pub bind_addr: SocketAddr,
    pub storage_backend: StorageBackendKind,
    pub assets_path: PathBuf,
    pub s3_bucket: Option<String>,
    pub s3_region: Option<String>,
    pub s3_endpoint: Option<String>,
    pub s3_prefix: String,
    // … 15+ other flat fields
}
```

This shape was fine for Unit 1 (only the four `s3_*` fields existed). Unit 2
(S3 Storage Backend) introduces seven more S3-related fields
(`s3_max_connections`, `s3_timeout_ms`, `s3_cb_threshold`, `s3_cb_cooldown_secs`,
`s3_max_retries`, `s3_retry_base_ms`, `s3_allow_insecure_endpoint`). Following
the same pattern through Units 3–7 (transform cache, embargo, auth, rate
limiting, observability) would push `AppConfig` to ~40–50 flat fields.

A flat struct at that size hurts readability, makes it harder to see which
fields belong together, and forces unrelated units to depend on the full
`AppConfig` just to read their own settings. It also makes constructors
awkward: `S3Storage::new(&cfg)` needs the whole struct because `cfg.s3_*`
fields are scattered.

## Decision

Group related configuration fields into **named sub-structs**, one per
logical subsystem.

Unit 2 introduces the first group:

```rust
pub struct AppConfig {
    pub bind_addr: SocketAddr,
    pub storage_backend: StorageBackendKind,
    pub assets_path: PathBuf,
    pub local_timeout_ms: u64,
    pub s3: S3Settings,      // nested group
    // … other top-level and future nested groups
}

pub struct S3Settings {
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub prefix: String,
    pub max_connections: u32,
    pub timeout_ms: u64,
    pub cb_threshold: u32,
    pub cb_cooldown_secs: u64,
    pub max_retries: u32,
    pub retry_base_ms: u64,
    pub allow_insecure_endpoint: bool,
}
```

Each component that consumes a group takes the group struct directly:

```rust
impl S3Storage {
    pub async fn new(settings: &S3Settings) -> Result<Self, StorageError>;
}
```

### Environment variable contract is unchanged

Nesting is a Rust-side refactor only. The `RENDITION_*` environment variables
remain flat — `envy::prefixed("RENDITION_")` continues to map
`RENDITION_S3_BUCKET` to the field named `s3_bucket` at some level of the
struct tree. The `#[serde(flatten)]` attribute on `AppConfig.s3` (or an
explicit per-field `#[serde(rename)]`) preserves this mapping. Operators and
`README.md` configuration tables see no change.

### Future units add new groups, not new top-level fields

Future units introduce their own groups:

- **Unit 3** — `CacheSettings` (`RENDITION_CACHE_*`)
- **Unit 5** — `EmbargoSettings` (`RENDITION_EMBARGO_*`), `OidcSettings` (`RENDITION_OIDC_*`)
- **Unit 6** — `RateLimitSettings` (`RENDITION_RL_*`)
- **Unit 7** — `MetricsSettings` (`RENDITION_METRICS_*`), `TracingSettings` (`RENDITION_TRACING_*`)

`AppConfig` stays at ~10 top-level fields rather than growing to ~50.

### Validation

Cross-field validation (e.g. "if `storage_backend == S3` then
`s3.bucket` and `s3.region` must be present") lives in
`AppConfig::validate()` as before. Each sub-struct may also have its own
per-field invariants — `S3Settings::validate()` enforces
`max_connections >= 1`, `timeout_ms >= 100`, etc. — invoked from
`AppConfig::validate()` so errors compose into one fail-fast startup check.

## Consequences

**Benefits:**

- **Readability scales.** A reader opening `config.rs` sees ten top-level
  groups instead of fifty flat fields.
- **Locality.** Each unit owns its `*Settings` struct; unrelated units don't
  touch it. Code review diffs are smaller and more contained.
- **Ergonomic constructors.** `S3Storage::new(&cfg.s3)` is more honest about
  its dependencies than `S3Storage::new(&cfg)` — the compiler prevents
  `S3Storage` from accidentally reaching into `cfg.cache.max_entries`.
- **Testable in isolation.** Unit tests can build a minimal
  `S3Settings { ..Default::default() }` rather than a full `AppConfig`.
- **No runtime cost.** `#[serde(flatten)]` is a compile-time attribute;
  `envy` still does one pass over the environment.

**Drawbacks:**

- **Unit 1 migration touches `config.rs` and two call sites** (`src/lib.rs`,
  `tests/config_test.rs`). Small, mechanical, reviewable.
- **`#[serde(flatten)]` complexity.** If future fields conflict with nested
  struct field names, serde's flatten semantics can surprise. Mitigated by
  keeping group names distinct (`s3`, `cache`, `embargo`) and not reusing
  field names across groups.
- **One more layer of indentation.** `cfg.s3.bucket` is three tokens instead
  of `cfg.s3_bucket`'s two. This is a cosmetic cost.

**Migration path if we regret it:** reverse the nesting by moving fields
back onto `AppConfig` with explicit `#[serde(rename)]`. Purely mechanical
and test-covered.

## Related

- **ADR-0014** — `envy` Crate for Environment Variable Configuration (base
  decision to use envy; this ADR refines the struct shape within it).
- **Unit 2 NFR Design plan** —
  `aidlc-docs/construction/plans/s3-storage-nfr-design-plan.md` Q2.
- **Unit 2 Logical Components** —
  `aidlc-docs/construction/s3-storage/nfr-design/logical-components.md`
  ("Configuration refactor" section).

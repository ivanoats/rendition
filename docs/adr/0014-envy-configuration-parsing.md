# ADR-0014: `envy` Crate for Environment Variable Configuration

## Status

Accepted

## Context

FR-02 requires all `RENDITION_*` environment variables to be parsed into a typed
`AppConfig` struct with fail-fast validation at startup. Three approaches were
evaluated:

| Criterion | `envy` | `config` crate | `std::env::var` manual |
|---|---|---|---|
| Lines of code | ~5 (derive + one call) | ~20 (builder chain) | ~60–100 (one var per field) |
| Type coercion | Automatic via serde | Automatic | Manual per field |
| Layered config (env + file) | No | Yes | No |
| Extra dependencies | 1 | 1 | 0 |
| v1 need for file-based config | No | Over-engineered | N/A |

Rendition's configuration source is exclusively environment variables: values are
injected via Kubernetes `ConfigMap` and `Secret` objects. No TOML or YAML files
are used.

## Decision

Use the **`envy` crate** to deserialise environment variables into `AppConfig`.

```rust
#[derive(Debug, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_bind_addr")]
    pub bind_addr: SocketAddr,
    pub storage_backend: StorageBackend,
    // … all RENDITION_* vars
}

pub fn load() -> Result<AppConfig> {
    let cfg = envy::prefixed("RENDITION_").from_env::<AppConfig>()?;
    cfg.validate()?;
    Ok(cfg)
}
```

`envy::prefixed("RENDITION_")` strips the prefix before serde field matching,
so `RENDITION_BIND_ADDR` deserialises into `bind_addr`. Cross-field validation
(e.g. confirming `s3_bucket` is present when `storage_backend == s3`) is
implemented in a `validate(&self) -> Result<()>` method called after
`envy::from_env()` succeeds.

## Consequences

**Benefits:**

- Near-zero boilerplate: adding a new config field is one line in the struct.
- Serde handles type coercion (`SocketAddr`, `u64`, `PathBuf`, `Option<T>`,
  `Vec<T>` from comma-separated values) without custom parsing code.
- Fail-fast: `envy` returns a typed error at startup if a required field is
  missing or cannot be parsed; the process exits with a clear message.
- `serde(default)` attributes provide documented defaults in code rather than
  in scattered `unwrap_or` calls.

**Drawbacks:**

- `envy` does not support layered configuration (env vars + file overrides). This
  is acceptable for v1 where all configuration is environment-variable-based.
  If a future version requires TOML configuration files, migrating to the `config`
  crate is the natural evolution.
- One additional crate dependency (`envy` + its transitive dependency on `serde`).
  `serde` is already in `Cargo.toml`, so the net addition is `envy` alone.

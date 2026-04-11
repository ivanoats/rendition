# Unit 1 — Config — NFR Requirements

## Scope

Unit 1 introduces typed environment variable configuration. This is a startup-only
component — none of its code runs on the request hot path. NFRs are correspondingly
modest.

## Performance

| Concern | Target |
|---|---|
| `AppConfig::load()` latency | < 50 ms (process startup, called once) |
| Memory footprint | < 1 KiB for the loaded `AppConfig` instance |
| Validation latency | < 1 ms (synchronous, in-process) |

Performance is not a binding constraint for Unit 1. The code runs once before
the Tokio runtime accepts its first request.

## Reliability

| Concern | Approach |
|---|---|
| Missing required field | `envy` returns typed error → process exits with non-zero status |
| Invalid type coercion | `envy` returns typed error → process exits |
| Cross-field violation | `validate()` returns `ConfigError` → process exits |
| Logged error format | Single human-readable line on stderr; no panic, no stack trace |

**Fail-fast principle:** any configuration error must prevent the process from
binding its listeners. There is no graceful degradation for misconfiguration.

## Security (Security Baseline applies)

| Rule | Implementation |
|---|---|
| SECURITY-03 | API key hashes only; raw keys never present in `AppConfig` |
| SECURITY-09 | `Display`/`Debug` for `AppConfig` must redact sensitive fields (S3 secret key, JWT signing keys) |
| SECURITY-15 | `envy::from_env()` failure mode is fail-closed |

The `Debug` derive must be customised to redact:

- `s3.secret_access_key` (when present)
- `admin_api_keys` (already SHA-256 hashes, but still redacted as a defence in depth)

## Test Coverage (NFR-01)

| Layer | Target |
|---|---|
| Unit tests for `AppConfig::load` and `validate` | ≥ 95% line coverage |
| Property-based tests (`proptest`) | All branches of `validate()` |
| Integration tests | Out of scope — Unit 1 has no I/O beyond env var reads |

Coverage measured by `cargo llvm-cov`. CI gate at 80% project-wide; Unit 1
should exceed this comfortably.

## Property-Based Testing (NFR-02)

`proptest` is used for two invariants in Unit 1:

1. **Round-trip invariant** — for any valid `AppConfig`, serialising to env var
   form and re-loading via `envy` produces an equivalent struct.
2. **Validation determinism** — `validate()` is a pure function: identical
   inputs always produce identical outputs.

PBT seed must be logged on failure (PBT-08 from the project PBT rules).

## Maintainability

- All `RENDITION_*` env vars must appear in the `AppConfig` struct with
  `#[serde(default = "...")]` attributes for fields with defaults
- Each field must have a doc comment explaining its purpose and default
- The `README.md` configuration table must match the struct field-for-field
  (drift between code and docs is a code review failure)

## Out of Scope for Unit 1

- Remote configuration sources (Consul, etcd) — not required by FR-02
- Hot-reload of configuration — `AppConfig` is immutable after startup
- File-based configuration — `envy` is env-var-only by ADR-0014
- Configuration encryption at rest — handled by Kubernetes `Secret` objects

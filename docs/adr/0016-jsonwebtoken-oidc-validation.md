# ADR-0016: `jsonwebtoken` for OIDC Token Validation

## Status

Accepted

## Context

FR-13 requires all `/admin/*` endpoints to validate OIDC JWTs issued by a
configured Identity Provider (Okta, Azure AD, Google Workspace, or any
standards-compliant OIDC provider). Validation must verify: signature (via the
IdP's JWKS endpoint), expiry, issuer, audience, and group/role claim.

Rendition does not implement OAuth2 redirect flows — those are handled by the
IdP and any BFF in front of Rendition. Rendition's requirement is token
validation only.

Two options were evaluated:

| Criterion | `jsonwebtoken` + manual JWKS cache | `openidconnect` crate |
|---|---|---|
| Scope | JWT decode + validate | Full OIDC client |
| JWKS fetch/cache | Manual (`reqwest` + `RwLock`) | Handled internally |
| Dependency weight | Light | Heavy |
| v1 need (token validation only) | Exact fit | Over-engineered |
| Future OIDC flows | Would require migration | Already supported |

## Decision

Use **`jsonwebtoken`** for JWT decoding and validation, with a hand-written
`JwksCache` for JWKS key fetching and rotation.

```rust
pub struct JwksCache {
    keys: RwLock<(Vec<DecodingKey>, Instant)>,
    issuer: String,
    audience: String,
    jwks_url: String,
    ttl: Duration,  // default: 1 hour
}

impl JwksCache {
    pub async fn validate(&self, token: &str) -> Result<AdminClaims> { … }
}
```

`AdminClaims` is a typed struct containing `sub`, `iss`, `aud`, `exp`,
and a `groups: Vec<String>` claim. After signature and standard claim
validation, the middleware checks that `groups` contains
`RENDITION_OIDC_ADMIN_GROUP`. If not present, the request is rejected with
`403 Forbidden`.

API key authentication (Mode B in FR-13) is handled in the same `AuthLayer`
middleware: if the `Authorization` header contains a key matching one of the
SHA-256-hashed values in `RENDITION_ADMIN_API_KEYS`, the request is admitted
without OIDC validation.

## Consequences

**Benefits:**

- `jsonwebtoken` is a focused, widely-audited crate that handles RS256, RS384,
  RS512, ES256, and ES384 — the signature algorithms used by all major OIDC
  providers.
- The `JwksCache` is ~60 lines of straightforward Rust. Its behaviour is
  fully visible and testable with mock JWKS endpoints.
- Lean dependency tree: `jsonwebtoken` + `reqwest` (already in `Cargo.toml`
  for S3 health checks) + `serde_json`.

**Drawbacks:**

- Manual `JwksCache` implementation must handle key rotation correctly:
  on a validation failure due to `InvalidSignature`, the cache is force-refreshed
  once and the token re-validated before returning `401`. This covers key
  rotation without causing a thundering herd on every bad token.
- If Rendition ever needs to support OIDC dynamic client registration or
  device flow, migrating to `openidconnect` will be necessary. The `AuthLayer`
  interface (`validate(token) → Result<AdminClaims>`) is designed to make
  this swap mechanical — only the implementation behind the trait changes.

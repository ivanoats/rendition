# ADR-0007: OIDC / SSO for Admin Authentication

## Status

Accepted

## Context

The embargo management API and named-preset API (FR-12, FR-18) introduce the first
authenticated surface in Rendition. Two classes of caller need access:

1. **Human admins** — merchandisers and content operations teams at retailers like
   lululemon who manage embargoes and presets through internal tooling or a future UI.
2. **Machine clients** — CI/CD pipelines, automation scripts, or integration services
   that call the admin API programmatically.

The choices for human admin authentication were:

- **API keys only** — simple but creates a credential management burden for human
  users. Keys must be rotated manually, cannot be scoped to individuals, and do not
  support single sign-on with the organisation's existing identity provider.
- **Username/password with a local user store** — Rendition would become an identity
  provider, requiring password hashing, MFA, session management, and brute-force
  protection. This is a significant scope expansion and security surface.
- **OIDC (OpenID Connect)** — delegates authentication entirely to the organisation's
  existing IdP (Okta, Azure AD / Entra ID, Google Workspace, Ping Identity). Rendition
  only validates the resulting JWT access token. SSO, MFA, and group management are
  handled by the IdP — which already has them.

## Decision

Implement **OIDC token validation** for human admin access alongside **API key
authentication** for machine clients.

- Rendition validates OIDC access tokens on every `/admin/*` request. It does **not**
  implement OAuth2 redirect flows — that is the responsibility of the IdP and any
  admin portal / BFF calling the API.
- Token validation checks: JWT signature (via IdP JWKS), expiry, issuer
  (`RENDITION_OIDC_ISSUER`), audience (`RENDITION_OIDC_AUDIENCE`), and group membership
  (`RENDITION_OIDC_ADMIN_GROUP` claim).
- JWKS keys are fetched from the IdP at startup and refreshed periodically (with
  in-memory cache and a forced refresh on key-ID miss).
- API keys (`RENDITION_ADMIN_API_KEYS`, SHA-256 hashed) continue to work for
  service-to-service callers that cannot participate in browser-based SSO flows.
- Both modes are accepted on the same endpoint via `Authorization: Bearer <token>`;
  the middleware attempts OIDC validation first, falls back to API key hash comparison.
- The auth middleware is a **replaceable Tower layer** — swapping the auth strategy
  requires no changes to handler code.

## Consequences

**Benefits:**

- Human admins use their existing corporate SSO credentials. No separate password to
  manage or rotate.
- MFA, session expiry, and account deprovisioning are handled by the IdP. When an
  employee leaves, revoking their IdP account immediately blocks access to Rendition.
- Group-based access control (`rendition-admins` group in the IdP) provides a
  centralised, auditable membership list without per-service user management.
- API keys remain available for automation without requiring browser flows.
- OIDC is a widely understood, audited standard. Library support (`jsonwebtoken`,
  `openidconnect` crate) is mature.

**Drawbacks:**

- Rendition gains a runtime dependency on an external IdP for admin authentication.
  If the JWKS endpoint is unreachable at startup, admin endpoints will be unavailable
  (fail-closed by design — see SECURITY-15).
- JWKS refresh introduces a small window where a recently revoked token may still
  validate (mitigated by short token lifetimes configured at the IdP, typically 1 h).
- Operators must configure OIDC parameters correctly; misconfiguration silently accepts
  or rejects all tokens. Startup validation of OIDC config parameters is required.
- Machine clients using API keys still require a manual rotation process and a secrets
  manager; this is considered acceptable for the service-account use case.

# ADR-0008: HTTP 451 for Embargoed Asset Responses

## Status

Accepted

## Context

When a client requests an asset that is under an active embargo, Rendition must return
an HTTP error response. The three candidate status codes were:

- **404 Not Found** — the asset appears not to exist. Deceptive: the asset does exist
  but is being withheld. This harms admin debugging ("is the asset missing or just
  embargoed?") and may trigger false-positive broken-link alerts in monitoring.
- **403 Forbidden** — access is denied. Reveals to the caller that the asset exists
  and that *they specifically* lack permission. For public CDN traffic this leaks
  commercial information (e.g. a product that exists but is not yet announced).
- **451 Unavailable For Legal Reasons** — defined in RFC 7725 (2016). Signals that
  the resource is being withheld for legal, regulatory, or commercial reasons, not
  that it is missing or that the caller lacks credentials. Supported by all modern
  HTTP clients, browsers, and CDN layers.

Enterprise retailers embargo assets for a variety of reasons: product launch dates,
regulatory holds, legal disputes, brand partnership embargoes, and regional
availability restrictions. `451` accurately models all of these.

## Decision

Return **HTTP 451 Unavailable For Legal Reasons** for all requests to embargoed assets.

- The response body MUST be generic (`"asset unavailable"`) — the `embargo_until`
  date and any commercial rationale MUST NOT be included.
- The response SHOULD include a `Link` header pointing to a public explanation page
  if the operator configures `RENDITION_EMBARGO_INFO_URL`:
  `Link: <https://example.com/media-policy>; rel="blocked-by"`
- The embargo check and `451` response MUST be applied before any storage fetch or
  transform — no asset bytes are read for embargoed paths.
- `451` responses MUST NOT be stored in the transform cache.
- Admin tooling (authenticated via OIDC/API key) MAY call a separate
  `/admin/embargoes/{path}` endpoint to inspect the full embargo record including
  `embargo_until` and audit fields.

## Consequences

**Benefits:**

- Semantically accurate: `451` correctly models commercial/legal withholding, not
  authentication failure or file absence.
- Does not leak asset existence to unauthenticated callers (unlike `403`).
- Does not trigger broken-link false positives in monitoring (unlike `404`).
- RFC 7725 compliance; search engines (Google) handle `451` gracefully — they
  de-index the URL rather than crawling indefinitely.
- Clear signal to admins and support teams that a `451` means "embargoed, check the
  admin API" rather than "missing file, check the storage backend".

**Drawbacks:**

- `451` is less commonly seen than `403` or `404`; frontend developers and CDN
  configurations may not handle it by default (e.g. custom error pages). Teams must
  be educated to configure `451` handling alongside `403`/`404`.
- Some older HTTP clients or proxy layers may not recognise `451` and fall back to
  treating it as a generic `4xx`. This is acceptable — the error still signals a
  client-side constraint, not a server failure.

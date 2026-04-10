# ADR-0012: Surrogate-Key Headers for CDN Cache Invalidation

## Status

Accepted

## Context

When an admin lifts or updates an embargo, or when a source asset is updated in
storage, the CDN edge cache may hold stale transformed versions of that asset.
Invalidating stale cached content efficiently is a critical operational requirement
for a media CDN serving embargoed or time-sensitive retail assets.

CDN cache invalidation strategies:

- **URL-based purge** — purge a specific URL (e.g.
  `GET /cdn/hero.jpg?wid=800&fmt=webp`). Requires enumerating every cached URL
  variant (every combination of `wid`, `hei`, `fmt`, etc.) — impractical for assets
  with many transform variants.
- **Wildcard / prefix purge** — purge all URLs matching a prefix. Supported by some
  CDNs (Fastly, Cloudflare) but not standardised across providers.
- **Surrogate-Key / Cache-Tag purge** — tag every response with a key representing the
  logical asset; purge all variants with a single API call using that key. Supported
  by Fastly (`Surrogate-Key`), Varnish (`Surrogate-Key`), Cloudflare (`Cache-Tag`),
  and Akamai (`Edge-Cache-Tag`).

The `Surrogate-Key` / Cache-Tag approach allows purging all cached variants of
`hero.jpg` — regardless of transform parameters — with a single API call. This is
the industry-standard pattern used by Cloudinary, Imgix, and Fastly customers.

## Decision

Emit a `Surrogate-Key` header on all CDN responses, valued as the logical asset path.

```
Surrogate-Key: asset:campaigns/aw26/hero.jpg
```

- The header name `Surrogate-Key` is used for Fastly/Varnish compatibility. Operators
  targeting Cloudflare MUST remap it to `Cache-Tag` at the CDN layer (Cloudflare
  does not support `Surrogate-Key` directly but accepts `Cache-Tag` with equivalent
  semantics).
- Multiple keys MAY be space-separated (e.g. `asset:hero.jpg collection:aw26`) for
  group invalidation. The `collection` key is derived from the first path segment.
- When an embargo is lifted or a preset is updated via the admin API, Rendition emits
  a webhook event or structured log entry including the `Surrogate-Key` value.
  An operator-provided CDN purge script (or future `/admin/purge` endpoint) uses this
  to issue the CDN purge API call.
- `Surrogate-Key` headers MUST NOT be forwarded to end clients (stripped at the CDN
  edge or by the Nginx/Kubernetes Ingress).

## Consequences

**Benefits:**

- A single purge operation invalidates all cached transform variants of an asset
  (all combinations of `wid`, `hei`, `fmt`, etc.) without enumerating them.
- Critical for embargo lifts: when an embargo expires or is manually lifted, the CDN
  cache is invalidated instantly rather than waiting for TTL expiry.
- Fastly, Varnish, and Cloudflare (via Cache-Tag) all support this pattern natively.
  CloudFront supports it via origin shield + custom invalidation paths.
- Low overhead: a single string header added to every response.

**Drawbacks:**

- CDN purge is a separate operational step — Rendition emits the key; the operator
  must integrate purge API calls into their tooling or use the future `/admin/purge`
  endpoint. This is a deliberate separation of concerns (Rendition does not need CDN
  credentials).
- CloudFront does not natively support `Surrogate-Key` / `Cache-Tag`. CloudFront
  users must implement invalidation via path-based wildcards
  (`/cdn/campaigns/aw26/hero.jpg*`) or use CloudFront Functions to map keys.

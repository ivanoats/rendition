# ADR-0011: Automatic Format Negotiation via Accept Header

## Status

Accepted

## Context

Modern image formats (AVIF, WebP) offer 30–50% smaller file sizes than JPEG at
equivalent visual quality, directly reducing CDN bandwidth costs and improving page
load times. However, browser support is not universal:

- AVIF: supported in Chrome 85+, Firefox 93+, Safari 16+. Not supported in older
  browsers or some embedded webviews.
- WebP: supported in all modern browsers since 2020. IE11 does not support it.

Scene7 requires callers to specify `fmt=webp` or `fmt=avif` explicitly. This means
front-end code must detect browser capabilities and construct different URLs per
client — adding complexity and maintenance burden.

Best-of-breed CDNs (Cloudinary, Imgix, ImageKit) support automatic format selection
where the CDN inspects the `Accept` request header and delivers the best supported
format without client-side logic.

The `Accept` header sent by browsers reliably signals format support:
- Chrome/Firefox: `Accept: image/avif,image/webp,image/apng,*/*`
- Safari (AVIF support): `Accept: image/avif,image/webp,*/*`
- Older browsers: `Accept: */*` or `Accept: image/jpeg,image/png,*/*`

## Decision

Implement `fmt=auto` as a first-class transform parameter value.

- When `fmt=auto` is specified, Rendition inspects the `Accept` request header and
  selects the format with the highest q-value that Rendition can produce.
- Preference order (all else equal): AVIF → WebP → PNG (for transparency) → JPEG.
- PNG is selected over JPEG only when the source asset has an alpha channel AND the
  client accepts PNG.
- The `Vary: Accept` response header MUST be set on all `fmt=auto` responses so that:
  - Browser caches store separate entries per `Accept` value.
  - CDN edge caches (CloudFront, Fastly) vary their cache keys by `Accept`,
    preventing AVIF bytes from being served to a client that only accepts JPEG.
- `fmt=auto` is resolved to a concrete format before the cache key is computed;
  the cache stores entries per resolved format, not per `fmt=auto`.

## Consequences

**Benefits:**

- Front-end teams use a single URL (`?fmt=auto`) for all clients. No browser
  detection logic required in the embedding application.
- Clients automatically receive the smallest supported format, reducing bandwidth
  and improving Core Web Vitals (LCP).
- As new format support lands in browsers, Rendition's preference order can be
  updated without any front-end changes.
- Closes a meaningful capability gap vs Cloudinary/Imgix/ImageKit.

**Drawbacks:**

- `Vary: Accept` increases CDN cache complexity. CDNs must be configured to vary on
  `Accept`; misconfiguration causes format mismatch (AVIF served to IE11). Operators
  must verify CDN `Vary` handling during deployment.
- AVIF encoding via libvips adds ~20% more CPU time than WebP for equivalent quality.
  This is acceptable — AVIF is only generated on cache miss; subsequent requests hit
  the CDN or transform cache.
- The `Accept` header can be spoofed by clients to force specific format selection.
  This is not a security issue (the formats are all valid outputs) but could
  artificially inflate cache misses. Mitigation: CDN normalisation of the `Accept`
  header to a small set of canonical values (AVIF+WebP, WebP-only, fallback).

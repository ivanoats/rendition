# ADR-0005: Scene7-Compatible URL Parameter Naming

## Status

Accepted

## Context

Many enterprise organisations use Adobe Scene7 (now Adobe Dynamic Media) as
their media CDN. Scene7 uses a well-known URL parameter convention for
image transformations:

```
https://s7d9.scene7.com/is/image/Company/product?wid=800&hei=600&fmt=webp&qlt=85
```

Organisations migrating away from Scene7 face a significant integration
challenge: every CDN URL is embedded in web apps, mobile apps, email templates,
and CMS configurations. A CDN with incompatible URL parameters forces a
find-and-replace migration across all of those surfaces simultaneously.

## Decision

Adopt **Scene7's URL parameter naming** as Rendition's native API:

| Parameter | Meaning | Scene7 equivalent |
|---|---|---|
| `wid` | Output width (px) | `wid` |
| `hei` | Output height (px) | `hei` |
| `fit` | Fit mode: `crop`, `fit`, `stretch`, `constrain` | `fit` |
| `fmt` | Output format: `jpeg`, `webp`, `avif`, `png` | `fmt` |
| `qlt` | Quality 1–100 | `qlt` |
| `crop` | Pre-resize crop as `x,y,w,h` | `crop` |
| `rotate` | Clockwise rotation: 90, 180, 270 | `rotate` |
| `flip` | Mirror: `h`, `v`, `hv` | `flip` |

This is not a full Scene7 implementation — Rendition does not support Scene7's
`is/image` path prefix, image sets, or viewer presets. The goal is parameter
compatibility for the most common transformation operations.

### Migration path

An organisation migrating from Scene7 can:

1. Deploy Rendition with assets mirrored or proxied from the same origin paths.
2. Update the CDN hostname in a single environment variable or DNS CNAME.
3. Most existing Scene7 image URLs will work without any per-URL changes.

## Consequences

**Benefits:**
- Dramatically reduces migration friction for Scene7 customers.
- Parameter names are well-understood by front-end and e-commerce teams
  already familiar with Scene7.
- Existing documentation, CMS plugins, and tooling that generates Scene7 URLs
  can be reused with Rendition.

**Drawbacks:**
- Parameter names like `wid`, `hei`, `qlt` are abbreviations, not idiomatic
  REST API design. A greenfield API might prefer `width`, `height`, `quality`.
- Rendition is implicitly compared to Scene7; deviations in behaviour (e.g.
  unsupported parameters silently ignored) may cause subtle migration issues.
- Future parameter additions must consider Scene7 compatibility or document
  the divergence explicitly.

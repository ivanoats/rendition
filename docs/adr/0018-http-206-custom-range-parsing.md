# ADR-0018: Custom `Range` Header Parsing for HTTP 206 Video Delivery

## Status

Accepted

## Context

FR-22 requires byte-range (`HTTP 206 Partial Content`) support for video assets
(`mp4`, `webm`, `mov`) so that HTML5 `<video>` seeking works without downloading
the full file. Rendition must support this across both `LocalStorage` and
`S3Storage` backends.

Two implementation approaches were evaluated:

| Criterion | `tower-http::ServeFile` | Custom `Range` header parsing |
|---|---|---|
| Works with `LocalStorage` | Yes (file-path based) | Yes |
| Works with `S3Storage` | No (requires filesystem path) | Yes |
| S3 native range fetch | Not applicable | Yes — pass `Range` to `GetObject` |
| Code volume | n/a (inapplicable for S3) | ~80 lines |
| Architecture consistency | Breaks hexagonal abstraction | Consistent with `StorageBackend` trait |

`tower-http::ServeFile` operates on filesystem paths and cannot be used with
`S3Storage`, which returns `Vec<u8>` bytes from the AWS SDK. Using it would
require splitting the video serving path by storage backend, violating the
hexagonal architecture principle established in ADR-0004.

## Decision

Implement **custom `Range` header parsing** in the `serve_asset` handler with a
corresponding extension to the `StorageBackend` trait.

The `StorageBackend` trait gains an optional `get_range` method:

```rust
async fn get_range(
    &self,
    path: &str,
    range: std::ops::Range<u64>,
) -> Result<Asset>;
```

Default implementation fetches the full asset and slices the byte vector.
`S3Storage` overrides this to issue a `GetObject` request with the `Range`
header set to `bytes=start-end`, downloading only the requested byte range
from S3 — critical for large video files where seeking would otherwise
download gigabytes unnecessarily.

`serve_asset` parses the `Range` request header (single byte-range only;
multi-range not required for HTML5 video), calls `get_range`, and returns:

- `206 Partial Content` with `Content-Range: bytes start-end/total` and
  `Accept-Ranges: bytes` when a valid `Range` header is present.
- `200 OK` with the full asset when no `Range` header is present (existing
  behaviour for images).

## Consequences

**Benefits:**

- Works uniformly across `LocalStorage` and `S3Storage` without per-backend
  branching in the handler.
- `S3Storage::get_range` passes the `Range` header directly to `GetObject`,
  so only the requested byte range is downloaded from S3. This is essential
  for video assets of 100 MB+ where a seek to the middle of the file should
  not trigger a full download.
- `Accept-Ranges: bytes` is set on all asset responses (not just video), which
  is correct HTTP behaviour and enables browser-side range requests for large
  images in future use cases.

**Drawbacks:**

- The `StorageBackend` trait gains a new method. Existing trait implementations
  (`LocalStorage`, `MockStorage` in tests) provide a default implementation
  (full-fetch + slice) that is correct but not optimised. Only `S3Storage`
  overrides it for efficiency. This is an acceptable trade-off — the default
  fallback is always correct.
- Multi-range requests (`Range: bytes=0-100, 200-300`) are not supported and
  return `416 Range Not Satisfiable`. HTML5 `<video>` never issues multi-range
  requests, so this is not a practical limitation.

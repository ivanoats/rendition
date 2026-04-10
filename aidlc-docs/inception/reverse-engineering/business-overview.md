# Business Overview

## Business Context Diagram

```mermaid
C4Context
  title Rendition — Business Context

  Person(consumer, "API Consumer", "Application or browser requesting media assets")
  System(rendition, "Rendition CDN", "Serves, transforms, and delivers media assets on demand")
  SystemExt(storage, "Asset Storage", "Local filesystem or S3-compatible object store")

  Rel(consumer, rendition, "HTTP GET /cdn/{asset}?params")
  Rel(rendition, storage, "Fetches raw asset bytes")
```

## Business Description

- **Business Description**: Rendition is an open-source enterprise media CDN and a modern
  alternative to Adobe Scene7. It receives HTTP requests for media assets, applies
  real-time image transformations (resize, crop, format conversion, rotate, flip, quality
  tuning), and streams the result to the caller. The system is designed to be
  storage-agnostic and horizontally scalable.
- **Business Transactions**:
  - **Serve Asset**: Retrieve a raw media asset from the configured storage backend and
    stream it to the client with the correct MIME type.
  - **Transform Asset**: Accept a Scene7-compatible URL with transform query parameters,
    apply the requested operations (resize, crop, format convert, rotate, flip), and
    return the processed bytes.
  - **Health Check**: Respond to liveness probes so orchestrators (Kubernetes, ECS, etc.)
    can determine service readiness.
- **Business Dictionary**:
  - **Asset**: A raw media file (image or video) stored in the configured backend,
    identified by a logical path (e.g. `products/shoe.jpg`).
  - **Transform**: A set of on-the-fly operations applied to an asset before delivery
    (resize, crop, format conversion, rotate, flip, quality).
  - **Fit mode**: Strategy used when both width and height are specified — `constrain`
    (preserve aspect ratio, scale down only), `crop` (fill box with center-crop),
    `stretch`/`fill` (scale each axis independently).
  - **Scene7**: Adobe Dynamic Media (formerly Scene7) — an enterprise media CDN whose URL
    convention Rendition mirrors for drop-in migration compatibility.
  - **libvips**: The underlying C image-processing library used for all pixel operations.

## Component Level Business Descriptions

### API Layer (`src/api/mod.rs`)

- **Purpose**: Exposes the HTTP interface. Parses transform parameters, orchestrates
  storage retrieval, and streams the transformed image back to the caller.
- **Responsibilities**: Route matching, query-string parsing, HTTP status code mapping,
  response assembly.

### Storage Layer (`src/storage/mod.rs`)

- **Purpose**: Abstracts the origin media store so the rest of the system is
  storage-agnostic.
- **Responsibilities**: Asset retrieval by logical path, asset existence checks, MIME type
  detection from file extension.

### Transform Pipeline (`src/transform/mod.rs`)

- **Purpose**: Implements all image processing operations using libvips.
- **Responsibilities**: Decode source bytes, apply crop → resize → rotate → flip in order,
  encode output to the requested format at the requested quality.

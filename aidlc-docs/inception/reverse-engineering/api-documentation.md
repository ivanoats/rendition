# API Documentation

## REST APIs

### Health Check

- **Method**: `GET`
- **Path**: `/health`
- **Purpose**: Liveness/readiness probe for orchestrators (Kubernetes, ECS, load
  balancers).
- **Request**: No parameters.
- **Response** (200 OK):

  ```json
  { "status": "ok", "service": "rendition" }
  ```

### Serve / Transform Asset

- **Method**: `GET`
- **Path**: `/cdn/{asset_path}`
- **Purpose**: Fetch an asset from the storage backend, apply optional on-the-fly
  transformations, and return the resulting bytes.
- **Path Parameter**: `asset_path` — logical path within the asset store (e.g.
  `products/shoe.jpg`, `banners/hero.png`).
- **Query Parameters**:

  | Parameter | Type   | Default      | Description                                         |
  |-----------|--------|--------------|-----------------------------------------------------|
  | `wid`     | u32    | original     | Output width in pixels                              |
  | `hei`     | u32    | original     | Output height in pixels                             |
  | `fit`     | string | `constrain`  | Fit mode: `crop`, `fit`, `stretch`, `fill`, `constrain`   |
  | `fmt`     | string | `jpeg`       | Output format: `webp`, `avif`, `jpeg`, `png`              |
  | `qlt`     | u8     | `85`         | Quality 1–100 (lossy formats only)                  |
  | `crop`    | string | none         | Pre-resize crop region as `x,y,w,h` (pixels)       |
  | `rotate`  | i32    | `0`          | Clockwise rotation: `90`, `180`, `270`              |
  | `flip`    | string | none         | Flip axis: `h`, `v`, `hv`                           |

- **Responses**:
  - `200 OK` — Transformed image bytes; `Content-Type` matches the output format.
  - `400 Bad Request` — Malformed query parameter (e.g. non-integer `wid`).
  - `404 Not Found` — Asset does not exist in the storage backend.
  - `500 Internal Server Error` — Storage read failure or transform pipeline error.

#### Fit Mode Semantics

| Mode         | Behaviour                                                                          |
|--------------|------------------------------------------------------------------------------------|
| `constrain`  | Scale down only, preserving aspect ratio, to fit within the requested box.         |
| `fit`        | Alias for `constrain`.                                                             |
| `crop`       | Scale to fill the box (upscale if needed), then center-crop to exact dimensions.   |
| `stretch`    | Scale each axis independently to fill the exact requested dimensions.              |
| `fill`       | Alias for `stretch`.                                                               |

#### Transform Pipeline Order

Pre-crop → resize → rotate → flip → encode

## Internal APIs

### `rendition::build_app(assets_path: &str) -> Router`

- **Purpose**: Constructs the Axum `Router` wired to `LocalStorage` at the given path.
- **Used by**: `main.rs` (production), `tests/e2e.rs` (integration tests).

### `api::router<S: StorageBackend>(state: AppState<S>) -> Router`

- **Purpose**: Produces the sub-router for all `/cdn/…` routes.
- **Used by**: `lib::build_app`.

### `storage::StorageBackend` (trait)

- `get(&self, path: &str) -> impl Future<Output = anyhow::Result<Asset>> + Send`
  — Retrieve asset bytes and metadata.
- `exists(&self, path: &str) -> impl Future<Output = bool> + Send`
  — Check asset existence without reading bytes.

### `transform::apply(source: Vec<u8>, params: TransformParams) -> anyhow::Result<(Vec<u8>, &'static str)>`

- **Purpose**: Public async entry point for the transform pipeline. Offloads to
  `spawn_blocking` to avoid blocking the Tokio reactor with libvips CPU work.
- **Returns**: Tuple of `(output_bytes, mime_type_str)`.

## Data Models

### `TransformParams`

Deserialized from URL query string via `serde::Deserialize`.

| Field    | Rust Type       | Description                                    |
|----------|-----------------|------------------------------------------------|
| `wid`    | `Option<u32>`   | Output width in pixels                         |
| `hei`    | `Option<u32>`   | Output height in pixels                        |
| `fit`    | `Option<String>`| Fit mode string                                |
| `fmt`    | `Option<String>`| Output format string                           |
| `qlt`    | `Option<u8>`    | Quality (1–100)                                |
| `crop`   | `Option<String>`| Crop string `"x,y,w,h"`                        |
| `rotate` | `Option<i32>`   | Rotation degrees (90, 180, 270)                |
| `flip`   | `Option<String>`| Flip axis (`"h"`, `"v"`, `"hv"`)               |

### `storage::Asset`

Returned by `StorageBackend::get`.

| Field          | Rust Type | Description                     |
|----------------|-----------|---------------------------------|
| `data`         | `Vec<u8>` | Raw media file bytes            |
| `content_type` | `String`  | MIME type (e.g. `"image/jpeg"`) |
| `size`         | `usize`   | Byte count of `data`            |

### `api::AppState<S>`

Axum shared state injected into every handler.

| Field     | Rust Type  | Description                              |
|-----------|------------|------------------------------------------|
| `storage` | `Arc<S>`   | Reference-counted storage backend handle |

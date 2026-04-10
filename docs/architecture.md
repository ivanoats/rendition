# Rendition — Architecture

Rendition is an open-source, enterprise-ready media CDN written in Rust.
It delivers on-demand image transformations via URL parameters, serving as a
modern alternative to Adobe Scene7.

---

## Level 1 — System Context

```mermaid
%%{init: {"flowchart": {"curve": "stepAfter", "diagramPadding": 40}}}%%
C4Context
    title System Context — Rendition

    Person(client, "Client Application", "Web or mobile app requesting media assets")
    Person(ops, "Operator", "Configures the service via environment variables")

    System(rendition, "Rendition", "On-demand media CDN. Fetches original assets from storage, transforms them per URL parameters, and streams the result.")

    System_Ext(storage, "Asset Storage", "Local filesystem, Amazon S3, or S3-compatible object store (MinIO, Cloudflare R2)")

    Rel(client, rendition, "GET /cdn/{path}?wid=800&fmt=webp", "HTTPS")
    Rel(ops, rendition, "Sets RENDITION_ASSETS_PATH, RUST_LOG", "Environment")
    Rel(rendition, storage, "Fetches original media files", "File I/O / S3 API")

    UpdateLayoutConfig($c4ShapeInRow="2", $c4BoundaryInRow="1")
```

**Primary responsibilities:**

- Accept HTTP requests with URL-encoded transform parameters (Scene7-compatible)
- Retrieve original assets from a pluggable storage backend
- Apply a sequential image transform pipeline (crop → resize → rotate → flip → encode)
- Stream the result with the correct `Content-Type` header

---

## Level 2 — Container View

```mermaid
C4Container
    title Container View — Rendition Service

    Person(client, "Client")

    Container_Boundary(svc, "Rendition Process") {
        Container(http, "HTTP Server", "Rust · Axum 0.7 · Tokio 1", "Listens on :3000. Routes /health and /cdn/* requests. Injects shared AppState.")
        Container(api, "API Handler", "Rust · async fn", "Extracts Path + Query params. Checks asset existence. Orchestrates storage → transform → response.")
        Container(pipeline, "Transform Pipeline", "Rust · libvips 8.x", "Decodes source bytes. Applies crop, resize, rotate, flip in sequence. Encodes to target format.")
        Container(adapters, "Storage Adapters", "Rust · trait StorageBackend", "Pluggable backends. LocalStorage reads from the filesystem. S3Storage stub for future use.")
    }

    System_Ext(fs, "Local Filesystem", "./assets or RENDITION_ASSETS_PATH")
    System_Ext(s3, "Amazon S3 / S3-compatible", "Future: bucket configured at startup")

    Rel(client, http, "GET /cdn/image.jpg?wid=800", "HTTP")
    Rel(http, api, "Path + Query + State extractors", "in-process")
    Rel(api, adapters, "exists() / get()", "async trait call")
    Rel(api, pipeline, "apply(bytes, params)", "tokio::spawn_blocking")
    Rel(adapters, fs, "tokio::fs::read()", "async file I/O")
    Rel(adapters, s3, "s3::GetObject (not yet impl)", "HTTP / AWS SDK")
    Rel(api, client, "200 image bytes / 404 / 500", "HTTP")
```

**Key runtime characteristics:**

| Concern | Approach |
|---|---|
| Concurrency | Tokio multi-threaded async executor |
| CPU-bound work | `tokio::task::spawn_blocking` for libvips calls |
| Shared state | `Arc<S>` where `S: StorageBackend` |
| Observability | `tower_http::TraceLayer` + `tracing` structured logs |
| Configuration | Environment variables (`RENDITION_ASSETS_PATH`, `RUST_LOG`) |

---

## Level 3 — Component View

```mermaid
C4Component
    title Component View — Rendition

    Container_Boundary(lib, "rendition (lib)") {
        Component(build_app, "build_app()", "pub fn → Router", "Wires LocalStorage into AppState, merges api::router, adds TraceLayer. Entry point for tests and main.")
        Component(health, "health_check", "async fn", "GET /health → {status:ok, service:rendition}")

        Component(router_fn, "api::router()", "pub fn → Router", "Registers GET /cdn/*asset_path with AppState<S>.")
        Component(serve_asset, "serve_asset<S>()", "async fn", "1. exists() check → 404. 2. get() → 500 on error. 3. apply() → 500 on error. 4. Return bytes + Content-Type.")
        Component(app_state, "AppState<S>", "pub struct", "Holds Arc<S: StorageBackend>. Cloned into each handler by Axum.")

        Component(storage_trait, "StorageBackend", "pub trait", "get(path) → Result<Asset>. exists(path) → bool. Send + Sync, RPITIT async.")
        Component(local_storage, "LocalStorage", "StorageBackend impl", "Resolves paths relative to root PathBuf. Reads bytes with tokio::fs::read. Detects MIME type from extension.")
        Component(s3_storage, "S3Storage", "StorageBackend stub", "Holds bucket + region. Both methods return todo!(). Present to validate the trait pattern.")
        Component(content_type, "content_type_from_ext()", "pub(crate) fn", "Maps file extension to &'static str MIME type. Falls back to application/octet-stream.")
        Component(asset, "Asset", "pub struct", "data: Vec<u8>, content_type: String, size: usize")

        Component(transform_params, "TransformParams", "pub struct · Deserialize", "wid, hei, fit, fmt, qlt, crop, rotate, flip. All Option<T>, default None.")
        Component(apply_fn, "transform::apply()", "pub async fn", "Wraps apply_blocking in spawn_blocking. Returns (Vec<u8>, &'static str).")
        Component(apply_blocking, "apply_blocking()", "fn", "Orchestrates the pipeline: decode → crop → resize → rotate → flip → encode.")
        Component(pipeline_steps, "apply_crop / apply_resize / apply_rotation / apply_flip / encode", "fn", "Pure functions VipsImage → Result<VipsImage>. encode returns (bytes, mime).")
        Component(vips_init, "ensure_vips()", "pub(crate) fn", "OnceLock<VipsApp> singleton. Called once per process before any libvips operation.")
    }

    Container_Boundary(bin, "rendition (bin)") {
        Component(main_fn, "main()", "async fn", "Reads RENDITION_ASSETS_PATH. Calls build_app(). Binds TCP listener on :3000.")
    }

    Rel(main_fn, build_app, "calls", "")
    Rel(build_app, router_fn, "merges", "")
    Rel(build_app, local_storage, "instantiates and wraps in AppState", "")
    Rel(router_fn, serve_asset, "routes to", "")
    Rel(serve_asset, app_state, "reads storage from", "")
    Rel(app_state, storage_trait, "holds impl of", "")
    Rel(local_storage, storage_trait, "implements", "")
    Rel(local_storage, content_type, "uses", "")
    Rel(local_storage, asset, "produces", "")
    Rel(serve_asset, apply_fn, "calls", "")
    Rel(apply_fn, apply_blocking, "spawn_blocking", "")
    Rel(apply_blocking, pipeline_steps, "chains", "")
    Rel(apply_blocking, vips_init, "calls first", "")
```

---

## Request Lifecycle — Sequence Diagram

```mermaid
sequenceDiagram
    participant C as Client
    participant H as serve_asset handler
    participant S as StorageBackend
    participant T as Transform Pipeline

    C->>H: GET /cdn/products/shoe.jpg?wid=800&fmt=webp&qlt=80
    H->>S: exists("products/shoe.jpg")
    S-->>H: true
    H->>S: get("products/shoe.jpg")
    S-->>H: Asset { data: [JPEG bytes], content_type: "image/jpeg" }
    H->>T: apply(bytes, {wid:800, fmt:"webp", qlt:80})
    note over T: spawn_blocking — runs on thread pool
    T->>T: VipsImage::new_from_buffer (decode JPEG)
    T->>T: apply_crop (no-op — crop not set)
    T->>T: apply_resize (scale to width 800, constrain aspect)
    T->>T: apply_rotation (no-op — rotate not set)
    T->>T: apply_flip (no-op — flip not set)
    T->>T: encode → webpsave_buffer_with_opts(q=80)
    T-->>H: (webp_bytes, "image/webp")
    H-->>C: 200 OK, Content-Type: image/webp, [binary body]
```

---

## Transform Pipeline — Operation Order

```mermaid
flowchart LR
    A[Raw bytes\nfrom storage] --> B[Decode\nnew_from_buffer]
    B --> C{crop\nparam set?}
    C -- yes --> D[extract_area\nx,y,w,h]
    C -- no --> E{wid or hei\nset?}
    D --> E
    E -- yes --> F{fit mode}
    E -- no --> G{rotate\nparam set?}
    F -- constrain\ndefault --> F1[ops::resize\nscale = min ratio]
    F -- crop --> F2[ops::resize\nscale = max ratio\n+ extract_area center]
    F -- stretch\nfill --> F3[ops::resize_with_opts\nindependent hscale/vscale]
    F1 --> G
    F2 --> G
    F3 --> G
    G -- 90/180/270 --> H[ops::rot]
    G -- none --> I{flip\nparam set?}
    H --> I
    I -- h/v/hv --> J[ops::flip]
    I -- none --> K{fmt param}
    J --> K
    K -- webp --> L[webpsave_buffer\nwith quality]
    K -- avif --> M[heifsave_buffer\nAV1 + quality]
    K -- png --> N[pngsave_buffer\nlossless]
    K -- jpeg\ndefault --> O[jpegsave_buffer\nwith quality]
    L --> P[Return\nbytes + MIME type]
    M --> P
    N --> P
    O --> P
```

---

## Storage Backend — Class Diagram

```mermaid
classDiagram
    class StorageBackend {
        <<trait>>
        +get(path: &str) Future~Result~Asset~~
        +exists(path: &str) Future~bool~
    }

    class LocalStorage {
        -root: PathBuf
        +new(root) LocalStorage
        +get(path) Future~Result~Asset~~
        +exists(path) Future~bool~
    }

    class S3Storage {
        +bucket: String
        +region: String
        +new(bucket, region) S3Storage
        +get(path) Future~Result~Asset~~
        +exists(path) Future~bool~
    }

    class Asset {
        +data: Vec~u8~
        +content_type: String
        +size: usize
    }

    class AppState~S~ {
        +storage: Arc~S~
    }

    StorageBackend <|.. LocalStorage : implements
    StorageBackend <|.. S3Storage : implements (stub)
    LocalStorage ..> Asset : produces
    AppState~S~ o-- StorageBackend : holds Arc of
```

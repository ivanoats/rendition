# ADR-0003: libvips for Image Processing

## Status

Accepted

## Context

On-demand image transformation is the core value-add of the CDN. The image
processing library must handle:

- **Throughput**: Process many concurrent resize/encode operations with low
  memory usage — source images may be 20–50 MB originals.
- **Format breadth**: Decode JPEG, PNG, WebP, AVIF, GIF, SVG; encode to JPEG,
  WebP, AVIF, PNG.
- **Quality control**: Fine-grained quality parameters for lossy formats.
- **Pipeline operations**: Crop, resize (multiple fit modes), rotate, flip —
  applied in sequence.
- **Rust integration**: Safe bindings or well-maintained FFI wrapper.

Alternatives considered:

| Library | Notes |
|---|---|
| **ImageMagick** (via `magick-rust`) | Battle-tested, broad format support. Historically had CVEs; slower than libvips for large images; bindings less ergonomic. |
| **image-rs** (pure Rust) | No native dependency, easy to build. Missing AVIF support, significantly slower for large images, limited quality control. |
| **Sharp** (Node.js) | Excellent libvips bindings, but Node.js is not the runtime. |
| **Photon** (WebAssembly) | Portable, but immature for production use; limited format support. |

## Decision

Use **libvips** via the `libvips` Rust crate (v1.7.3).

libvips is a demand-driven, horizontally threaded image processing library. Key
properties that drove this choice:

- **Memory efficiency**: Operates on image strips rather than loading the full
  image into memory; a 50 MB JPEG can be resized with a fraction of the memory
  a pixel-based approach would require.
- **Speed**: Benchmarks consistently show libvips 4–8× faster than
  ImageMagick for resize-heavy workloads.
- **AVIF support**: `heifsave_buffer_with_opts` with `ForeignHeifCompression::Av1`
  supports AVIF encoding, which is on the roadmap as a primary output format.
- **libvips Rust crate**: Provides auto-generated bindings to the full libvips
  operation set with Rust-idiomatic error handling (`Result<VipsImage>`).

### Important implementation note

`vips_thumbnail_image` (the load-shrink-optimised thumbnail operation) fails on
VipsImages already decoded into memory because it requires re-opening the source
for load-time shrink. Rendition uses `ops::resize()` with calculated scale
factors instead, which works correctly on any in-memory VipsImage regardless
of how it was decoded. See ADR-0004 for how the pipeline is structured.

libvips is initialised once per process via `OnceLock<VipsApp>` in
`transform::ensure_vips()`, preventing concurrent initialisation races.
CPU-bound libvips calls are offloaded to Tokio's blocking thread pool via
`tokio::task::spawn_blocking` to avoid stalling the async executor.

## Consequences

**Benefits:**
- Best-in-class throughput and memory efficiency for server-side image
  processing.
- AVIF encoding available today; no additional library required.
- Broad input format support out of the box.

**Drawbacks:**
- **Native dependency**: libvips must be installed on the build host and at
  runtime. On macOS, this is via Homebrew (`brew install vips`); on Linux, via
  the system package manager or a Docker base image with libvips pre-installed.
- **Dynamic linking**: The binary is not fully self-contained; `DYLD_LIBRARY_PATH`
  / `LD_LIBRARY_PATH` must include the libvips install path, or the library
  must be on the system path.
- **C FFI boundary**: Bugs or vulnerabilities in libvips itself are not caught
  by the Rust borrow checker.
- **Binding staleness**: The `libvips` crate may lag behind upstream libvips
  releases for new operation support.

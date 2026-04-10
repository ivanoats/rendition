# ADR-0001: Rust as the Primary Runtime

## Status

Accepted

## Context

Rendition is a media CDN that must handle high concurrency (many simultaneous
image requests), process untrusted binary input (arbitrary media files), and
perform CPU-intensive work (image decoding, resizing, encoding via libvips).
These requirements place competing demands on the runtime:

- **Throughput**: The service must sustain tens of thousands of concurrent
  requests on a single node without degrading latency.
- **Memory safety**: Media processing involves parsing untrusted bytes. Buffer
  overflows or use-after-free bugs in this path create security vulnerabilities.
- **C FFI**: The best open-source image processor (libvips) is a C library.
  The language must support safe, low-overhead FFI.
- **Binary simplicity**: A single statically-linked binary simplifies
  deployment and containerisation.
- **GC pauses**: Unpredictable stop-the-world pauses would cause latency
  spikes on large image operations.

Alternatives considered: Go, Node.js (with Sharp/libvips bindings), Python
(with pyvips), Java/Kotlin.

## Decision

Use **Rust** as the primary implementation language.

- **Tokio** provides a high-performance async executor with a multi-threaded
  runtime, enabling concurrent I/O without threads-per-request overhead.
- **Ownership model** eliminates entire classes of memory errors at compile
  time, critical when processing untrusted media bytes.
- **`unsafe` boundary is minimal**: Only the libvips FFI layer touches unsafe
  code; all application logic is safe Rust.
- **No GC**: Predictable latency with no stop-the-world pauses.
- **Single binary**: `cargo build --release` produces a statically-linked
  binary suitable for minimal container images.
- **libvips FFI**: The `libvips` crate provides ergonomic Rust bindings to the
  C library with minimal overhead.

## Consequences

**Benefits:**
- Compile-time memory safety eliminates buffer overflow and use-after-free
  risks in the media processing path.
- Tokio + Axum handle high concurrency with low memory overhead.
- Zero-cost abstractions mean trait dispatch (e.g. `StorageBackend`) compiles
  to direct calls with monomorphisation.
- Easy containerisation: one binary, no runtime dependency on language VM.

**Drawbacks:**
- Steeper learning curve than Go or Python for contributors unfamiliar with
  ownership and the borrow checker.
- Compile times are longer than interpreted languages; incremental builds
  mitigate this during development.
- Ecosystem for some integrations (e.g. AWS SDK) is less mature than Go/Java.
- libvips must be installed as a native dependency on the build host and at
  runtime (dynamically linked).

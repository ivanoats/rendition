//! Image transformation pipeline.
//!
//! This module will house the core media processing logic — resizing, cropping,
//! format conversion, quality adjustment, and compositing.  The initial
//! implementation uses [libvips](https://www.libvips.org/) via the `libvips`
//! crate; the trait-based design keeps us free to swap backends later.

/// Parameters parsed from a Rendition transform URL.
///
/// URL syntax mirrors Scene7 for easy migration:
/// `/cdn/image.jpg?wid=800&hei=600&fit=crop&fmt=webp&qlt=85`
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct TransformParams {
    /// Output width in pixels.
    pub wid: Option<u32>,
    /// Output height in pixels.
    pub hei: Option<u32>,
    /// Fit mode: `crop`, `fit`, `stretch`, `constrain` (default: `constrain`).
    pub fit: Option<String>,
    /// Output format: `webp`, `avif`, `jpeg`, `png` (default: original).
    pub fmt: Option<String>,
    /// Quality 1–100 (applies to lossy formats, default: 85).
    pub qlt: Option<u8>,
    /// Crop rectangle as `x,y,w,h` in pixels (applied before resize).
    pub crop: Option<String>,
}

/// Apply `params` to the raw bytes of a source image.
///
/// Returns the transformed image bytes and the MIME type of the output format.
/// Returns an error if the image cannot be decoded or the transformation fails.
///
/// # Errors
/// Propagates I/O and image-processing errors via [`anyhow::Error`].
pub async fn apply(
    _source: Vec<u8>,
    _params: TransformParams,
) -> anyhow::Result<(Vec<u8>, &'static str)> {
    // TODO: implement with libvips or image-rs
    anyhow::bail!("image transformation not yet implemented")
}

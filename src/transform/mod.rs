//! Image transformation pipeline.
//!
//! This module houses the core media processing logic — resizing, cropping,
//! format conversion, quality adjustment.  Built on libvips for fast,
//! memory-efficient processing of large images.

use anyhow::Context;
#[cfg(test)]
use libvips::ops::BlackOptions;
use libvips::{
    ops::{
        self, ForeignHeifCompression, HeifsaveBufferOptions, JpegsaveBufferOptions, ResizeOptions,
    },
    VipsApp, VipsImage,
};
use std::sync::OnceLock;

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
    /// Fit mode: `crop`, `fit`, `stretch`, `fill`, `constrain` (default: `constrain`).
    pub fit: Option<String>,
    /// Output format: `webp`, `avif`, `jpeg`, `png` (default: `jpeg`).
    pub fmt: Option<String>,
    /// Quality 1–100 for lossy formats (default: 85).
    pub qlt: Option<u8>,
    /// Pre-resize crop rectangle as `x,y,w,h` in pixels.
    pub crop: Option<String>,
    /// Clockwise rotation in degrees.  Supported values: 90, 180, 270.
    pub rotate: Option<i32>,
    /// Flip axis: `h` (horizontal mirror), `v` (vertical mirror), `hv` (both).
    pub flip: Option<String>,
}

// ---- libvips initialisation ------------------------------------------------

static VIPS_APP: OnceLock<VipsApp> = OnceLock::new();

/// Ensure libvips is initialised exactly once for the process lifetime.
pub(crate) fn ensure_vips() {
    VIPS_APP.get_or_init(|| VipsApp::new("rendition", false).expect("Cannot initialize libvips"));
}

// ---- Public API ------------------------------------------------------------

/// Apply `params` to the raw bytes of a source image.
///
/// Returns the transformed image bytes and the MIME type of the output format.
///
/// # Errors
/// Propagates image-processing errors via [`anyhow::Error`].
pub async fn apply(
    source: Vec<u8>,
    params: TransformParams,
) -> anyhow::Result<(Vec<u8>, &'static str)> {
    tokio::task::spawn_blocking(move || apply_blocking(source, params))
        .await
        .context("transform task panicked")?
}

// ---- Core (synchronous) transform logic ------------------------------------

fn apply_blocking(
    source: Vec<u8>,
    params: TransformParams,
) -> anyhow::Result<(Vec<u8>, &'static str)> {
    ensure_vips();

    let image = VipsImage::new_from_buffer(&source, "").context("failed to decode source image")?;

    let image = apply_crop(image, &params)?;
    let image = apply_resize(image, &params)?;
    let image = apply_rotation(image, &params)?;
    let image = apply_flip(image, &params)?;
    encode(image, &params)
}

fn apply_crop(image: VipsImage, params: &TransformParams) -> anyhow::Result<VipsImage> {
    let Some(crop_str) = &params.crop else {
        return Ok(image);
    };
    let parts: Vec<i32> = crop_str
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<i32>()
                .context("crop values must be integers")
        })
        .collect::<anyhow::Result<_>>()?;
    anyhow::ensure!(
        parts.len() == 4,
        "crop must be x,y,w,h (got {} parts)",
        parts.len()
    );
    ops::extract_area(&image, parts[0], parts[1], parts[2], parts[3])
        .context("crop (extract_area) failed")
}

fn apply_resize(image: VipsImage, params: &TransformParams) -> anyhow::Result<VipsImage> {
    if params.wid.is_none() && params.hei.is_none() {
        return Ok(image);
    }

    let src_w = image.get_width() as f64;
    let src_h = image.get_height() as f64;
    let fit = params.fit.as_deref().unwrap_or("constrain");

    match fit {
        "stretch" | "fill" => {
            // Scale each axis independently to exactly fill the target box.
            let target_w = params.wid.map(|v| v as f64).unwrap_or(src_w);
            let target_h = params.hei.map(|v| v as f64).unwrap_or(src_h);
            ops::resize_with_opts(
                &image,
                target_w / src_w,
                &ResizeOptions {
                    vscale: target_h / src_h,
                    ..Default::default()
                },
            )
            .context("resize (stretch) failed")
        }
        "crop" => {
            // Scale to fill the target box, then center-crop to exact dimensions.
            let target_w = params.wid.map(|v| v as f64).unwrap_or(src_w);
            let target_h = params.hei.map(|v| v as f64).unwrap_or(src_h);
            let scale = (target_w / src_w).max(target_h / src_h);
            let scaled = ops::resize(&image, scale).context("resize (crop scale) failed")?;
            let scaled_w = scaled.get_width();
            let scaled_h = scaled.get_height();
            let crop_x = ((scaled_w - target_w as i32) / 2).max(0);
            let crop_y = ((scaled_h - target_h as i32) / 2).max(0);
            let crop_w = (target_w as i32).min(scaled_w);
            let crop_h = (target_h as i32).min(scaled_h);
            ops::extract_area(&scaled, crop_x, crop_y, crop_w, crop_h)
                .context("resize (crop extract) failed")
        }
        _ => {
            // "constrain", "fit", or unrecognised → fit within the box preserving aspect ratio.
            let target_w = params.wid.map(|v| v as f64).unwrap_or(f64::MAX);
            let target_h = params.hei.map(|v| v as f64).unwrap_or(f64::MAX);
            let scale = (target_w / src_w).min(target_h / src_h).min(1.0);
            ops::resize(&image, scale).context("resize failed")
        }
    }
}

fn apply_rotation(image: VipsImage, params: &TransformParams) -> anyhow::Result<VipsImage> {
    match params.rotate.unwrap_or(0) {
        90 => ops::rot(&image, ops::Angle::D90).context("rotate 90° failed"),
        180 => ops::rot(&image, ops::Angle::D180).context("rotate 180° failed"),
        270 => ops::rot(&image, ops::Angle::D270).context("rotate 270° failed"),
        _ => Ok(image),
    }
}

fn apply_flip(image: VipsImage, params: &TransformParams) -> anyhow::Result<VipsImage> {
    match params.flip.as_deref().unwrap_or("") {
        "h" => ops::flip(&image, ops::Direction::Horizontal).context("flip horizontal failed"),
        "v" => ops::flip(&image, ops::Direction::Vertical).context("flip vertical failed"),
        "hv" | "vh" => {
            let h =
                ops::flip(&image, ops::Direction::Horizontal).context("flip horizontal failed")?;
            ops::flip(&h, ops::Direction::Vertical).context("flip vertical failed")
        }
        _ => Ok(image),
    }
}

fn encode(image: VipsImage, params: &TransformParams) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let quality = params.qlt.unwrap_or(85) as i32;

    match params.fmt.as_deref().unwrap_or("jpeg") {
        "webp" => {
            let bytes = webp_save_buffer(&image, quality).context("webp encode failed")?;
            Ok((bytes, "image/webp"))
        }
        "png" => {
            let bytes = ops::pngsave_buffer(&image).context("png encode failed")?;
            Ok((bytes, "image/png"))
        }
        "avif" => {
            let bytes = ops::heifsave_buffer_with_opts(
                &image,
                &HeifsaveBufferOptions {
                    q: quality,
                    compression: ForeignHeifCompression::Av1,
                    ..Default::default()
                },
            )
            .context("avif encode failed")?;
            Ok((bytes, "image/avif"))
        }
        _ => {
            // "jpeg" or any unrecognised format
            let bytes = ops::jpegsave_buffer_with_opts(
                &image,
                &JpegsaveBufferOptions {
                    q: quality,
                    ..Default::default()
                },
            )
            .context("jpeg encode failed")?;
            Ok((bytes, "image/jpeg"))
        }
    }
}

// ---- Private helpers -------------------------------------------------------

/// Encode `image` to a WebP buffer at the given quality.
///
/// The high-level [`ops::webpsave_buffer_with_opts`] passes options such as
/// `smart-deblock` and `passes` that were introduced after libvips 8.15 and
/// cause the C function to return an error on older installs.  Using
/// [`VipsImage::image_write_to_buffer`] with an option-encoded suffix avoids
/// this version skew while still honoring the requested quality.
fn webp_save_buffer(image: &VipsImage, quality: i32) -> anyhow::Result<Vec<u8>> {
    let suffix = format!(".webp[Q={}]", quality);
    image
        .image_write_to_buffer(&suffix)
        .map_err(|e| anyhow::anyhow!("webp encode failed: {}", e))
}

// ---- Test helpers ----------------------------------------------------------

/// Create a small solid-black RGB JPEG in memory.  Only compiled during tests.
#[cfg(test)]
pub(crate) fn test_jpeg(w: i32, h: i32) -> Vec<u8> {
    ensure_vips();
    let image = ops::black_with_opts(w, h, &BlackOptions { bands: 3 })
        .expect("failed to create test image");
    ops::jpegsave_buffer(&image).expect("failed to encode test JPEG")
}

// ---- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode `bytes` with libvips and return `(width, height)`.
    fn image_dims(bytes: &[u8]) -> (i32, i32) {
        ensure_vips();
        let img = VipsImage::new_from_buffer(bytes, "").expect("failed to decode output image");
        (img.get_width(), img.get_height())
    }

    #[test]
    fn default_params_are_all_none() {
        let p = TransformParams::default();
        assert!(p.wid.is_none());
        assert!(p.hei.is_none());
        assert!(p.fit.is_none());
        assert!(p.fmt.is_none());
        assert!(p.qlt.is_none());
        assert!(p.crop.is_none());
        assert!(p.rotate.is_none());
        assert!(p.flip.is_none());
    }

    #[test]
    fn params_struct_accepts_expected_values() {
        let p = TransformParams {
            wid: Some(800),
            hei: Some(600),
            fit: Some("crop".to_string()),
            fmt: Some("webp".to_string()),
            qlt: Some(85),
            crop: Some("10,20,100,200".to_string()),
            rotate: Some(90),
            flip: Some("h".to_string()),
        };
        assert_eq!(p.wid, Some(800));
        assert_eq!(p.hei, Some(600));
        assert_eq!(p.rotate, Some(90));
        assert_eq!(p.flip.as_deref(), Some("h"));
    }

    #[tokio::test]
    async fn passthrough_returns_jpeg() {
        let bytes = test_jpeg(64, 64);
        let (out, mime) = apply(bytes, TransformParams::default()).await.unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(!out.is_empty());
        // No resize params → source dimensions unchanged.
        let (w, h) = image_dims(&out);
        assert_eq!((w, h), (64, 64));
    }

    #[tokio::test]
    async fn resize_width_only() {
        // 64×64 source, constrain to wid=32 (no hei) → 32×32 (aspect preserved).
        let bytes = test_jpeg(64, 64);
        let params = TransformParams {
            wid: Some(32),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params).await.unwrap();
        assert_eq!(mime, "image/jpeg");
        let (w, h) = image_dims(&out);
        assert_eq!(w, 32, "width should be exactly 32");
        assert_eq!(
            h, 32,
            "height should be 32 (aspect ratio preserved for square source)"
        );
    }

    #[tokio::test]
    async fn resize_to_webp_with_quality() {
        // 64×64 source, constrain to 32×32 → exactly 32×32.
        let bytes = test_jpeg(64, 64);
        let params = TransformParams {
            wid: Some(32),
            hei: Some(32),
            fmt: Some("webp".to_string()),
            qlt: Some(80),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params).await.unwrap();
        assert_eq!(mime, "image/webp");
        let (w, h) = image_dims(&out);
        assert_eq!((w, h), (32, 32));
    }

    #[tokio::test]
    async fn crop_fit_fills_target_box() {
        // 64×64 source, fit=crop → output must be exactly the requested dimensions.
        let bytes = test_jpeg(64, 64);
        let params = TransformParams {
            wid: Some(20),
            hei: Some(40),
            fit: Some("crop".to_string()),
            fmt: Some("png".to_string()),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params).await.unwrap();
        assert_eq!(mime, "image/png");
        let (w, h) = image_dims(&out);
        assert_eq!(w, 20, "crop fit: width must equal requested wid");
        assert_eq!(h, 40, "crop fit: height must equal requested hei");
    }

    #[tokio::test]
    async fn constrain_fit_preserves_aspect_ratio() {
        // Non-square source: 64×32.  Constrain to a 32×32 box.
        // scale = min(32/64, 32/32) = min(0.5, 1.0) = 0.5 → output 32×16.
        let bytes = test_jpeg(64, 32);
        let params = TransformParams {
            wid: Some(32),
            hei: Some(32),
            fit: Some("constrain".to_string()),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params).await.unwrap();
        assert_eq!(mime, "image/jpeg");
        let (w, h) = image_dims(&out);
        assert_eq!(w, 32, "constrain: width must not exceed requested wid");
        assert_eq!(
            h, 16,
            "constrain: height must preserve aspect ratio (32×16)"
        );
    }

    #[tokio::test]
    async fn stretch_fit_exact_dimensions() {
        // fit=stretch must produce exactly the requested dimensions regardless of aspect ratio.
        let bytes = test_jpeg(64, 64);
        let params = TransformParams {
            wid: Some(20),
            hei: Some(40),
            fit: Some("stretch".to_string()),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params).await.unwrap();
        assert_eq!(mime, "image/jpeg");
        let (w, h) = image_dims(&out);
        assert_eq!(w, 20, "stretch fit: width must equal requested wid");
        assert_eq!(h, 40, "stretch fit: height must equal requested hei");
    }

    #[tokio::test]
    async fn pre_crop_and_rotate_90() {
        // Pre-crop extracts a 32×32 region; rotating a square by 90° keeps 32×32.
        let bytes = test_jpeg(64, 64);
        let params = TransformParams {
            crop: Some("0,0,32,32".to_string()),
            rotate: Some(90),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params).await.unwrap();
        assert_eq!(mime, "image/jpeg");
        let (w, h) = image_dims(&out);
        assert_eq!(
            (w, h),
            (32, 32),
            "32×32 cropped region rotated 90° must stay 32×32"
        );
    }

    #[tokio::test]
    async fn flip_both_axes() {
        // Flipping does not change dimensions.
        let bytes = test_jpeg(64, 64);
        let params = TransformParams {
            flip: Some("hv".to_string()),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params).await.unwrap();
        assert_eq!(mime, "image/jpeg");
        let (w, h) = image_dims(&out);
        assert_eq!((w, h), (64, 64), "flip must not change image dimensions");
    }

    #[tokio::test]
    async fn avif_encode() {
        let bytes = test_jpeg(64, 64);
        let params = TransformParams {
            wid: Some(32),
            fmt: Some("avif".to_string()),
            qlt: Some(60),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params).await.unwrap();
        assert_eq!(mime, "image/avif");
        let (w, h) = image_dims(&out);
        assert_eq!(
            (w, h),
            (32, 32),
            "avif output must be constrained to requested width"
        );
    }
}

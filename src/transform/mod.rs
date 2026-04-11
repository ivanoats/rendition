//! Image transformation pipeline.
//!
//! This module houses the core media processing logic — resizing, cropping,
//! format conversion, quality adjustment.  Built on libvips for fast,
//! memory-efficient processing of large images.

use anyhow::Context;
use libvips::{
    ops::{
        self, ForeignHeifCompression, HeifsaveBufferOptions, JpegsaveBufferOptions,
        ResizeOptions, WebpsaveBufferOptions,
    },
    VipsApp, VipsImage,
};
#[cfg(test)]
use libvips::ops::BlackOptions;
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
fn ensure_vips() {
    VIPS_APP.get_or_init(|| {
        VipsApp::new("rendition", false).expect("Cannot initialize libvips")
    });
}

// ---- Public API ------------------------------------------------------------

/// Apply `params` to the raw bytes of a source image.
///
/// `original_content_type` is the MIME type of the source asset (e.g.
/// `"image/png"`).  It is used as the default output format when the caller
/// does not supply an explicit `fmt` parameter.
///
/// Returns the transformed image bytes and the MIME type of the output format.
///
/// # Errors
/// Propagates image-processing errors via [`anyhow::Error`].
pub async fn apply(
    source: Vec<u8>,
    params: TransformParams,
    original_content_type: &str,
) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let original_content_type = original_content_type.to_owned();
    tokio::task::spawn_blocking(move || apply_blocking(source, params, &original_content_type))
        .await
        .context("transform task panicked")?
}

// ---- Core (synchronous) transform logic ------------------------------------

fn apply_blocking(
    source: Vec<u8>,
    params: TransformParams,
    original_content_type: &str,
) -> anyhow::Result<(Vec<u8>, &'static str)> {
    ensure_vips();

    let image = VipsImage::new_from_buffer(&source, "")
        .context("failed to decode source image")?;

    let image = apply_crop(image, &params)?;
    let image = apply_resize(image, &params)?;
    let image = apply_rotation(image, &params)?;
    let image = apply_flip(image, &params)?;
    encode(image, &params, original_content_type)
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

    let orig_w = image.get_width() as f64;
    let orig_h = image.get_height() as f64;
    let fit = params.fit.as_deref().unwrap_or("constrain");

    // Scale factors: only computed for dimensions that are explicitly provided.
    let sx = params.wid.map(|w| w as f64 / orig_w);
    let sy = params.hei.map(|h| h as f64 / orig_h);

    let (hscale, vscale) = match fit {
        // Fill the box and crop — scale up to the larger factor.
        // When only one dimension is given, scale uniformly by that factor.
        "crop" => {
            let s = match (sx, sy) {
                (Some(x), Some(y)) => x.max(y),
                (Some(x), None) | (None, Some(x)) => x,
                (None, None) => return Ok(image),
            };
            (s, s)
        }
        // Stretch each axis independently.
        // When a dimension is absent, do not scale that axis (factor = 1.0).
        "stretch" | "fill" => (sx.unwrap_or(1.0), sy.unwrap_or(1.0)),
        // Constrain / fit within the box — scale by the smaller factor.
        // When only one dimension is given, scale uniformly by that factor.
        _ => {
            let s = match (sx, sy) {
                (Some(x), Some(y)) => x.min(y),
                (Some(x), None) | (None, Some(x)) => x,
                (None, None) => return Ok(image),
            };
            (s, s)
        }
    };

    // ops::resize takes hscale and an optional vscale (defaults to hscale).
    let resized = if (hscale - vscale).abs() < f64::EPSILON {
        ops::resize(&image, hscale).context("resize failed")?
    } else {
        ops::resize_with_opts(&image, hscale, &ResizeOptions { vscale, ..Default::default() })
            .context("resize failed")?
    };

    // For crop mode, extract the centre to the exact requested dimensions.
    if fit == "crop" {
        let rw = resized.get_width();
        let rh = resized.get_height();
        let tw = params.wid.map(|w| (w as i32).min(rw)).unwrap_or(rw);
        let th = params.hei.map(|h| (h as i32).min(rh)).unwrap_or(rh);
        let x = (rw - tw) / 2;
        let y = (rh - th) / 2;
        ops::extract_area(&resized, x, y, tw, th).context("centre-crop failed")
    } else {
        Ok(resized)
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
            let h = ops::flip(&image, ops::Direction::Horizontal)
                .context("flip horizontal failed")?;
            ops::flip(&h, ops::Direction::Vertical).context("flip vertical failed")
        }
        _ => Ok(image),
    }
}

/// Map a MIME type to the short format name used by the `fmt` query param.
fn mime_to_fmt(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/avif" => "avif",
        _ => "jpeg",
    }
}

fn encode(
    image: VipsImage,
    params: &TransformParams,
    original_content_type: &str,
) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let quality = params.qlt.unwrap_or(85) as i32;
    let default_fmt = mime_to_fmt(original_content_type);

    match params.fmt.as_deref().unwrap_or(default_fmt) {
        "webp" => {
            let bytes = ops::webpsave_buffer_with_opts(
                &image,
                &WebpsaveBufferOptions {
                    q: quality,
                    ..Default::default()
                },
            )
            .context("webp encode failed")?;
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

// ---- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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

    // Helper: create a small 3-band (RGB) JPEG in memory using libvips.
    fn make_test_jpeg(w: i32, h: i32) -> Vec<u8> {
        ensure_vips();
        let image = ops::black_with_opts(w, h, &BlackOptions { bands: 3 })
            .expect("failed to create test image");
        ops::jpegsave_buffer(&image).expect("failed to encode test image as JPEG")
    }

    // The tests below require libvips to be installed on the system.
    // Run with: cargo test -- --include-ignored

    #[tokio::test]
    #[ignore = "requires libvips installed"]
    async fn passthrough_returns_jpeg() {
        let bytes = make_test_jpeg(64, 64);
        let (out, mime) = apply(bytes, TransformParams::default(), "image/jpeg")
            .await
            .unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(!out.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires libvips installed"]
    async fn resize_width_only() {
        let bytes = make_test_jpeg(64, 64);
        let params = TransformParams {
            wid: Some(32),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(!out.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires libvips installed"]
    async fn resize_to_webp_with_quality() {
        let bytes = make_test_jpeg(64, 64);
        let params = TransformParams {
            wid: Some(32),
            hei: Some(32),
            fmt: Some("webp".to_string()),
            qlt: Some(80),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
        assert_eq!(mime, "image/webp");
        assert!(!out.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires libvips installed"]
    async fn crop_fit_fills_target_box() {
        let bytes = make_test_jpeg(64, 64);
        let params = TransformParams {
            wid: Some(20),
            hei: Some(40),
            fit: Some("crop".to_string()),
            fmt: Some("png".to_string()),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
        assert_eq!(mime, "image/png");
        assert!(!out.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires libvips installed"]
    async fn pre_crop_and_rotate_90() {
        let bytes = make_test_jpeg(64, 64);
        let params = TransformParams {
            crop: Some("0,0,32,32".to_string()),
            rotate: Some(90),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(!out.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires libvips installed"]
    async fn flip_both_axes() {
        let bytes = make_test_jpeg(64, 64);
        let params = TransformParams {
            flip: Some("hv".to_string()),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(!out.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires libvips installed"]
    async fn avif_encode() {
        let bytes = make_test_jpeg(64, 64);
        let params = TransformParams {
            wid: Some(32),
            fmt: Some("avif".to_string()),
            qlt: Some(60),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
        assert_eq!(mime, "image/avif");
        assert!(!out.is_empty());
    }
}

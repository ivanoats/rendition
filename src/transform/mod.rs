//! Image transformation pipeline.
//!
//! This module houses the core media processing logic — resizing, cropping,
//! format conversion, quality adjustment.  Built on libvips for fast,
//! memory-efficient processing of large images.

use anyhow::{anyhow, Context};
#[cfg(test)]
use libvips::ops::BlackOptions;
use libvips::{
    ops::{
        self, ForeignHeifCompression, HeifsaveBufferOptions, JpegsaveBufferOptions, ResizeOptions,
    },
    VipsApp, VipsImage,
};
use std::ffi::CString;
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

// ---- Format capability detection -------------------------------------------
//
// libvips can be built with or without optional savers (HEIF/AVIF, JXL, etc.).
// Calling a saver that is not registered causes the underlying C function to
// return NULL, and the `libvips` crate (1.7.3) constructs a `Vec` from that
// NULL pointer without checking — triggering an unrecoverable, non-unwinding
// `SIGABRT` via Rust's UB checks.
//
// To avoid this we probe support before invoking the saver. The probe uses
// `vips_foreign_find_save_buffer`, which is a pure capability check: it
// returns the saver function name (non-null) if a saver exists for the
// given suffix, or NULL otherwise. It does not invoke the saver itself,
// so it cannot trigger the `new_byte_array` bug.

/// Returns `true` if libvips on this host can save AVIF images.
///
/// The result is cached after the first call.
pub fn avif_supported() -> bool {
    static AVIF: OnceLock<bool> = OnceLock::new();
    *AVIF.get_or_init(|| save_buffer_supported(".avif"))
}

/// Returns `true` if libvips on this host can save WebP images.
///
/// The result is cached after the first call.
pub fn webp_supported() -> bool {
    static WEBP: OnceLock<bool> = OnceLock::new();
    *WEBP.get_or_init(|| save_buffer_supported(".webp"))
}

fn save_buffer_supported(suffix: &str) -> bool {
    ensure_vips();
    let Ok(c_suffix) = CString::new(suffix) else {
        return false;
    };
    // SAFETY: `vips_foreign_find_save_buffer` is a pure capability lookup.
    // It accepts a null-terminated suffix and returns either a pointer to
    // a static C string (the saver function name) or NULL. We never
    // dereference the returned pointer; we only check whether it is null.
    let ptr = unsafe { libvips::bindings::vips_foreign_find_save_buffer(c_suffix.as_ptr()) };
    !ptr.is_null()
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

    let image = VipsImage::new_from_buffer(&source, "").context("failed to decode source image")?;

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
    let orig_w = image.get_width() as f64;
    let orig_h = image.get_height() as f64;

    match (params.wid, params.hei) {
        (None, None) => Ok(image),
        (Some(w), None) => {
            let scale = w as f64 / orig_w;
            ops::resize(&image, scale).context("resize (width-only) failed")
        }
        (None, Some(h)) => {
            let scale = h as f64 / orig_h;
            ops::resize(&image, scale).context("resize (height-only) failed")
        }
        (Some(w), Some(h)) => {
            let fit = params.fit.as_deref().unwrap_or("constrain");
            match fit {
                "stretch" => {
                    // resize to exact w, then stretch height
                    let hscale = w as f64 / orig_w;
                    let resized = ops::resize(&image, hscale).context("resize (stretch w) failed")?;
                    let vscale = h as f64 / resized.get_height() as f64;
                    ops::resize_with_opts(
                        &resized,
                        1.0,
                        &ResizeOptions {
                            vscale,
                            ..Default::default()
                        },
                    )
                    .context("resize (stretch h) failed")
                }
                "crop" => {
                    // scale to fill, then centre-crop
                    let scale = f64::max(w as f64 / orig_w, h as f64 / orig_h);
                    let scaled = ops::resize(&image, scale).context("resize (crop scale) failed")?;
                    let x = ((scaled.get_width() - w as i32) / 2).max(0);
                    let y = ((scaled.get_height() - h as i32) / 2).max(0);
                    ops::extract_area(&scaled, x, y, w as i32, h as i32)
                        .context("resize (crop extract) failed")
                }
                "fill" => {
                    // scale to fill (same as crop scale but no cropping)
                    let scale = f64::max(w as f64 / orig_w, h as f64 / orig_h);
                    ops::resize(&image, scale).context("resize (fill) failed")
                }
                _ => {
                    // constrain: fit within box preserving aspect ratio
                    let scale = f64::min(w as f64 / orig_w, h as f64 / orig_h);
                    ops::resize(&image, scale).context("resize (constrain) failed")
                }
            }
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
            if !webp_supported() {
                return Err(anyhow!(
                    "webp encoding is not supported by this libvips build"
                ));
            }
            let bytes = webp_save_buffer(&image, quality).context("webp encode failed")?;
            Ok((bytes, "image/webp"))
        }
        "png" => {
            let bytes = ops::pngsave_buffer(&image).context("png encode failed")?;
            Ok((bytes, "image/png"))
        }
        "avif" => {
            // Critical: probe AVIF support BEFORE calling heifsave_buffer.
            // libvips returns NULL when AVIF support is missing (e.g. no AV1
            // encoder linked into libheif), and the libvips Rust crate then
            // constructs a Vec from a null pointer — triggering an
            // unrecoverable SIGABRT via Rust's UB checks. We must not let
            // the call happen at all in that case.
            if !avif_supported() {
                return Err(anyhow!(
                    "avif encoding is not supported by this libvips build"
                ));
            }
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
        let (out, mime) = apply(bytes, TransformParams::default(), "image/jpeg")
            .await
            .unwrap();
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
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
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
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
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
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
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
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
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
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
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
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
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
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
        assert_eq!(mime, "image/jpeg");
        let (w, h) = image_dims(&out);
        assert_eq!((w, h), (64, 64), "flip must not change image dimensions");
    }

    #[tokio::test]
    async fn avif_encode() {
        if !avif_supported() {
            eprintln!("skipping avif_encode: libvips on this host has no AVIF saver");
            return;
        }
        let bytes = test_jpeg(64, 64);
        let params = TransformParams {
            wid: Some(32),
            fmt: Some("avif".to_string()),
            qlt: Some(60),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
        assert_eq!(mime, "image/avif");
        let (w, h) = image_dims(&out);
        assert_eq!(
            (w, h),
            (32, 32),
            "avif output must be constrained to requested width"
        );
    }

    #[tokio::test]
    async fn fill_fit_mode() {
        // fit=fill scales to cover the box without cropping.
        // Source 64×32, target 20×40: scale = max(20/64, 40/32) = max(0.3125, 1.25) = 1.25
        // Output: 80×40 (wider than requested, no crop).
        let bytes = test_jpeg(64, 32);
        let params = TransformParams {
            wid: Some(20),
            hei: Some(40),
            fit: Some("fill".to_string()),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
        assert_eq!(mime, "image/jpeg");
        let (w, h) = image_dims(&out);
        assert_eq!(h, 40, "fill fit: height must equal requested hei");
        assert!(w >= 20, "fill fit: width must be at least the requested wid");
    }

    #[tokio::test]
    async fn stretch_fit_mode() {
        // fit=stretch must produce exactly the requested dimensions regardless of aspect ratio.
        let bytes = test_jpeg(64, 32);
        let params = TransformParams {
            wid: Some(20),
            hei: Some(40),
            fit: Some("stretch".to_string()),
            ..Default::default()
        };
        let (out, mime) = apply(bytes, params, "image/jpeg").await.unwrap();
        assert_eq!(mime, "image/jpeg");
        let (w, h) = image_dims(&out);
        assert_eq!(w, 20, "stretch fit: width must equal requested wid");
        assert_eq!(h, 40, "stretch fit: height must equal requested hei");
    }

    #[tokio::test]
    async fn avif_encode_returns_error_when_unsupported() {
        // Inverse of avif_encode: when libvips lacks AVIF support, the
        // encode call must return a typed error rather than letting
        // libvips's heifsave_buffer return NULL and trigger an unrecoverable
        // SIGABRT inside the libvips Rust crate's `new_byte_array`.
        if avif_supported() {
            eprintln!("skipping unsupported-avif test: libvips on this host has an AVIF saver");
            return;
        }
        let bytes = test_jpeg(32, 32);
        let params = TransformParams {
            fmt: Some("avif".to_string()),
            ..Default::default()
        };
        let err = apply(bytes, params, "image/jpeg").await.unwrap_err();
        assert!(
            err.to_string().contains("avif encoding is not supported"),
            "expected avif unsupported error, got: {err}"
        );
    }
}

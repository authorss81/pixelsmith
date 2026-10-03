//! Memory-safe local image processing engine.
//!
//! Design rules, in priority order:
//! 1. **No network I/O.** This crate has no HTTP/TLS dependency and cannot phone home.
//! 2. **No panics on untrusted input.** Every decode path returns [`Error`].
//! 3. **Bounded memory.** Decode limits and pixel caps are enforced before allocation grows.
//! 4. **Predictable pipeline.** Transform order is fixed: crop -> orient -> resize.
//!    See [`pipeline`].

pub mod error;
pub mod exif;
pub mod ffi;
pub mod ffi_abi;
pub mod format;
pub mod pipeline;
pub mod presets;
pub mod target;
pub mod validate;
pub mod worker;

pub use error::{Error, Result};
pub use format::{OutputFormat, detect_format};
pub use pipeline::{CropSpec, FitMode, Orientation, Pipeline, ResampleFilter, ResizeSpec};
pub use presets::{Preset, all_presets, find_preset};
pub use target::TargetBytes;
pub use validate::{Limits, ValidateReport, inspect, validate_bytes};
pub use worker::{BatchReport, CancelToken, Job, Outcome, Settings, process_batch, process_one};

/// Crate version, surfaced through the FFI so the UI can prove which engine it
/// loaded.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Compile-time summary of optional codecs, so the UI can grey out a format it
/// cannot actually write instead of failing at export time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capabilities {
    pub jpeg: bool,
    pub png: bool,
    pub webp_lossy: bool,
    pub webp_lossless: bool,
    pub avif_encode: bool,
    pub gif: bool,
    pub tiff: bool,
    pub bmp: bool,
    pub ico: bool,
    pub max_input_bytes: u64,
    pub max_pixels: u64,
}

pub fn capabilities() -> Capabilities {
    let limits = Limits::default();
    Capabilities {
        jpeg: true,
        png: true,
        webp_lossy: cfg!(feature = "webp-lossy"),
        webp_lossless: true,
        // A constant, not a `cfg!`: there is no `avif` feature, because the
        // `ravif`-based encoder was deleted rather than shipped broken.
        // `OutputFormat::Avif` still exists so AVIF *input* is recognised, and
        // `format::encode` rejects it explicitly. phase-07 adds the encoder and
        // turns this into a real flag.
        avif_encode: false,
        gif: true,
        tiff: true,
        bmp: true,
        ico: true,
        max_input_bytes: limits.max_input_bytes as u64,
        max_pixels: limits.max_pixels,
    }
}

/// Rejection sampling for "downscale until the file fits this many bytes".
///
/// Binary search beats a linear quality ramp because it converges in ~7 encoder
/// passes instead of ~40, and it never returns an image that is *smaller* than
/// necessary for the same visual quality: we always keep the highest quality
/// whose output still fits.
///
/// Returns `(encoded_bytes, quality_used, target_met)`. `target_met` is false
/// when even the floor quality overshoots, so the caller can warn rather than
/// silently hand back something oversized.
pub fn encode_to_target(
    img: &image::DynamicImage,
    format: OutputFormat,
    target: TargetBytes,
    encoder: target::Encoder<'_>,
) -> Result<(Vec<u8>, u8, bool)> {
    target.encode_with(img, format, encoder)
}

/// Run the whole pipeline and encode once at a fixed quality.
pub fn process(
    input: &[u8],
    pipeline: &Pipeline,
    format: OutputFormat,
    quality: u8,
    limits: &Limits,
) -> Result<Vec<u8>> {
    let decoded = decode_bounded(input, limits)?;
    let out = pipeline.apply(&decoded)?;
    let stripped = if pipeline.strip_metadata {
        crate::exif::strip(&out)?
    } else {
        out
    };
    encode_fixed(&stripped, format, quality)
}

/// Decode with all limits enforced.
pub fn decode_bounded(input: &[u8], limits: &Limits) -> Result<image::DynamicImage> {
    validate::validate_bytes(input, limits)?;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(input)).with_guessed_format()?;
    limits.apply_to_decoder(&mut reader);
    let img = reader.decode()?;
    limits.check_decoded(&img)?;
    Ok(img)
}

/// Encode at a fixed quality (no search).
pub fn encode_fixed(
    img: &image::DynamicImage,
    format: OutputFormat,
    quality: u8,
) -> Result<Vec<u8>> {
    format::encode(img, format, quality)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_match_the_enabled_codecs() {
        let caps = capabilities();
        assert!(caps.jpeg && caps.png && caps.webp_lossless);
        assert_eq!(caps.webp_lossy, cfg!(feature = "webp-lossy"));
        assert!(caps.max_pixels > 0 && caps.max_input_bytes > 0);
    }

    #[test]
    fn process_runs_the_whole_chain() {
        let src = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(800, 600, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 10])
        }));
        let input = encode_fixed(&src, OutputFormat::Jpeg, 95).unwrap();
        let pipeline = Pipeline::new().with_resize(ResizeSpec {
            width: Some(400),
            height: None,
            fit: FitMode::Width,
            ..Default::default()
        });
        let out = process(
            &input,
            &pipeline,
            OutputFormat::Jpeg,
            80,
            &Limits::default(),
        )
        .unwrap();
        let decoded = image::load_from_memory(&out).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (400, 300));
    }

    #[test]
    fn process_rejects_a_hostile_input() {
        let err = process(
            &[0u8; 1024],
            &Pipeline::new(),
            OutputFormat::Jpeg,
            80,
            &Limits::default(),
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}

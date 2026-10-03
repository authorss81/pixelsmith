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
pub mod heic;
pub mod pipeline;
pub mod presets;
pub mod target;
pub mod validate;
pub mod worker;

pub use error::{Error, Result};
pub use format::{ChromaSubsampling, EncodingOptions, OutputFormat, detect_format};
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
    /// Whether AVIF can be *read*. Separate from `avif_encode` on purpose, and
    /// currently false in every build: this tree has no AV1 decoder (`image`'s
    /// dav1d path is not enabled and `heic-rs` decodes HEVC only), so a UI that
    /// wrote an AVIF cannot preview it. One boolean for both directions would
    /// have told a user we could open the file we just produced.
    pub avif_decode: bool,
    /// Whether HEVC-coded HEIC/HEIF can be *read*. Separate from the encode
    /// flags above because the two directions answer different questions, and a
    /// user with an iPhone photo only ever asks the first one.
    pub heic_decode: bool,
    /// Whether the JPEG encoder writes progressive scans, and whether it honours
    /// a chroma resolution. Both are read off the format rather than written as
    /// constants, so the flag cannot drift from what `format::encode` does.
    pub jpeg_progressive: bool,
    pub jpeg_chroma_subsampling: bool,
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
        // A `cfg!`, not a constant: `format::encode` dispatches on exactly the
        // same feature, so the flag cannot promise an encoder this build has not
        // got. It is on by default — see the reasoning in `Cargo.toml`.
        avif_encode: cfg!(feature = "avif"),
        // Not a `cfg!` because there is nothing to configure: no AV1 decoder is
        // in the tree in any configuration. Stated rather than omitted so a UI
        // cannot infer "writable implies readable".
        avif_decode: false,
        // A `cfg!`, not a constant: HEIC decode is real but opt-in until
        // phase-08 has built it for every shipped target. A `cfg!` rather than a
        // probe of the codec, so the flag cannot lie about a decoder that is
        // present but broken.
        heic_decode: cfg!(feature = "heic"),
        jpeg_progressive: OutputFormat::Jpeg.supports_progressive(),
        jpeg_chroma_subsampling: OutputFormat::Jpeg.supports_chroma_subsampling(),
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
    encoding: EncodingOptions,
    encoder: target::Encoder<'_>,
) -> Result<(Vec<u8>, u8, bool)> {
    target.encode_with(img, format, encoding, encoder)
}

/// Run the whole pipeline and encode once at a fixed quality.
pub fn process(
    input: &[u8],
    pipeline: &Pipeline,
    format: OutputFormat,
    encoding: EncodingOptions,
    limits: &Limits,
) -> Result<Vec<u8>> {
    let decoded = decode_bounded(input, limits)?;
    let out = pipeline.apply(&decoded)?;
    let stripped = if pipeline.strip_metadata {
        crate::exif::strip(&out)?
    } else {
        out
    };
    // Same precedence as `worker::process_one`: the pipeline describes the
    // picture, so it owns the chroma decision.
    encode_fixed(
        &stripped,
        format,
        encoding.with_chroma(pipeline.chroma_subsampling),
    )
}

/// Decode with all limits enforced.
pub fn decode_bounded(input: &[u8], limits: &Limits) -> Result<image::DynamicImage> {
    let format = validate::validate_bytes(input, limits)?.format;
    let img = match format {
        // A HEIF file's geometry is in a container box, so the decoder that
        // understands the container is also the one that enforces the limits.
        // Routing by the format `detect_format` already reported keeps this from
        // parsing the file twice.
        OutputFormat::Heic | OutputFormat::Heif => heic::decode(input, limits)?,
        _ => {
            let mut reader =
                image::ImageReader::new(std::io::Cursor::new(input)).with_guessed_format()?;
            limits.apply_to_decoder(&mut reader);
            let img = reader.decode()?;
            limits.check_decoded(&img)?;
            img
        }
    };
    Ok(img)
}

/// Encode at fixed options (no search).
pub fn encode_fixed(
    img: &image::DynamicImage,
    format: OutputFormat,
    encoding: EncodingOptions,
) -> Result<Vec<u8>> {
    format::encode(img, format, encoding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_match_the_enabled_codecs() {
        let caps = capabilities();
        assert!(caps.jpeg && caps.png && caps.webp_lossless);
        assert_eq!(caps.webp_lossy, cfg!(feature = "webp-lossy"));
        assert_eq!(caps.heic_decode, cfg!(feature = "heic"));
        assert_eq!(caps.avif_encode, cfg!(feature = "avif"));
        assert!(caps.max_pixels > 0 && caps.max_input_bytes > 0);
    }

    #[test]
    fn capabilities_do_not_claim_an_av1_decoder() {
        // Writable and readable are different questions, and this tree answers
        // only the first. A UI that inferred "we can write AVIF, therefore we can
        // open one" would offer to preview a file it cannot decode.
        let caps = capabilities();
        assert!(!caps.avif_decode);
        assert!(
            !(caps.avif_encode && !caps.avif_decode) || !caps.avif_encode,
            "if this ever gains a decoder, this test is the place to notice"
        );
    }

    #[test]
    fn the_jpeg_capabilities_are_read_off_the_format() {
        // Not constants: derived from the same predicates the UI reads per
        // format, so a change to what the encoder can do cannot leave the
        // capability list describing the old build.
        let caps = capabilities();
        assert_eq!(
            caps.jpeg_progressive,
            OutputFormat::Jpeg.supports_progressive()
        );
        assert_eq!(
            caps.jpeg_chroma_subsampling,
            OutputFormat::Jpeg.supports_chroma_subsampling()
        );
        assert!(caps.jpeg_progressive && caps.jpeg_chroma_subsampling);
    }

    #[test]
    fn process_runs_the_whole_chain() {
        let src = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(800, 600, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 10])
        }));
        let input = encode_fixed(
            &src,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(95),
        )
        .unwrap();
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
            EncodingOptions::default().with_quality(80),
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
            EncodingOptions::default().with_quality(80),
            &Limits::default(),
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}

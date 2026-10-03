use crate::error::{Error, Result};
use image::ImageEncoder;
use serde::{Deserialize, Serialize};

/// Formats we can write. Reading is broader than writing, which is normal:
/// every decoder can read what its encoder emits, not the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Jpeg,
    Png,
    WebP,
    Gif,
    Tiff,
    Bmp,
    Ico,
    /// Recognised on input. Encoding needs `ravif`, which is a heavy build and
    /// is therefore opt-in via the `avif` feature.
    Avif,
}

impl OutputFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
            Self::Gif => "gif",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Ico => "ico",
            Self::Avif => "avif",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::WebP => "image/webp",
            Self::Gif => "image/gif",
            Self::Tiff => "image/tiff",
            Self::Bmp => "image/bmp",
            Self::Ico => "image/x-icon",
            Self::Avif => "image/avif",
        }
    }

    /// True when the quality knob is ignored, so the UI can hide it.
    pub fn is_lossless(self) -> bool {
        match self {
            Self::Jpeg => false,
            Self::WebP => !lossy_webp_enabled(),
            // GIF is palette-quantised, and the `image` encoder emits a single
            // full-colour frame, so quality has no meaningful effect.
            Self::Gif | Self::Png | Self::Bmp | Self::Tiff | Self::Ico | Self::Avif => true,
        }
    }

    pub fn supports_alpha(self) -> bool {
        matches!(
            self,
            Self::Png | Self::WebP | Self::Gif | Self::Ico | Self::Avif
        )
    }

    pub fn supports_animation(self) -> bool {
        matches!(self, Self::Gif)
    }

    pub fn accepts_multiple_frames(self) -> bool {
        matches!(self, Self::Gif)
    }

    /// Only meaningful for formats whose encoder honours a quality value.
    pub fn supports_quality(self) -> bool {
        !self.is_lossless()
    }

    /// Whether a byte ceiling can be enforced for this format at all.
    pub fn supports_byte_target(self) -> bool {
        self.supports_quality()
    }

    /// Maps a file extension to a format. Only trusted after [`detect_format`]
    /// confirms the magic bytes agree.
    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "jpe" => Self::Jpeg,
            "png" => Self::Png,
            "webp" => Self::WebP,
            "gif" => Self::Gif,
            "tif" | "tiff" => Self::Tiff,
            "bmp" => Self::Bmp,
            "ico" => Self::Ico,
            "avif" => Self::Avif,
            _ => return None,
        })
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Jpeg,
            Self::WebP,
            Self::Png,
            Self::Gif,
            Self::Tiff,
            Self::Bmp,
            Self::Ico,
            Self::Avif,
        ]
    }
}

/// Lossy WebP needs libwebp. When the `webp-lossy` feature is off we fall back
/// to the pure-Rust lossless encoder, which still beats PNG for photographs but
/// cannot be quality-controlled, so the capability flags above reflect it.
#[cfg(feature = "webp-lossy")]
const fn lossy_webp_enabled() -> bool {
    true
}
#[cfg(not(feature = "webp-lossy"))]
const fn lossy_webp_enabled() -> bool {
    false
}

/// Identify the format from magic bytes only.
///
/// This is the security-relevant check: the UI is handed a filename from an
/// untrusted source, and trusting the extension would let `payload.png` through
/// as a JPEG. `image` does its own sniffing during decode, so this is about
/// giving the *caller* an answer it can act on.
pub fn detect_format(input: &[u8]) -> Result<OutputFormat> {
    use image::ImageFormat as F;
    let format = image::guess_format(input).map_err(|_| Error::UnknownFormat)?;
    Ok(match format {
        F::Jpeg => OutputFormat::Jpeg,
        F::Png => OutputFormat::Png,
        F::WebP => OutputFormat::WebP,
        F::Gif => OutputFormat::Gif,
        F::Tiff => OutputFormat::Tiff,
        F::Bmp => OutputFormat::Bmp,
        F::Ico => OutputFormat::Ico,
        F::Avif => OutputFormat::Avif,
        _ => return Err(Error::UnknownFormat),
    })
}

/// Encode a still image.
pub fn encode(img: &image::DynamicImage, format: OutputFormat, quality: u8) -> Result<Vec<u8>> {
    let quality = quality.clamp(1, 100);
    let mut out: Vec<u8> = Vec::with_capacity(estimate_capacity(img));
    let mut cursor = std::io::Cursor::new(&mut out);

    match format {
        OutputFormat::Jpeg => {
            // Flatten alpha onto white. JPEG has no alpha channel, and leaving
            // it unset turns every transparent PNG into a black rectangle.
            let flat = flatten_alpha(img);
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality).write_image(
                flat.as_raw(),
                flat.width(),
                flat.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
        OutputFormat::Png => {
            // Best compression + adaptive filtering. Larger encode time, smaller
            // file, and no readability cost on our side.
            image::codecs::png::PngEncoder::new_with_quality(
                &mut cursor,
                image::codecs::png::CompressionType::Best,
                image::codecs::png::FilterType::Adaptive,
            )
            .write_image(
                img.as_bytes(),
                img.width(),
                img.height(),
                img.color().into(),
            )?;
        }
        OutputFormat::WebP => encode_webp(img, &mut cursor, quality)?,
        OutputFormat::Gif => {
            image::codecs::gif::GifEncoder::new(&mut cursor)
                .encode_frame(image::Frame::new(img.to_rgba8()))?;
        }
        OutputFormat::Tiff => {
            image::codecs::tiff::TiffEncoder::new(&mut cursor).write_image(
                img.as_bytes(),
                img.width(),
                img.height(),
                img.color().into(),
            )?;
        }
        OutputFormat::Bmp => {
            image::codecs::bmp::BmpEncoder::new(&mut cursor).write_image(
                img.as_bytes(),
                img.width(),
                img.height(),
                img.color().into(),
            )?;
        }
        OutputFormat::Ico => {
            let rgba = to_rgba8(img);
            image::codecs::ico::IcoEncoder::new(&mut cursor).write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        OutputFormat::Avif => {
            #[cfg(feature = "avif")]
            {
                let rgba = to_rgba8(img);
                let avif = ravif::AvifEncoder::new()
                    .with_quality(quality)
                    .with_speed(8)
                    .write_rgb8(
                        &rgba,
                        ravif::ColorSpace::Srgb,
                        ravif::ChromaSubsampling::A420,
                    )
                    .map_err(Error::Encode)?;
                out.clear();
                out.extend_from_slice(&avif);
            }
            #[cfg(not(feature = "avif"))]
            return Err(Error::UnknownFormat);
        }
    }

    Ok(out)
}

fn encode_webp(
    img: &image::DynamicImage,
    cursor: &mut std::io::Cursor<&mut Vec<u8>>,
    quality: u8,
) -> Result<()> {
    let rgba = to_rgba8(img);
    #[cfg(feature = "webp-lossy")]
    {
        use std::io::Write;
        if img.color().has_alpha() {
            let encoded = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
                .encode(f32::from(quality));
            cursor.write_all(&encoded)?;
        } else {
            // libwebp wants tightly packed RGB for opaque input; handing it RGBA
            // it does not need costs 25% more before compression even starts.
            let mut packed = Vec::with_capacity(rgba.width() as usize * rgba.height() as usize * 3);
            for px in rgba.pixels() {
                packed.extend_from_slice(&px.0[..3]);
            }
            let encoded = webp::Encoder::from_rgb(&packed, rgba.width(), rgba.height())
                .encode(f32::from(quality));
            cursor.write_all(&encoded)?;
        }
        Ok(())
    }
    #[cfg(not(feature = "webp-lossy"))]
    {
        let _ = quality;
        image::codecs::webp::WebPEncoder::new_lossless(&mut *cursor).write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )?;
        Ok(())
    }
}

/// Compose transparency over white. The only sane choice for JPEG output,
/// because there is no other way to represent alpha.
fn flatten_alpha(img: &image::DynamicImage) -> image::RgbImage {
    match img {
        image::DynamicImage::ImageRgb8(v) => v.clone(),
        other => {
            let rgba = to_rgba8(other);
            let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
            for (dst, src) in rgb.pixels_mut().zip(rgba.pixels()) {
                let alpha = f32::from(src.0[3]) / 255.0;
                for c in 0..3 {
                    dst.0[c] = (f32::from(src.0[c]) * alpha + 255.0 * (1.0 - alpha)).round() as u8;
                }
            }
            rgb
        }
    }
}

pub(crate) fn to_rgba8(img: &image::DynamicImage) -> image::RgbaImage {
    img.to_rgba8()
}

/// Pre-size the output buffer so a 20 MP JPEG does not realloc a dozen times.
fn estimate_capacity(img: &image::DynamicImage) -> usize {
    let px = img.width() as usize * img.height() as usize;
    // Empirically ~0.6 bytes/px for q75 JPEG; PNG is worst case 4 bytes/px.
    (px / 2).clamp(64 * 1024, 64 * 1024 * 1024)
}

/// Splice an APP1/Exif segment in directly after the JPEG SOI marker.
///
/// The encoders in `image` cannot write EXIF, and round-tripping through a
/// re-decode would re-filter the image. Writing the segment by hand keeps the
/// compressed scan data byte-identical.
///
/// The payload must begin with the six-byte `Exif\0\0` identifier from
/// [Exif 2.3 section 4.7.2]; readers search for that prefix and will not
/// recognise the segment without it.
pub fn append_exif(jpeg: &mut Vec<u8>, exif_tiff: &[u8]) -> Result<()> {
    /// "Exif\0\0", the APP1 identifier every Exif reader looks for.
    const EXIF_ID: [u8; 6] = [0x45, 0x78, 0x69, 0x66, 0x00, 0x00];

    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return Err(Error::UnknownFormat);
    }
    let payload_len = exif_tiff.len() + EXIF_ID.len();
    let len = u16::try_from(payload_len + 2).map_err(|_| Error::UnknownFormat)?;

    let mut segment = Vec::with_capacity(payload_len + 4);
    segment.extend_from_slice(&[0xFF, 0xE1]);
    segment.extend_from_slice(&len.to_be_bytes());
    segment.extend_from_slice(&EXIF_ID);
    segment.extend_from_slice(exif_tiff);

    // Insert immediately after SOI, before any other marker. If the file
    // already carries an APP1 we simply prepend ours; readers take the first.
    jpeg.splice(2..2, segment);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 5) as u8, 128])
        }))
    }

    #[test]
    fn sniffs_by_magic_bytes_not_extension() {
        for format in [
            OutputFormat::Png,
            OutputFormat::Jpeg,
            OutputFormat::WebP,
            OutputFormat::Gif,
        ] {
            let bytes = encode(&sample(), format, 80).unwrap();
            assert_eq!(detect_format(&bytes).unwrap(), format);
        }
    }

    #[test]
    fn roundtrips_every_writable_format() {
        for format in [
            OutputFormat::Jpeg,
            OutputFormat::Png,
            OutputFormat::WebP,
            OutputFormat::Tiff,
            OutputFormat::Bmp,
            OutputFormat::Gif,
            OutputFormat::Ico,
        ] {
            let bytes = encode(&sample(), format, 80).unwrap();
            assert!(!bytes.is_empty(), "{format:?} produced nothing");
            let back = image::load_from_memory(&bytes).unwrap();
            assert_eq!((back.width(), back.height()), (64, 48), "{format:?} drift");
        }
    }

    #[test]
    fn jpeg_flattens_alpha_onto_white() {
        let transparent = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            16,
            16,
            image::Rgba([0, 0, 0, 0]),
        ));
        let bytes = encode(&transparent, OutputFormat::Jpeg, 90).unwrap();
        let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
        let px = back.get_pixel(8, 8).0;
        assert!(
            px[0] > 240 && px[1] > 240 && px[2] > 240,
            "expected white, got {px:?}"
        );
    }

    #[test]
    fn lossy_quality_monotonically_increases_size() {
        if !OutputFormat::Jpeg.supports_quality() {
            return;
        }
        let small = encode(&sample(), OutputFormat::Jpeg, 10).unwrap().len();
        let large = encode(&sample(), OutputFormat::Jpeg, 95).unwrap().len();
        assert!(large > small, "q95 ({large}) should exceed q10 ({small})");
    }

    #[test]
    fn png_is_lossless() {
        let img = sample();
        let bytes = encode(&img, OutputFormat::Png, 1).unwrap();
        let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(img.to_rgb8().into_raw(), back.into_raw());
    }

    #[test]
    fn extensions_and_mime_types_are_consistent() {
        for format in OutputFormat::all() {
            assert_eq!(
                OutputFormat::from_extension(format.extension()),
                Some(*format)
            );
            assert!(format.mime().starts_with("image/"));
        }
    }

    #[test]
    fn byte_targets_are_only_offered_where_they_work() {
        for format in OutputFormat::all() {
            assert_eq!(
                format.supports_byte_target(),
                format.supports_quality(),
                "{format:?} capability flags disagree"
            );
        }
    }

    #[test]
    fn exif_segment_is_inserted_after_soi() {
        let mut jpeg = encode(&sample(), OutputFormat::Jpeg, 80).unwrap();
        append_exif(&mut jpeg, b"II*\0fake").unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpeg[2..4], &[0xFF, 0xE1]);
        // Still decodes, which is the property that actually matters.
        assert!(image::load_from_memory(&jpeg).is_ok());
    }

    #[test]
    fn exif_append_refuses_non_jpeg() {
        let mut not_jpeg = b"PNG\r\n\x1a\n".to_vec();
        assert!(append_exif(&mut not_jpeg, b"x").is_err());
    }
}

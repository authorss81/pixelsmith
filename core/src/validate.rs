use crate::error::{Error, Result};
use image::AnimationDecoder;

/// Limits applied *before* and *during* decode.
///
/// These exist because the number-one crash in an image tool is a hostile or
/// merely unlucky file: a 60000x60000 PNG header costs ~14 GB once decoded, and
/// a crafted GIF can ask for far more. Both are rejected from the header alone,
/// before any pixel buffer is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Limits {
    /// Largest accepted input file.
    pub max_input_bytes: usize,
    /// Largest accepted decoded pixel count.
    pub max_pixels: u64,
    /// Reject headers whose dimensions exceed this before trusting them at all.
    pub max_dimension: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // 512 MiB is far above any real photo and below anything that hurts.
            max_input_bytes: 512 * 1024 * 1024,
            // ~128 MP covers a 12k x 11k panorama and nothing sane beyond it.
            max_pixels: 128_000_000,
            max_dimension: 30_000,
        }
    }
}

impl Limits {
    /// Tighter profile for phones, where a 128 MP decode would OOM the process.
    pub fn mobile() -> Self {
        Self {
            max_input_bytes: 128 * 1024 * 1024,
            max_pixels: 40_000_000,
            max_dimension: 16_000,
        }
    }

    /// Push decoder-side limits down so we never materialise the whole image.
    ///
    /// `image::Limits` is `#[non_exhaustive]`, so it starts from `Default` and
    /// narrows only the fields we care about.
    pub fn apply_to_decoder<R: std::io::BufRead + std::io::Seek>(
        &self,
        reader: &mut image::ImageReader<R>,
    ) {
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(self.max_dimension);
        limits.max_image_height = Some(self.max_dimension);
        limits.max_alloc = Some(self.max_pixels.saturating_mul(4));
        reader.limits(limits);
    }

    /// Post-decode assertion. Cheap, and catches headers that lied.
    pub fn check_decoded(&self, img: &image::DynamicImage) -> Result<()> {
        let (w, h) = (img.width(), img.height());
        if w == 0 || h == 0 {
            return Err(Error::ZeroDimension);
        }
        if w > self.max_dimension || h > self.max_dimension {
            return Err(Error::SuspiciousDimensions {
                w,
                h,
                mp: megapixels(w, h),
            });
        }
        let pixels = u64::from(w) * u64::from(h);
        if pixels > self.max_pixels {
            return Err(Error::PixelBudgetExceeded {
                limit: self.max_pixels,
                actual: megapixels(w, h),
            });
        }
        Ok(())
    }
}

pub fn megapixels(w: u32, h: u32) -> f64 {
    f64::from(w) * f64::from(h) / 1_000_000.0
}

/// What we learned from a file before trusting it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidateReport {
    pub format: crate::format::OutputFormat,
    pub width: u32,
    pub height: u32,
    pub megapixels: f64,
    pub has_exif: bool,
    pub has_animated: bool,
    pub sensitive_tags: Vec<String>,
    pub orientation: Option<u32>,
    /// Human-readable warning. `None` means the file looks ordinary.
    pub suspicious: Option<String>,
}

/// Inspect bytes without committing to a full decode.
///
/// Returns the report *and* the image, because callers almost always want both
/// and decoding twice would double the peak memory cost.
pub fn inspect(input: &[u8], limits: &Limits) -> Result<(ValidateReport, image::DynamicImage)> {
    let report = validate_bytes(input, limits)?;
    let img = crate::decode_bounded(input, limits)?;
    Ok((report, img))
}

/// Cheap header check. Reads dimensions from the header only, so it is safe to
/// call on thousands of files in a folder before deciding what to process.
pub fn validate_bytes(input: &[u8], limits: &Limits) -> Result<ValidateReport> {
    if input.is_empty() {
        return Err(Error::UnknownFormat);
    }
    if input.len() > limits.max_input_bytes {
        return Err(Error::InputTooLarge {
            limit: limits.max_input_bytes,
            actual: input.len(),
        });
    }
    let format = crate::format::detect_format(input)?;

    let mut reader = image::ImageReader::new(std::io::Cursor::new(input)).with_guessed_format()?;
    limits.apply_to_decoder(&mut reader);
    let (w, h) = reader.into_dimensions()?;

    let suspicious = if w > limits.max_dimension || h > limits.max_dimension {
        Some(format!(
            "{w}x{h} exceeds the {} px per-side limit",
            limits.max_dimension
        ))
    } else if u64::from(w) * u64::from(h) > limits.max_pixels {
        Some(format!(
            "{:.1} MP exceeds the {:.1} MP budget",
            megapixels(w, h),
            limits.max_pixels as f64 / 1_000_000.0
        ))
    } else {
        None
    };

    let exif = crate::exif::read(input).unwrap_or_default();

    Ok(ValidateReport {
        format,
        width: w,
        height: h,
        megapixels: megapixels(w, h),
        has_exif: !exif.entries.is_empty(),
        has_animated: format.supports_animation() && count_frames(input, format) > 1,
        sensitive_tags: exif.sensitive_tags.clone(),
        orientation: exif.orientation,
        suspicious,
    })
}

fn count_frames(input: &[u8], format: crate::format::OutputFormat) -> usize {
    // Only GIF exposes a multi-frame decoder in this build. APNG and animated
    // WebP are reported as single-frame, which means the UI treats them as
    // stills and says so rather than quietly flattening them.
    match format {
        crate::format::OutputFormat::Gif => {
            image::codecs::gif::GifDecoder::new(std::io::Cursor::new(input))
                .map(|d| d.into_frames().count())
                .unwrap_or(1)
        }
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        crate::encode_fixed(
            &image::DynamicImage::ImageRgb8(image::RgbImage::new(w, h)),
            crate::format::OutputFormat::Jpeg,
            80,
        )
        .unwrap()
    }

    #[test]
    fn empty_input_is_rejected() {
        assert!(matches!(
            validate_bytes(&[], &Limits::default()),
            Err(Error::UnknownFormat)
        ));
    }

    #[test]
    fn oversize_input_is_rejected_before_parsing() {
        let limits = Limits {
            max_input_bytes: 16,
            ..Limits::default()
        };
        assert!(matches!(
            validate_bytes(&[0u8; 64], &limits),
            Err(Error::InputTooLarge { .. })
        ));
    }

    #[test]
    fn garbage_bytes_are_rejected() {
        assert!(validate_bytes(b"not an image at all", &Limits::default()).is_err());
    }

    #[test]
    fn a_truncated_jpeg_is_rejected_rather_than_half_decoded() {
        let full = jpeg(64, 64);
        let truncated = &full[..full.len() / 3];
        assert!(validate_bytes(truncated, &Limits::default()).is_err());
    }

    #[test]
    fn zero_dimension_is_rejected() {
        let err = Limits::default()
            .check_decoded(&image::DynamicImage::new_rgb8(0, 10))
            .unwrap_err();
        assert!(matches!(err, Error::ZeroDimension));
    }

    #[test]
    fn pixel_budget_is_enforced() {
        let limits = Limits {
            max_pixels: 1000,
            ..Limits::default()
        };
        let img = image::DynamicImage::new_rgb8(100, 100);
        assert!(matches!(
            limits.check_decoded(&img),
            Err(Error::PixelBudgetExceeded { .. })
        ));
    }

    #[test]
    fn reports_a_healthy_file_as_clean() {
        let report = validate_bytes(&jpeg(120, 90), &Limits::default()).unwrap();
        assert_eq!((report.width, report.height), (120, 90));
        assert_eq!(report.format, crate::format::OutputFormat::Jpeg);
        assert!(report.suspicious.is_none());
        assert!(!report.has_animated);
    }

    #[test]
    fn inspect_returns_the_image_too() {
        let (report, img) = inspect(&jpeg(50, 40), &Limits::default()).unwrap();
        assert_eq!((img.width(), img.height()), (report.width, report.height));
    }

    #[test]
    fn mobile_limits_are_stricter_than_desktop() {
        let (m, d) = (Limits::mobile(), Limits::default());
        assert!(m.max_pixels < d.max_pixels);
        assert!(m.max_dimension < d.max_dimension);
        assert!(m.max_input_bytes < d.max_input_bytes);
    }
}

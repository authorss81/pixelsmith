use crate::error::Result;
use crate::format::{EncodingOptions, OutputFormat, encode};
use serde::{Deserialize, Serialize};

/// Pluggable encoder, so the search algorithm can be tested without pixels.
///
/// Options rather than a bare quality: the search varies only the quality, and
/// the rest of the settings have to survive every pass in the loop unchanged.
pub type Encoder<'a> =
    &'a dyn Fn(&image::DynamicImage, OutputFormat, EncodingOptions) -> Result<Vec<u8>>;

/// "Make this file fit under N bytes."
///
/// This is the single most-requested operation in a resizer and the one every
/// existing tool does badly: they expose a quality slider and make the user
/// guess. A binary search over quality converges in ~7 encodes and returns the
/// *highest* quality that fits, so the result is never needlessly degraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetBytes {
    pub bytes: u64,
    /// Floor for the search. Below ~30 the encoders produce visibly broken
    /// output and going lower trades real artefacts for a few hundred bytes.
    pub min_quality: u8,
    pub max_quality: u8,
}

impl TargetBytes {
    pub fn new(bytes: u64) -> Self {
        Self {
            bytes,
            min_quality: 30,
            max_quality: 95,
        }
    }

    /// Human-facing preset sizes, because typing "3145728" is not a workflow.
    /// `max_bytes` is capped at 2 MiB because most email relays reject more,
    /// and a larger ceiling is not what the user meant.
    pub fn for_email_attachment(bytes: u64) -> Self {
        Self {
            min_quality: 25,
            ..Self::new(bytes.min(2 * 1024 * 1024))
        }
    }

    /// Encode until the file fits `bytes`.
    ///
    /// Returns `(bytes, quality_used, target_met)`. `quality_used` is **0 when
    /// the format has no quality setting** and no search happened — see
    /// [`crate::format::OutputFormat::supports_quality`]. Reporting a number for
    /// a control that did nothing would be a claim the caller then displays.
    pub fn encode_with(
        &self,
        img: &image::DynamicImage,
        format: OutputFormat,
        base: EncodingOptions,
        encoder: Encoder<'_>,
    ) -> Result<(Vec<u8>, u8, bool)> {
        if format.is_lossless() || !format.supports_byte_target() {
            // Lossless output does not respond to quality, so searching is
            // pointless: encode once and report honestly — including reporting
            // that no quality was used, and whether the ceiling was met at all.
            let bytes = encoder(img, format, base.with_quality(self.min_quality))?;
            let len = bytes.len();
            return Ok((bytes, 0, len as u64 <= self.bytes));
        }

        let lo = self.min_quality.clamp(1, 100);
        let hi = self.max_quality.clamp(lo, 100);

        let mut best: Option<(Vec<u8>, u8)> = None;

        // Coarse pass: establish that *something* fits, or learn the floor.
        let mut low = lo;
        let mut high = hi;
        while low <= high {
            let mid = low + (high - low) / 2;
            let out = encoder(img, format, base.with_quality(mid))?;
            let len = out.len() as u64;
            if len <= self.bytes {
                best = Some((out, mid));
                low = mid.saturating_add(1);
            } else {
                if mid == lo {
                    break;
                }
                high = mid - 1;
            }
        }

        match best {
            Some((bytes, q)) => Ok((bytes, q, true)),
            None => {
                // Nothing fit. Return the floor-quality attempt so the caller
                // can show a real file plus a warning, instead of failing.
                let bytes = encoder(img, format, base.with_quality(lo))?;
                Ok((bytes, lo, false))
            }
        }
    }
}

/// Default encoder used by [`TargetBytes`]. Split out so tests can substitute a
/// synthetic encoder and assert the search behaviour without encoding pixels.
pub fn default_encoder(
    img: &image::DynamicImage,
    format: OutputFormat,
    options: EncodingOptions,
) -> Result<Vec<u8>> {
    encode(img, format, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encoder whose output size is a known function of quality, so the search
    /// can be checked exactly. Real encoders grow as quality rises; the fixture
    /// must too, or the search is being tested against the wrong shape.
    fn synthetic(
        size_at: impl Fn(u8) -> usize,
    ) -> impl Fn(&image::DynamicImage, OutputFormat, EncodingOptions) -> Result<Vec<u8>> {
        move |_, _, opts| Ok(vec![0u8; size_at(opts.quality)])
    }

    #[test]
    fn finds_the_highest_quality_that_fits() {
        let img = image::DynamicImage::new_rgb8(8, 8);
        // size = q * 1000, so q=40 fits exactly under 40 000 and q=41 does not.
        let enc = synthetic(|q| q as usize * 1000);
        let t = TargetBytes {
            bytes: 40_000,
            min_quality: 1,
            max_quality: 100,
        };
        let (bytes, q, fits) = t
            .encode_with(&img, OutputFormat::Jpeg, EncodingOptions::default(), &enc)
            .unwrap();
        assert!(fits);
        assert_eq!(q, 40);
        assert_eq!(bytes.len(), 40_000);
    }

    #[test]
    fn reports_failure_when_nothing_fits() {
        let img = image::DynamicImage::new_rgb8(8, 8);
        let enc = synthetic(|_| 5_000);
        let t = TargetBytes {
            bytes: 100,
            min_quality: 40,
            max_quality: 90,
        };
        let (bytes, q, fits) = t
            .encode_with(&img, OutputFormat::Jpeg, EncodingOptions::default(), &enc)
            .unwrap();
        assert!(!fits, "must report that the target was not met");
        assert_eq!(q, 40);
        assert_eq!(bytes.len(), 5_000);
    }

    #[test]
    fn succeeds_when_everything_fits() {
        let img = image::DynamicImage::new_rgb8(8, 8);
        let enc = synthetic(|q| q as usize);
        let t = TargetBytes {
            bytes: 10_000_000,
            min_quality: 30,
            max_quality: 95,
        };
        let (_, q, fits) = t
            .encode_with(&img, OutputFormat::Jpeg, EncodingOptions::default(), &enc)
            .unwrap();
        assert!(fits);
        assert_eq!(q, 95, "should not need to drop quality at all");
    }

    #[test]
    fn lossless_formats_do_not_search() {
        let img = image::DynamicImage::new_rgb8(8, 8);
        let calls = std::cell::Cell::new(0);
        let enc = |_: &image::DynamicImage, _: OutputFormat, _: EncodingOptions| {
            calls.set(calls.get() + 1);
            Ok(vec![0u8; 900])
        };
        let t = TargetBytes {
            bytes: 1000,
            min_quality: 30,
            max_quality: 95,
        };
        let (_, _, fits) = t
            .encode_with(&img, OutputFormat::Png, EncodingOptions::default(), &enc)
            .unwrap();
        assert_eq!(calls.get(), 1, "PNG must be encoded exactly once");
        assert!(fits);
    }

    #[test]
    fn real_jpeg_lands_under_the_target() {
        // A smooth, photo-like gradient. Pure noise is incompressible, so a
        // byte ceiling cannot be met at any quality and this would be testing
        // the impossible rather than the search.
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(600, 400, |x, y| {
            let fx = f64::from(x as u16) / 600.0;
            let fy = f64::from(y as u16) / 400.0;
            let wave = (fx * 6.0).sin() * 90.0 + (fy * 4.0).cos() * 60.0;
            image::Rgb([
                (128.0 + wave).clamp(0.0, 255.0) as u8,
                (90.0 + wave * 0.7).clamp(0.0, 255.0) as u8,
                (160.0 - wave * 0.4).clamp(0.0, 255.0) as u8,
            ])
        }));
        let target = TargetBytes::new(20_000);
        let (bytes, q, fits) = target
            .encode_with(
                &img,
                OutputFormat::Jpeg,
                EncodingOptions::default(),
                &default_encoder,
            )
            .unwrap();
        assert!(fits, "q={q} produced {} bytes", bytes.len());
        assert!(
            bytes.len() as u64 <= 20_000,
            "q={q} produced {} bytes",
            bytes.len()
        );
        assert!((25..=95).contains(&q));
    }

    #[test]
    fn real_jpeg_reports_failure_when_the_target_is_impossible() {
        // White noise at 1200x800 cannot reach 5 KB at any sane quality. The
        // engine must say so rather than pretend.
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(1200, 800, |x, y| {
            image::Rgb([
                (x.wrapping_mul(2654435761) >> 13) as u8,
                (y.wrapping_mul(40503) >> 7) as u8,
                ((x ^ y).wrapping_mul(97)) as u8,
            ])
        }));
        let target = TargetBytes::new(5_000);
        let (bytes, q, fits) = target
            .encode_with(
                &img,
                OutputFormat::Jpeg,
                EncodingOptions::default(),
                &default_encoder,
            )
            .unwrap();
        assert!(!fits, "5 KB is not reachable here");
        assert_eq!(q, target.min_quality);
        assert!(!bytes.is_empty(), "a real file is still returned");
    }

    #[test]
    fn converges_in_few_passes() {
        let img = image::DynamicImage::new_rgb8(64, 64);
        let calls = std::cell::Cell::new(0);
        let enc = |_: &image::DynamicImage, _: OutputFormat, opts: EncodingOptions| {
            calls.set(calls.get() + 1);
            Ok(vec![0u8; opts.quality as usize * 1500])
        };
        let t = TargetBytes {
            bytes: 60_000,
            min_quality: 30,
            max_quality: 95,
        };
        let _ = t
            .encode_with(&img, OutputFormat::Jpeg, EncodingOptions::default(), &enc)
            .unwrap();
        assert!(
            calls.get() <= 9,
            "binary search took {} encodes, expected <= 9",
            calls.get()
        );
    }
}

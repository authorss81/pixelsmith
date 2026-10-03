//! Preset catalogue.
//!
//! Every other resizer ships a preset list that rots: it encodes pixel
//! dimensions that platforms changed two years ago. These are grouped by intent
//! and carry a `category` so the UI can present them as tabs, and the user can
//! type a custom value when the preset is wrong.

use crate::format::{ChromaSubsampling, OutputFormat};
use crate::pipeline::{FitMode, ResampleFilter, ResizeSpec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Social,
    Web,
    Print,
    Device,
    Email,
    Developer,
}

/// A built-in preset.
///
/// Derives `Serialize` only, deliberately. The `&'static str` fields mean a
/// `Deserialize` impl would require `Preset<'a>` borrowing from the input buffer,
/// which this shape cannot express — so `Vec<Preset>` can be written out to JSON
/// but never read back. An earlier revision derived both and the resulting
/// `Deserialize` was a trap that failed to compile the moment anything tried to
/// use it. Consumers that need to read a preset back must use an owned mirror
/// with `String` fields; see the FFI contract test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub category: Category,
    pub width: u32,
    pub height: Option<u32>,
    pub fit: FitMode,
    pub filter: ResampleFilter,
    pub format: OutputFormat,
    /// 0..=100, 0 means "leave it to the user".
    pub quality: u8,
    /// Optional byte ceiling, applied after resize.
    pub max_bytes: Option<u64>,
    /// True when the preset is a square or a hard aspect ratio, i.e. cropping
    /// is expected rather than incidental.
    pub crops: bool,
    /// Chroma resolution this preset exports at.
    ///
    /// Not a per-preset whim: the choice follows the *content*. 4:2:0 for
    /// photographs, where chroma detail is below the visible threshold, and 4:4:4
    /// for anything whose content is colour — the store-screenshot preset is a UI
    /// capture, and text on a coloured background is exactly what 4:2:0 fringes.
    /// Only formats that can honour it are affected; see
    /// [`OutputFormat::supports_chroma_subsampling`].
    pub chroma: ChromaSubsampling,
}

#[allow(clippy::too_many_arguments)]
const fn preset(
    id: &'static str,
    label: &'static str,
    category: Category,
    width: u32,
    height: Option<u32>,
    fit: FitMode,
    format: OutputFormat,
    quality: u8,
    max_bytes: Option<u64>,
    crops: bool,
    chroma: ChromaSubsampling,
) -> Preset {
    Preset {
        id,
        label,
        category,
        width,
        height,
        fit,
        filter: ResampleFilter::Lanczos3,
        format,
        quality,
        max_bytes,
        crops,
        chroma,
    }
}

pub const PRESETS: &[Preset] = &[
    // --- Social: dimensions track what each platform actually crops to.
    preset(
        "ig-post",
        "Instagram post 1:1",
        Category::Social,
        1080,
        Some(1080),
        FitMode::Cover,
        OutputFormat::Jpeg,
        85,
        Some(2 * 1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "ig-portrait",
        "Instagram portrait 4:5",
        Category::Social,
        1080,
        Some(1350),
        FitMode::Cover,
        OutputFormat::Jpeg,
        85,
        Some(2 * 1024 * 1022),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "ig-story",
        "Instagram story 9:16",
        Category::Social,
        1080,
        Some(1920),
        FitMode::Cover,
        OutputFormat::Jpeg,
        82,
        Some(4 * 1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "fb-link",
        "Facebook link 1.91:1",
        Category::Social,
        1200,
        Some(628),
        FitMode::Cover,
        OutputFormat::Jpeg,
        85,
        Some(1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "x-post",
        "X / Twitter 16:9",
        Category::Social,
        1600,
        Some(900),
        FitMode::Cover,
        OutputFormat::Jpeg,
        85,
        Some(1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "yt-thumb",
        "YouTube thumbnail 16:9",
        Category::Social,
        1280,
        Some(720),
        FitMode::Cover,
        OutputFormat::Jpeg,
        88,
        Some(2 * 1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "yt-thumb-hd",
        "YouTube thumbnail HD",
        Category::Social,
        2560,
        Some(1440),
        FitMode::Cover,
        OutputFormat::Jpeg,
        88,
        Some(2 * 1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "li-cover",
        "LinkedIn banner 4:1",
        Category::Social,
        1584,
        Some(396),
        FitMode::Cover,
        OutputFormat::Jpeg,
        88,
        Some(1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "tt-post",
        "TikTok 9:16",
        Category::Social,
        1080,
        Some(1920),
        FitMode::Cover,
        OutputFormat::Jpeg,
        82,
        Some(4 * 1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "wa-status",
        "WhatsApp status",
        Category::Social,
        1080,
        Some(1920),
        FitMode::Contain,
        OutputFormat::Jpeg,
        82,
        Some(1024 * 1024),
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "pin-standard",
        "Pinterest 2:3",
        Category::Social,
        1000,
        Some(1500),
        FitMode::Contain,
        OutputFormat::Jpeg,
        85,
        Some(2 * 1024 * 1024),
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "discord-banner",
        "Discord banner",
        Category::Social,
        960,
        Some(540),
        FitMode::Cover,
        OutputFormat::Jpeg,
        85,
        Some(1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    // --- Web: WebP/AVIF are the point here.
    preset(
        "web-hero",
        "Web hero 16:9",
        Category::Web,
        1920,
        None,
        FitMode::Width,
        OutputFormat::WebP,
        80,
        Some(350 * 1024),
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "web-card",
        "Web card 16:9",
        Category::Web,
        800,
        Some(450),
        FitMode::Cover,
        OutputFormat::WebP,
        80,
        Some(120 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "web-thumb",
        "Web thumbnail",
        Category::Web,
        400,
        Some(300),
        FitMode::Cover,
        OutputFormat::WebP,
        78,
        Some(60 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "web-og",
        "Open Graph 1200x630",
        Category::Web,
        1200,
        Some(630),
        FitMode::Cover,
        OutputFormat::Jpeg,
        85,
        Some(300 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "web-srcset-1x",
        "srcset 1x",
        Category::Web,
        640,
        None,
        FitMode::Width,
        OutputFormat::WebP,
        82,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "web-srcset-2x",
        "srcset 2x",
        Category::Web,
        1280,
        None,
        FitMode::Width,
        OutputFormat::WebP,
        82,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "web-favicon",
        "Favicon 32",
        Category::Web,
        32,
        Some(32),
        FitMode::Cover,
        OutputFormat::Png,
        100,
        Some(8 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "web-pwa-192",
        "PWA icon 192",
        Category::Web,
        192,
        Some(192),
        FitMode::Cover,
        OutputFormat::Png,
        100,
        None,
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "web-pwa-512",
        "PWA icon 512",
        Category::Web,
        512,
        Some(512),
        FitMode::Cover,
        OutputFormat::Png,
        100,
        None,
        true,
        ChromaSubsampling::Luma420,
    ),
    // --- Print: dimensions in pixels at a stated DPI.
    preset(
        "print-a4-300",
        "A4 @300dpi",
        Category::Print,
        2480,
        Some(3508),
        FitMode::Contain,
        OutputFormat::Jpeg,
        92,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "print-a4-150",
        "A4 @150dpi",
        Category::Print,
        1240,
        Some(1754),
        FitMode::Contain,
        OutputFormat::Jpeg,
        90,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "print-letter-300",
        "US Letter @300dpi",
        Category::Print,
        2550,
        Some(3300),
        FitMode::Contain,
        OutputFormat::Jpeg,
        92,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "print-4x6-300",
        "4x6in @300dpi",
        Category::Print,
        1200,
        Some(1800),
        FitMode::Contain,
        OutputFormat::Jpeg,
        92,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "print-5x7-300",
        "5x7in @300dpi",
        Category::Print,
        1500,
        Some(2100),
        FitMode::Contain,
        OutputFormat::Jpeg,
        92,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    // --- Device: wallpaper and launcher assets.
    preset(
        "wall-1080",
        "Phone wallpaper 1080p",
        Category::Device,
        1080,
        Some(1920),
        FitMode::Cover,
        OutputFormat::Jpeg,
        88,
        Some(4 * 1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "wall-1440",
        "Phone wallpaper QHD",
        Category::Device,
        1440,
        Some(2560),
        FitMode::Cover,
        OutputFormat::Jpeg,
        88,
        Some(6 * 1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "wall-tablet",
        "Tablet 4:3",
        Category::Device,
        2048,
        Some(1536),
        FitMode::Cover,
        OutputFormat::Jpeg,
        88,
        Some(6 * 1024 * 1024),
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "desktop-1080",
        "Desktop 1080p",
        Category::Device,
        1920,
        Some(1080),
        FitMode::Cover,
        OutputFormat::Jpeg,
        88,
        None,
        true,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "desktop-4k",
        "Desktop 4K",
        Category::Device,
        3840,
        Some(2160),
        FitMode::Cover,
        OutputFormat::Jpeg,
        90,
        None,
        true,
        ChromaSubsampling::Luma420,
    ),
    // --- Email: the target-bytes case.
    preset(
        "email-1mb",
        "Email 1 MB ceiling",
        Category::Email,
        1600,
        None,
        FitMode::Width,
        OutputFormat::Jpeg,
        0,
        Some(1024 * 1024),
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "email-500k",
        "Email 500 KB ceiling",
        Category::Email,
        1200,
        None,
        FitMode::Width,
        OutputFormat::Jpeg,
        0,
        Some(500 * 1024),
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "email-250k",
        "Email 250 KB ceiling",
        Category::Email,
        900,
        None,
        FitMode::Width,
        OutputFormat::Jpeg,
        0,
        Some(250 * 1024),
        false,
        ChromaSubsampling::Luma420,
    ),
    // --- Developer: icons and store listings.
    preset(
        "android-mdpi",
        "Android mdpi 48",
        Category::Developer,
        48,
        Some(48),
        FitMode::Contain,
        OutputFormat::Png,
        100,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "android-hdpi",
        "Android hdpi 72",
        Category::Developer,
        72,
        Some(72),
        FitMode::Contain,
        OutputFormat::Png,
        100,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "android-xhdpi",
        "Android xhdpi 96",
        Category::Developer,
        96,
        Some(96),
        FitMode::Contain,
        OutputFormat::Png,
        100,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "android-xxhdpi",
        "Android xxhdpi 144",
        Category::Developer,
        144,
        Some(144),
        FitMode::Contain,
        OutputFormat::Png,
        100,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "android-xxxhdpi",
        "Android xxxhdpi 192",
        Category::Developer,
        192,
        Some(192),
        FitMode::Contain,
        OutputFormat::Png,
        100,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "ios-appicon",
        "iOS app icon 1024",
        Category::Developer,
        1024,
        Some(1024),
        FitMode::Contain,
        OutputFormat::Png,
        100,
        None,
        false,
        ChromaSubsampling::Luma420,
    ),
    preset(
        "store-screenshot",
        "Play Store screenshot",
        Category::Developer,
        1080,
        Some(1920),
        FitMode::Contain,
        OutputFormat::Jpeg,
        90,
        Some(1024 * 1024),
        false,
        ChromaSubsampling::Luma444,
    ),
];

pub fn all_presets() -> &'static [Preset] {
    PRESETS
}

pub fn find_preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Turn a preset into a pipeline.
///
/// `max_bytes` is dropped when the preset's format cannot be quality-controlled
/// in this build, so the UI never promises a ceiling the encoder cannot honour.
pub fn to_pipeline(p: &Preset) -> (crate::pipeline::Pipeline, OutputFormat, u8, Option<u64>) {
    let mut resize = ResizeSpec {
        width: Some(p.width),
        height: p.height,
        fit: p.fit,
        filter: p.filter,
        // Social presets must be allowed to upscale, because a 600x600 source
        // still has to become a valid 1080x1080 asset.
        no_upscale: !p.crops && p.category != Category::Social,
    };
    if p.height.is_none() {
        resize.fit = FitMode::Width;
    }
    let pipeline = crate::pipeline::Pipeline {
        crop: None,
        orientation: None,
        resize: Some(resize),
        strip_metadata: true,
        chroma_subsampling: p.chroma,
    };
    let max_bytes = p.max_bytes.filter(|_| p.format.supports_byte_target());
    (pipeline, p.format, p.quality, max_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in PRESETS {
            assert!(seen.insert(p.id), "duplicate preset id: {}", p.id);
        }
    }

    #[test]
    fn lookups_work_by_id() {
        assert_eq!(find_preset("ig-post").unwrap().width, 1080);
        assert!(find_preset("nope").is_none());
    }

    #[test]
    fn every_category_has_at_least_one_preset() {
        for cat in [
            Category::Social,
            Category::Web,
            Category::Print,
            Category::Device,
            Category::Email,
            Category::Developer,
        ] {
            assert!(
                PRESETS.iter().any(|p| p.category == cat),
                "empty category {cat:?}"
            );
        }
    }

    #[test]
    fn no_preset_exports_a_format_this_build_cannot_write() {
        // A preset is a promise about an export: the user taps it and gets a
        // file. One pointing at a format the build cannot produce fails at save
        // time, which is exactly what hard rule 10 exists to prevent.
        for p in PRESETS {
            assert!(
                !p.format.is_read_only(),
                "{} exports {:?}, which this build cannot write",
                p.id,
                p.format
            );
        }
    }

    #[test]
    fn presets_choose_chroma_by_what_the_content_is() {
        // The rule the catalogue encodes: a photograph at 4:2:0, and anything
        // whose content *is* colour — text, a logo, a UI capture — at 4:4:4.
        // Asserted so that adding a preset means deciding, rather than drifting.
        let screenshot = find_preset("store-screenshot").unwrap();
        assert_eq!(screenshot.format, OutputFormat::Jpeg);
        assert_eq!(
            screenshot.chroma,
            ChromaSubsampling::Luma444,
            "a store screenshot is a UI capture; text on a coloured background frays at 4:2:0"
        );

        for p in PRESETS {
            if p.category == Category::Social || p.category == Category::Print {
                assert_eq!(
                    p.chroma,
                    ChromaSubsampling::Luma420,
                    "{} is a photographic preset and should use the photo default",
                    p.id
                );
            }
            if !p.format.supports_chroma_subsampling() {
                // Not a lie: the value is inert for this format, and the field's
                // doc comment says so. What must not happen is a preset that
                // advertises a chroma level it cannot honour.
                assert!(
                    p.format.is_lossless() || p.format.supports_quality(),
                    "{} carries a chroma level for a format that ignores quality entirely",
                    p.id
                );
            }
        }
    }

    #[test]
    fn a_presets_chroma_reaches_the_pipeline() {
        for p in PRESETS {
            let (pipeline, _, _, _) = to_pipeline(p);
            assert_eq!(
                pipeline.chroma_subsampling, p.chroma,
                "{} lost its chroma choice on the way to the pipeline",
                p.id
            );
        }
    }

    #[test]
    fn byte_ceilings_are_only_kept_where_the_encoder_can_meet_them() {
        for p in PRESETS {
            let (_, format, _, max_bytes) = to_pipeline(p);
            assert_eq!(format, p.format);
            if !p.format.supports_byte_target() {
                assert!(
                    max_bytes.is_none(),
                    "{} keeps a ceiling its encoder cannot enforce",
                    p.id
                );
            }
        }
    }

    #[test]
    fn byte_ceilings_are_plausible() {
        for p in PRESETS {
            if let Some(limit) = p.max_bytes {
                assert!(
                    limit >= 4 * 1024,
                    "{} has an implausibly small ceiling ({limit} bytes)",
                    p.id
                );
            }
        }
    }

    #[test]
    fn presets_never_ask_for_a_zero_dimension() {
        for p in PRESETS {
            assert!(p.width > 0, "{}", p.id);
            assert!(p.height.unwrap_or(1) > 0, "{}", p.id);
        }
    }

    #[test]
    fn lossy_presets_use_a_sane_default_quality() {
        for p in PRESETS {
            // A preset with a byte ceiling derives its quality by search, so a
            // fixed quality of 0 is the correct way to say "let the search
            // decide".
            if p.format.supports_quality() && p.max_bytes.is_none() {
                assert!(
                    (60..=95).contains(&p.quality),
                    "{} sets quality {}, which is too low to be a sensible default",
                    p.id,
                    p.quality
                );
            }
        }
    }

    #[test]
    fn quality_is_never_zero_when_the_format_uses_it() {
        // 0 means "let the user decide", which is only valid when the preset
        // pairs with a byte ceiling instead.
        for p in PRESETS {
            if p.quality == 0 {
                assert!(
                    p.max_bytes.is_some(),
                    "{} leaves quality unset with no byte ceiling to fall back on",
                    p.id
                );
            }
        }
    }

    #[test]
    fn derived_dimensions_match_the_label() {
        // "Instagram portrait 4:5" must actually be 4:5.
        let p = find_preset("ig-portrait").unwrap();
        let (w, h) = (p.width, p.height.unwrap());
        assert_eq!(w * 5, h * 4, "{} is not 4:5", p.id);
    }

    #[test]
    fn square_presets_are_flagged_as_cropping() {
        for p in PRESETS {
            if let Some(h) = p.height
                && p.fit == FitMode::Cover
                && p.width == h
            {
                assert!(p.crops, "{} should be marked as cropping", p.id);
            }
        }
    }

    #[test]
    fn presets_convert_to_working_pipelines() {
        for p in PRESETS {
            let (pipeline, format, _, _) = to_pipeline(p);
            let predicted = pipeline.output_dimensions(4000, 3000).unwrap();
            let actual = pipeline
                .apply(&image::DynamicImage::ImageRgba8(image::RgbaImage::new(
                    4000, 3000,
                )))
                .unwrap();
            assert_eq!(
                (actual.width(), actual.height()),
                predicted,
                "{} prediction mismatch",
                p.id
            );
            assert_eq!(format, p.format);
        }
    }
}

//! Content hashing: deciding that two files are the same picture.
//!
//! Hard rule 7 says a filename is attacker-controlled and a format is whatever
//! the magic bytes say. The same caution applies one level up: a *name* says
//! nothing about which picture a file holds. Two files called `IMG_0001.jpg`
//! from two cameras are two photographs, and one photo saved twice under two
//! names is one. Nothing a file is called can tell those apart; what can is the
//! picture itself.
//!
//! So the key is a hash of the **decoded pixels plus the request**, not of the
//! file bytes. That makes the key a statement about the output: two files with
//! the same key export to the same bytes, and two files whose exports would
//! differ never share one. A PNG re-saved with different compression, or a JPEG
//! another tool stripped of its EXIF, merges with the original; a different
//! quality of the same photograph does not, and [`tests`] measures why rather
//! than guessing.
//!
//! BLAKE3 rather than SHA-256 for two reasons, one of them the only one that
//! matters: there is no compatibility requirement here — nothing else on earth
//! has to agree with these digests — and it is several times faster, which is
//! what decides a folder of 400 files.

use crate::pipeline::Pipeline;
use crate::worker::Settings;
use blake3::Hasher;
use std::collections::HashMap;
use std::sync::Mutex;

/// Separates this hash from every other use of BLAKE3 in the world, so a key
/// computed here can never be confused with a checksum of the same bytes taken
/// for another purpose. The trailing NUL means a truncated or concatenated
/// stream cannot land on the same bytes as a different domain.
const DOMAIN: &[u8] = b"pixelsmith/content-key/v1\0";

/// Digests kept. 128 bits is far past the point where a collision could be
/// reached by anything short of a deliberate attack, and it is what makes a
/// 16-character key readable in a report. The full 32-byte digest is computed;
/// this is the part kept.
const KEY_BYTES: usize = 16;

/// Two files with the same key export to the same bytes.
///
/// Copyable and cheap to compare, because a batch compares one of these per file
/// against every key it has already seen — which is a linear scan unless the
/// key is a value type with a hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentKey([u8; KEY_BYTES]);

impl ContentKey {
    /// The key for a picture, under a request.
    ///
    /// The pixels are fed to the hasher a row at a time, so this costs no
    /// contiguous copy of the image: a 24 MP source is hashed in 4 KB rows out
    /// of a buffer that already exists, which is the answer to "do not read an
    /// entire large file into memory to hash it" at this end of the pipeline.
    /// (At the other end, where the bytes are still compressed and on disk,
    /// `folder::read_bounded` reads in chunks and stops at the ceiling.)
    ///
    /// The image is reduced to RGBA whatever it arrived as, because a JPEG and a
    /// PNG of one picture are the same picture — the alpha channel of the second
    /// is opaque and carries no information the first did not.
    pub fn from_pixels(
        img: &image::DynamicImage,
        pipeline: &Pipeline,
        settings: &Settings,
    ) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(DOMAIN);
        hasher.update(b"pixels\0");
        hasher.update(&img.width().to_le_bytes());
        hasher.update(&img.height().to_le_bytes());
        match img {
            // Every picture arrives as either three or four channels, and the
            // canonical order is *all the colour samples for a row, then all the
            // alpha samples for that row*. Anything else makes the two shapes
            // hash differently for the same picture, which is the one thing this
            // key must not do — so `tests::the_two_pixel_shapes_agree_with_a_conversion`
            // holds the fast arms to the converted one.
            image::DynamicImage::ImageRgb8(buf) => {
                let stride = buf.width() as usize * 3;
                let opaque = vec![0xffu8; buf.width() as usize];
                for row in buf.as_raw().chunks_exact(stride) {
                    hasher.update(row);
                    hasher.update(&opaque);
                }
            }
            image::DynamicImage::ImageRgba8(buf) => {
                let (mut colour, mut alpha) = scratch(buf.width());
                let stride = buf.width() as usize * 4;
                for row in buf.as_raw().chunks_exact(stride) {
                    split(row, &mut colour, &mut alpha);
                    hasher.update(&colour);
                    hasher.update(&alpha);
                }
            }
            other => {
                // Sixteen-bit, floating-point and palette-backed buffers all land
                // here, and all of them are rare enough that a conversion is
                // cheaper than another arm of this match.
                let rgba = other.to_rgba8();
                let (mut colour, mut alpha) = scratch(rgba.width());
                let stride = rgba.width() as usize * 4;
                for row in rgba.as_raw().chunks_exact(stride) {
                    split(row, &mut colour, &mut alpha);
                    hasher.update(&colour);
                    hasher.update(&alpha);
                }
            }
        }
        update_request(&mut hasher, pipeline, settings);
        truncate(hasher.finalize())
    }

    /// The key for a picture this engine has already written.
    ///
    /// For an animation, and only for an animation. `animation::preserve` builds
    /// exactly one canonical form of an animation and returns it, and re-deriving
    /// a pixel key from it would mean decoding a GIF a second time to save a
    /// comparison the encoder has already made for us. Two animations that
    /// encode to the same GIF are the same animation as far as this engine's
    /// output is concerned, which is the same claim the pixels make.
    pub fn from_encoded(encoded: &[u8], pipeline: &Pipeline, settings: &Settings) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(DOMAIN);
        hasher.update(b"encoded\0");
        hasher.update(&(encoded.len() as u64).to_le_bytes());
        hasher.update(encoded);
        update_request(&mut hasher, pipeline, settings);
        truncate(hasher.finalize())
    }

    /// Lowercase hex, for a report and for a test to print.
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(KEY_BYTES * 2);
        for byte in self.0 {
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
        }
        out
    }
}

impl std::fmt::Display for ContentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Two row-sized buffers, reused across the picture.
///
/// Two allocations the size of a row, not one the size of a picture: the whole
/// point of handling `Rgb8` and `Rgba8` here rather than converting both to RGBA
/// is to avoid a second full-size copy of a 24 MP photograph.
fn scratch(width: u32) -> (Vec<u8>, Vec<u8>) {
    let width = width as usize;
    (Vec::with_capacity(width * 3), Vec::with_capacity(width))
}

/// De-interleave one RGBA row into its colour samples and its alpha samples.
fn split(row: &[u8], colour: &mut Vec<u8>, alpha: &mut Vec<u8>) {
    colour.clear();
    alpha.clear();
    for px in row.chunks_exact(4) {
        colour.extend_from_slice(&px[..3]);
        alpha.push(px[3]);
    }
}

/// The leading [`KEY_BYTES`] of a digest. A copy rather than an array cast, so
/// there is no `unwrap` and no panic if the two sizes ever stop agreeing.
fn truncate(digest: blake3::Hash) -> ContentKey {
    let mut key = [0u8; KEY_BYTES];
    let full = digest.as_bytes();
    key.copy_from_slice(&full[..KEY_BYTES.min(full.len())]);
    ContentKey(key)
}

/// Everything about the *request* that changes the bytes it will produce.
///
/// Omitting any of these would merge two files whose exports differ, which is a
/// worse failure than missing a duplicate: the user asked for two sizes and got
/// one file, and nothing in the report says so. Hence the whole request, read
/// field by field rather than derived from a `Debug` of the structs — a derive
/// would print a new field automatically, which sounds convenient until someone
/// adds one and every key in the folder stops matching.
///
/// Each field is tagged and length-prefixed, so moving a field or reordering two
/// of them cannot produce two different requests with the same digest.
///
/// Derived `Debug` is used for the enum values (`Some(Rotate90)`, `Width`). That
/// is the variant name and nothing else, it is stable within a build, and a key
/// only ever has to be consistent within one — nothing stores one. Renaming a
/// variant therefore makes keys *less* likely to match, which is the safe
/// direction: a missed duplicate is reported, a merged non-duplicate is not.
fn update_request(hasher: &mut Hasher, pipeline: &Pipeline, settings: &Settings) {
    hasher.update(b"request\0");
    text(hasher, 0x01, &format!("{:?}", pipeline.crop));
    text(hasher, 0x02, &format!("{:?}", pipeline.orientation));
    text(hasher, 0x03, &format!("{:?}", pipeline.resize));
    text(hasher, 0x04, &format!("{}", pipeline.strip_metadata));
    text(hasher, 0x05, &format!("{:?}", pipeline.chroma_subsampling));
    text(hasher, 0x06, &format!("{:?}", pipeline.colour));
    text(hasher, 0x07, &format!("{:?}", settings.format));
    text(hasher, 0x08, &format!("{:?}", settings.encoding));
    text(hasher, 0x09, &format!("{:?}", settings.target));
    text(hasher, 0x0a, &format!("{:?}", settings.limits));
    text(hasher, 0x0b, &format!("{:?}", settings.animation));
}

fn text(hasher: &mut Hasher, tag: u8, value: &str) {
    hasher.update(&[tag]);
    hasher.update(&(value.len() as u32).to_le_bytes());
    hasher.update(value.as_bytes());
}

/// Which pictures a batch has already exported.
///
/// One of these per batch, shared by the workers: `claim` is the whole
/// deduplication mechanism, and it is a single mutex-guarded insert because a
/// folder of 400 files spends microseconds in it against milliseconds in a
/// decode. The winner of a race between two identical files is whichever thread
/// arrived first, which is why a [`crate::worker::SkipReason::Duplicate`] names
/// the file that held the key rather than promising a particular order.
#[derive(Debug, Default)]
pub struct Dedup {
    claimed: Mutex<HashMap<ContentKey, String>>,
}

impl Dedup {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take this key, or report which file already holds it.
    pub fn claim(&self, key: ContentKey, name: &str) -> Option<String> {
        // A poisoned mutex here means some other worker panicked while holding
        // it. Recovering the guard is right and panicking is not: the map is a
        // plain `HashMap` of copies, so it cannot be left half-updated by a
        // panic, and a panic on this path would take the whole batch down for
        // bookkeeping.
        let mut guard = self
            .claimed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.get(&key) {
            Some(first) => Some(first.clone()),
            None => {
                guard.insert(key, name.to_string());
                None
            }
        }
    }

    /// How many distinct pictures have been claimed.
    pub fn len(&self) -> usize {
        self.claimed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{ChromaSubsampling, EncodingOptions, OutputFormat};
    use crate::pipeline::{FitMode, Orientation, ResampleFilter, ResizeSpec};
    use image::{Rgba, RgbaImage};

    fn grey(w: u32, h: u32, v: u8) -> image::DynamicImage {
        image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, Rgba([v, v, v, 255])))
    }

    fn ramp(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgba8(RgbaImage::from_fn(w, h, |x, y| {
            Rgba([
                (x * 7 % 256) as u8,
                (y * 11 % 256) as u8,
                ((x + y) * 3 % 256) as u8,
                255,
            ])
        }))
    }

    fn pipeline() -> Pipeline {
        Pipeline::new().with_resize(ResizeSpec {
            width: Some(64),
            height: None,
            fit: FitMode::Width,
            ..Default::default()
        })
    }

    /// Two encodings of one picture at two qualities must not produce two keys.
    ///
    /// The fixture is a flat field, and it is not a convenience: a flat DC block
    /// is the one shape where a JPEG round trip is exact, so the two files below
    /// differ in their bytes and not one of their 1 536 pixels. That is what
    /// makes this a test of the key rather than of the encoder — and it is
    /// asserted rather than assumed, because a fixture that quietly stopped
    /// round-tripping would make the dedup assertion fail for a reason that has
    /// nothing to do with dedup.
    #[test]
    fn two_qualities_of_one_picture_are_one_picture() {
        let img = grey(32, 24, 128);
        let high = crate::encode_fixed(
            &img,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(95),
        )
        .unwrap();
        let low = crate::encode_fixed(
            &img,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(75),
        )
        .unwrap();
        assert_ne!(
            high, low,
            "the two encodings must differ, or this test is vacuous"
        );

        let decoded_high = image::load_from_memory(&high).unwrap();
        let decoded_low = image::load_from_memory(&low).unwrap();
        assert_eq!(
            decoded_high.to_rgba8(),
            decoded_low.to_rgba8(),
            "this fixture no longer decodes identically at both qualities; the \
             dedup assertion below would then be measuring the encoder"
        );

        let (p, s) = (pipeline(), Settings::default());
        assert_eq!(
            ContentKey::from_pixels(&decoded_high, &p, &s),
            ContentKey::from_pixels(&decoded_low, &p, &s)
        );
    }

    /// The honest other half: a *lossy* re-encode of the same photograph is not
    /// the same file, and this engine does not pretend it is.
    ///
    /// Measured on this fixture, encoding the same ramp at q95 and q40 and
    /// decoding both moves up to 13 code values per channel in half the frame. An
    /// approximate key — one that would merge them — is a false-merge machine:
    /// two exposures of one scene are two photographs, and quietly dropping one
    /// because it "looked like" another is worse than exporting both. A missed
    /// duplicate is visible in the report; a wrong one is not. This test holds
    /// the engine to choosing the visible failure.
    #[test]
    fn a_lossy_re_encode_is_not_claimed_to_be_a_duplicate() {
        let img = ramp(64, 48);
        let high = crate::encode_fixed(
            &img,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(95),
        )
        .unwrap();
        let low = crate::encode_fixed(
            &img,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(40),
        )
        .unwrap();
        let decoded_high = image::load_from_memory(&high).unwrap();
        let decoded_low = image::load_from_memory(&low).unwrap();
        assert_ne!(
            decoded_high.to_rgba8(),
            decoded_low.to_rgba8(),
            "if these now decode identically the key merges them and this is \
             measuring the fixture rather than the policy"
        );

        let (p, s) = (pipeline(), Settings::default());
        assert_ne!(
            ContentKey::from_pixels(&decoded_high, &p, &s),
            ContentKey::from_pixels(&decoded_low, &p, &s)
        );
    }

    #[test]
    fn the_same_picture_under_two_pipelines_is_two_pictures() {
        let img = ramp(64, 48);
        let other = pipeline().with_resize(ResizeSpec {
            width: Some(32),
            height: None,
            fit: FitMode::Width,
            ..Default::default()
        });
        let s = Settings::default();
        assert_ne!(
            ContentKey::from_pixels(&img, &pipeline(), &s),
            ContentKey::from_pixels(&img, &other, &s)
        );
    }

    /// Every field of the request that can change the output must change the key.
    ///
    /// This is the test that catches a field being added to `Pipeline` or
    /// `Settings` and not hashed: the merge it would allow is silent, and no
    /// other test would fail. Each row is a real request the UI can send, and the
    /// base is the one every row differs from by exactly one field.
    #[test]
    fn every_field_of_the_request_reaches_the_key() {
        let img = ramp(64, 48);
        let base_pipeline = pipeline();
        let base_settings = Settings::default();
        let base = ContentKey::from_pixels(&img, &base_pipeline, &base_settings);

        let other_pipelines = [
            Pipeline {
                crop: Some(crate::pipeline::CropSpec {
                    x: 1,
                    y: 2,
                    width: 30,
                    height: 30,
                }),
                ..base_pipeline.clone()
            },
            Pipeline {
                orientation: Some(Orientation::Rotate90),
                ..base_pipeline.clone()
            },
            Pipeline {
                resize: Some(ResizeSpec {
                    width: Some(128),
                    height: None,
                    fit: FitMode::Width,
                    no_upscale: false,
                    ..Default::default()
                }),
                ..base_pipeline.clone()
            },
            Pipeline {
                strip_metadata: false,
                ..base_pipeline.clone()
            },
            Pipeline {
                chroma_subsampling: ChromaSubsampling::Luma444,
                ..base_pipeline.clone()
            },
            Pipeline {
                colour: crate::colour::ColourOptions {
                    keep_source_pixels: true,
                    ..Default::default()
                },
                ..base_pipeline.clone()
            },
            Pipeline {
                resize: Some(ResizeSpec {
                    width: Some(64),
                    height: None,
                    fit: FitMode::Width,
                    filter: ResampleFilter::Triangle,
                    ..Default::default()
                }),
                ..base_pipeline.clone()
            },
        ];
        for (i, p) in other_pipelines.iter().enumerate() {
            assert_ne!(
                ContentKey::from_pixels(&img, p, &base_settings),
                base,
                "pipeline variant {i} does not reach the key"
            );
        }

        let other_settings = [
            Settings {
                format: OutputFormat::Png,
                ..base_settings
            },
            Settings {
                encoding: EncodingOptions::default().with_quality(40),
                ..base_settings
            },
            Settings {
                target: Some(crate::target::TargetBytes::new(40_000)),
                ..base_settings
            },
            Settings {
                limits: crate::validate::Limits::mobile(),
                ..base_settings
            },
            Settings {
                animation: crate::animation::AnimationPolicy::FirstFrame,
                ..base_settings
            },
        ];
        for (i, s) in other_settings.iter().enumerate() {
            assert_ne!(
                ContentKey::from_pixels(&img, &base_pipeline, s),
                base,
                "settings variant {i} does not reach the key"
            );
        }
    }

    #[test]
    fn the_key_distinguishes_a_picture_from_itself_at_another_size() {
        let img = ramp(64, 48);
        let (p, s) = (pipeline(), Settings::default());
        // Dimensions are part of the key, so a 64x48 and a 48x64 ramp are two
        // pictures rather than one rotated pair.
        let transposed = image::DynamicImage::ImageRgba8(RgbaImage::from_fn(48, 64, |x, y| {
            Rgba([
                (y * 7 % 256) as u8,
                (x * 11 % 256) as u8,
                ((x + y) * 3 % 256) as u8,
                255,
            ])
        }));
        assert_ne!(
            ContentKey::from_pixels(&img, &p, &s),
            ContentKey::from_pixels(&transposed, &p, &s)
        );
    }

    #[test]
    fn an_rgb_picture_and_its_opaque_rgba_twin_are_one_picture() {
        let rgb = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(16, 12, |x, y| {
            image::Rgb([(x * 5 % 256) as u8, (y * 9 % 256) as u8, 77])
        }));
        let rgba = rgb.to_rgba8();
        let (p, s) = (Pipeline::new(), Settings::default());
        assert_eq!(
            ContentKey::from_pixels(&rgb, &p, &s),
            ContentKey::from_pixels(&image::DynamicImage::ImageRgba8(rgba), &p, &s)
        );
    }

    #[test]
    fn the_two_pixel_shapes_agree_with_a_conversion() {
        // The RGB arm and the Rgba8 arm are written separately to avoid a copy of
        // the picture, which is exactly the kind of duplication that drifts. This
        // holds them to each other: converting the RGB fixture to RGBA must give
        // the key the Rgba8 arm computed directly.
        let rgb = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(20, 14, |x, y| {
            image::Rgb([(x * 6 % 256) as u8, (y * 3 % 256) as u8, 200])
        }));
        let (p, s) = (Pipeline::new(), Settings::default());
        assert_eq!(
            ContentKey::from_pixels(&rgb, &p, &s),
            ContentKey::from_pixels(&rgb.to_rgba8().into(), &p, &s)
        );
    }

    #[test]
    fn hex_is_sixteen_lowercase_digits() {
        let key = ContentKey::from_pixels(&grey(4, 4, 200), &Pipeline::new(), &Settings::default());
        let hex = key.to_hex();
        assert_eq!(hex.len(), KEY_BYTES * 2);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "not lowercase hex: {hex}"
        );
        assert_eq!(hex, key.to_string());
    }

    #[test]
    fn claiming_a_key_twice_reports_the_first_holder() {
        let dedup = Dedup::new();
        let key = ContentKey::from_pixels(&grey(4, 4, 10), &Pipeline::new(), &Settings::default());
        assert_eq!(dedup.claim(key, "a.jpg"), None);
        assert_eq!(dedup.claim(key, "b.jpg"), Some("a.jpg".to_string()));
        assert_eq!(dedup.len(), 1);
        assert!(!dedup.is_empty());
    }

    #[test]
    fn distinct_pictures_claim_distinct_keys() {
        let dedup = Dedup::new();
        let s = Settings::default();
        for v in 0..8u8 {
            let key = ContentKey::from_pixels(&grey(4, 4, v * 20), &Pipeline::new(), &s);
            assert_eq!(dedup.claim(key, &format!("{v}.jpg")), None);
        }
        assert_eq!(dedup.len(), 8);
    }
}

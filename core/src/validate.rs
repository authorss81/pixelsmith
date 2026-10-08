use crate::error::{Error, Result};

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
        self.check_header(w, h)
    }

    /// The same bounds, applied to a size read out of a *header* rather than
    /// after a decode.
    ///
    /// Split out from [`Limits::check_decoded`] because two formats state their
    /// geometry somewhere an image decoder cannot reach: a HEIF file keeps it in
    /// an `ispe` property box, and reading it costs nothing while decoding costs
    /// the whole picture. A limit that only exists after the decode is not a
    /// limit; this is where a decompression bomb is actually stopped.
    pub fn check_header(&self, w: u32, h: u32) -> Result<()> {
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

    /// The pixel budget for a decode that never materialises the image.
    ///
    /// A streamed decode's peak is about the *destination*, so the pixel budget
    /// stops being a memory ceiling and becomes a CPU one: nobody wants to spend
    /// twenty seconds decoding a picture they cannot use. It is therefore a
    /// multiple of the materialising budget rather than the same number, and the
    /// multiple is [`STREAMED_PIXEL_FACTOR`].
    ///
    /// Two consequences worth stating, because they are the reason this is a
    /// method rather than a field:
    ///
    /// * A build without the `streaming` feature never consults it, so the
    ///   in-memory path's ceiling is unchanged and hard rule 4 still holds on that
    ///   path. Raising `max_pixels` itself would have done the opposite: it would
    ///   have let the in-memory decode try to materialise 160 MP on a phone.
    /// * It is derived from the profile, so `Limits::mobile()` stays the one
    ///   place a target's affordability is expressed. A target with no sandbox
    ///   outside it — Windows, where `setrlimit` has no equivalent and
    ///   [`crate::sandbox`] enforces nothing — has this and
    ///   [`Limits::streaming_memory_budget`] as its whole defence.
    pub fn streamed_pixels_budget(&self) -> u64 {
        self.max_pixels.saturating_mul(STREAMED_PIXEL_FACTOR)
    }

    /// Ceiling on the working memory of one streamed decode, in bytes.
    ///
    /// Derived from `max_pixels` because that field already states how much
    /// memory the profile is prepared to spend on a picture: four bytes per pixel
    /// is what materialising an RGBA8 buffer costs, which is exactly what
    /// `Limits::apply_to_decoder` pushes into `image::Limits::max_alloc`. So this
    /// is the same number the profile would have allowed a decode to spend, held
    /// to by a path that spends far less of it.
    ///
    /// [`crate::stream::decode_resized`] refuses a request whose
    /// `working_set_bytes` exceeds this, before the first row is read.
    pub fn streaming_memory_budget(&self) -> u64 {
        self.max_pixels.saturating_mul(4)
    }

    /// The same budget applied to every frame of an animation at once.
    ///
    /// An animation's frames are all held in memory while the encoder writes
    /// them, so a 200-frame export costs 200 times what one frame costs — and
    /// [`Limits::check_header`] is called per frame, so each check would pass.
    /// The budget is therefore applied to the whole animation rather than to
    /// each frame, which is the honest reading of `max_pixels`: the profile says
    /// how many pixels of decoded picture this device will hold at once, and an
    /// animation presents them all at once.
    ///
    /// Checked before the first frame is decoded, so a refusal costs nothing but
    /// the header walk that found the frames.
    pub fn check_animation(&self, w: u32, h: u32, frames: u32) -> Result<()> {
        self.check_header(w, h)?;
        let pixels = u64::from(w) * u64::from(h) * u64::from(frames.max(1));
        if pixels > self.max_pixels {
            return Err(Error::PixelBudgetExceeded {
                limit: self.max_pixels,
                actual: pixels as f64 / 1_000_000.0,
            });
        }
        Ok(())
    }

    /// The header check for a streamed decode.
    ///
    /// Same per-side ceiling as [`Limits::check_header`], and a pixel budget of
    /// [`Limits::streamed_pixels_budget`]. Split out rather than reusing
    /// `check_header` because the two budgets are different claims about
    /// different memory models, and a single method taking a flag would let a
    /// caller pass the wrong one.
    pub fn check_streamed_header(&self, w: u32, h: u32) -> Result<()> {
        if w > self.max_dimension || h > self.max_dimension {
            return Err(Error::SuspiciousDimensions {
                w,
                h,
                mp: megapixels(w, h),
            });
        }
        let pixels = u64::from(w) * u64::from(h);
        if pixels > self.streamed_pixels_budget() {
            return Err(Error::PixelBudgetExceeded {
                limit: self.streamed_pixels_budget(),
                actual: megapixels(w, h),
            });
        }
        if w == 0 || h == 0 {
            return Err(Error::ZeroDimension);
        }
        Ok(())
    }
}

/// How many times the materialising pixel budget a streamed decode may take.
///
/// Four, because it is a statement about time rather than memory: `Limits::mobile()`
/// is 40 MP, so a streamed phone decode accepts a 160 MP input, which decodes in
/// well under two seconds for JPEG or PNG. See [`Limits::streamed_pixels_budget`]
/// for why this is a method and not a change to `max_pixels`.
pub const STREAMED_PIXEL_FACTOR: u64 = 4;

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
    /// How many frames the file's container claims.
    ///
    /// 1 for a still, which is the overwhelming majority of files, and 1 for
    /// every format this build cannot see frames in. It is the number the UI
    /// shows *before* an export, so the count beside the finished file is the
    /// same number the user was warned with.
    ///
    /// Read from the container rather than by decoding — see
    /// [`FrameScan`] — so a 40 MB animation costs a walk of its own bytes
    /// instead of a full decode during a folder scan.
    pub frames: u32,
    /// Whether the frame walk stopped before the trailer, which makes `frames` a
    /// lower bound rather than a count.
    ///
    /// Its own field because the two cannot be told apart from `frames` alone,
    /// and the difference decides whether an animation may be written out at
    /// all: `frames` says how many there are, this says whether that is knowable.
    /// A file that says "at least 3" cannot be checked against a write of 3.
    pub frames_truncated: bool,
    pub sensitive_tags: Vec<String>,
    pub orientation: Option<u32>,
    /// The colour profile the file carries, or that it does not. This is the
    /// field that lets the UI say "Display-P3: this photo will be converted to
    /// sRGB" *before* the user exports, which is the whole difference between
    /// colour management and colour management by surprise.
    pub colour: crate::colour::ColourProfile,
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

    // A HEIF file states its geometry in an `ispe` property box, which no image
    // decoder reads, so the container is asked instead. Read once: the answer
    // serves the dimensions *and* the metadata report below.
    let heif_header = match format {
        crate::format::OutputFormat::Heic | crate::format::OutputFormat::Heif => {
            Some(crate::heic::header(input)?)
        }
        _ => None,
    };

    let (w, h) = match &heif_header {
        Some(header) => (header.width, header.height),
        None => {
            let mut reader =
                image::ImageReader::new(std::io::Cursor::new(input)).with_guessed_format()?;
            limits.apply_to_decoder(&mut reader);
            reader.into_dimensions()?
        }
    };

    // A GIF's frames are counted from its container rather than by decoding
    // them, because this function runs over a whole folder before the user has
    // committed to anything. See `scan_gif_frames`.
    let scan = if format.supports_animation() {
        scan_gif_frames(input)
    } else {
        FrameScan::STILL
    };

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
    } else if scan.truncated {
        // Last, because a file that is both oversize and damaged should be told
        // about the size: that is the reason it will be refused.
        Some(
            "this GIF's frame structure is damaged, so the number of frames in it is a \
             lower bound and it may not open at all"
                .into(),
        )
    } else {
        None
    };

    let exif = crate::exif::read(input).unwrap_or_default();
    // A HEIF file keeps its EXIF in a separate item, so `exif::read` cannot see
    // it. Asking the container is what keeps the report honest: a photo with a
    // GPS fix in it must not be reported as carrying no metadata, or the app
    // tells the user there is nothing to strip.
    let heif_exif = heif_header.is_some_and(|header| header.has_exif);

    Ok(ValidateReport {
        format,
        width: w,
        height: h,
        megapixels: megapixels(w, h),
        has_exif: !exif.entries.is_empty() || heif_exif,
        has_animated: scan.frames > 1,
        frames: scan.frames,
        frames_truncated: scan.truncated,
        sensitive_tags: exif.sensitive_tags.clone(),
        orientation: exif.orientation,
        // Taken from the `exif::read` above rather than re-walked: it read the
        // same container to find the colour profile, and two walks of a
        // hostile container per file is two chances to be wrong.
        colour: exif.colour,
        suspicious,
    })
}

/// What a GIF's container says about its frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FrameScan {
    /// Image descriptors found. Zero when the container could not be read at
    /// all, which is a broken file rather than a still one — `truncated` says so
    /// and the decode that follows says it in a sentence.
    pub frames: u32,
    /// The walk stopped early: the buffer ended before the trailer, or a byte
    /// appeared where a block introducer had to be.
    ///
    /// Carried rather than absorbed because it changes what the frame count
    /// means. `frames` is then a lower bound, and this is how a caller knows it
    /// is one.
    pub truncated: bool,
}

impl FrameScan {
    /// A file with no container to walk: one frame, and nothing damaged.
    pub const STILL: Self = Self {
        frames: 1,
        truncated: false,
    };
}

/// How many frames a GIF holds, read from the container without decoding pixels.
///
/// GIF is the only format in this build with more than one frame, so this is
/// the whole animation-detection story: APNG and animated WebP are reported as
/// single-frame, which means the UI treats them as stills and says so rather
/// than quietly flattening them.
///
/// The previous implementation counted frames with `into_frames().count()`,
/// which *decodes* every frame — so the folder scan in `validate_bytes`, whose
/// entire selling point is being cheap enough to run on a whole folder, was
/// materialising every animation in it. It also had no way to distinguish "one
/// frame" from "a container too damaged to read", and swallowed the difference
/// into `unwrap_or(1)`. Both are what [`FrameScan`] now states.
///
/// Every offset is checked against the buffer length and every walk is bounded
/// by it, because the bytes come off the user's disk: the loop advances only
/// past a block it has found the end of, so a sub-block header claiming more
/// bytes than the file holds ends the walk instead of indexing past it. It
/// cannot panic, and it cannot loop, and it allocates nothing.
pub fn scan_gif_frames(input: &[u8]) -> FrameScan {
    /// `GIF87a` or `GIF89a`.
    const SIGNATURE: usize = 6;
    /// Logical screen descriptor: four 16-bit fields and one packed byte.
    const SCREEN_DESCRIPTOR: usize = 7;
    const EXTENSION_INTRODUCER: u8 = 0x21;
    const IMAGE_SEPARATOR: u8 = 0x2c;
    const TRAILER: u8 = 0x3b;
    /// Legal filler between blocks in some encoders' output.
    const PADDING: u8 = 0x00;

    if input.len() < SIGNATURE + SCREEN_DESCRIPTOR
        || (&input[..SIGNATURE] != b"GIF87a" && &input[..SIGNATURE] != b"GIF89a")
    {
        return FrameScan {
            frames: 0,
            truncated: true,
        };
    }

    let mut at = SIGNATURE + SCREEN_DESCRIPTOR;
    // The global colour table sits between the screen descriptor and the first
    // block rather than being part of the block stream. Skipping it is what
    // stops a table entry that happens to hold 0x2c from being counted as a
    // frame — a 256-colour table has a 25% chance of containing one.
    let packed = input[SIGNATURE + 4];
    if packed & 0x80 != 0 {
        let entries = 2usize << (packed & 0x07);
        at = at.saturating_add(entries * 3);
    }

    let mut frames: u32 = 0;
    'walk: loop {
        let Some(&introducer) = input.get(at) else {
            break 'walk FrameScan {
                frames,
                truncated: true,
            };
        };
        match introducer {
            TRAILER => {
                break 'walk FrameScan {
                    frames,
                    truncated: false,
                };
            }
            PADDING => at = at.saturating_add(1),
            EXTENSION_INTRODUCER => {
                // Introducer, label, then a sub-block chain.
                let Some(next) = skip_sub_blocks(input, at.saturating_add(2)) else {
                    break 'walk damaged(frames);
                };
                at = next;
            }
            IMAGE_SEPARATOR => {
                frames = frames.saturating_add(1);
                let Some(next) = after_image_descriptor(input, at) else {
                    break 'walk damaged(frames);
                };
                at = next;
            }
            // Not a byte that can start a block. The stream is out of sync and
            // every later guess about where a frame begins would be fiction.
            _ => break 'walk damaged(frames),
        }
    }
}

/// A walk that stopped before the trailer, having counted this many frames.
fn damaged(frames: u32) -> FrameScan {
    FrameScan {
        frames,
        truncated: true,
    }
}

/// Step past one image descriptor: the descriptor, an optional **local** colour
/// table, the LZW minimum code size, then the frame's own sub-block chain.
///
/// The local table is the trap, and it is not a rare one. It sits inside the
/// descriptor block, so those nine fixed bytes are not the whole block — and
/// `image`'s own GIF encoder emits one for every frame it writes, so a walk
/// that forgets it counts exactly one frame in an animation of four and reports
/// a perfectly ordinary still.
fn after_image_descriptor(input: &[u8], at: usize) -> Option<usize> {
    /// Image descriptor introducer plus its nine bytes.
    const DESCRIPTOR: usize = 10;
    let end = at.checked_add(DESCRIPTOR)?;
    // The packed byte is the last of the descriptor's nine.
    let packed = *input.get(end.checked_sub(1)?)?;
    let mut next = end;
    if packed & 0x80 != 0 {
        let entries = 2usize << (packed & 0x07);
        next = next.checked_add(entries * 3)?;
    }
    // One byte of LZW minimum code size, then the frame's data.
    skip_sub_blocks(input, next.checked_add(1)?)
}

/// Skip a chain of GIF sub-blocks, returning the offset of the byte after it.
///
/// Each sub-block is a length byte followed by that many bytes; a zero length
/// ends the chain. A length that runs past the end of the buffer returns `None`
/// rather than a position past it, which is the only way this walk can end
/// other than at a trailer.
fn skip_sub_blocks(input: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let len = usize::from(*input.get(at)?);
        at = at.checked_add(1)?;
        if len == 0 {
            return Some(at);
        }
        at = at.checked_add(len)?;
        if at > input.len() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colour::ColourSpace;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        crate::encode_fixed(
            &image::DynamicImage::ImageRgb8(image::RgbImage::new(w, h)),
            crate::format::OutputFormat::Jpeg,
            crate::format::EncodingOptions::default().with_quality(80),
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

    /// Bytes that are not an image at all, named as such.
    ///
    /// Was `assert!(...is_err())`, in a module with eight named variants. That
    /// assertion is satisfied by `Decode`, by `InputTooLarge`, by a panic caught
    /// somewhere upstream — anything that ends in an `Err` — and the distinction
    /// is the whole point of the variant: this file is not a picture, as opposed
    /// to being a picture we failed to read, and hard rule 9 wants the user told
    /// which of the two they are holding.
    #[test]
    fn garbage_bytes_are_rejected() {
        assert!(
            matches!(
                validate_bytes(b"not an image at all", &Limits::default()),
                Err(Error::UnknownFormat)
            ),
            "bytes that are not an image must be refused as unrecognised, not as a \
             decode failure - a user holding a .txt should not be told the decoder \
             failed on it"
        );
        // And the same for a file that *is* an image with its first bytes
        // overwritten, which is the near-miss that `UnknownFormat` and `Decode`
        // must not be confused about.
        let mut png = crate::encode_fixed(
            &image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8)),
            crate::format::OutputFormat::Png,
            crate::format::EncodingOptions::default(),
        )
        .expect("png fixture");
        png[1] = b'Z';
        assert!(
            matches!(
                validate_bytes(&png, &Limits::default()),
                Err(Error::UnknownFormat)
            ),
            "a PNG whose signature was overwritten is not a picture this build can \
             open, whatever its dimensions say"
        );
    }

    /// A truncated file is refused whole: no report, no partial geometry, no
    /// half-decoded picture for a caller to write out.
    ///
    /// The name made that claim and the body checked `is_err()` — which is
    /// necessary and nowhere near sufficient, because `Err` is the *only* shape a
    /// refusal can take. The two halves of "rather than half decoded" are
    /// therefore asserted separately: the refusal names a variant, and both
    /// entry points that hand a caller an image agree that there is none.
    #[test]
    fn a_truncated_jpeg_is_rejected_rather_than_half_decoded() {
        let full = jpeg(64, 64);
        let truncated = &full[..full.len() / 3];

        let err = validate_bytes(truncated, &Limits::default())
            .expect_err("a JPEG with two thirds of its entropy-coded data missing is not a picture");
        // A strict allow-list rather than `is_err()`: a decoder that started
        // reporting `Metadata` for a file whose *pixels* are unreadable would
        // pass `is_err()` and move the blame onto the tags.
        match &err {
            Error::Decode(_) | Error::UnknownFormat | Error::Heic(_) => {}
            other => panic!(
                "a truncated JPEG must be refused as a decode failure, got {other:?}"
            ),
        }
        // Nothing partial: `validate_bytes` returns either a report or an error,
        // and the error means there is no report. `inspect` is the function that
        // would hand a caller both halves, so it is the one that has to agree.
        assert!(
            inspect(truncated, &Limits::default()).is_err(),
            "inspect returned a report for a file validate_bytes refused, which is \
             the half-decoded picture this test is named after"
        );
        assert!(
            crate::decode_bounded(truncated, &Limits::default()).is_err(),
            "decode_bounded produced an image from a file that could not be read"
        );
        // The positive control: a third of the way is past the header but nowhere
        // near the end, and the fixture is complete at full length.
        assert_eq!(
            validate_bytes(&full, &Limits::default())
                .expect("the untruncated fixture must validate")
                .width,
            64
        );
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

    /// The report is how the UI learns a photo is wide-gamut *before* the user
    /// exports, so all three states have to be distinguishable in it: no profile,
    /// an sRGB profile, and a Display-P3 profile.
    #[test]
    fn the_report_names_the_colour_space_of_a_file() {
        let patch = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            32,
            32,
            image::Rgb([200, 100, 90]),
        ));
        let cases = [
            (None, ColourSpace::Untagged, false),
            (
                Some(crate::colour::fixtures::srgb_profile()),
                ColourSpace::Srgb,
                true,
            ),
            (
                Some(crate::colour::fixtures::p3_profile()),
                ColourSpace::DisplayP3,
                true,
            ),
        ];
        for (icc, expected, has_icc) in cases {
            let bytes = crate::colour::fixtures::encode_with(
                &patch,
                crate::format::OutputFormat::Jpeg,
                icc.as_deref(),
            );
            let report = validate_bytes(&bytes, &Limits::default()).unwrap();
            assert_eq!(report.colour.source, expected);
            assert_eq!(report.colour.icc_present, has_icc, "{expected:?}");
            if has_icc {
                assert_eq!(report.colour.declared.as_deref(), Some("RGB"));
            } else {
                assert_eq!(report.colour.declared, None);
                assert_eq!(report.colour.icc_bytes, 0);
            }
        }
    }

    /// The report and `exif::read` are two doors onto the same reading, and a UI
    /// that showed the colour from one and the metadata from the other would be
    /// showing two different facts about one file.
    #[test]
    fn the_report_and_the_metadata_read_agree_about_the_colour() {
        let bytes = crate::colour::fixtures::encode_with(
            &image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                32,
                32,
                image::Rgb([1, 2, 3]),
            )),
            crate::format::OutputFormat::Jpeg,
            Some(&crate::colour::fixtures::p3_profile()),
        );
        let report = validate_bytes(&bytes, &Limits::default()).unwrap();
        let exif = crate::exif::read(&bytes).unwrap();
        assert_eq!(report.colour, exif.colour);
    }
}

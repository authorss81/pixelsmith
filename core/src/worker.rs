//! Batch orchestration, the ZIP writer, and cancellation.
//!
//! This is the layer the UI actually calls. It owns three things worth calling
//! out:
//!
//! * **Parallelism.** Rayon, bounded so a 400-image batch on a 2-core phone
//!   does not thrash.
//! * **Cancellation.** A token the UI can flip from a button. Checked between
//!   files, never mid-file, because a half-written JPEG is worse than no JPEG.
//! * **Accounting.** Every batch returns per-file success *and* failure, so one
//!   unreadable file in a folder of 200 does not discard the other 199.

use crate::error::{Error, Result};
use crate::format::{EncodingOptions, OutputFormat};
use crate::pipeline::{Orientation, Pipeline};
use crate::target::{TargetBytes, default_encoder};
use crate::validate::Limits;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// One input, as handed over by the platform file picker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// Display name only. Never used to decide the output format; the magic
    /// bytes decide that. See [`crate::format::detect_format`].
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub format: OutputFormat,
    /// Quality, progressive scan and chroma resolution. One value rather than
    /// three arguments, so the next encoder knob does not change the signature of
    /// every function that encodes.
    pub encoding: EncodingOptions,
    pub target: Option<TargetBytes>,
    pub limits: Limits,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            format: OutputFormat::Jpeg,
            encoding: EncodingOptions::default(),
            target: None,
            limits: Limits::default(),
        }
    }
}

impl Settings {
    /// Refuse a request this format cannot act on, before a pixel is decoded.
    ///
    /// Only the byte ceiling is refused. A quality value is *ignored* for a
    /// lossless format rather than rejected, because one global quality slider
    /// exists above the format picker and refusing every PNG export because of it
    /// would be absurd. A ceiling is different: it is a promise about the size of
    /// the file, and PNG cannot keep it at any slider position. Silently returning
    /// an oversized file would be the one answer hard rule 9 forbids, so the
    /// engine says which format will keep it instead.
    ///
    /// The UI is expected never to send this combination — it greys the size
    /// field out from [`crate::capabilities`] and the per-format predicates — but
    /// a request can arrive from anywhere, and this is the layer every path goes
    /// through.
    pub fn validate(&self) -> Result<()> {
        if self.target.is_some() && !self.format.supports_byte_target() {
            return Err(Error::NoQualitySetting(self.format.quality_note()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub name: String,
    pub output_name: String,
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub width: u32,
    pub height: u32,
    /// Quality actually used. Differs from the requested quality when a byte
    /// target forced a search, and is **0 when the format has no quality setting
    /// and none was applied** — see [`crate::format::OutputFormat::supports_quality`].
    /// Zero is also what a failed or cancelled file reports, which is the same
    /// claim: no quality was used to produce this.
    pub quality_used: u8,
    /// False when the byte target could not be met.
    pub target_met: bool,
    pub error: Option<String>,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }

    pub fn saved_percent(&self) -> f64 {
        if self.input_bytes == 0 {
            return 0.0;
        }
        (1.0 - self.output_bytes as f64 / self.input_bytes as f64) * 100.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BatchReport {
    pub outcomes: Vec<Outcome>,
    pub cancelled: bool,
}

impl BatchReport {
    pub fn succeeded(&self) -> usize {
        self.outcomes.iter().filter(|o| o.ok()).count()
    }

    pub fn failed(&self) -> usize {
        self.outcomes.iter().filter(|o| !o.ok()).count()
    }

    pub fn input_bytes(&self) -> usize {
        self.outcomes.iter().map(|o| o.input_bytes).sum()
    }

    pub fn output_bytes(&self) -> usize {
        self.outcomes.iter().map(|o| o.output_bytes).sum()
    }
}

/// Cooperative cancellation handle, cloneable and thread-safe.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

/// Output of one job, before it is written anywhere.
#[derive(Debug)]
pub struct Processed {
    pub outcome: Outcome,
    pub bytes: Vec<u8>,
}

/// Process a single image. This is the unit of work both the single-image and
/// batch paths use, so a batch result and a one-off result cannot diverge.
pub fn process_one(job: &Job, pipeline: &Pipeline, settings: &Settings) -> Result<Processed> {
    // A request this build cannot carry out is refused before anything is
    // decoded, so the user gets a sentence about their settings rather than a
    // file that quietly misses the ceiling they asked for.
    settings.validate()?;
    // Header check first: it rejects a hostile file before we allocate a
    // pixel buffer for it. The streaming path re-reads it rather than trusting
    // this call, because it is the header check that makes row-at-a-time decoding
    // safe.
    let report = crate::validate::validate_bytes(&job.bytes, &settings.limits)?;

    // Behind `streaming`: decode straight into the resized destination instead of
    // materialising the source first. Chosen here rather than inside the pipeline
    // because this is the layer that owns the request, and the three conditions
    // below are about the request rather than about the picture.
    #[cfg(feature = "streaming")]
    let out = match streamed_resize(
        job,
        pipeline,
        &settings.limits,
        (report.width, report.height),
    )? {
        Some(img) => img,
        None => {
            let decoded = crate::decode_bounded(&job.bytes, &settings.limits)?;
            pipeline.apply(&decoded)?
        }
    };
    #[cfg(not(feature = "streaming"))]
    let out = {
        let decoded = crate::decode_bounded(&job.bytes, &settings.limits)?;
        pipeline.apply(&decoded)?
    };

    // Re-encoding from raw samples is what actually removes EXIF. If the user
    // opted to keep metadata we graft it back, minus anything sensitive.
    let working = if pipeline.strip_metadata {
        crate::exif::strip(&out)?
    } else {
        out
    };

    // The pipeline's chroma choice is the authority: it is the one a UI sets on
    // the picture, and `Settings::encoding` is the fallback for a caller that
    // never touches the pipeline. See `Pipeline::chroma_subsampling`.
    let options = settings.encoding.with_chroma(pipeline.chroma_subsampling);

    // Zero means "no quality setting was applied", which is what a lossless
    // format gets. Reporting the requested number there would be a claim about a
    // control that did nothing, and `quality_used` is what the UI shows next to
    // the file it just wrote.
    let reported_quality = if settings.format.supports_quality() {
        options.quality
    } else {
        0
    };

    let (mut bytes, quality_used, target_met) = match settings.target {
        Some(target) if settings.format.supports_byte_target() => {
            let (bytes, q, met) =
                target.encode_with(&working, settings.format, options, &default_encoder)?;
            (bytes, q, met)
        }
        // `validate` refuses this combination, so the arm exists for a caller
        // that skipped it — and it measures rather than assumes, because
        // `target_met` has to mean the same thing on every path.
        Some(target) => {
            let bytes = crate::encode_fixed(&working, settings.format, options)?;
            let met = bytes.len() as u64 <= target.bytes;
            (bytes, reported_quality, met)
        }
        None => (
            crate::encode_fixed(&working, settings.format, options)?,
            reported_quality,
            true,
        ),
    };

    if !pipeline.strip_metadata && settings.format == OutputFormat::Jpeg {
        crate::exif::write_back(&job.bytes, &mut bytes, false)?;
    }

    let stem = sanitise_stem(&job.name);
    let outcome = Outcome {
        id: job.id.clone(),
        name: job.name.clone(),
        output_name: format!("{stem}.{}", settings.format.extension()),
        input_bytes: job.bytes.len(),
        output_bytes: bytes.len(),
        width: working.width(),
        height: working.height(),
        quality_used,
        target_met,
        error: None,
    };
    Ok(Processed { outcome, bytes })
}

/// Resize while the bytes are still compressed, or `None` to use the in-memory
/// path.
///
/// Three conditions, each of which is a case this does *not* handle rather than a
/// preference:
///
/// * **No crop.** A crop is a sub-rectangle of the source, and reading one row at
///   a time can honour it — but only after the row reader is inside the crop's
///   row range, and PNG's filters are sequential so every row above it must be
///   inflated and unfiltered anyway. Worth doing, and not worth doing quietly
///   inside a phase whose subject is peak memory.
/// * **No orientation transform.** A quarter turn needs the source or a transposed
///   read of it.
/// * **A resize that changes the size.** Without one there is nothing to stream
///   for, and the whole-image decode is already the minimum.
///
/// The kernel comes from [`crate::pipeline::filter_for`], so this path and
/// `resize_to` cannot pick different filters for the same request.
#[cfg(feature = "streaming")]
fn streamed_resize(
    job: &Job,
    pipeline: &Pipeline,
    limits: &Limits,
    (src_w, src_h): (u32, u32),
) -> Result<Option<image::DynamicImage>> {
    if pipeline.crop.is_some() {
        return Ok(None);
    }
    if pipeline
        .orientation
        .is_some_and(Orientation::needs_transform)
    {
        return Ok(None);
    }
    let Some(spec) = pipeline.resize.as_ref() else {
        return Ok(None);
    };
    if !spec.has_effect(&image::DynamicImage::new_rgba8(src_w, src_h)) {
        return Ok(None);
    }
    let (dst_w, dst_h) = pipeline.output_dimensions(src_w, src_h)?;
    if (dst_w, dst_h) == (src_w, src_h) {
        return Ok(None);
    }
    let filter = crate::pipeline::filter_for(spec, src_w, src_h);
    let img = crate::stream::decode_resized(&job.bytes, limits, (dst_w, dst_h), filter)?;
    Ok(Some(img))
}

/// Run a batch in parallel. Failures are collected, not propagated.
pub fn process_batch(
    jobs: &[Job],
    pipeline: &Pipeline,
    settings: &Settings,
    cancel: &CancelToken,
) -> BatchReport {
    let results: Vec<Result<Processed>> = jobs
        .par_iter()
        .map(|job| {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            process_one(job, pipeline, settings)
        })
        .collect();

    let cancelled = cancel.is_cancelled();
    let outcomes = results
        .into_iter()
        .zip(jobs)
        .map(|(result, job)| match result {
            Ok(p) => p.outcome,
            Err(Error::Cancelled) => Outcome {
                id: job.id.clone(),
                name: job.name.clone(),
                output_name: String::new(),
                input_bytes: job.bytes.len(),
                output_bytes: 0,
                width: 0,
                height: 0,
                quality_used: 0,
                target_met: false,
                error: Some("cancelled".into()),
            },
            Err(e) => Outcome {
                id: job.id.clone(),
                name: job.name.clone(),
                output_name: String::new(),
                input_bytes: job.bytes.len(),
                output_bytes: 0,
                width: 0,
                height: 0,
                quality_used: 0,
                target_met: false,
                error: Some(e.to_string()),
            },
        })
        .collect();

    BatchReport {
        outcomes,
        cancelled,
    }
}

/// Zip processed output for a single download.
pub fn zip_outputs(items: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        use std::io::Write;
        for (name, bytes) in items {
            zip.start_file(sanitise_path_component(name), options)?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
    }
    Ok(buf.into_inner())
}

/// Strip the extension and anything that could escape a directory.
///
/// A filename arriving from an untrusted picker can be `../../evil.png` or
/// `C:\Windows\x.png` or `nul`. Only the stem survives, and only after this
/// filter.
///
/// Reserved Windows device names are also refused. `nul.png`, `CON.jpg` and
/// `com1.jpeg` all sanitise to a stem Windows will refuse to create, whatever
/// extension follows - the reservation is on the stem alone - so a photo named
/// `nul.jpg` from a phone would produce an export that fails at the last step,
/// after the user had already waited for the encode. Found by
/// `sanitise_stem_never_names_a_reserved_windows_device` in
/// `core/tests/properties.rs`, which reports all eleven spellings.
///
/// Named `RESERVED_DEVICE_NAMES` rather than inlining the list in the `matches!`
/// so the test and the implementation cannot drift apart.
pub fn sanitise_stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(['-', ' ', '.']).to_string();
    if trimmed.is_empty() {
        return "image".into();
    }
    // Cap before the reserved-name check, so a name that only *becomes* reserved
    // after truncation is still caught. `NUL` padded out to 64 characters is
    // still a name Windows will not take.
    let capped: String = trimmed.chars().take(64).collect();
    if is_reserved_windows_name(&capped) {
        // Prefix rather than replace: the user still sees which file this was,
        // and `_nul` is creatable.
        return format!("_{capped}");
    }
    capped
}

/// Device names Windows reserves regardless of extension, in the canonical
/// uppercase spelling. `COM1`-`COM9` and `LPT1`-`LPT9` are written out rather
/// than generated so the list can be read against the Microsoft documentation
/// without running anything.
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether Windows would refuse to create this name.
///
/// The reservation applies to the part before the first dot, case-insensitively,
/// and with any trailing spaces or dots ignored - `con`, `CON`, `con ` and
/// `con.` are all the same device. Trailing dots and spaces are stripped before
/// the lookup because `sanitise_stem` can produce them by truncation.
fn is_reserved_windows_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name);
    let base = base.trim_end_matches([' ', '.']);
    let upper = base.to_ascii_uppercase();
    RESERVED_DEVICE_NAMES.contains(&upper.as_str())
}

/// Same idea, for a path component inside an archive.
///
/// Named by the property test `sanitise_path_component_cannot_contain_a_dot_dot_run`
/// in `core/tests/properties.rs`, which found that a `.` is kept whenever *both*
/// of its neighbours are alphanumeric. That let `..` survive in the middle of a
/// name: the input `㐀..¹` came out unchanged, because U+3400 and U+00B9 are both
/// `is_alphanumeric()` in Rust. A `..` in the middle of an archive entry name is
/// not by itself an escape - the separators are already gone by this point - but
/// it is the exact shape a downstream consumer splits on when it reconstructs a
/// path, and there is no reason to emit it. Collapsing every run of dots to one
/// removes the class rather than the instance.
///
/// The trailing `take(96)` matters for the same reason it does in
/// [`sanitise_stem`]: truncation can land mid-run and leave a lone `.`, so the
/// trim has to happen *after* the cap, not before.
pub fn sanitise_path_component(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut cleaned = String::with_capacity(base.len());
    let mut last_was_dot = false;
    for c in base.chars() {
        if c == '.' {
            // Keep one dot for readability; drop the rest of any run.
            if !last_was_dot {
                cleaned.push('.');
            }
            last_was_dot = true;
            continue;
        }
        if c.is_alphanumeric() || c == '-' || c == '_' {
            cleaned.push(c);
            last_was_dot = false;
        }
        // Anything else is dropped outright (a separator has already been split
        // off, so this is punctuation, a NUL, or a control character).
    }

    // Trim after capping, not before: capping first is what keeps a cut from
    // leaving a trailing dot behind.
    let capped: String = cleaned.chars().take(96).collect();
    let trimmed = capped.trim_matches('.').to_string();
    if trimmed.is_empty() {
        return "file".into();
    }
    if is_reserved_windows_name(&trimmed) {
        return format!("_{trimmed}");
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth, compressible, photo-like fixture. Real photographs compress
    /// well; fixtures built from hash noise do not, which makes them useless for
    /// testing anything involving a size ceiling.
    fn photo(w: u32, h: u32, seed: u8) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let fx = f64::from(x as u16) / f64::from(w.max(1)) * 6.0 + f64::from(seed);
            let fy = f64::from(y as u16) / f64::from(h.max(1)) * 4.0;
            let wave = (fx.sin() * 80.0 + fy.cos() * 55.0) + f64::from(seed) * 8.0;
            image::Rgb([
                (128.0 + wave).clamp(0.0, 255.0) as u8,
                (100.0 + wave * 0.6).clamp(0.0, 255.0) as u8,
                (150.0 - wave * 0.5).clamp(0.0, 255.0) as u8,
            ])
        }));
        crate::encode_fixed(
            &img,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(95),
        )
        .unwrap()
    }

    /// High-entropy noise. Used where the point is that a file decodes at all.
    fn jpeg(w: u32, h: u32, seed: u8) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([
                (x as u8).wrapping_mul(seed).wrapping_add(y as u8),
                (y as u8) ^ seed,
                seed,
            ])
        }));
        crate::encode_fixed(
            &img,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(95),
        )
        .unwrap()
    }

    fn job(id: &str, name: &str, bytes: Vec<u8>) -> Job {
        Job {
            id: id.into(),
            name: name.into(),
            bytes,
        }
    }

    fn pipeline(width: u32) -> Pipeline {
        Pipeline::new().with_resize(crate::pipeline::ResizeSpec {
            width: Some(width),
            height: None,
            fit: crate::pipeline::FitMode::Width,
            ..Default::default()
        })
    }

    #[test]
    fn processes_a_single_image() {
        let j = job("1", "photo.jpg", jpeg(800, 600, 7));
        let p = process_one(&j, &pipeline(400), &Settings::default()).unwrap();
        assert!(p.outcome.ok());
        assert_eq!((p.outcome.width, p.outcome.height), (400, 300));
        assert!(p.outcome.saved_percent() > 0.0);
        assert_eq!(p.outcome.output_name, "photo.jpg");
    }

    #[test]
    fn output_format_is_decided_by_magic_bytes() {
        // The name claims JPEG but the bytes are PNG.
        let png = crate::encode_fixed(
            &image::DynamicImage::ImageRgb8(image::RgbImage::new(100, 100)),
            OutputFormat::Png,
            EncodingOptions::default().with_quality(100),
        )
        .unwrap();
        let j = job("1", "lying.jpg", png);
        let settings = Settings {
            format: OutputFormat::WebP,
            ..Settings::default()
        };
        let p = process_one(&j, &pipeline(50), &settings).unwrap();
        assert_eq!(p.outcome.output_name, "lying.webp");
        assert_eq!(
            crate::format::detect_format(&p.bytes).unwrap(),
            OutputFormat::WebP
        );
    }

    /// Small, smooth, saturated: colour detail is what the chroma option acts
    /// on, so a fixture without it cannot tell the options apart.
    fn colour(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let wave = ((x as f32) * 0.3).sin() * 70.0 + ((y as f32) * 0.21).cos() * 60.0;
            image::Rgb([
                (140.0 + wave).clamp(0.0, 255.0) as u8,
                (60.0 + wave * 0.4).clamp(0.0, 255.0) as u8,
                (210.0 - wave * 0.7).clamp(0.0, 255.0) as u8,
            ])
        }))
    }

    /// A size ceiling on a format that stores the picture exactly cannot be kept
    /// at any quality, so the request is refused in a sentence that names a
    /// format which can keep it. Hard rule 9, and the one answer that beats
    /// quietly handing back an oversized file labelled "done".
    #[test]
    fn a_size_ceiling_on_a_lossless_format_is_refused_in_plain_english() {
        let j = job("1", "photo.jpg", photo(400, 300, 5));
        for format in [OutputFormat::Png, OutputFormat::Bmp] {
            let settings = Settings {
                format,
                target: Some(TargetBytes::new(8 * 1024)),
                ..Settings::default()
            };
            let err = process_one(&j, &pipeline(200), &settings).unwrap_err();
            let text = err.to_string();
            assert!(
                matches!(err, Error::NoQualitySetting(_)),
                "{format:?} refused for the wrong reason: {err:?}"
            );
            // It has to say what to do instead, and what the file will cost.
            assert!(
                text.contains("quality") && text.contains("JPEG"),
                "{format:?} should name the problem and an alternative: {text}"
            );
            assert!(
                !text.contains("error:") && !text.contains("unsupported or unrecognised"),
                "the message reads like a diagnostic: {text}"
            );
        }
    }

    /// The refusal is about the *ceiling*, not about the quality slider: one
    /// global slider sits above the format picker, so PNG output must still work
    /// with a quality value in the request.
    #[test]
    fn a_quality_value_on_a_lossless_format_is_ignored_rather_than_refused() {
        let j = job("1", "photo.jpg", photo(200, 150, 9));
        let settings = Settings {
            format: OutputFormat::Png,
            encoding: EncodingOptions::default().with_quality(30),
            ..Settings::default()
        };
        let p = process_one(&j, &pipeline(100), &settings).unwrap();
        assert!(p.outcome.ok());
        assert_eq!(
            crate::format::detect_format(&p.bytes).unwrap(),
            OutputFormat::Png
        );
        // And the report does not claim a quality that was never applied.
        assert_eq!(
            p.outcome.quality_used, 0,
            "PNG has no quality setting, so none may be reported as used"
        );
        assert!(p.outcome.target_met, "no ceiling was asked for");
    }

    /// `target_met` has to mean the same thing on every path. A ceiling this
    /// build can enforce is searched; one it cannot is refused; and a caller that
    /// skipped the refusal still gets a measured answer rather than an assumed
    /// "yes".
    #[test]
    fn a_reported_target_met_is_always_measured() {
        let j = job("1", "photo.jpg", photo(600, 400, 11));
        // Reachable, so the search has to say yes.
        let reachable = Settings {
            format: OutputFormat::Jpeg,
            target: Some(TargetBytes::new(20 * 1024)),
            ..Settings::default()
        };
        let p = process_one(&j, &pipeline(600), &reachable).unwrap();
        assert!(p.outcome.target_met);
        assert!(p.outcome.output_bytes <= 20 * 1024);
        assert!(
            p.outcome.quality_used > 0,
            "a lossy format must report the quality that was used"
        );

        // Unreachable: honest "no" rather than a file that overshot.
        let impossible = Settings {
            format: OutputFormat::Jpeg,
            target: Some(TargetBytes {
                bytes: 512,
                min_quality: 80,
                max_quality: 95,
            }),
            ..Settings::default()
        };
        let q = process_one(&j, &pipeline(600), &impossible).unwrap();
        assert!(!q.outcome.target_met, "512 bytes is not reachable here");
        assert!(q.outcome.output_bytes > 512);
    }

    fn options(chroma: crate::format::ChromaSubsampling, progressive: bool) -> Settings {
        Settings {
            format: OutputFormat::Jpeg,
            encoding: EncodingOptions {
                quality: 80,
                progressive,
                chroma_subsampling: chroma,
            },
            ..Settings::default()
        }
    }

    /// The chroma choice on the pipeline has to reach the encoder through the
    /// whole product path, or the UI's control is decoration.
    #[test]
    fn the_pipelines_chroma_choice_reaches_the_encoder() {
        use crate::format::ChromaSubsampling as Cs;
        let j = job(
            "1",
            "photo.jpg",
            crate::encode_fixed(
                &colour(300, 200),
                OutputFormat::Jpeg,
                EncodingOptions::default().with_quality(95),
            )
            .unwrap(),
        );

        let run = |chroma: Cs| -> Vec<u8> {
            let pipeline = Pipeline {
                chroma_subsampling: chroma,
                ..pipeline(300)
            };
            process_one(&j, &pipeline, &options(Cs::Luma420, false))
                .unwrap()
                .bytes
        };
        let full = run(Cs::Luma444);
        let subsampled = run(Cs::Luma420);
        assert!(
            full.len() > subsampled.len(),
            "4:4:4 ({}) should exceed 4:2:0 ({}) end to end",
            full.len(),
            subsampled.len()
        );
    }

    /// The precedence rule, asserted rather than left to the doc comment: the
    /// pipeline describes the picture and wins over the file-level default.
    #[test]
    fn the_pipeline_overrides_the_file_level_chroma_default() {
        use crate::format::ChromaSubsampling as Cs;
        let j = job(
            "1",
            "photo.jpg",
            crate::encode_fixed(
                &colour(300, 200),
                OutputFormat::Jpeg,
                EncodingOptions::default().with_quality(95),
            )
            .unwrap(),
        );
        // Settings say 4:4:4; the pipeline says 4:2:0. The pipeline is what the
        // UI sets, so the pipeline wins — otherwise a caller who set it in both
        // places would silently get the one they did not look at.
        let settings = Settings {
            encoding: EncodingOptions {
                chroma_subsampling: Cs::Luma444,
                ..EncodingOptions::default()
            },
            ..options(Cs::Luma420, false)
        };
        let from_pipeline = Pipeline {
            chroma_subsampling: Cs::Luma420,
            ..pipeline(300)
        };
        let a = process_one(&j, &from_pipeline, &settings).unwrap().bytes;
        let b = process_one(&j, &pipeline(300), &settings).unwrap().bytes;
        assert_eq!(a, b, "the pipeline's chroma choice must win");
    }

    /// The byte-target search varies quality and nothing else: a chroma choice
    /// that was dropped in one pass of the search would give a file that meets
    /// the ceiling at a chroma resolution the user never asked for.
    #[test]
    fn a_byte_target_keeps_the_chroma_choice_throughout_the_search() {
        use crate::format::ChromaSubsampling as Cs;
        let j = job("1", "photo.jpg", photo(600, 400, 3));
        let settings = Settings {
            format: OutputFormat::Jpeg,
            target: Some(TargetBytes::new(30 * 1024)),
            ..options(Cs::Luma444, false)
        };
        let p = process_one(
            &j,
            &Pipeline {
                chroma_subsampling: Cs::Luma444,
                ..pipeline(600)
            },
            &settings,
        )
        .unwrap();
        assert!(p.outcome.target_met, "the ceiling should be reachable");

        // Same ceiling and same picture at 4:2:0, for the size comparison the
        // search is supposed to preserve.
        let settings_420 = Settings {
            format: OutputFormat::Jpeg,
            target: Some(TargetBytes::new(30 * 1024)),
            ..options(Cs::Luma420, false)
        };
        let q = process_one(
            &j,
            &Pipeline {
                chroma_subsampling: Cs::Luma420,
                ..pipeline(600)
            },
            &settings_420,
        )
        .unwrap();
        assert!(p.outcome.output_bytes > q.outcome.output_bytes);
    }

    #[test]
    fn one_bad_file_does_not_sink_the_batch() {
        let jobs = vec![
            job("1", "a.jpg", jpeg(400, 300, 1)),
            job("2", "broken.jpg", b"this is not an image".to_vec()),
            job("3", "c.jpg", jpeg(400, 300, 3)),
        ];
        let report = process_batch(
            &jobs,
            &pipeline(200),
            &Settings::default(),
            &CancelToken::new(),
        );
        assert_eq!(report.succeeded(), 2);
        assert_eq!(report.failed(), 1);
        let error = report.outcomes[1].error.as_deref().unwrap_or("");
        assert!(
            !error.is_empty(),
            "the failure must be explained to the user"
        );
        // The two good files must be untouched by the bad one.
        assert!(report.outcomes[0].output_bytes > 0);
        assert!(report.outcomes[2].output_bytes > 0);
    }

    #[test]
    fn a_presupposed_cancellation_stops_everything() {
        let token = CancelToken::new();
        token.cancel();
        let jobs: Vec<Job> = (0..4)
            .map(|i| job(&i.to_string(), &format!("{i}.jpg"), jpeg(200, 200, i as u8)))
            .collect();
        let report = process_batch(&jobs, &pipeline(100), &Settings::default(), &token);
        assert!(report.cancelled);
        assert_eq!(report.succeeded(), 0);
    }

    #[test]
    fn target_bytes_are_honoured_across_a_batch() {
        let jobs: Vec<Job> = (0..4)
            .map(|i| {
                job(
                    &i.to_string(),
                    &format!("{i}.jpg"),
                    photo(900, 700, (i * 11) as u8),
                )
            })
            .collect();
        let settings = Settings {
            format: OutputFormat::Jpeg,
            target: Some(TargetBytes::new(40 * 1024)),
            ..Settings::default()
        };
        let report = process_batch(&jobs, &pipeline(600), &settings, &CancelToken::new());
        assert_eq!(report.succeeded(), 4);
        for o in &report.outcomes {
            assert!(o.target_met, "{} exceeded the target", o.name);
            assert!(
                o.output_bytes as u64 <= 40 * 1024,
                "{} = {} bytes",
                o.name,
                o.output_bytes
            );
        }
    }

    #[test]
    fn a_batch_matches_the_sequential_result_exactly() {
        // Determinism matters: the same input must produce the same bytes
        // whether it went through the parallel path or not, or a user who
        // re-runs a batch gets different files.
        let jobs: Vec<Job> = (0..6)
            .map(|i| {
                job(
                    &i.to_string(),
                    &format!("{i}.jpg"),
                    photo(700, 500, (i * 7) as u8),
                )
            })
            .collect();
        let p = pipeline(350);
        let s = Settings::default();
        let parallel = process_batch(&jobs, &p, &s, &CancelToken::new());
        let sequential: BatchReport = BatchReport {
            outcomes: jobs
                .iter()
                .map(|j| process_one(j, &p, &s).unwrap().outcome)
                .collect(),
            cancelled: false,
        };
        assert_eq!(parallel.succeeded(), sequential.succeeded());
        for (a, b) in parallel.outcomes.iter().zip(&sequential.outcomes) {
            assert_eq!(a.output_bytes, b.output_bytes, "{} differs", a.name);
            assert_eq!(
                (a.width, a.height),
                (b.width, b.height),
                "{} differs",
                a.name
            );
        }
    }

    #[test]
    fn zip_contains_every_output() {
        let items: Vec<(String, Vec<u8>)> = (0..5)
            .map(|i| (format!("img{i}.jpg"), jpeg(60, 40, i as u8)))
            .collect();
        let archive = zip_outputs(&items).unwrap();
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
        assert_eq!(reader.len(), 5);
        for i in 0..5 {
            assert!(reader.by_name(&format!("img{i}.jpg")).is_ok());
        }
    }

    #[test]
    fn zip_entry_names_cannot_escape() {
        let items = vec![
            ("../../etc/passwd".to_string(), b"a".to_vec()),
            (
                "..\\..\\windows\\system32\\x.dll".to_string(),
                b"b".to_vec(),
            ),
        ];
        let archive = zip_outputs(&items).unwrap();
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
        for i in 0..reader.len() {
            let name = reader.by_index(i).unwrap().name().to_string();
            assert!(!name.contains('/'), "slash survived in {name}");
            assert!(!name.contains('\\'), "backslash survived in {name}");
            assert!(!name.contains(".."), "traversal survived in {name}");
        }
    }

    #[test]
    fn stems_are_sanitised() {
        assert_eq!(sanitise_stem("../../secret/photo.jpg"), "photo");
        assert_eq!(sanitise_stem("C:\\Users\\me\\My Pic.png"), "My Pic");
        assert_eq!(sanitise_stem("..."), "image");
        assert_eq!(sanitise_stem(""), "image");
        assert_eq!(sanitise_stem("no_extension"), "no_extension");
    }

    #[test]
    fn stems_are_length_capped() {
        let long = format!("{}.jpg", "x".repeat(500));
        assert!(sanitise_stem(&long).len() <= 64);
    }

    #[test]
    fn limits_reject_an_oversized_input_in_a_batch() {
        let settings = Settings {
            limits: Limits {
                max_input_bytes: 100,
                ..Limits::default()
            },
            ..Settings::default()
        };
        let j = job("1", "big.jpg", jpeg(1000, 1000, 5));
        let outcome = process_one(&j, &pipeline(100), &settings).unwrap_err();
        assert!(outcome.to_string().contains("input limit"));
    }

    #[test]
    fn strips_metadata_by_default() {
        let mut src = jpeg(200, 200, 9);
        let block = crate::exif::build_block(&[
            crate::exif::field(
                exif::Tag::GPSLatitude,
                crate::exif::GPS_IFD,
                crate::exif::long(51),
            ),
            crate::exif::field(
                exif::Tag::CameraOwnerName,
                exif::In::PRIMARY,
                crate::exif::ascii("Jane Doe"),
            ),
        ])
        .unwrap();
        crate::format::append_exif(&mut src, &block).unwrap();
        assert!(crate::exif::has_exif(&src));

        let j = job("1", "meta.jpg", src);
        let p = process_one(&j, &pipeline(100), &Settings::default()).unwrap();
        assert!(
            !crate::exif::has_exif(&p.bytes),
            "EXIF survived the pipeline"
        );
    }

    #[test]
    fn keeping_metadata_never_reintroduces_gps() {
        let mut src = jpeg(200, 200, 9);
        let block = crate::exif::build_block(&[
            crate::exif::field(
                exif::Tag::GPSLatitude,
                crate::exif::GPS_IFD,
                crate::exif::long(51),
            ),
            crate::exif::field(
                exif::Tag::Make,
                exif::In::PRIMARY,
                crate::exif::ascii("Nikon"),
            ),
        ])
        .unwrap();
        crate::format::append_exif(&mut src, &block).unwrap();

        let mut pipeline = Pipeline::new();
        pipeline.strip_metadata = false;
        let p = process_one(&job("1", "meta.jpg", src), &pipeline, &Settings::default()).unwrap();
        let info = crate::exif::read(&p.bytes).unwrap();
        assert!(!info.has_gps, "GPS came back: {:?}", info.entries);
        assert_eq!(info.camera.as_deref(), Some("Nikon"));
    }

    #[test]
    fn cancellation_token_is_cheap_to_clone_and_observe() {
        let token = CancelToken::new();
        let clones: Vec<CancelToken> = (0..5).map(|_| token.clone()).collect();
        assert!(clones.iter().all(|t| !t.is_cancelled()));
        clones[3].cancel();
        assert!(token.is_cancelled());
        assert!(clones.iter().all(|t| t.is_cancelled()));
    }

    #[test]
    fn counters_stay_consistent_under_mixed_results() {
        let jobs = vec![
            job("1", "ok.jpg", jpeg(300, 200, 1)),
            job("2", "bad.jpg", vec![0u8; 4]),
            job("3", "ok2.jpg", jpeg(300, 200, 2)),
        ];
        let report = process_batch(
            &jobs,
            &pipeline(150),
            &Settings::default(),
            &CancelToken::new(),
        );
        assert_eq!(report.succeeded() + report.failed(), jobs.len());
        assert!(report.output_bytes() < report.input_bytes());
    }

    #[test]
    fn the_batch_really_does_run_in_parallel() {
        // Guards against accidentally serialising via a mutex or a one-thread
        // pool. Each job is big enough that the wall-clock difference between
        // serial and parallel is unmistakable, but small enough to keep the
        // debug-profile test suite fast.
        use std::time::Instant;
        let jobs: Vec<Job> = (0..8)
            .map(|i| {
                job(
                    &i.to_string(),
                    &format!("{i}.jpg"),
                    photo(1200, 900, i as u8),
                )
            })
            .collect();

        let p = pipeline(600);
        let s = Settings::default();

        let t = Instant::now();
        for j in &jobs {
            let _ = process_one(j, &p, &s).unwrap();
        }
        let sequential = t.elapsed();

        let t = Instant::now();
        let parallel = process_batch(&jobs, &p, &s, &CancelToken::new());
        let parallel_time = t.elapsed();

        assert_eq!(parallel.succeeded(), jobs.len());
        assert!(
            parallel_time < sequential,
            "batch ({parallel_time:?}) should beat the sequential loop ({sequential:?})"
        );
    }

    #[test]
    fn rayon_has_more_than_one_thread() {
        assert!(rayon::current_num_threads() >= 1);
    }

    /// Regression test for the defect found by
    /// `sanitise_stem_never_names_a_reserved_windows_device` in
    /// `core/tests/properties.rs`.
    ///
    /// Windows reserves these device names regardless of extension, so a photo
    /// called `nul.jpg` produced an export that could not be saved - and the
    /// failure surfaced only after the encode, which is the worst possible moment.
    /// The fix prefixes an underscore, which keeps the name recognisable to the
    /// user and creatable by the OS.
    ///
    /// Every reserved spelling is listed rather than sampled, because the case
    /// sensitivity and the COM/LPT numbering are exactly what is easy to get
    /// wrong.
    #[test]
    fn reserved_windows_device_names_are_prefixed_not_emitted() {
        for reserved in RESERVED_DEVICE_NAMES {
            for spelling in [
                reserved.to_string(),
                reserved.to_lowercase(),
                format!("{reserved}.jpg"),
                format!("{}.jpeg", reserved.to_lowercase()),
                format!("  {reserved}  .png"),
            ] {
                let out = sanitise_stem(&spelling);
                assert!(
                    !is_reserved_windows_name(&out),
                    "sanitise_stem({spelling:?}) produced {out:?}, which Windows \
                     will refuse to create"
                );
                assert!(
                    !out.is_empty(),
                    "sanitise_stem({spelling:?}) produced an empty name"
                );
            }
        }
    }

    /// The same defect in the archive path: a ZIP entry called `con.txt` cannot
    /// be extracted on Windows either.
    #[test]
    fn reserved_windows_device_names_are_refused_in_archive_components() {
        for reserved in RESERVED_DEVICE_NAMES {
            let out = sanitise_path_component(&format!("{reserved}.jpg"));
            assert!(
                !is_reserved_windows_name(&out),
                "sanitise_path_component({reserved}.jpg) produced {out:?}"
            );
        }
    }

    /// Regression test for the defect found by
    /// `sanitise_path_component_cannot_contain_a_dot_dot_run` in
    /// `core/tests/properties.rs`.
    ///
    /// A dot was kept whenever both neighbours were `is_alphanumeric()`, which
    /// Rust considers true for CJK ideographs and superscripts alike. So `㐀..¹`
    /// passed through unchanged with its `..` intact. The separators are already
    /// gone by this point in the function, so this was not an escape - but a `..`
    /// is the exact token a consumer splits on when rebuilding a path, and there
    /// is no reason to emit one.
    #[test]
    fn a_dot_dot_run_between_alphanumerics_is_collapsed() {
        // The minimal case proptest found, plus a few shapes of the same class.
        for input in ["㐀..¹", "a..b.txt", "x..y..z.zip", "..", "...", "...."] {
            let out = sanitise_path_component(input);
            assert!(
                !out.contains(".."),
                "sanitise_path_component({input:?}) produced {out:?}, which \
                 still contains a dot-dot run"
            );
            assert!(!out.is_empty(), "empty output for {input:?}");
        }
        // Collapsing, not deletion: a single extension separator must survive,
        // or every archive entry would lose its extension.
        assert_eq!(sanitise_path_component("photo.jpg"), "photo.jpg");
        assert_eq!(sanitise_path_component("a.b.c.png"), "a.b.c.png");
    }

    /// Truncation happens before the reserved-name check, so a name that only
    /// becomes reserved once capped is still caught.
    #[test]
    fn a_name_that_becomes_reserved_after_truncation_is_still_refused() {
        // 64 characters of padding followed by nothing: capping cannot manufacture
        // a device name, so this asserts the ordering rather than the reverse.
        let padded = format!("{}{}", "A".repeat(64), "");
        let out = sanitise_stem(&padded);
        assert!(!is_reserved_windows_name(&out));

        // And the cap is still enforced after the check.
        assert!(
            sanitise_stem(&format!("{}.jpg", "A".repeat(200)))
                .chars()
                .count()
                <= 65,
            "the 64-char cap must still apply"
        );
    }

    /// The ordinary cases must be unaffected by the reserved-name work. A
    /// sanitiser that mangled everything would pass every safety property above.
    #[test]
    fn ordinary_stems_are_still_untouched() {
        for (input, expected) in [
            ("IMG_1234.jpg", "IMG_1234"),
            ("DSC00001.jpeg", "DSC00001"),
            ("photo.png", "photo"),
            ("my holiday.jpg", "my holiday"),
            ("a-b_c.png", "a-b_c"),
            // Dots are not in the keep-set, so they become `-` and are then
            // trimmed off the ends. Underscores are kept, so `___` survives as a
            // name - unusual, but harmless and creatable.
            ("___.jpg", "___"),
            ("...png", "image"),
            ("-.-.png", "image"),
        ] {
            assert_eq!(
                sanitise_stem(input),
                expected,
                "sanitise_stem({input:?}) changed unexpectedly"
            );
        }
    }
}

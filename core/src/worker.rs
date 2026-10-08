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

use crate::animation::{self, AnimationOutcome, AnimationPolicy};
use crate::colour::ColourOutcome;
use crate::error::{Error, Result};
use crate::format::{EncodingOptions, OutputFormat};
#[cfg(feature = "streaming")]
use crate::pipeline::Orientation;
use crate::pipeline::Pipeline;
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
    /// What to do when the output format cannot hold every frame of an
    /// animation. The default keeps the frames or refuses the export; see
    /// [`crate::animation`] and `docs/GIF.md`.
    pub animation: AnimationPolicy,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            format: OutputFormat::Jpeg,
            encoding: EncodingOptions::default(),
            target: None,
            limits: Limits::default(),
            animation: AnimationPolicy::Keep,
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

/// What the numbers in a [`SkipReason::TooLarge`] are counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeUnit {
    Bytes,
    Pixels,
}

/// Why a file produced no output on purpose.
///
/// A skip is not a failure and not a crash: the engine read the file, decided,
/// and can say why in a sentence ([`SkipReason::note`]). What it must never do
/// is decide quietly. A folder of 400 that turns into 370 with nothing to show
/// for the other thirty is the one answer hard rule 9 forbids, and it is the
/// reason this is an enum with one variant per answer rather than a boolean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SkipReason {
    /// Another file in this batch already exported this picture. `of` is that
    /// file's name, so the report says which one was kept — and not which one
    /// should have been: under parallelism either may win the race.
    Duplicate { of: String },
    /// The bytes are not a picture this build can open, or the picture in them
    /// would not decode. Not the engine's fault and not the format's: the file
    /// is damaged, or it is not the image its name claims.
    Unreadable,
    /// A real image in a container this build cannot decode. The user's file is
    /// fine — the limitation is ours, and `format` names it so the report can
    /// point at the capability flag that says so.
    UnsupportedFormat { format: OutputFormat },
    /// Over a ceiling the limit profile states. `unit` says which: the input
    /// byte limit, or the decoded pixel budget, which is the same idea applied to
    /// the picture rather than to the file it arrived in.
    TooLarge {
        limit: u64,
        actual: u64,
        unit: SizeUnit,
    },
    /// The request asks for a bigger picture than the file has and the pipeline
    /// refused to invent the detail. See
    /// [`crate::pipeline::Pipeline::refused_upscale`] and the argument for it in
    /// `docs/ARCHITECTURE.md`.
    WouldUpscale {
        /// What the request asked for, which the pipeline would not deliver.
        requested: (u32, u32),
        /// What the file actually is.
        actual: (u32, u32),
    },
}

impl SkipReason {
    /// A sentence for the person whose folder this is.
    ///
    /// Written per variant rather than from a template, because the two cases
    /// that look alike need different next steps: a file that is not an image
    /// needs nothing from the user, while one this build cannot open is our
    /// limitation and has to say which one.
    pub fn note(&self) -> String {
        match self {
            Self::Duplicate { of } => {
                format!("the same picture is already exported as {of}")
            }
            Self::Unreadable => {
                "this file could not be read as an image; it may be damaged, or not the \
                 kind of file its name says"
                    .to_string()
            }
            Self::UnsupportedFormat { format } => format!(
                "this build cannot open {format:?} files; convert them to JPEG, PNG, WebP \
                 or GIF, or use a build with that decoder"
            ),
            Self::TooLarge {
                limit,
                actual,
                unit,
            } => match unit {
                SizeUnit::Bytes => {
                    format!("this file is {actual} bytes and this device accepts up to {limit}")
                }
                SizeUnit::Pixels => format!(
                    "this picture is {actual} pixels once decoded and this device accepts \
                     up to {limit}"
                ),
            },
            Self::WouldUpscale { requested, actual } => format!(
                "this picture is {}x{} and the request asked for {}x{}; enlarging it adds \
                 no detail, so it was left alone",
                actual.0, actual.1, requested.0, requested.1
            ),
        }
    }

    /// Whether this is the case the phase prompt opens with: a picture this
    /// batch already has.
    pub fn is_duplicate(&self) -> bool {
        matches!(self, Self::Duplicate { .. })
    }

    /// Which variant this is, ignoring the detail.
    ///
    /// [`BatchReport::skip_reasons`] groups on this, because the detail is what
    /// differs between two duplicates — each names a different file that held the
    /// key — and a summary that reads "30 duplicates" is the thing a report is
    /// for. Grouping on the whole value would produce thirty groups of one.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Duplicate { .. } => "duplicate",
            Self::Unreadable => "unreadable",
            Self::UnsupportedFormat { .. } => "unsupported_format",
            Self::TooLarge { .. } => "too_large",
            Self::WouldUpscale { .. } => "would_upscale",
        }
    }
}

/// What a batch does with a file it will not process.
///
/// A field rather than a mode, because the three answers are independent: a
/// caller may want duplicates collapsed and still want a corrupt file reported
/// as a failure, or the reverse. Every one of them defaults to on, because the
/// default for *this* function — a folder of many files — is the one where
/// quietly dropping or quietly failing are both worse.
///
/// None of it applies to [`process_one`], which has no batch to reason about:
/// a user who picks one file and gets an error is being told the truth about
/// that file, and a folder is a different question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchPolicy {
    /// Collapse a file whose content is already exported. See
    /// [`crate::dedupe`].
    pub deduplicate: bool,
    /// Report a file that cannot be processed at all — not an image, damaged, or
    /// over a ceiling — as a skip with a reason rather than as a failure.
    pub skip_unprocessable: bool,
    /// Report a file whose requested geometry is larger than the picture as a
    /// skip rather than writing it out at its own size. See
    /// [`crate::pipeline::Pipeline::refused_upscale`].
    pub skip_upscales: bool,
}

impl Default for BatchPolicy {
    fn default() -> Self {
        Self {
            deduplicate: true,
            skip_unprocessable: true,
            skip_upscales: true,
        }
    }
}

impl BatchPolicy {
    /// Every answer off: report everything as a failure, as a caller that wants
    /// to handle its own reporting would.
    pub fn report_everything() -> Self {
        Self {
            deduplicate: false,
            skip_unprocessable: false,
            skip_upscales: false,
        }
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
    /// What the engine did about colour, so the UI can say "this was a
    /// Display-P3 photo and is now sRGB" next to the file it just wrote rather
    /// than leaving the user to notice.
    pub colour: ColourOutcome,
    /// What happened to this file's frames.
    ///
    /// Present on every outcome, including failures and cancellations, because
    /// a caller must be able to ask "did the animation survive?" without
    /// decoding the output and without parsing the error string. A failed file
    /// carries [`AnimationOutcome::unknown`], which claims nothing.
    pub animation: AnimationOutcome,
    /// Why no file was written, when no file was written on purpose.
    ///
    /// Present on every outcome and `None` whenever something was written, so a
    /// caller never has to infer it from an absent error: a skip is not a
    /// failure, and a batch report that conflated the two would report a
    /// deliberate decision as an error.
    pub skipped: Option<SkipReason>,
    pub error: Option<String>,
}

impl Outcome {
    /// True when a file was written. A skip is not a success and not a failure:
    /// it is the third thing, and `ok()` says "there is a file".
    pub fn ok(&self) -> bool {
        self.error.is_none() && self.skipped.is_none()
    }

    /// An outcome for a file the engine read and then declined to write.
    ///
    /// Every field a written file would have carried is at its "nothing happened"
    /// value — zero bytes, no dimensions, no output name — because reporting a
    /// size for a file that does not exist is the kind of small wrong number a
    /// UI cannot tell apart from a right one.
    pub fn skipped(job: &Job, reason: SkipReason) -> Self {
        Self {
            id: job.id.clone(),
            name: job.name.clone(),
            output_name: String::new(),
            input_bytes: job.bytes.len(),
            output_bytes: 0,
            width: 0,
            height: 0,
            // No quality was applied to nothing, which is what zero has meant
            // everywhere else on this struct.
            quality_used: 0,
            target_met: false,
            colour: ColourOutcome::unknown(),
            animation: AnimationOutcome::unknown(),
            skipped: Some(reason),
            error: None,
        }
    }

    /// An outcome for a file the engine could not process at all.
    pub fn failed(job: &Job, error: &Error, animation: AnimationOutcome) -> Self {
        Self {
            id: job.id.clone(),
            name: job.name.clone(),
            output_name: String::new(),
            input_bytes: job.bytes.len(),
            output_bytes: 0,
            width: 0,
            height: 0,
            quality_used: 0,
            target_met: false,
            colour: ColourOutcome::unknown(),
            animation,
            skipped: None,
            error: Some(error.to_string()),
        }
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
        self.outcomes
            .iter()
            .filter(|o| !o.ok() && o.skipped.is_none())
            .count()
    }

    /// Files the engine read and then chose not to write, each with a reason.
    ///
    /// Its own number rather than `outcomes.len() - succeeded() - failed()`,
    /// because a caller showing "370 of 400" needs to be able to say *why* the
    /// other thirty are not failures, and a subtraction cannot.
    pub fn skipped(&self) -> usize {
        self.outcomes.iter().filter(|o| o.skipped.is_some()).count()
    }

    /// Of the skipped files, how many were pictures this batch had already
    /// exported. The headline number for the case this phase exists for: 370
    /// written out of 400 offered.
    pub fn duplicates(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.skipped, Some(SkipReason::Duplicate { .. })))
            .count()
    }

    /// Every kind of skip that occurred, with how many files carried it and one
    /// representative reason, so a report can be summarised rather than
    /// re-derived by the UI.
    pub fn skip_reasons(&self) -> Vec<(SkipReason, usize)> {
        let mut out: Vec<(SkipReason, usize)> = Vec::new();
        for reason in self.outcomes.iter().filter_map(|o| o.skipped.clone()) {
            match out
                .iter_mut()
                .find(|(known, _)| known.kind() == reason.kind())
            {
                Some((_, count)) => *count += 1,
                None => out.push((reason, 1)),
            }
        }
        out
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
///
/// Called without a [`BatchContext`], because a batch is the only place where
/// "this file is not worth writing" is a question with an answer: with one file
/// on screen the honest response to anything the engine cannot do is to say so
/// and fail. [`process_batch`] supplies the context and the same code runs.
pub fn process_one(job: &Job, pipeline: &Pipeline, settings: &Settings) -> Result<Processed> {
    process_one_within(job, pipeline, settings, None)
}

/// Shared by [`process_one`] and [`process_batch`].
fn process_one_within(
    job: &Job,
    pipeline: &Pipeline,
    settings: &Settings,
    batch: Option<&BatchContext>,
) -> Result<Processed> {
    // A request this build cannot carry out is refused before anything is
    // decoded, so the user gets a sentence about their settings rather than a
    // file that quietly misses the ceiling they asked for.
    settings.validate()?;
    // The same for the two colour answers that contradict each other: refused here
    // rather than resolved by picking a winner at the encode.
    pipeline.colour.validate()?;
    // Header check first: it rejects a hostile file before we allocate a
    // pixel buffer for it. The streaming path re-reads it rather than trusting
    // this call, because it is the header check that makes row-at-a-time decoding
    // safe.
    let report = crate::validate::validate_bytes(&job.bytes, &settings.limits)?;

    // Whether the request asks for a bigger picture than this file is, decided
    // from the header alone — no decode, and no encode, to find out. Inside a
    // batch that is a skip with a reason; for a single file it is not a question
    // at all, so this arm does not exist there.
    if let Some(ctx) = batch
        && ctx.policy.skip_upscales
        && let Some((wanted_w, wanted_h)) = pipeline.refused_upscale(report.width, report.height)?
    {
        return Err(Error::WouldUpscale {
            requested_width: wanted_w,
            requested_height: wanted_h,
            actual_width: report.width,
            actual_height: report.height,
        });
    }

    // What happens to the frames is decided before a pixel is decoded, so a
    // refusal costs the header walk that found them and nothing more. Three
    // outcomes: a still runs the ordinary single-image path below, an animation
    // into a format that holds several goes frame by frame through
    // `animation::preserve`, and an animation into a format that holds one is
    // either flattened on request or refused here.
    let animation = animation::decide(report.frames, settings.format, settings.animation)?;

    // The pipeline's chroma choice is the authority: it is the one a UI sets on
    // the picture, and `Settings::encoding` is the fallback for a caller that
    // never touches the pipeline. See `Pipeline::chroma_subsampling`. Computed
    // here rather than further down because the animation path needs it too.
    let options = settings.encoding.with_chroma(pipeline.chroma_subsampling);

    if animation.is_preserved() {
        return process_animation(job, pipeline, settings, &report, animation, batch);
    }

    let decoded = crate::decode_bounded(&job.bytes, &settings.limits)?;

    // Colour first, geometry second, and it has to be first: the source space is a
    // fact about the file, the working space is where the rest of the chain
    // expects to be, and converting after a downscale would resample values in the
    // wrong space. It is also a no-op for an untagged or sRGB file, so the common
    // path allocates nothing — see `colour::apply`.
    let (working_colour, colour) =
        crate::colour::apply(&decoded, &report.colour, pipeline.colour, settings.format)?;

    // The duplicate check sits here, between the decode and the geometry, for
    // two reasons. It costs nothing extra: the picture is already in hand, and
    // the key is a row-at-a-time hash of it rather than a second decode. And it
    // is after the colour conversion, so a Display-P3 copy of a photo and an
    // sRGB copy of it are one picture — which is what a user means by "the same
    // photo" — rather than two, because from here on they are the same bytes.
    if let Some(ctx) = batch
        && ctx.policy.deduplicate
    {
        let key = crate::dedupe::ContentKey::from_pixels(&working_colour, pipeline, settings);
        if let Some(first) = ctx.dedup.claim(key, &job.name) {
            return Err(Error::Duplicate { of: first });
        }
    }

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
        colour.converted,
    )? {
        Some(img) => img,
        None => pipeline.apply(&working_colour)?,
    };
    #[cfg(not(feature = "streaming"))]
    let out = pipeline.apply(&working_colour)?;

    // Re-encoding from raw samples is what actually removes EXIF. If the user
    // opted to keep metadata we graft it back, minus anything sensitive.
    let working = if pipeline.strip_metadata {
        crate::exif::strip(&out)?
    } else {
        out
    };

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

    // The opt-in third answer to "what happens to the profile": carry the source
    // profile into the output so the values keep their meaning for a
    // colour-managed viewer. The original bytes are re-read rather than cached in
    // the report, because putting three kilobytes of profile in every
    // `ValidateReport` the UI receives would be absurd.
    if colour.profile_embedded
        && let Some(icc) = crate::colour::profile_bytes(&job.bytes, report.format)
    {
        match settings.format {
            OutputFormat::Jpeg => crate::format::append_icc(&mut bytes, &icc)?,
            OutputFormat::Png => crate::format::png_insert_icc(&mut bytes, &icc)?,
            // Refused in `colour::apply` before a pixel was decoded, so this arm
            // is unreachable rather than merely unlikely.
            other => {
                return Err(Error::ColourProfile(other.icc_note()));
            }
        }
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
        colour,
        animation,
        skipped: None,
        error: None,
    };
    Ok(Processed { outcome, bytes })
}

/// The animation branch of [`process_one`]: every frame through the pipeline,
/// one animation out.
///
/// Separate from the still path rather than a flag inside it, because almost
/// everything the still path does is a no-op here and pretending otherwise is how
/// a reader ends up looking for the ICC arm in the wrong place: metadata is
/// already gone (the frames are built from decoded samples, which is what
/// `exif::strip` does for a still), no profile can be embedded (`colour::apply`
/// refuses GIF, the only format this reaches), EXIF write-back is JPEG-only, and
/// a byte ceiling is impossible because `Settings::validate` refuses a ceiling on
/// a format with no quality setting.
fn process_animation(
    job: &Job,
    pipeline: &Pipeline,
    settings: &Settings,
    report: &crate::validate::ValidateReport,
    animation: AnimationOutcome,
    batch: Option<&BatchContext>,
) -> Result<Processed> {
    let preserved = animation::preserve(
        &job.bytes,
        report,
        &settings.limits,
        pipeline,
        settings.format,
    )?;
    // `preserve` refuses rather than shortening the animation, so this is an
    // assertion about an invariant it enforces, not a second guess at the count.
    let animation = AnimationOutcome::preserved(preserved.frames, animation.policy);

    // The same duplicate check as the still path, keyed on what `preserve` built
    // rather than on the decoded frames: those frames are already gone by now, and
    // the encoded animation is the one canonical form this engine produces. See
    // `ContentKey::from_encoded`.
    if let Some(ctx) = batch
        && ctx.policy.deduplicate
    {
        let key = crate::dedupe::ContentKey::from_encoded(&preserved.bytes, pipeline, settings);
        if let Some(first) = ctx.dedup.claim(key, &job.name) {
            return Err(Error::Duplicate { of: first });
        }
    }

    let stem = sanitise_stem(&job.name);
    let outcome = Outcome {
        id: job.id.clone(),
        name: job.name.clone(),
        output_name: format!("{stem}.{}", settings.format.extension()),
        input_bytes: job.bytes.len(),
        output_bytes: preserved.bytes.len(),
        width: preserved.width,
        height: preserved.height,
        // GIF is palette-quantised, so no quality setting was applied and none is
        // reported — the same rule the still path uses.
        quality_used: 0,
        // No ceiling was asked for, or one was refused before this point: GIF has
        // no quality setting for a search to move.
        target_met: settings.target.is_none(),
        colour: preserved.colour,
        animation,
        skipped: None,
        error: None,
    };
    Ok(Processed {
        outcome,
        bytes: preserved.bytes,
    })
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
    colour_converted: bool,
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
    // A colour conversion needs a decoded picture to apply to, and this path
    // exists so that a large source never becomes one. The two genuinely do not
    // fit in one pass: converting afterwards would resample values in the source
    // space and converting beforehand would materialise exactly what the flag was
    // added to avoid. So a wide-gamut file takes the in-memory path, which is
    // also where every P3 file went before this phase.
    if colour_converted {
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

/// What a batch shares between its workers: the deduplication table and the
/// policy that decides what a skip is.
pub struct BatchContext {
    policy: BatchPolicy,
    dedup: crate::dedupe::Dedup,
}

impl BatchContext {
    pub fn new(policy: BatchPolicy) -> Self {
        Self {
            policy,
            dedup: crate::dedupe::Dedup::new(),
        }
    }
}

/// Run a batch in parallel. Failures are collected, not propagated.
///
/// Four things are decided here rather than in [`process_one`], because they are
/// questions about a *folder* and have no answer for a single file:
///
/// * one picture exported once, via [`crate::dedupe`];
/// * a file the engine cannot process reported as a skip with a
///   [`SkipReason`] rather than as a failure;
/// * a file the request cannot be carried out on, reported the same way; and
/// * the pool, which [`crate::folder::pool_size`] sizes to the machine's memory
///   as well as its core count.
pub fn process_batch(
    jobs: &[Job],
    pipeline: &Pipeline,
    settings: &Settings,
    policy: &BatchPolicy,
    cancel: &CancelToken,
) -> BatchReport {
    let context = BatchContext::new(*policy);
    process_all(jobs, pipeline, settings, &context, cancel, |job: &Job| {
        Ok::<_, (String, Error)>(std::borrow::Cow::Borrowed(job))
    })
}

/// The one batch loop, over anything that can become a [`Job`].
///
/// The loader is what lets the folder path ([`crate::folder::process_folder`])
/// read each file from disk *inside* its own task rather than handing
/// `process_batch` a folder already resident in memory — 400 photographs is
/// gigabytes, and the alternative is a batch that OOMs before it starts. A
/// `Cow` rather than two functions because the in-memory case must not copy the
/// bytes it was given.
pub(crate) fn process_all<'a, T, L>(
    items: &'a [T],
    pipeline: &Pipeline,
    settings: &Settings,
    context: &BatchContext,
    cancel: &CancelToken,
    load: L,
) -> BatchReport
where
    T: Sync,
    L: Fn(&'a T) -> Loaded<'a> + Sync,
{
    // The outcome is built inside the closure rather than after it, and that is
    // load-bearing rather than tidy: the loader may have read the file from disk
    // to produce the job, and doing that twice to recover a name for the error
    // would read 400 photographs off the disk twice. `par_iter().map().collect()`
    // preserves order, so the report still lines up with the input list.
    let outcomes: Vec<Outcome> = items
        .par_iter()
        .map(|item| {
            if cancel.is_cancelled() {
                return cancelled_outcome(&load(item));
            }
            let job = match load(item) {
                Ok(job) => job,
                // A file that could not even be read is reported with its name
                // and no bytes, rather than being dropped from the report: a
                // folder of 400 that lost a file to a read error would otherwise
                // report 399 outcomes and no reason for the missing one.
                Err((name, error)) => {
                    return Outcome::skipped(
                        &Job {
                            id: String::new(),
                            name,
                            bytes: Vec::new(),
                        },
                        skip_reason(&error, context.policy).unwrap_or(SkipReason::Unreadable),
                    );
                }
            };
            match process_one_within(job.as_ref(), pipeline, settings, Some(context)) {
                Ok(processed) => processed.outcome,
                Err(e) => match skip_reason(&e, context.policy) {
                    Some(reason) => Outcome::skipped(&job, reason),
                    None => {
                        // A refusal is the one failure where the frames are known and
                        // the file is fine, so it reports them rather than claiming
                        // nothing happened. Every other failure is
                        // `AnimationOutcome::unknown()`.
                        let animation = match &e {
                            Error::AnimationRefused { frames, .. } => {
                                AnimationOutcome::refused(*frames, settings.animation)
                            }
                            _ => AnimationOutcome::unknown(),
                        };
                        Outcome::failed(&job, &e, animation)
                    }
                },
            }
        })
        .collect();

    BatchReport {
        outcomes,
        cancelled: cancel.is_cancelled(),
    }
}

/// What a batch loader produces: a job, or the name it could not produce one
/// for and why.
///
/// Spelled out rather than reusing [`Result`], because this one has a *two*-field
/// error: a file that cannot be read still has a name, and dropping it would mean
/// a report with an outcome nobody can attribute.
pub type Loaded<'a> = std::result::Result<std::borrow::Cow<'a, Job>, (String, Error)>;

/// The outcome for a file the batch never started, naming the file when the
/// loader could not even open it.
fn cancelled_outcome(loaded: &Loaded<'_>) -> Outcome {
    match loaded {
        Ok(job) => Outcome::failed(job.as_ref(), &Error::Cancelled, AnimationOutcome::unknown()),
        Err((name, _)) => Outcome::failed(
            &Job {
                id: String::new(),
                name: name.clone(),
                bytes: Vec::new(),
            },
            &Error::Cancelled,
            AnimationOutcome::unknown(),
        ),
    }
}

/// Classify a failure as a skip, when the policy says this kind of failure is
/// one.
///
/// Only *file-level* problems map: a file that is not an image, one that will
/// not decode, one over a ceiling, one this build cannot open. A refusal to
/// carry out the *request* stays a failure, because there is nothing about the
/// file to change and a folder is no reason to hide it.
fn skip_reason(error: &Error, policy: BatchPolicy) -> Option<SkipReason> {
    match error {
        Error::Duplicate { of } => Some(SkipReason::Duplicate { of: of.clone() }),
        Error::WouldUpscale {
            requested_width,
            requested_height,
            actual_width,
            actual_height,
        } => Some(SkipReason::WouldUpscale {
            requested: (*requested_width, *requested_height),
            actual: (*actual_width, *actual_height),
        }),
        Error::UnknownFormat => policy.skip_unprocessable.then_some(SkipReason::Unreadable),
        Error::Decode(_) => policy.skip_unprocessable.then_some(SkipReason::Unreadable),
        Error::Io(_) => policy.skip_unprocessable.then_some(SkipReason::Unreadable),
        Error::InputTooLarge { limit, actual } => {
            policy.skip_unprocessable.then_some(SkipReason::TooLarge {
                limit: *limit as u64,
                actual: *actual as u64,
                unit: SizeUnit::Bytes,
            })
        }
        Error::PixelBudgetExceeded { limit, actual } => {
            policy.skip_unprocessable.then_some(SkipReason::TooLarge {
                limit: *limit,
                actual: (*actual * 1_000_000.0).round() as u64,
                unit: SizeUnit::Pixels,
            })
        }
        Error::SuspiciousDimensions { w, h, .. } => {
            policy.skip_unprocessable.then_some(SkipReason::TooLarge {
                // The per-side ceiling is what was actually broken, so that is the
                // number the note compares against: the pixel budget is checked
                // after this one and never reached.
                limit: u64::from(*w.max(h)),
                actual: u64::from(*w) * u64::from(*h),
                unit: SizeUnit::Pixels,
            })
        }
        // A container this build has no decoder for, and nothing wrong with the
        // file. Anything else in this enum is the file's own problem, and saying
        // so would be blaming a user's photograph for our feature flags.
        Error::Heic(
            crate::heic::HeicError::NotBuilt | crate::heic::HeicError::UnsupportedCoding,
        ) => policy
            .skip_unprocessable
            .then_some(SkipReason::UnsupportedFormat {
                format: OutputFormat::Heic,
            }),
        // The container parsed and was then refused. That is the file's own
        // problem, so it does not borrow the sentence above.
        Error::Heic(crate::heic::HeicError::Rejected { .. }) => {
            policy.skip_unprocessable.then_some(SkipReason::Unreadable)
        }
        _ => None,
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
    use crate::colour::ColourSpace;

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

    /// A picture that is *distinct from every other seed*, for tests that need
    /// hundreds of them.
    ///
    /// `photo` above is deliberately compressible and varies by a phase offset, so
    /// its images repeat every few seeds — fine for one fixture, useless for the
    /// 370-distinct-pictures case, which silently deduplicated down to 55 and would
    /// have been green for the wrong reason. This one is keyed on a multiply-xor
    /// mix of the pixel and the seed, so no two of them agree.
    fn distinct(w: u32, h: u32, seed: u32) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let mut n = x
                .wrapping_mul(0x9e37_79b9)
                .wrapping_add(y)
                .wrapping_add(seed.wrapping_mul(0x85eb_ca6b))
                .wrapping_add(0x1234_5678);
            n ^= n >> 15;
            n = n.wrapping_mul(0x2545_f491);
            image::Rgb([(n >> 16) as u8, (n >> 8) as u8, n as u8])
        }));
        crate::encode_fixed(
            &img,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
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

    // -------------------------------------------------------------------------
    // phase-14: deduplication and skips
    // -------------------------------------------------------------------------

    /// The phase's headline claim, in the shape the prompt states it: two byte-
    /// different encodings of one picture are one output.
    ///
    /// The fixture is a flat field because that is the one shape a JPEG round trip
    /// is exact on, so the two files differ in every byte the container and the
    /// quantisation tables touch while decoding to the same 1 536 pixels. A file-
    /// bytes key would call these two; this must call them one.
    #[test]
    fn two_byte_different_encodings_of_one_picture_produce_one_output() {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            96,
            64,
            image::Rgb([128, 128, 128]),
        ));
        let at = |q: u8| {
            crate::encode_fixed(
                &img,
                OutputFormat::Jpeg,
                EncodingOptions::default().with_quality(q),
            )
            .unwrap()
        };
        let (high, low) = (at(95), at(75));
        assert_ne!(
            high, low,
            "the two encodings must differ or this is vacuous"
        );

        let jobs = vec![job("1", "first.jpg", high), job("2", "second.jpg", low)];
        let report = process_batch(
            &jobs,
            &Pipeline::new(),
            &Settings::default(),
            &BatchPolicy::default(),
            &CancelToken::new(),
        );
        assert_eq!(report.succeeded(), 1);
        assert_eq!(report.skipped(), 1);
        assert_eq!(report.duplicates(), 1);
        assert!(report.outcomes.iter().all(|o| o.error.is_none()));
        assert_eq!(
            report
                .outcomes
                .iter()
                .filter(|o| o.output_bytes > 0)
                .count(),
            1,
            "and only one file on disk"
        );
    }

    /// The other half of the claim: deduplication is *within a request*.
    ///
    /// The same file under a different resize is a different output, because the
    /// exports differ and merging them would hand the user one picture where they
    /// asked for two sizes. Two batches rather than one, because a batch has one
    /// pipeline — the case being tested is two requests, not two files.
    #[test]
    fn the_same_picture_under_two_pipelines_produces_two_outputs() {
        let bytes = photo(120, 90, 3);
        let jobs = vec![job("1", "a.jpg", bytes.clone())];
        let s = Settings::default();
        let first = process_batch(
            &jobs,
            &pipeline(60),
            &s,
            &BatchPolicy::default(),
            &CancelToken::new(),
        );
        let second = process_batch(
            &jobs,
            &pipeline(30),
            &s,
            &BatchPolicy::default(),
            &CancelToken::new(),
        );
        assert_eq!(first.succeeded(), 1);
        assert_eq!(second.succeeded(), 1);
        assert_eq!(first.outcomes.iter().find(|o| o.ok()).unwrap().width, 60);
        assert_eq!(second.outcomes.iter().find(|o| o.ok()).unwrap().width, 30);

        // And with a picture that *is* duplicated, each of the two requests still
        // collapses its own pair — the policy is per batch, not global.
        let pair = vec![job("1", "a.jpg", bytes.clone()), job("2", "b.jpg", bytes)];
        for width in [60, 30] {
            let report = process_batch(
                &pair,
                &pipeline(width),
                &s,
                &BatchPolicy::default(),
                &CancelToken::new(),
            );
            assert_eq!(report.succeeded(), 1, "width {width}");
            assert_eq!(report.duplicates(), 1, "width {width}");
            // Whichever of the two won the race is the one with a width: the other
            // is the duplicate, and which of them that is has never been promised.
            let written = report.outcomes.iter().find(|o| o.ok()).unwrap();
            assert_eq!(written.width, width);
        }
    }

    /// 400 files offered, 370 written, 30 skipped, every skip named.
    ///
    /// The prompt's arithmetic: a folder of 400 in which 30 pictures appear twice
    /// yields 370 distinct pictures. The two files of each duplicated pair are the
    /// *same bytes*, which is the case a real folder has — a library synced twice,
    /// a "copy 2" from a file manager — and the count is what the phase is for.
    #[test]
    fn four_hundred_files_with_thirty_duplicates_writes_three_hundred_and_seventy() {
        const TOTAL: usize = 400;
        const DUPLICATED: usize = 30;
        let distinct_pictures = TOTAL - DUPLICATED;

        let mut jobs: Vec<Job> = Vec::with_capacity(TOTAL);
        for i in 0..distinct_pictures {
            // Small on purpose: 400 photographs' worth of pixels would make this a
            // wall-clock measurement rather than an accounting one, and the
            // accounting is what is being asserted. The content still has to differ
            // per file, which is what the seed is for.
            jobs.push(job(
                &i.to_string(),
                &format!("{i:03}.jpg"),
                distinct(64, 48, i as u32),
            ));
        }
        for i in 0..DUPLICATED {
            let bytes = jobs[i].bytes.clone();
            jobs.push(job(&format!("copy{i}"), &format!("{i:03}-copy.jpg"), bytes));
        }

        let report = process_batch(
            &jobs,
            &pipeline(32),
            &Settings::default(),
            &BatchPolicy::default(),
            &CancelToken::new(),
        );
        assert_eq!(report.outcomes.len(), TOTAL);
        assert_eq!(report.succeeded(), distinct_pictures, "370 written");
        assert_eq!(report.skipped(), DUPLICATED, "30 skipped");
        assert_eq!(report.failed(), 0);
        assert_eq!(report.duplicates(), DUPLICATED);
        assert_eq!(report.skip_reasons().len(), 1, "one reason, thirty times");
        assert_eq!(report.skip_reasons()[0].1, DUPLICATED);

        // Every skip names a file that is in the same report and was written, so
        // the reason can be acted on rather than merely read.
        for outcome in report.outcomes.iter().filter(|o| o.skipped.is_some()) {
            let Some(SkipReason::Duplicate { of }) = &outcome.skipped else {
                panic!("expected a duplicate, got {:?}", outcome.skipped);
            };
            assert!(
                report.outcomes.iter().any(|o| o.name == *of && o.ok()),
                "{of} should be one of the files that was written"
            );
            assert_eq!(outcome.output_bytes, 0, "a skipped file has no bytes");
            assert!(outcome.error.is_none(), "a skip is not a failure");
        }

        // The same folder with deduplication off is 400 files and 400 outputs,
        // which is what this phase changed and what a caller can turn off.
        let report = process_batch(
            &jobs,
            &pipeline(32),
            &Settings::default(),
            &BatchPolicy::report_everything(),
            &CancelToken::new(),
        );
        assert_eq!(report.succeeded(), TOTAL);
        assert_eq!(report.skipped(), 0);
    }

    /// Every reason, produced by something real, in one place.
    ///
    /// The prompt asks for this because a skip reason with no test is a sentence
    /// nobody has checked — and because an unreachable variant is a lie told in a
    /// report: the UI would offer a row the engine can never fill. Four of the
    /// five come from files a person could actually hand over; the fifth has none
    /// in this configuration, because the `heic` feature is on and an HEIC decodes.
    #[test]
    fn every_skip_reason_is_reachable() {
        let s = Settings::default();
        let policy = BatchPolicy::default();
        let cancel = CancelToken::new();

        let flat = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            32,
            24,
            image::Rgb([120, 130, 140]),
        ));
        let at = |q: u8| {
            crate::encode_fixed(
                &flat,
                OutputFormat::Jpeg,
                EncodingOptions::default().with_quality(q),
            )
            .unwrap()
        };
        // Two copies of one picture, one file that is not an image, and one small
        // picture that a 4000 px request cannot be carried out on.
        let folder = vec![
            job("1", "a.jpg", at(95)),
            job("2", "b.jpg", at(75)),
            job("3", "junk.jpg", b"not an image at all".to_vec()),
            job("4", "small.jpg", photo(64, 48, 9)),
        ];
        let upscaler = Pipeline::new().with_resize(crate::pipeline::ResizeSpec {
            width: Some(4000),
            height: None,
            fit: crate::pipeline::FitMode::Width,
            no_upscale: true,
            ..Default::default()
        });
        let huge = photo(400, 300, 11);
        let tiny_limits = Settings {
            limits: crate::validate::Limits {
                max_input_bytes: 64,
                ..Default::default()
            },
            ..s
        };
        let oversized = vec![job("9", "huge.jpg", huge.clone())];

        let plain = process_batch(&folder, &Pipeline::new(), &s, &policy, &cancel);
        let upscaled = process_batch(&folder, &upscaler, &s, &policy, &cancel);
        let too_big = process_batch(&oversized, &Pipeline::new(), &tiny_limits, &policy, &cancel);

        let wanted = [
            (
                SkipReason::Unreadable,
                &plain,
                "bytes that are not a picture",
            ),
            (
                SkipReason::TooLarge {
                    limit: 64,
                    actual: huge.len() as u64,
                    unit: SizeUnit::Bytes,
                },
                &too_big,
                "a file past the input ceiling",
            ),
            (
                SkipReason::WouldUpscale {
                    requested: (4000, 3000),
                    actual: (64, 48),
                },
                &upscaled,
                "a picture smaller than the request",
            ),
        ];
        for (expected, report, what) in wanted {
            let found = report
                .outcomes
                .iter()
                .filter(|o| o.skipped.as_ref() == Some(&expected))
                .count();
            assert_eq!(found, 1, "{what}: {expected:?} never came back");
        }

        // The duplicate is checked apart from the others because of *which* file
        // it names. A batch runs in parallel and `Dedup::claim` is a race, so
        // "a.jpg" or "b.jpg" is whichever thread got there first — asserting one
        // of them here would be asserting the scheduler. What is guaranteed, and
        // what the assertion is about, is that the two encodings of one picture
        // became one output and the reason names a file that was written.
        let dup = plain
            .outcomes
            .iter()
            .find_map(|o| match &o.skipped {
                Some(SkipReason::Duplicate { of }) => Some(of.clone()),
                _ => None,
            })
            .expect("two encodings of one picture must produce one duplicate");
        assert!(
            ["a.jpg", "b.jpg"].contains(&dup.as_str()),
            "a duplicate may only name one of the two files that hold the picture, \
             not {dup}"
        );
        assert!(
            plain.outcomes.iter().any(|o| o.name == dup && o.ok()),
            "a duplicate names the file that was written, so {dup} was written"
        );

        // The fifth has no file this build can fail on, so it is reached through
        // the classification itself, with the exact error a build without the codec
        // produces. The error is a value here rather than a file, which is what
        // makes this half of the test independent of the feature set.
        assert_eq!(
            skip_reason(&Error::Heic(crate::heic::HeicError::NotBuilt), policy),
            Some(SkipReason::UnsupportedFormat {
                format: OutputFormat::Heic
            }),
            "a container this build cannot decode is our limitation, not a bad file"
        );
        // A container that was read and then refused is the file's own problem, and
        // must not borrow that sentence.
        assert_eq!(
            skip_reason(
                &Error::Heic(crate::heic::HeicError::Rejected {
                    detail: "no primary item".into()
                }),
                policy
            ),
            Some(SkipReason::Unreadable)
        );
    }

    /// Nothing here changes what a single-image request does.
    ///
    /// A user who picks one 64 px file and asks for 4000 px wide gets that file at
    /// its own size, exactly as before this phase: "would upscale" is a question
    /// about a folder, and answering it with a refusal would take away a working
    /// path to compress and re-encode a small picture.
    #[test]
    fn a_single_image_request_is_unchanged_by_the_skip_policy() {
        let j = job("1", "small.jpg", photo(64, 48, 2));
        let upscaler = Pipeline::new().with_resize(crate::pipeline::ResizeSpec {
            width: Some(4000),
            height: None,
            fit: crate::pipeline::FitMode::Width,
            no_upscale: true,
            ..Default::default()
        });
        let processed = process_one(&j, &upscaler, &Settings::default()).unwrap();
        assert!(processed.outcome.ok());
        assert_eq!(processed.outcome.skipped, None);
        assert_eq!(
            (processed.outcome.width, processed.outcome.height),
            (64, 48)
        );
        assert!(processed.outcome.output_bytes > 0, "a file was written");
    }

    /// Turning the skip answers off turns them back into failures.
    ///
    /// A caller that wants to handle its own reporting — a CLI counting errors, a
    /// CI job that must fail on one bad file — has to be able to say so, and
    /// `BatchPolicy::report_everything` is that switch.
    #[test]
    fn the_policy_is_what_turns_a_skip_into_a_failure() {
        let jobs = vec![
            job("1", "a.jpg", photo(80, 60, 1)),
            job("2", "b.jpg", b"not an image".to_vec()),
        ];
        let strict = process_batch(
            &jobs,
            &Pipeline::new(),
            &Settings::default(),
            &BatchPolicy::report_everything(),
            &CancelToken::new(),
        );
        assert_eq!(strict.failed(), 1);
        assert_eq!(strict.skipped(), 0);
        assert!(strict.outcomes[1].error.is_some());

        let lenient = process_batch(
            &jobs,
            &Pipeline::new(),
            &Settings::default(),
            &BatchPolicy::default(),
            &CancelToken::new(),
        );
        assert_eq!(lenient.failed(), 0);
        assert_eq!(lenient.skipped(), 1);
    }

    /// Every skip carries a sentence, and none of them is "skipped".
    #[test]
    fn every_skip_reason_says_something_to_the_user() {
        let reasons = [
            SkipReason::Duplicate {
                of: "one.jpg".into(),
            },
            SkipReason::Unreadable,
            SkipReason::UnsupportedFormat {
                format: OutputFormat::Heic,
            },
            SkipReason::TooLarge {
                limit: 1000,
                actual: 2000,
                unit: SizeUnit::Bytes,
            },
            SkipReason::TooLarge {
                limit: 40_000_000,
                actual: 60_000_000,
                unit: SizeUnit::Pixels,
            },
            SkipReason::WouldUpscale {
                requested: (1920, 1080),
                actual: (640, 480),
            },
        ];
        for reason in &reasons {
            let note = reason.note();
            assert!(note.len() > 20, "{reason:?} says nothing: {note:?}");
            assert!(!note.contains("Error:"), "{reason:?} leaks a diagnostic");
        }
        assert!(reasons[0].note().contains("one.jpg"));
        assert!(SkipReason::Duplicate { of: "x".into() }.is_duplicate());
        assert!(!SkipReason::Unreadable.is_duplicate());
    }

    /// A skipped outcome reports the file's size and nothing else.
    ///
    /// The temptation with a struct this wide is to leave a decoded width on a file
    /// that was never processed, and the UI would then show a picture's dimensions
    /// for a picture it does not have.
    #[test]
    fn a_skipped_outcome_reports_no_output_at_all() {
        let j = job("7", "x.jpg", vec![1, 2, 3, 4]);
        let outcome = Outcome::skipped(&j, SkipReason::Unreadable);
        assert_eq!(outcome.id, "7");
        assert_eq!(outcome.name, "x.jpg");
        assert_eq!(outcome.input_bytes, 4);
        assert_eq!(outcome.output_bytes, 0);
        assert_eq!((outcome.width, outcome.height), (0, 0));
        assert_eq!(outcome.quality_used, 0);
        assert!(outcome.output_name.is_empty());
        assert!(outcome.error.is_none());
        assert!(!outcome.ok());
        assert!(!outcome.animation.animated());
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
            &BatchPolicy::default(),
            &CancelToken::new(),
        );
        assert_eq!(report.succeeded(), 2);
        // A file that is not an image is a *skip* in a batch, not a failure: this
        // phase moved it there deliberately, because a folder of 200 with three
        // junk files in it is three lines of explanation rather than three
        // failures, and the explanation is still on the outcome.
        assert_eq!(report.failed(), 0);
        assert_eq!(report.skipped(), 1);
        assert_eq!(
            report.outcomes[1].skipped,
            Some(SkipReason::Unreadable),
            "the reason must be the one the plan would have given"
        );
        assert!(
            !report.outcomes[1]
                .skipped
                .as_ref()
                .unwrap()
                .note()
                .is_empty(),
            "the skip must be explained to the user"
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
        let report = process_batch(
            &jobs,
            &pipeline(100),
            &Settings::default(),
            &BatchPolicy::default(),
            &token,
        );
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
        let report = process_batch(
            &jobs,
            &pipeline(600),
            &settings,
            &BatchPolicy::default(),
            &CancelToken::new(),
        );
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
        let parallel = process_batch(&jobs, &p, &s, &BatchPolicy::default(), &CancelToken::new());
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
            &BatchPolicy::default(),
            &CancelToken::new(),
        );
        // Every file is accounted for exactly once, and a skip is a third
        // category rather than being folded into either of the other two: this
        // phase added it, and a report that lost one of the three would be a
        // report that cannot say how many files it handled.
        assert_eq!(
            report.succeeded() + report.failed() + report.skipped(),
            jobs.len()
        );
        assert_eq!(report.skipped(), 1);
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
        let parallel = process_batch(&jobs, &p, &s, &BatchPolicy::default(), &CancelToken::new());
        let parallel_time = t.elapsed();

        assert_eq!(parallel.succeeded(), jobs.len());
        assert!(
            parallel_time < sequential,
            "batch ({parallel_time:?}) should beat the sequential loop ({sequential:?})"
        );
    }

    /// The claim the batch path's speed rests on, asserted rather than assumed.
    ///
    /// This was `assert!(rayon::current_num_threads() >= 1)`, which is a
    /// tautology: it passes on a pool of one, on a build with `rayon` stubbed to
    /// run everything on the calling thread, and on any machine at all. The name
    /// claimed more than one thread and the body checked for at least one.
    ///
    /// Raising it to what the name says is the better of the two repairs the
    /// phase prompt offers, because "the batch path is parallel" is a claim this
    /// project makes in `docs/ARCHITECTURE.md` and in the folder plan's sizing —
    /// renaming the test to `rayon_has_at_least_one_thread` would have made the
    /// suite green while the claim went on being unmeasured.
    ///
    /// A single-core runner genuinely cannot satisfy it, and `RAYON_NUM_THREADS=1`
    /// is a supported way to configure rayon in production, so the skip is honest
    /// and names the machine rather than pretending the assertion held. Note what
    /// is *not* skipped on a single thread: `the_batch_really_does_run_in_parallel`
    /// above compares wall clock against a sequential loop and would fail there,
    /// which is the correct answer — on one core the batch path is not faster and
    /// the project should hear about it rather than be told it is.
    #[test]
    fn rayon_has_more_than_one_thread() {
        let threads = rayon::current_num_threads();
        assert!(
            threads >= 1,
            "rayon reported {threads} threads, which cannot happen; if this fires the \\
             pool is not initialised and the batch path is running nowhere"
        );
        if threads == 1 {
            eprintln!(
                "skipping: this machine gave rayon one thread ({}) and the batch path \\
                 cannot be parallel here",
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(0)
            );
            return;
        }
        assert!(
            threads > 1,
            "the batch path is documented as parallel across files and this pool has \\
             one thread; either the pool is mis-sized or the machine has one core, \\
             which is the case the message above reports"
        );
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

    // ---- colour ---------------------------------------------------------

    /// A flat mid-saturation red: the content that tells a colour conversion from
    /// a no-op, and a small enough picture that the assertion can name a pixel.
    fn red_patch() -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            32,
            32,
            image::Rgb([200, 100, 90]),
        ))
    }

    fn colour_job(name: &str, icc: Option<&[u8]>) -> Job {
        job(
            "1",
            name,
            crate::colour::fixtures::encode_with(&red_patch(), OutputFormat::Jpeg, icc),
        )
    }

    fn first_pixel(bytes: &[u8]) -> [u8; 3] {
        let px = image::load_from_memory(bytes)
            .unwrap()
            .to_rgb8()
            .get_pixel(0, 0)
            .0;
        [px[0], px[1], px[2]]
    }

    /// The phase's central claim, end to end: stripping the metadata of a
    /// Display-P3 photo must not leave the file's numbers in place.
    ///
    /// The comparison is against the naive path — the same photo exported with
    /// "keep the original colour values" — rather than against a hard-coded
    /// triple, because the naive path is exactly what the engine used to do and
    /// exactly what it must now not do. "The output is not identical" would be a
    /// weak assertion, so the direction is checked too: a P3 colour read as sRGB
    /// is duller, so the converted file has to be the *more* saturated one.
    #[test]
    fn a_p3_photo_stripped_of_its_profile_is_converted_rather_than_left_alone() {
        let j = colour_job("p3.jpg", Some(&crate::colour::fixtures::p3_profile()));
        let converted = process_one(&j, &Pipeline::new(), &Settings::default()).unwrap();
        let naive = process_one(
            &j,
            &Pipeline {
                colour: crate::colour::ColourOptions {
                    keep_source_pixels: true,
                    ..Default::default()
                },
                ..Pipeline::new()
            },
            &Settings::default(),
        )
        .unwrap();

        assert_eq!(converted.outcome.colour.source, ColourSpace::DisplayP3);
        assert_eq!(converted.outcome.colour.output, ColourSpace::Srgb);
        assert!(
            converted.outcome.colour.converted,
            "the report must say the pixels moved"
        );
        assert!(!naive.outcome.colour.converted);

        let (after, before) = (first_pixel(&converted.bytes), first_pixel(&naive.bytes));
        assert_ne!(
            after, before,
            "converting to sRGB must change the pixels; {before:?} == {after:?}"
        );
        let saturation = |p: [u8; 3]| {
            let (r, g, b) = (f32::from(p[0]), f32::from(p[1]), f32::from(p[2]));
            (r.max(g).max(b) - r.min(g).min(b)) / 255.0
        };
        assert!(
            saturation(after) > saturation(before),
            "the converted file must be the more saturated one: {before:?} -> {after:?}"
        );
    }

    /// An untagged file is the overwhelmingly common case, and it has to be
    /// completely unaffected: the same bytes, because the pipeline never touched a
    /// pixel. This is the assertion that a colour feature cannot quietly cost the
    /// ordinary user anything.
    #[test]
    fn an_untagged_photo_exports_byte_for_byte_as_it_did_before() {
        let j = colour_job("plain.jpg", None);
        let default = process_one(&j, &Pipeline::new(), &Settings::default()).unwrap();
        let passthrough = process_one(
            &j,
            &Pipeline {
                colour: crate::colour::ColourOptions {
                    keep_source_pixels: true,
                    ..Default::default()
                },
                ..Pipeline::new()
            },
            &Settings::default(),
        )
        .unwrap();
        assert_eq!(default.bytes, passthrough.bytes);
        assert_eq!(default.outcome.colour.source, ColourSpace::Untagged);
        assert!(
            !default.outcome.colour.converted,
            "nothing was converted, so nothing may be reported as converted"
        );
        // And the picture itself is unchanged, not merely the byte count. One code
        // of tolerance, because the fixture itself is a JPEG and re-encoding one
        // moves a channel by one; the byte comparison above is the real claim.
        for (got, want) in first_pixel(&default.bytes).into_iter().zip([200, 100, 90]) {
            assert!(got.abs_diff(want) <= 1, "{got} against {want}");
        }
    }

    /// The same, for a file that *is* tagged sRGB: a no-op conversion is a no-op.
    #[test]
    fn an_srgb_tagged_photo_is_left_exactly_as_it_was() {
        let j = colour_job("srgb.jpg", Some(&crate::colour::fixtures::srgb_profile()));
        let p = process_one(&j, &Pipeline::new(), &Settings::default()).unwrap();
        assert_eq!(p.outcome.colour.source, ColourSpace::Srgb);
        assert!(!p.outcome.colour.converted);
        // The profile is dropped because the pixels were moved into the space it
        // described — not because profiles are stripped on sight.
        let out_profile = crate::colour::ColourProfile::read(&p.bytes, OutputFormat::Jpeg);
        assert!(!out_profile.icc_present, "the output carries no profile");
        for (got, want) in first_pixel(&p.bytes).into_iter().zip([200, 100, 90]) {
            assert!(got.abs_diff(want) <= 1, "{got} against {want}");
        }
    }

    /// The explicit opt-out that keeps a wide-gamut photo wide-gamut: the source
    /// pixels are untouched and the *original* profile is written back, so the
    /// values keep their meaning for a viewer that reads profiles.
    #[test]
    fn carrying_the_profile_through_keeps_the_original_bytes_and_the_original_pixels() {
        let icc = crate::colour::fixtures::p3_profile();
        let j = colour_job("p3.jpg", Some(&icc));
        let settings = Settings::default();
        let carried = process_one(
            &j,
            &Pipeline {
                colour: crate::colour::ColourOptions {
                    embed_profile: true,
                    ..Default::default()
                },
                ..Pipeline::new()
            },
            &settings,
        )
        .unwrap();
        let converted = process_one(&j, &Pipeline::new(), &settings).unwrap();

        assert!(carried.outcome.colour.profile_embedded);
        assert!(!carried.outcome.colour.converted);
        let carried_profile =
            crate::colour::ColourProfile::read(&carried.bytes, OutputFormat::Jpeg);
        assert!(carried_profile.icc_present);
        assert_eq!(
            crate::colour::profile_bytes(&carried.bytes, OutputFormat::Jpeg).unwrap(),
            icc,
            "the embedded profile must be the source profile, byte for byte"
        );
        assert_ne!(
            first_pixel(&carried.bytes),
            first_pixel(&converted.bytes),
            "carrying the profile must not also convert the pixels"
        );
    }

    /// Refused rather than half-honoured: a file carrying a profile that does not
    /// describe its pixels is worse than one carrying none.
    #[test]
    fn carrying_a_profile_this_build_cannot_write_is_refused_in_plain_english() {
        let j = colour_job("p3.jpg", Some(&crate::colour::fixtures::p3_profile()));
        let err = process_one(
            &j,
            &Pipeline {
                colour: crate::colour::ColourOptions {
                    embed_profile: true,
                    ..Default::default()
                },
                ..Pipeline::new()
            },
            &Settings {
                format: OutputFormat::WebP,
                ..Settings::default()
            },
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("JPEG"),
            "it should name what to choose: {text}"
        );
        assert!(!text.contains("error:"), "reads like a diagnostic: {text}");
    }

    /// A profile this build cannot convert is a refusal, not a wrong colour. The
    /// alternative — passing the values through untagged — is the exact bug this
    /// phase exists to fix, and it is the one answer hard rule 9 forbids.
    #[test]
    fn a_space_we_cannot_convert_is_refused_rather_than_passed_through() {
        let j = colour_job("cmyk.jpg", Some(&crate::colour::fixtures::cmyk_profile()));
        let err = process_one(&j, &Pipeline::new(), &Settings::default()).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("CMYK"), "it should name the space: {text}");
        assert!(
            text.contains("keep the original"),
            "and offer the way out: {text}"
        );
    }

    /// A batch reports per file, so a wide-gamut photo next to an ordinary one
    /// cannot take the ordinary one down with it.
    #[test]
    fn a_batch_converts_the_wide_gamut_file_and_leaves_the_others_alone() {
        let jobs = vec![
            job(
                "1",
                "plain.jpg",
                crate::colour::fixtures::encode_with(&red_patch(), OutputFormat::Jpeg, None),
            ),
            job(
                "2",
                "p3.jpg",
                crate::colour::fixtures::encode_with(
                    &red_patch(),
                    OutputFormat::Jpeg,
                    Some(&crate::colour::fixtures::p3_profile()),
                ),
            ),
            job(
                "3",
                "cmyk.jpg",
                crate::colour::fixtures::encode_with(
                    &red_patch(),
                    OutputFormat::Jpeg,
                    Some(&crate::colour::fixtures::cmyk_profile()),
                ),
            ),
        ];
        let report = process_batch(
            &jobs,
            &Pipeline::new(),
            &Settings::default(),
            &BatchPolicy::default(),
            &CancelToken::new(),
        );
        assert_eq!(report.succeeded(), 2);
        assert_eq!(report.failed(), 1);
        assert!(!report.outcomes[0].colour.converted);
        assert!(report.outcomes[1].colour.converted);
        assert_eq!(report.outcomes[1].colour.source, ColourSpace::DisplayP3);
        // And the failed file says nothing about colour rather than claiming the
        // default.
        assert_eq!(report.outcomes[2].colour, ColourOutcome::unknown());
        assert!(
            report.outcomes[2]
                .error
                .as_deref()
                .unwrap()
                .contains("CMYK")
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

//! What this engine does with an animation, and what it says about it.
//!
//! The policy, in one sentence: **keep every frame when the output format can
//! hold them, refuse the export when it cannot, and never drop frames without
//! saying so in a field the caller can read.** The reasoning, and why the two
//! alternatives were rejected, is in `docs/GIF.md`; this module is the
//! implementation of it.
//!
//! Three types make it visible:
//!
//! * [`AnimationPolicy`] — what to do when the frames cannot all be written. `Keep` is
//!   the default, and it is the default because the alternative is a still file
//!   that looks like a successful export.
//! * [`AnimationAction`] — what actually happened to this file's frames, as a word
//!   rather than as something inferred from two counts.
//! * [`AnimationOutcome`] — both counts, the action and the policy, carried on
//!   [`crate::worker::Outcome`] so a caller never has to inspect bytes to find
//!   out whether the animation survived.
//!
//! The other thing this module owns is the animation *path* through the
//! pipeline: [`preserve`] runs every frame through the same crop → orient →
//! resize chain as a still, which is one resampling pass per frame and no more.
//! What it deliberately does not do is re-implement GIF composition — see
//! `docs/GIF.md` for why that is the one job here worth delegating.

use crate::colour::{self, ColourOutcome};
use crate::error::{Error, Result};
use crate::format::OutputFormat;
use crate::pipeline::Pipeline;
use crate::validate::{Limits, ValidateReport};
use image::AnimationDecoder;
use serde::{Deserialize, Serialize};

/// What to do when the output format cannot hold every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationPolicy {
    /// Keep every frame when the output format can hold them; refuse the export
    /// when it cannot.
    ///
    /// The default, and the reason `docs/GIF.md` exists — see the doc for the
    /// argument. Note what it does *not* say: it
    /// does not say "refuse animations". A GIF exported as a GIF is not a
    /// request to lose anything, so nothing is refused. Only a request the
    /// format cannot honour is.
    #[default]
    Keep,
    /// Export the first frame and record how many frames were dropped.
    ///
    /// The opt-in for a caller who has decided a still is what they want. It is
    /// never the default because a user who did not ask to lose an animation
    /// should not lose one, and a warning in a list of two hundred outcomes is
    /// not something anybody reads.
    FirstFrame,
}

/// What happened to one file's frames.
///
/// A word rather than a pair of counts, because the counts alone cannot tell
/// "this was never an animation" from "we could not read it", and that
/// difference is the difference between a report and a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationAction {
    /// Nothing is known: the file failed, was cancelled, or was refused before
    /// its frames were read. Zero frames in *and* out, which is the only honest
    /// count for "we never got that far".
    Unknown,
    /// One frame in, one frame out. The file was a still, and nothing was lost.
    Still,
    /// Every frame was carried through the pipeline with its delay.
    Preserved,
    /// One frame was written out of several, deliberately, because the caller
    /// asked for that with [`AnimationPolicy::FirstFrame`].
    Flattened,
    /// Nothing was written, because the format could not hold the frames and
    /// the caller had not asked for them to be dropped.
    Refused,
}

/// The animation half of a file's report.
///
/// On [`crate::worker::Outcome`] and in the JSON the UI receives, so a caller
/// can find out what happened to an animation without decoding the output and
/// without reading the error string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationOutcome {
    /// How many frames the input had.
    pub frames_in: u32,
    /// How many the written file has. Zero when nothing was written, which
    /// covers both a refusal and a failure.
    pub frames_out: u32,
    /// The policy the request was made under.
    pub policy: AnimationPolicy,
    /// What was done.
    pub action: AnimationAction,
}

impl AnimationOutcome {
    /// What a failed, cancelled or refused file reports: nothing known.
    pub fn unknown() -> Self {
        Self {
            frames_in: 0,
            frames_out: 0,
            policy: AnimationPolicy::Keep,
            action: AnimationAction::Unknown,
        }
    }

    /// A file that was never an animation.
    pub fn still() -> Self {
        Self {
            frames_in: 1,
            frames_out: 1,
            policy: AnimationPolicy::Keep,
            action: AnimationAction::Still,
        }
    }

    /// Every frame written, so `frames_in == frames_out`.
    pub fn preserved(frames: u32, policy: AnimationPolicy) -> Self {
        Self {
            frames_in: frames,
            frames_out: frames,
            policy,
            action: AnimationAction::Preserved,
        }
    }

    /// One frame written out of `frames`.
    pub fn flattened(frames: u32, policy: AnimationPolicy) -> Self {
        Self {
            frames_in: frames,
            frames_out: 1,
            policy,
            action: AnimationAction::Flattened,
        }
    }

    /// Nothing written, and the reason is a decision rather than a failure.
    pub fn refused(frames: u32, policy: AnimationPolicy) -> Self {
        Self {
            frames_in: frames,
            frames_out: 0,
            policy,
            action: AnimationAction::Refused,
        }
    }

    /// How many frames the user did not get.
    pub fn dropped(&self) -> u32 {
        self.frames_in.saturating_sub(self.frames_out)
    }

    /// Whether the file had more than one frame.
    pub fn animated(&self) -> bool {
        self.frames_in > 1
    }

    /// Whether the whole animation is going through the pipeline frame by frame.
    pub fn is_preserved(&self) -> bool {
        self.action == AnimationAction::Preserved
    }

    /// What to tell a person about this file's frames, or `None` when there is
    /// nothing to say.
    ///
    /// Only [`AnimationAction::Flattened`] has anything to say: it is the one case where
    /// a file was written and something the user gave us is not in it. A refusal
    /// wrote nothing, so its explanation is the error the caller already has,
    /// and a preserved animation lost nothing.
    ///
    /// The sentence names the count, what the file actually is, and what would
    /// have kept the rest — the three things someone who has just lost an
    /// animation needs to know, in that order.
    pub fn note(&self) -> Option<String> {
        match self.action {
            AnimationAction::Flattened => Some(format!(
                "This file was an animation of {} frames. The exported image is the first \
                 frame; the other {} were not written. Choose GIF output to keep every frame.",
                self.frames_in,
                self.dropped(),
            )),
            _ => None,
        }
    }
}

/// Decide what happens to one file's frames, before a pixel is decoded.
///
/// `frames_in` comes from [`crate::validate::ValidateReport::frames`], which
/// reads the container rather than decoding it, so a refusal costs the header
/// walk that found the frames and nothing else.
///
/// Three answers, and the middle one is reached in two ways:
///
/// * not an animation — [`AnimationAction::Still`], and the caller runs the ordinary
///   single-image path;
/// * an animation into a format that holds several frames —
///   [`AnimationAction::Preserved`], and the caller runs [`preserve`];
/// * an animation into a format that holds one — [`AnimationAction::Flattened`] if the
///   caller asked for that with [`AnimationPolicy::FirstFrame`], and otherwise
///   [`Error::AnimationRefused`], which writes nothing.
pub fn decide(
    frames_in: u32,
    output: OutputFormat,
    policy: AnimationPolicy,
) -> Result<AnimationOutcome> {
    if frames_in <= 1 {
        return Ok(AnimationOutcome::still());
    }
    if output.accepts_multiple_frames() {
        return Ok(AnimationOutcome::preserved(frames_in, policy));
    }
    match policy {
        AnimationPolicy::FirstFrame => Ok(AnimationOutcome::flattened(frames_in, policy)),
        AnimationPolicy::Keep => Err(Error::AnimationRefused {
            frames: frames_in,
            note: refusal_note(frames_in),
        }),
    }
}

/// The sentence for [`Error::AnimationRefused`].
///
/// A function rather than a constant because the frame count is the useful part
/// of it: a user who is told "this is an animation" learns nothing they did not
/// know, and a user who is told how many frames are in it can decide whether
/// the first one is worth having.
pub fn refusal_note(frames: u32) -> String {
    format!(
        "This file is an animation of {frames} frames, and the format you chose holds one \
         picture, so nothing was written rather than quietly dropping the rest. Choose GIF \
         output to keep every frame, or ask for the first frame only if that is what you want."
    )
}

/// One file's animation, re-encoded frame by frame.
#[derive(Debug)]
pub struct Preserved {
    pub bytes: Vec<u8>,
    /// How many frames the output actually has.
    pub frames: u32,
    /// The colour report for the file. Every frame gets the same treatment, so
    /// this is the first frame's — and it is a real answer rather than a
    /// placeholder, because [`colour::apply`] runs per frame and would have
    /// refused the file otherwise.
    pub colour: ColourOutcome,
    pub width: u32,
    pub height: u32,
}

/// Carry every frame through the pipeline and write one animation.
///
/// Each frame goes through [`colour::apply`] and [`Pipeline::apply`], which is
/// crop → orient → resize and **one resampling pass per frame**. That is the
/// reading of hard rule 5 this phase takes: the rule bans a second pass over the
/// *same* pixels, and a frame is not a pass over the frame before it.
///
/// Two things this deliberately does not do, both because they are the traps the
/// prompt names and the reasons a naive implementation produces a file that
/// plays *wrong* rather than one that plays still:
///
/// * **It does not compose frames.** [`image`]'s GIF decoder already composites
///   each frame against the ones before it, honouring disposal and the
///   transparent index, and hands back a complete canvas with the frame's delay
///   attached. So the input side of the problem — which is where a naive
///   implementation gets it wrong — is the decoder's job, and doing it twice is
///   how a resizer ends up with doubled or half-erased frames.
/// * **It does not re-derive disposal on the way out.** Every frame written is a
///   full canvas, so the output needs no disposal method to get right. `image`'s
///   encoder writes `Background` for every frame regardless, and for a
///   full-canvas frame that is correct rather than lossy: a transparent pixel in a
///   composited frame means nothing was ever painted there.
///
/// The budget is checked against the whole animation before the first frame is
/// decoded ([`Limits::check_animation`]), because the frames are all in memory
/// at once and a per-frame check would pass every one of them.
pub fn preserve(
    input: &[u8],
    report: &ValidateReport,
    limits: &Limits,
    pipeline: &Pipeline,
    output: OutputFormat,
) -> Result<Preserved> {
    // Unreachable through `decide`, which refuses before this is called. Present
    // because `encode_frames` refuses too and two guards that disagree are worse
    // than one that is redundant.
    if !output.accepts_multiple_frames() {
        return Err(Error::UnsupportedFormat(
            "this format holds one picture, so it cannot carry an animation. Choose GIF \
             output to keep every frame",
        ));
    }
    let frames_in = report.frames;
    if report.frames_truncated {
        // The walk stopped before the trailer, so `frames` is a lower bound and
        // the invariant below — every frame in, every frame out — cannot be
        // checked. Writing the frames we did read would produce an animation
        // that is quietly shorter than the original, which is the one thing
        // this module exists to make impossible. A GIF missing its trailer is
        // the ordinary case here, and refusing it is the same judgement as
        // refusing a frame that will not decode: the file is damaged, and
        // `AnimationPolicy::FirstFrame` is the way out for a user who wants a
        // still anyway.
        return Err(unexpected_eof(
            "this GIF's frame structure is damaged part-way through, so the frames that could \
             be read are not all of them. Nothing was written rather than quietly shortening \
             the animation; ask for the first frame only if that is what you want",
        ));
    }
    let (dst_w, dst_h) = pipeline.output_dimensions(report.width, report.height)?;
    limits.check_animation(dst_w, dst_h, frames_in)?;

    let decoder =
        image::codecs::gif::GifDecoder::new(std::io::Cursor::new(input)).map_err(Error::Decode)?;

    let mut frames: Vec<image::Frame> = Vec::new();
    let mut colour = ColourOutcome::unknown();
    for frame in decoder.into_frames() {
        // A frame that will not decode refuses the whole file rather than
        // shortening the animation. Writing 99 frames of a 100-frame GIF and
        // reporting success is the same class of claim this module exists to
        // stop, one level up.
        let frame = frame.map_err(Error::Decode)?;
        // The delay is the one thing about a frame the pipeline must not touch,
        // so it is read off the decoded frame and carried across the resize.
        // `image` holds it as a millisecond ratio and its encoder divides by ten
        // again, so a round trip is exact to the format's own hundredths.
        let delay = frame.delay();
        let source = image::DynamicImage::ImageRgba8(frame.into_buffer());
        let (converted, outcome) = colour::apply(&source, &report.colour, pipeline.colour, output)?;
        let resized = pipeline.apply(&converted)?;
        // The whole animation was bounded above; this is the belt to that
        // braces, in case a frame's own geometry differs from the canvas's.
        limits.check_decoded(&resized)?;
        if frames.is_empty() {
            colour = outcome;
        }
        frames.push(image::Frame::from_parts(resized.to_rgba8(), 0, 0, delay));
    }

    if frames.is_empty() {
        return Err(unexpected_eof(
            "this GIF's frame structure ended before any frame was read",
        ));
    }
    // The container claimed more frames than the decoder produced. Rather than
    // report a preserved animation that is quietly shorter than the original,
    // refuse the file: `frames_in == frames_out` is the invariant that makes
    // `AnimationAction::Preserved` true rather than aspirational.
    let written = u32::try_from(frames.len()).unwrap_or(u32::MAX);
    if written != frames_in {
        return Err(unexpected_eof(&format!(
            "this GIF claims {frames_in} frames but only {written} of them could be read, so \
             writing them would quietly shorten the animation"
        )));
    }

    let bytes = crate::format::encode_frames(&frames, output)?;
    Ok(Preserved {
        bytes,
        frames: written,
        colour,
        width: dst_w,
        height: dst_h,
    })
}

fn unexpected_eof(what: &str) -> Error {
    Error::Decode(image::ImageError::IoError(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        what,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::EncodingOptions;
    use crate::pipeline::ResizeSpec;

    /// A GIF with `frames` frames, each a flat colour, each 40 ms long.
    ///
    /// Built rather than committed: the pixels going in are then known exactly,
    /// so a test can name the colour of frame two and be right about it, and
    /// there is no third-party file in the tree whose licence anyone would have
    /// to reason about.
    fn animation(w: u32, h: u32, frames: u32, delay_ms: u16) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        {
            let mut cursor = std::io::Cursor::new(&mut out);
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut cursor);
            for i in 0..frames {
                let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                    w,
                    h,
                    image::Rgba([(i * 60) as u8, 10, 200u8.wrapping_sub((i * 40) as u8), 255]),
                ));
                encoder
                    .encode_frame(image::Frame::from_parts(
                        img.to_rgba8(),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(u32::from(delay_ms), 1),
                    ))
                    .unwrap();
            }
        }
        out
    }

    fn report(bytes: &[u8]) -> ValidateReport {
        crate::validate::validate_bytes(bytes, &Limits::default()).unwrap()
    }

    fn settings(format: OutputFormat) -> crate::worker::Settings {
        crate::worker::Settings {
            format,
            ..Default::default()
        }
    }

    fn pipeline(width: u32) -> Pipeline {
        Pipeline::new().with_resize(ResizeSpec {
            width: Some(width),
            height: None,
            fit: crate::pipeline::FitMode::Width,
            ..Default::default()
        })
    }

    /// The policy's central claim, as a table: a still is a still, an
    /// animation into GIF keeps its frames, an animation into anything else is
    /// refused, and an explicit opt-in flattens it.
    #[test]
    fn the_decision_has_four_answers_and_no_fifth() {
        let gif = animation(8, 8, 3, 40);
        assert_eq!(
            decide(1, OutputFormat::Jpeg, AnimationPolicy::Keep).unwrap(),
            AnimationOutcome::still()
        );
        assert_eq!(
            decide(3, OutputFormat::Gif, AnimationPolicy::Keep)
                .unwrap()
                .action,
            AnimationAction::Preserved
        );
        assert!(matches!(
            decide(3, OutputFormat::Jpeg, AnimationPolicy::Keep),
            Err(Error::AnimationRefused { frames: 3, .. })
        ));
        assert_eq!(
            decide(3, OutputFormat::Jpeg, AnimationPolicy::FirstFrame)
                .unwrap()
                .action,
            AnimationAction::Flattened
        );
        // And the refusal only ever happens for a format that cannot hold the
        // frames: GIF output is not a request to lose anything.
        assert!(decide(3, OutputFormat::Gif, AnimationPolicy::Keep).is_ok());
        assert_eq!(report(&gif).frames, 3);
    }

    /// The sentence a user reads when the engine refuses. It has to say what
    /// happened, how much, and what to do instead — hard rule 9, and the phase
    /// prompt's own question about whether someone who lost an animation would
    /// understand it.
    #[test]
    fn the_refusal_says_what_happened_and_what_to_choose() {
        let err = decide(240, OutputFormat::Jpeg, AnimationPolicy::Keep).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("240 frames"),
            "it must name the count: {text}"
        );
        assert!(text.contains("nothing was written"), "and the fact: {text}");
        assert!(text.contains("GIF"), "and what to choose: {text}");
        assert!(!text.contains("error:"), "reads like a diagnostic: {text}");
    }

    /// A single-frame GIF is a still, not a one-frame animation. Getting this
    /// wrong in the other direction would put a warning on every GIF ever
    /// exported, which is how warnings stop being read.
    #[test]
    fn a_single_frame_gif_is_not_an_animation() {
        let bytes = animation(8, 8, 1, 40);
        let r = report(&bytes);
        assert_eq!(r.frames, 1);
        assert!(!r.has_animated);
        let p = crate::worker::process_one(
            &crate::worker::Job {
                id: "1".into(),
                name: "still.gif".into(),
                bytes,
            },
            &Pipeline::new(),
            &settings(OutputFormat::Jpeg),
        )
        .unwrap();
        assert_eq!(p.outcome.animation, AnimationOutcome::still());
        assert!(!p.outcome.animation.animated());
        assert!(p.outcome.animation.note().is_none());
    }

    /// The frame count is what the UI shows *before* the export, so it has to be
    /// the same number the outcome reports after it.
    #[test]
    fn the_report_and_the_outcome_agree_about_how_many_frames_there_were() {
        for frames in [1u32, 2, 5] {
            let bytes = animation(8, 8, frames, 40);
            assert_eq!(report(&bytes).frames, frames);
            let p = crate::worker::process_one(
                &crate::worker::Job {
                    id: "1".into(),
                    name: "a.gif".into(),
                    bytes,
                },
                &Pipeline::new(),
                &settings(OutputFormat::Jpeg),
            );
            // All of these are refused into JPEG; the point is the count in the
            // refusal, which is the only number the user gets.
            if let Err(Error::AnimationRefused {
                frames: claimed, ..
            }) = p
            {
                assert_eq!(claimed, frames);
            } else if frames == 1 {
                assert!(p.is_ok());
            } else {
                panic!("{frames} frames should have been refused");
            }
        }
    }

    /// The preservation path, end to end through `process_one`: every frame
    /// survives the resize, the delays survive it, and the outcome says so.
    #[test]
    fn an_animation_exported_as_gif_keeps_every_frame_and_its_delay() {
        let bytes = animation(16, 12, 4, 40);
        let p = crate::worker::process_one(
            &crate::worker::Job {
                id: "1".into(),
                name: "spin.gif".into(),
                bytes,
            },
            &pipeline(8),
            &settings(OutputFormat::Gif),
        )
        .unwrap();

        assert!(p.outcome.ok());
        assert_eq!(p.outcome.animation.action, AnimationAction::Preserved);
        assert_eq!(p.outcome.animation.frames_in, 4);
        assert_eq!(p.outcome.animation.frames_out, 4);
        assert_eq!(p.outcome.animation.dropped(), 0);
        assert!(
            p.outcome.animation.note().is_none(),
            "nothing was lost, so there is nothing to warn about"
        );
        assert_eq!(p.outcome.output_name, "spin.gif");
        assert_eq!(
            crate::format::detect_format(&p.bytes).unwrap(),
            OutputFormat::Gif
        );
        // The resize happened, once per frame, and applied to every frame.
        assert_eq!((p.outcome.width, p.outcome.height), (8, 6));

        let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&p.bytes)).unwrap();
        let frames: Vec<_> = decoder.into_frames().map(|f| f.unwrap()).collect();
        assert_eq!(frames.len(), 4, "the written file must hold every frame");
        assert!(
            frames.iter().all(|f| f.delay() == frames[0].delay()),
            "every delay must have survived the round trip"
        );
        assert_ne!(
            frames[0].buffer().get_pixel(0, 0),
            frames[1].buffer().get_pixel(0, 0),
            "the frames must be the frames that went in, not one frame repeated"
        );
    }

    /// One resampling pass per frame, and no more. The phase prompt asks for it
    /// and hard rule 5 asks for it; the way to show it is that the animation
    /// path goes through the same `Pipeline::apply` the still path does, so
    /// there is exactly one `resize::resample` call site in the crate and every
    /// frame passes through it once.
    #[test]
    fn every_frame_takes_the_same_pipeline_as_a_still() {
        let animated = crate::worker::process_one(
            &crate::worker::Job {
                id: "1".into(),
                name: "a.gif".into(),
                bytes: animation(16, 12, 3, 40),
            },
            &pipeline(8),
            &settings(OutputFormat::Gif),
        )
        .unwrap();
        let still = crate::worker::process_one(
            &crate::worker::Job {
                id: "2".into(),
                name: "a.jpg".into(),
                bytes: crate::encode_fixed(
                    &image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                        16,
                        12,
                        image::Rgba([0, 10, 200, 255]),
                    )),
                    OutputFormat::Jpeg,
                    EncodingOptions::default().with_quality(95),
                )
                .unwrap(),
            },
            &pipeline(8),
            &settings(OutputFormat::Jpeg),
        )
        .unwrap();
        // Same request, same geometry out of both paths.
        assert_eq!(
            (animated.outcome.width, animated.outcome.height),
            (still.outcome.width, still.outcome.height)
        );
    }

    /// The deliberate opt-in: a caller who has said "the first frame is what I
    /// want" gets a still, and the outcome says how many frames are not in it.
    #[test]
    fn asking_for_the_first_frame_produces_a_still_and_says_what_it_cost() {
        let bytes = animation(16, 12, 9, 40);
        let j = crate::worker::Job {
            id: "1".into(),
            name: "meme.gif".into(),
            bytes,
        };
        let p = crate::worker::process_one(
            &j,
            &Pipeline::new(),
            &crate::worker::Settings {
                format: OutputFormat::Jpeg,
                animation: AnimationPolicy::FirstFrame,
                ..settings(OutputFormat::Jpeg)
            },
        )
        .unwrap();
        assert_eq!(p.outcome.animation.action, AnimationAction::Flattened);
        assert_eq!(p.outcome.animation.frames_in, 9);
        assert_eq!(p.outcome.animation.frames_out, 1);
        assert_eq!(p.outcome.animation.dropped(), 8);
        let note = p
            .outcome
            .animation
            .note()
            .expect("a lost animation is worth a sentence");
        assert!(note.contains('9'), "names the count: {note}");
        assert!(note.contains("first frame"), "says what it is: {note}");
        assert!(note.contains("GIF"), "says what to choose: {note}");
        assert_eq!(
            crate::format::detect_format(&p.bytes).unwrap(),
            OutputFormat::Jpeg
        );
    }

    /// The default is the safe answer, asserted rather than left to the
    /// `#[default]` attribute: a change of mind here is a change of product, and
    /// it should have to be made in a test first.
    #[test]
    fn the_default_policy_refuses_rather_than_flattening() {
        assert_eq!(AnimationPolicy::default(), AnimationPolicy::Keep);
        assert!(settings(OutputFormat::Jpeg).animation == AnimationPolicy::Keep);
        assert!(
            decide(4, OutputFormat::Jpeg, AnimationPolicy::default()).is_err(),
            "the default must not drop frames on its own"
        );
    }

    /// A corrupt frame structure is a file the user picked off their disk, so it
    /// is exactly the input hard rule 3 is about. Every variant returns `Err`
    /// rather than panicking, and the container walk never claims more frames
    /// than it found image descriptors for.
    #[test]
    fn a_corrupt_frame_structure_returns_err_rather_than_panicking() {
        let good = animation(8, 8, 4, 40);
        let corrupt = [
            // Truncated inside the last frame's data.
            good[..good.len() - 6].to_vec(),
            // Truncated inside the header.
            good[..3].to_vec(),
            // Truncated before the first frame at all.
            good[..14].to_vec(),
            // The trailer removed. Every frame is still readable, so a decoder
            // that stops at end-of-file would hand back four frames and call it
            // complete; the walk is what notices, and the truncation flag is
            // what turns that into a refusal.
            good[..good.len() - 1].to_vec(),
            // A byte where a block introducer has to be, so the stream is out
            // of sync and every later offset would be a guess.
            {
                let mut v = good.clone();
                v[19] = 0x7f;
                v
            },
            // Nothing but a signature.
            b"GIF89a".to_vec(),
            // Every byte a valid GIF might be made of, none of which is one.
            vec![0xff; 64],
        ];
        for bytes in corrupt {
            // The walk is total: it answers, and never invents a frame.
            let scan = crate::validate::scan_gif_frames(&bytes);
            assert!(
                u32::try_from(bytes.len()).unwrap_or(u32::MAX) / 2 >= scan.frames,
                "the walk claimed more frames ({}) than a buffer of {} bytes could hold",
                scan.frames,
                bytes.len()
            );
            // And the product path answers rather than panicking. Nothing is
            // written in any of these cases: a damaged animation cannot be
            // re-encoded, and it certainly cannot be shortened to a still
            // without saying so.
            match crate::worker::process_one(
                &crate::worker::Job {
                    id: "1".into(),
                    name: "broken.gif".into(),
                    bytes,
                },
                &Pipeline::new(),
                &settings(OutputFormat::Gif),
            ) {
                Err(_) => {}
                Ok(p) => assert!(
                    p.outcome.animation.is_preserved(),
                    "a damaged animation came back as {:?}",
                    p.outcome.animation
                ),
            }
        }
    }

    /// A batch reports per file, so an animation that cannot be honoured is one
    /// refusal among two hundred successes rather than the end of the batch —
    /// and the refusal is in a *field*, not only in a string, so a UI can show
    /// it without parsing the message.
    #[test]
    fn a_batch_refuses_the_animation_and_keeps_the_rest() {
        let jobs = vec![
            crate::worker::Job {
                id: "1".into(),
                name: "a.jpg".into(),
                bytes: crate::encode_fixed(
                    &image::DynamicImage::ImageRgb8(image::RgbImage::new(32, 24)),
                    OutputFormat::Jpeg,
                    EncodingOptions::default().with_quality(95),
                )
                .unwrap(),
            },
            crate::worker::Job {
                id: "2".into(),
                name: "spin.gif".into(),
                bytes: animation(8, 8, 6, 40),
            },
            crate::worker::Job {
                id: "3".into(),
                name: "b.jpg".into(),
                bytes: crate::encode_fixed(
                    &image::DynamicImage::ImageRgb8(image::RgbImage::new(32, 24)),
                    OutputFormat::Jpeg,
                    EncodingOptions::default().with_quality(95),
                )
                .unwrap(),
            },
        ];
        let report = crate::worker::process_batch(
            &jobs,
            &Pipeline::new(),
            &settings(OutputFormat::Jpeg),
            &crate::worker::CancelToken::new(),
        );
        assert_eq!(report.succeeded(), 2);
        assert_eq!(report.failed(), 1);
        assert_eq!(
            report.outcomes[1].animation.action,
            AnimationAction::Refused
        );
        assert_eq!(report.outcomes[1].animation.frames_in, 6);
        assert_eq!(report.outcomes[1].animation.frames_out, 0);
        assert!(report.outcomes[0].output_bytes > 0);
        assert!(report.outcomes[2].output_bytes > 0);
        // And a file that was never an animation says so, rather than claiming
        // the refusal.
        assert_eq!(report.outcomes[0].animation.action, AnimationAction::Still);
    }

    /// A GIF that survives as a GIF cannot be a memory bomb, and the budget is
    /// applied to the whole animation rather than to each frame — which is the
    /// only way the check can notice.
    #[test]
    fn a_hundred_thousand_frames_of_a_real_size_is_refused_before_decoding() {
        let bytes = animation(200, 200, 2, 40);
        let mut scaled = report(&bytes);
        scaled.frames = 100_000;
        let limits = Limits {
            max_pixels: 40_000_000,
            ..Limits::default()
        };
        let err =
            preserve(&bytes, &scaled, &limits, &pipeline(200), OutputFormat::Gif).unwrap_err();
        assert!(
            matches!(err, Error::PixelBudgetExceeded { .. }),
            "100000 x 200 x 200 is not a phone's budget: {err:?}"
        );
        // And a genuinely single-frame file of the same size is inside it, so
        // the refusal above is about the frame count and not about GIF.
        let still = animation(200, 200, 1, 40);
        let one = report(&still);
        assert_eq!(one.frames, 1);
        let kept = preserve(&still, &one, &limits, &pipeline(200), OutputFormat::Gif);
        assert!(kept.is_ok(), "{:?}", kept.err());
    }

    /// A caller that skipped `decide` and handed `preserve` a format that holds
    /// one picture gets the same sentence, not a still.
    #[test]
    fn preserve_refuses_a_format_that_cannot_hold_an_animation() {
        let bytes = animation(8, 8, 3, 40);
        let err = preserve(
            &bytes,
            &report(&bytes),
            &Limits::default(),
            &Pipeline::new(),
            OutputFormat::Jpeg,
        )
        .unwrap_err();
        assert!(err.to_string().contains("GIF"), "{err}");
    }

    /// `encode_frames` is the format layer's half of the policy, so it has to
    /// refuse on its own rather than trusting its caller.
    #[test]
    fn the_format_layer_refuses_to_write_frames_it_would_drop() {
        let frame = image::Frame::from_parts(
            image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])),
            0,
            0,
            image::Delay::from_numer_denom_ms(100, 1),
        );
        assert!(
            crate::format::encode_frames(std::slice::from_ref(&frame), OutputFormat::Jpeg).is_err()
        );
        assert!(
            crate::format::encode_frames(std::slice::from_ref(&frame), OutputFormat::Png).is_err()
        );
        assert!(
            crate::format::encode_frames(std::slice::from_ref(&frame), OutputFormat::Gif).is_ok(),
            "GIF is the format that takes frames"
        );
        // And an animation with no frames is not a valid animation.
        assert!(crate::format::encode_frames(&[], OutputFormat::Gif).is_err());
    }
}

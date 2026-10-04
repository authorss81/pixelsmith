//! Property tests for the parts of the engine that fuzzing cannot reach.
//!
//! Fuzzing finds crashes. It does not find "the aspect ratio is off by one when
//! the source is 3x2 and the target is 1xN", because that is not a crash - it is
//! a wrong answer, delivered confidently, to a user who asked for a specific
//! size. So the properties here are about *arithmetic and naming*: dimension
//! resolution, the agreement between a prediction and the pixels that come out,
//! filename sanitisation, and the target-bytes search.
//!
//! # Determinism
//!
//! Every `proptest!` below pins a seed and asks for 256 cases. Both matter:
//!
//! * **The seed** makes a failure reproducible from the commit. A shrinking
//!   counterexample that cannot be replayed is a bug report nobody can act on.
//! * **256 cases** is a floor, not a tuning decision. The default of 100 is a
//!   number nobody chose; these properties are cheap (integer arithmetic, string
//!   scanning) and 256 catches arithmetic edge cases that 100 does not.
//!
//! `cargo test` twice must produce identical output, which the phase gate checks.
//! Do not remove a seed to make a suite pass - fix the code or, if the property
//! is genuinely wrong, change the property and say why in a comment.

use pixelsmith_core::error::Error;
use pixelsmith_core::format::{EncodingOptions, OutputFormat};
use pixelsmith_core::pipeline::{
    CropSpec, FitMode, Orientation, Pipeline, ResampleFilter, ResizeSpec,
};
use pixelsmith_core::target::TargetBytes;
use pixelsmith_core::worker::{sanitise_path_component, sanitise_stem};
use proptest::prelude::*;

/// Windows refuses to create a file with any of these names, whatever extension
/// is appended. A sanitiser that emits `con` produces an output the app then
/// cannot save - a failure the user sees only at the last step.
const RESERVED_STEMS: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A deterministic proptest configuration.
///
/// The seed is the point. Proptest's default seed is random per run, which means
/// a failure cannot be replayed: the shrunk counterexample it prints is
/// meaningless if you cannot get the same seed back. Pinning it means
/// `git bisect` over a failing property actually works.
///
/// `PX_PROPTEST_SEED` overrides it, so a deliberate search for a new counterexample
/// (raising `cases`, or re-running with fresh seeds to shake out a rare state) does
/// not require editing this file and re-committing.
fn config(seed: u64) -> ProptestConfig {
    let effective = std::env::var("PX_PROPTEST_SEED")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(seed);
    ProptestConfig {
        cases: 256,
        rng_seed: proptest::test_runner::RngSeed::Fixed(effective),
        // Shrinking is what turns "something failed" into a minimal example, so
        // it stays on. Only the seed is pinned.
        ..ProptestConfig::default()
    }
}

fn is_reserved_windows_name(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    // The check is on the name before the first dot, because that is what
    // Windows actually reserves.
    let base = upper.split('.').next().unwrap_or(&upper);
    RESERVED_STEMS.contains(&base)
}

// ---------------------------------------------------------------------------
// ResizeSpec::resolve
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(0x5EED_0003))]

    /// Output is never zero on either axis, whatever the input.
    ///
    /// A zero in a dimension is not a small image, it is a decode failure later:
    /// `image` rejects a 0xN buffer, so the user gets an error from a resize they
    /// thought succeeded.
    #[test]
    fn resolve_never_returns_a_zero_dimension(
        src_w in 1u32..=20_000, src_h in 1u32..=20_000, target in 1u32..=4_000,
    ) {
        let spec = ResizeSpec {
            width: Some(target),
            height: None,
            fit: FitMode::Width,
            filter: ResampleFilter::Nearest,
            no_upscale: false,
        };
        let (w, h) = spec.resolve(src_w, src_h).unwrap();
        prop_assert!(w > 0, "width was zero for {src_w}x{src_h} -> {target}");
        prop_assert!(h > 0, "height was zero for {src_w}x{src_h} -> {target}");
    }

    /// `no_upscale` is honoured. Upscaling a photo adds bytes and no detail, so
    /// an engine that quietly enlarges is lying about the file it is producing.
    #[test]
    fn resolve_never_upscales_when_asked_not_to(
        src_w in 1u32..=8_000, src_h in 1u32..=8_000, target in 1u32..=8_000,
    ) {
        let spec = ResizeSpec {
            width: Some(target),
            height: None,
            fit: FitMode::Width,
            filter: ResampleFilter::Nearest,
            no_upscale: true,
        };
        let (w, _h) = spec.resolve(src_w, src_h).unwrap();
        prop_assert!(
            w <= src_w,
            "no_upscale produced {w} from a source {src_w} wide (target {target})"
        );
    }

    /// Aspect ratio survives `FitMode::Width` to within a pixel.
    ///
    /// One pixel, not zero: a 1-pixel difference is the unavoidable rounding in
    /// `round(src_h * w / src_w)`. Demanding exact equality would be demanding
    /// arithmetic that cannot exist.
    #[test]
    fn width_fit_preserves_aspect_ratio_within_a_pixel(
        src_w in 1u32..=20_000, src_h in 1u32..=20_000, target in 1u32..=4_000,
    ) {
        let spec = ResizeSpec {
            width: Some(target),
            height: None,
            fit: FitMode::Width,
            filter: ResampleFilter::Nearest,
            no_upscale: false,
        };
        let (w, h) = spec.resolve(src_w, src_h).unwrap();
        let expected = f64::from(src_h) * f64::from(w) / f64::from(src_w);
        prop_assert!(
            (f64::from(h) - expected).abs() <= 1.0,
            "{src_w}x{src_h} -> width {target} gave {w}x{h}, expected height {expected}"
        );
    }

    /// And the same for `FitMode::Height`, which is a different code path.
    #[test]
    fn height_fit_preserves_aspect_ratio_within_a_pixel(
        src_w in 1u32..=20_000, src_h in 1u32..=20_000, target in 1u32..=4_000,
    ) {
        let spec = ResizeSpec {
            width: None,
            height: Some(target),
            fit: FitMode::Height,
            filter: ResampleFilter::Nearest,
            no_upscale: false,
        };
        let (w, h) = spec.resolve(src_w, src_h).unwrap();
        let expected = f64::from(src_w) * f64::from(h) / f64::from(src_h);
        prop_assert!(
            (f64::from(w) - expected).abs() <= 1.0,
            "{src_w}x{src_h} -> height {target} gave {w}x{h}, expected width {expected}"
        );
    }

    /// `Contain` fits inside the box on both axes and never crops. A `Contain`
    /// that produced an image larger than the box on either axis would silently
    /// ignore the user's constraint.
    #[test]
    fn contain_fits_inside_the_box_on_both_axes(
        src_w in 1u32..=20_000, src_h in 1u32..=20_000,
        box_w in 1u32..=4_000, box_h in 1u32..=4_000,
    ) {
        let spec = ResizeSpec {
            width: Some(box_w),
            height: Some(box_h),
            fit: FitMode::Contain,
            filter: ResampleFilter::Nearest,
            no_upscale: false,
        };
        let (w, h) = spec.resolve(src_w, src_h).unwrap();
        prop_assert!(w <= box_w, "{w} exceeded the box width {box_w}");
        prop_assert!(h <= box_h, "{h} exceeded the box height {box_h}");
    }

/// A square source resized by width must give the same answer as the same
    /// square source resized by height. This is the transposed-input check below,
    /// restricted to squares so the aspect-ratio argument does not apply.
    #[test]
    fn a_square_source_resolves_the_same_by_width_and_by_height(
        side in 1u32..=4_000, target in 1u32..=4_000,
    ) {
        let by_width = ResizeSpec {
            width: Some(target),
            height: None,
            fit: FitMode::Width,
            filter: ResampleFilter::Nearest,
            no_upscale: false,
        };
        let by_height = ResizeSpec {
            width: None,
            height: Some(target),
            fit: FitMode::Height,
            filter: ResampleFilter::Nearest,
            no_upscale: false,
        };
        prop_assert_eq!(
            by_width.resolve(side, side).unwrap(),
            by_height.resolve(side, side).unwrap(),
            "a {}x{} square resolved differently by width and by height",
            side,
            side
        );
    }

    /// The eight EXIF values map to eight distinct orientations, and the two
    /// 90-degree families are exactly the ones that swap axes.
    ///
    /// Kept as an explicit table rather than a `matches!` restating the
    /// implementation: the point is to pin the *spec* mapping, so a change to
    /// `from_exif` that transposes a photo is caught here.
    #[test]
    fn every_exif_value_maps_to_the_documented_orientation(value in 1u32..=8) {
        let expected = match value {
            1 => Orientation::Normal,
            2 => Orientation::MirrorHorizontal,
            3 => Orientation::Rotate180,
            4 => Orientation::MirrorVertical,
            5 => Orientation::MirrorHorizontalRotate270,
            6 => Orientation::Rotate90,
            7 => Orientation::MirrorHorizontalRotate90,
            8 => Orientation::Rotate270,
            _ => Orientation::Normal, // 0 and 9+ are undefined; treated as upright
        };
        prop_assert_eq!(
            Orientation::from_exif(value),
            expected,
            "EXIF orientation {} should map to {:?}",
            value,
            expected
        );
    }

    /// Only the four 90/270 cases transpose the axes. Getting this wrong makes
    /// every auto-rotated photo come out stretched, which is the most common
    /// complaint about orientation handling.
    #[test]
    fn only_the_quarter_turn_orientations_swap_axes(value in 1u32..=8) {
        let o = Orientation::from_exif(value);
        let should_swap = matches!(value, 5..=8);
        prop_assert_eq!(
            o.swaps_axes(),
            should_swap,
            "EXIF orientation {} ({:?}) disagrees about swapping axes",
            value,
            o
        );
    }
}

// ---------------------------------------------------------------------------
// output_dimensions must agree with apply
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(0x5EED_0004))]

    /// The prediction and the pixels must agree.
    ///
    /// `output_dimensions` is what the UI shows the user *before* the export, and
    /// `apply` is what it gets. If they disagree, the app has told the user a
    /// size it did not produce - and it can do so while every unit test passes,
    /// because a unit test would have to construct the same pipeline twice and
    /// notice.
#[test]
    fn output_dimensions_agrees_with_apply(
        src_w in 8u32..=600, src_h in 8u32..=600, target in 1u32..=500,
        fit_idx in 0usize..5,
        crop_x in 0u32..100, crop_y in 0u32..100,
        crop_w in 1u32..=200, crop_h in 1u32..=200,
        use_crop in any::<bool>(), use_orientation in any::<bool>(),
        orientation in 1u32..=8,
    ) {
        let fit = match fit_idx {
            0 => FitMode::Contain,
            1 => FitMode::Cover,
            2 => FitMode::Fill,
            3 => FitMode::Width,
            _ => FitMode::Height,
        };
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_pixel(src_w, src_h, image::Rgb([90, 120, 150])),
        );

        let mut pipeline = Pipeline::new().with_resize(ResizeSpec {
            width: Some(target),
            height: Some(target),
            fit,
            filter: ResampleFilter::Nearest,
            no_upscale: false,
        });

        // A crop that does not fit inside the source is not a legal crop, so
        // skip those rather than asserting on an error the caller would never
        // have constructed. `Pipeline`'s fields are public, so this is a struct
        // update rather than a builder call.
        let crop = CropSpec { x: crop_x, y: crop_y, width: crop_w, height: crop_h };
        if use_crop {
            if crop.x + crop.width > src_w || crop.y + crop.height > src_h {
                return Ok(());
            }
            pipeline.crop = Some(crop);
        }
        if use_orientation && orientation != 1 {
            pipeline.orientation = Some(Orientation::from_exif(orientation));
        }

        let predicted = pipeline.output_dimensions(src_w, src_h);
        let actual = pipeline.apply(&img).map(|i| (i.width(), i.height()));

        match (predicted, actual) {
            (Ok((pw, ph)), Ok((aw, ah))) => {
                prop_assert_eq!(
                    (pw, ph),
                    (aw, ah),
                    "predicted {}x{} but produced {}x{} (source {}x{}, target {}, \
                     fit {:?}, crop {:?}, orientation {})",
                    pw,
                    ph,
                    aw,
                    ah,
                    src_w,
                    src_h,
                    target,
                    fit,
                    crop,
                    orientation
                );
            }
            (Err(_), Err(_)) => {}
            (Ok(p), Err(e)) => panic!(
                "output_dimensions promised {p:?} but apply failed: {e:?} \
                 (source {src_w}x{src_h}, fit {fit:?}, crop {crop:?})"
            ),
            (Err(e), Ok(a)) => panic!(
                "output_dimensions refused with {e:?} but apply produced {a:?}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Filename sanitisation
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(0x5EED_0005))]

    /// `sanitise_stem` output is safe to use as a filename on any platform.
    ///
    /// This is the property that matters, and it is a conjunction: a sanitiser
    /// can pass each half alone and still emit `..` or a path separator.
    #[test]
    fn sanitise_stem_never_emits_a_path_or_a_reserved_name(
        input in ".{0,80}",
    ) {
        let out = sanitise_stem(&input);

        prop_assert!(!out.is_empty(), "empty output for {input:?}");
        prop_assert!(!out.contains('/'), "separator in {out:?} from {input:?}");
        prop_assert!(!out.contains('\\'), "backslash in {out:?} from {input:?}");
        prop_assert!(!out.contains('\0'), "NUL in {out:?} from {input:?}");
        prop_assert!(out != "..", "dot-dot in output from {input:?}");
        prop_assert!(!out.contains(".."), "traversal in {out:?} from {input:?}");
        prop_assert!(
            !out.starts_with('.'),
            "leading dot in {out:?} from {input:?} - a dotfile the user cannot see"
        );
        prop_assert!(
            out.chars().count() <= 64,
            "{} chars, over the 64 cap, from {input:?}",
            out.chars().count()
        );
        prop_assert!(
            !is_reserved_windows_name(&out),
            "{out:?} is a reserved Windows device name, from {input:?}"
        );
    }

    /// Archive components have the same guarantees under a different cap and a
    /// different fallback. Checked separately because the two functions are
    /// separately implemented, and a fix to one is not automatically a fix to the
    /// other.
    #[test]
    fn sanitise_path_component_never_emits_a_path_or_a_reserved_name(
        input in ".{0,100}",
    ) {
        let out = sanitise_path_component(&input);

        prop_assert!(!out.is_empty(), "empty output for {input:?}");
        prop_assert!(!out.contains('/'), "separator in {out:?} from {input:?}");
        prop_assert!(!out.contains('\\'), "backslash in {out:?} from {input:?}");
        prop_assert!(!out.contains('\0'), "NUL in {out:?} from {input:?}");
        prop_assert!(!out.contains(".."), "traversal in {out:?} from {input:?}");
        prop_assert!(!out.starts_with('.'), "leading dot in {out:?} from {input:?}");
        prop_assert!(
            out.chars().count() <= 96,
            "{} chars, over the 96 cap, from {input:?}",
            out.chars().count()
        );
        prop_assert!(
            !is_reserved_windows_name(&out),
            "{out:?} is a reserved Windows device name, from {input:?}"
        );
    }

    /// Sanitising is idempotent. Running the output back through must not change
    /// it, or a second pass in a later phase could mangle a name that the first
    /// pass had already made safe.
    #[test]
    fn sanitising_twice_changes_nothing(input in ".{0,60}") {
        let once = sanitise_stem(&input);
        let twice = sanitise_stem(&once);
        // Explicit arguments, not inline captures: `prop_assert_eq!` builds its
        // message with `concat!`, and captured identifiers are not in scope for
        // the format string it constructs.
        prop_assert_eq!(
            &once,
            &twice,
            "sanitise_stem is not idempotent: {:?} -> {:?} -> {:?}",
            input,
            once,
            twice
        );
    }

    /// A safe name survives unchanged. Without this the properties above would
    /// also pass against a sanitiser that replaced everything with `-`, which
    /// would be safe and useless.
    #[test]
    fn an_ordinary_photo_name_is_left_alone(
        n in 1usize..20, a in 1u32..9_000, b in 1u32..9_000,
    ) {
        // Three shapes a real photo file has: a bare stem, a stem with an
        // extension, and a camera-style name where the counter precedes the
        // stem. The extension is always dropped; the stem is never otherwise
        // touched.
        // A camera-app name: a stem, a counter, an extension. The whole
        // `stem_counter` part is the stem, so all of it must survive - only the
        // extension is dropped.
        let stem = format!("IMG{n}");
        prop_assert_eq!(sanitise_stem(&stem), stem.as_str());
        prop_assert_eq!(
            sanitise_stem(&format!("{stem}_{a}.jpg")),
            format!("{stem}_{a}")
        );
        prop_assert_eq!(
            sanitise_stem(&format!("{stem}_{b}.jpeg")),
            format!("{stem}_{b}")
        );
    }
}

// ---------------------------------------------------------------------------
// TargetBytes::encode_with
// ---------------------------------------------------------------------------

/// A synthetic encoder whose output size is strictly monotonic in quality.
///
/// Real encoders are only *approximately* monotonic - noise can invert a step -
/// which is exactly why `encode_with` needs a test with an encoder whose
/// behaviour is known. `size = base + quality * step`, so the highest quality
/// that fits `target` is exactly `(target - base) / step`.
fn monotonic_encoder(
    base: usize,
    step: usize,
) -> impl Fn(&image::DynamicImage, OutputFormat, EncodingOptions) -> Result<Vec<u8>, Error> {
    move |_img, _format, options| Ok(vec![0u8; base + step * usize::from(options.quality)])
}

proptest! {
    #![proptest_config(config(0x5EED_0006))]

    /// The binary search returns the *highest* quality that fits, not merely
    /// *some* quality that fits.
    ///
    /// Getting this wrong is invisible in a photo: the file is under the limit
    /// either way, just visibly softer than it needed to be. So the property is
    /// about the ceiling of the feasible set, not about feasibility.
    #[test]
    fn target_bytes_returns_the_highest_quality_that_fits(
        base in 100usize..5_000, step in 1usize..500, quality in 1u16..=100,
    ) {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 4));
        // Land exactly on a quality the encoder can produce.
        let target = base + step * usize::from(quality);

        // The search runs over [min_quality, max_quality], not [1, 100] -
        // `TargetBytes::new` starts at 30 because below that the encoders produce
        // visibly broken output. A target reachable only below the floor is not
        // reachable at all, so widen the floor rather than assert on a case the
        // caller could not have asked for.
        let mut search = TargetBytes::new(target as u64);
        search.min_quality = 1;
        search.max_quality = 100;

        let (bytes, used, met) = search
            .encode_with(
                &img,
                OutputFormat::Jpeg,
                EncodingOptions::default(),
                &monotonic_encoder(base, step),
            )
            .unwrap();

        prop_assert!(met, "an exactly-reachable target must be reported as met");
        prop_assert!(
            bytes.len() <= target,
            "returned {} bytes for a {target} byte target",
            bytes.len()
        );
        prop_assert_eq!(
            usize::from(used), usize::from(quality),
            "should have chosen the highest fitting quality"
        );
        // The next quality up genuinely does not fit, so this really is the
        // ceiling rather than an accident of the search order.
        if usize::from(quality) < 100 {
            let next = base + step * (usize::from(quality) + 1);
            prop_assert!(next > target, "test setup: q+1 should not fit");
        }
    }

    /// Determinism. The same inputs must give byte-identical output, or the UI
    /// would show the user a size that changes when they press export twice.
    #[test]
    fn target_bytes_is_deterministic(
        base in 100usize..5_000, step in 1usize..500, target in 200usize..40_000,
    ) {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8));
        let search = TargetBytes::new(target as u64);

        let first = search
            .encode_with(
                &img,
                OutputFormat::Jpeg,
                EncodingOptions::default(),
                &monotonic_encoder(base, step),
            )
            .unwrap();
        let second = search
            .encode_with(
                &img,
                OutputFormat::Jpeg,
                EncodingOptions::default(),
                &monotonic_encoder(base, step),
            )
            .unwrap();

        prop_assert_eq!(first.0, second.0, "output bytes differed between runs");
        prop_assert_eq!(first.1, second.1, "quality differed between runs");
        prop_assert_eq!(first.2, second.2, "target_met differed between runs");
    }

    /// When nothing fits, the floor quality is returned with `target_met: false`
    /// rather than an error. The user gets a real file plus a warning, instead of
    /// a failure they cannot act on.
    #[test]
    fn an_unreachable_target_returns_the_floor_and_says_so(
        base in 50_000usize..200_000, step in 1usize..500, target in 1u64..10_000,
    ) {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 4));
        let mut search = TargetBytes::new(target);
        search.min_quality = 1;
        search.max_quality = 100;
        let (bytes, used, met) = search
            .encode_with(
                &img,
                OutputFormat::Jpeg,
                EncodingOptions::default(),
                &monotonic_encoder(base, step),
            )
            .unwrap();

        prop_assert!(!met, "an unreachable target must not claim success");
        prop_assert!(
            bytes.len() > target as usize,
            "the floor attempt must still exceed the target, or it would have fit"
        );
        prop_assert!(used >= 1, "quality {used} is below the floor of 1");
    }

    /// A lossless format must not pretend a quality search happened. Reporting a
    /// quality number for a control that did nothing is a claim the caller then
    /// displays (see `TargetBytes::encode_with`).
    #[test]
    fn a_lossless_format_reports_no_quality(target in 1u64..100_000) {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 4));
        let search = TargetBytes::new(target);
        let (_, used, _) = search
            .encode_with(
                &img,
                OutputFormat::Png,
                EncodingOptions::default(),
                &monotonic_encoder(10, 1000),
            )
            .unwrap();
        prop_assert_eq!(used, 0, "PNG must report quality 0, not a searched value");
    }
}

// ---------------------------------------------------------------------------
// Failure paths stay explained
// ---------------------------------------------------------------------------

/// Hard rule 3: a zero dimension on the way in is an error, not a panic. The
/// `expect` is the assertion.
#[test]
fn zero_source_dimensions_are_an_error_not_a_panic() {
    let spec = ResizeSpec {
        width: Some(100),
        height: None,
        fit: FitMode::Width,
        filter: ResampleFilter::Nearest,
        no_upscale: false,
    };
    let result = spec.resolve(0, 100);
    assert!(
        matches!(result, Err(Error::ZeroDimension)),
        "a zero source width must be ZeroDimension, got {result:?}"
    );
}

/// A `Fill` to a zero target is refused, and the message says what happened.
#[test]
fn a_zero_target_is_refused_with_an_explained_message() {
    let spec = ResizeSpec {
        width: Some(0),
        height: Some(0),
        fit: FitMode::Fill,
        filter: ResampleFilter::Nearest,
        no_upscale: false,
    };
    match spec.resolve(100, 100) {
        Err(e) => {
            let text = e.to_string();
            assert!(
                text.contains("greater than zero"),
                "the message must explain the problem: {text}"
            );
        }
        Ok(dim) => panic!("a 0x0 target must be refused, got {dim:?}"),
    }
}

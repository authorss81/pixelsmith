// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: `validate_bytes` — the header-only check the UI runs on a whole
//! folder before the user commits to anything.
//!
//! Two properties matter here and neither is "it returned".
//!
//! * **It stays header-only.** A folder of 200 files must be inspectable without
//!   decoding 200 images, so the measured peak heap has to track the *file* size,
//!   not the pixel count. The one documented exception is GIF, which counts its
//!   frames to answer `has_animated` and therefore decodes as it goes.
//! * **Its report is self-consistent.** Every field the UI shows is computed here,
//!   and a report that claims 0x0, or a megapixel count that disagrees with its
//!   own dimensions, is worse than no report: the user acts on it.

use libfuzzer_sys::fuzz_target;
use pixelsmith_core::{OutputFormat, decode_bounded, detect_format, validate_bytes};
use px_fuzz::{Case, HeapBudget, fuzz_limits, is_resource_error, measure_peak};

fuzz_target!(|data: &[u8]| {
    let Some((case, tail)) = Case::from_bytes(data) else {
        return;
    };
    let bytes = case.payload(case.format(), tail);
    let limits = fuzz_limits(case.profile);

    // GIF is the exception, and it is asserted rather than assumed: the report's
    // `has_animated` is computed by walking every frame, so a GIF's inspection
    // legitimately decodes. The ceiling for it comes from the pixel budget, which
    // is the same ceiling a full decode gets.
    let is_gif = matches!(detect_format(&bytes), Ok(OutputFormat::Gif));
    let budget = if is_gif {
        HeapBudget {
            ceiling: limits.max_pixels as usize * 4 + px_fuzz::DECODER_SLACK,
            hard_cap: px_fuzz::HARD_CAP,
            max_pixels: limits.max_pixels,
            what: "validate_bytes (animated gif, frames are counted)",
        }
    } else {
        HeapBudget::header_only(bytes.len(), "validate_bytes")
    };

    let (report, peak) = measure_peak(|| validate_bytes(&bytes, &limits));
    budget.assert_within(peak);

let report = match report {
        Ok(report) => report,
        Err(e) => {
            // A file over the input ceiling must be rejected as such, before a
            // single byte of it is parsed. The user is told why, and no work was
            // wasted on a file that was never going to be accepted.
            if bytes.len() > limits.max_input_bytes {
                assert!(
                    matches!(e, pixelsmith_core::Error::InputTooLarge { .. }),
                    "an oversize file must be rejected as oversize, got: {e}"
                );
            }
            return;
        }
    };

    assert!(report.width > 0 && report.height > 0, "accepted a zero-dimension image");
    assert!(
        u64::from(report.width) * u64::from(report.height) <= limits.max_pixels,
        "accepted {}x{} under a {} pixel budget",
        report.width,
        report.height,
        limits.max_pixels
    );
    assert!(
        report.width <= limits.max_dimension && report.height <= limits.max_dimension,
        "accepted {}x{} under a {} px per-side limit",
        report.width,
        report.height,
        limits.max_dimension
    );

    // `megapixels` is what the UI prints next to the file size, so it has to be
    // the product of the dimensions it was printed next to.
    let expected = f64::from(report.width) * f64::from(report.height) / 1_000_000.0;
    assert!(
        (report.megapixels - expected).abs() < 1e-6,
        "report says {} MP for {}x{}",
        report.megapixels,
        report.width,
        report.height
    );

    // `suspicious` is the warning badge. It must appear for exactly the two
    // reasons the function documents, or the user is shown a warning that means
    // nothing — or not shown one that should be there.
    let over_budget = u64::from(report.width) * u64::from(report.height) > limits.max_pixels;
    let over_sides = report.width > limits.max_dimension || report.height > limits.max_dimension;
    assert_eq!(
        report.suspicious.is_some(),
        over_budget || over_sides,
        "suspicious = {:?} for {}x{} under max_pixels {} / max_dimension {}",
        report.suspicious,
        report.width,
        report.height,
        limits.max_pixels,
        limits.max_dimension
    );
    if let Some(note) = &report.suspicious {
        assert!(!note.is_empty(), "an empty warning tells the user nothing");
    }

    // Animation is only reported for formats that can carry it, or the UI offers
    // to flatten something that was never animated.
    assert!(
        !report.has_animated || report.format.supports_animation(),
        "{:?} reported as animated",
        report.format
    );
    // `validate_bytes` returns `Err` unless the sniffer accepted the file, so
    // this is not a formality: it checks that the report's format is the format
    // the bytes actually are.
    let sniffed = detect_format(&bytes).expect("validate_bytes accepted what the sniffer rejects");
    assert_eq!(report.format, sniffed, "the report disagrees with the sniffer");

    // An orientation outside 1..=8 is not a value the pipeline can act on; it is
    // read out of the file and must not reach the UI as-is.
    if let Some(orientation) = report.orientation {
        assert!(
            (1..=8).contains(&orientation),
            "reported EXIF orientation {orientation}, which is not a real value"
        );
    }

    // The cross-check that matters most. Validation accepted, so a resource
    // refusal at decode time would mean the two disagree about the budget — a
    // hole in hard rule 4 rather than a codec saying no to garbage.
    let budget = HeapBudget {
        ceiling: limits.max_pixels as usize * 4 + px_fuzz::DECODER_SLACK,
        hard_cap: px_fuzz::HARD_CAP,
        max_pixels: limits.max_pixels,
        what: "decode_bounded after validate_bytes accepted",
    };
    let (decoded, peak) = measure_peak(|| decode_bounded(&bytes, &limits));
    budget.assert_within(peak);
    match decoded {
        Ok(img) => {
            // The report's dimensions are what the preview is sized from and what
            // the output filename is derived from. If the decoder disagrees with
            // the header the user was shown, the crop rectangle and the exported
            // image are for different pictures.
            assert_eq!(
                (img.width(), img.height()),
                (report.width, report.height),
                "report said {}x{} and the decoder produced {}x{}",
                report.width,
                report.height,
                img.width(),
                img.height()
            );
        }
        Err(e) => assert!(
            !is_resource_error(&e),
            "validate_bytes accepted a file that decode_bounded refused as too big: {e}"
        ),
    }
});
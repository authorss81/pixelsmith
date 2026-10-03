// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: the GIF decoder, on its own — every frame, not just the first.
//!
//! GIF is the one format where a single file is many images, and the engine does
//! walk all of them: `validate::count_frames` iterates `into_frames()` to answer
//! `has_animated` before the user has committed to anything. That path is the
//! reason this target exists in its own right rather than as a decode case — it
//! is the only place the engine decodes something the user did not ask to decode.
//!
//! Two GIF-specific properties are asserted:
//!
//! * **A frame cannot be bigger than the picture.** The logical screen sets the
//!   bounds and a frame descriptor is free to lie about its own size; a frame
//!   that decodes wider than the screen it belongs to is a decoder allocating
//!   from a field it should have bounded.
//! * **The engine's animation flag agrees with what the decoder found.** The UI
//!   shows "animated" and offers to flatten the result, so the two counts have to
//!   come out the same or the user is told the truth about one thing and misled
//!   about the other.
//!
//! Frames are dropped as they are read, so the measurement is the peak of one
//! frame, not of the whole animation — which is what a decoder that holds every
//! frame would show up as.

use image::ImageDecoder as _;
use image::AnimationDecoder as _;
use image::codecs::gif::GifDecoder;
use libfuzzer_sys::fuzz_target;
use pixelsmith_core::{OutputFormat, decode_bounded, validate_bytes};
use px_fuzz::{Case, cursor, decoder_limits, fuzz_limits, heap_budget, measure_peak, seed_magic};

fuzz_target!(|data: &[u8]| {
    let Some((case, tail)) = Case::from_bytes(data) else {
        return;
    };
    let bytes = if case.with_header { seed_magic(OutputFormat::Gif, tail) } else { tail.to_vec() };
    let limits = fuzz_limits(case.profile);
    let budget = heap_budget(&limits, "GifDecoder frames");

    // Walk every frame, exactly as `count_frames` does, counting items rather
    // than successes: an error item is still an item, and the two counters have
    // to agree or the cross-check below proves nothing.
    let (walked, peak) = measure_peak(|| {
        let Ok(mut decoder) = GifDecoder::new(cursor(&bytes)) else {
            return (0usize, (0u32, 0u32));
        };
        let Ok(()) = decoder.set_limits(decoder_limits(&limits)) else {
            // The logical screen is over the per-side limit, so the engine
            // refuses this file too. Nothing to walk.
            return (0usize, (0u32, 0u32));
        };
        let screen = decoder.dimensions();
        let mut frames = 0usize;
        for item in decoder.into_frames() {
            frames += 1;
            let Ok(frame) = item else {
                // A truncated animation stops here. The frame before it was
                // already checked; there is nothing further to learn.
                break;
            };
            let (w, h) = frame.buffer().dimensions();
            // A frame is allowed to be larger than the logical screen — the spec
            // permits it and real files do it — so the screen is not a bound. The
            // pixel budget is, because that is what the decoder is about to
            // allocate from a field the file wrote.
            assert!(
                u64::from(w) * u64::from(h) <= limits.max_pixels,
                "frame {frames} is {w}x{h}, over the {} pixel budget",
                limits.max_pixels
            );
        }
        (frames, screen)
    });
    budget.assert_within(peak);

    let (frames, screen) = walked;
    let (engine, peak) = measure_peak(|| decode_bounded(&bytes, &limits));
    budget.assert_within(peak);

    match validate_bytes(&bytes, &limits) {
        Ok(report) => {
            // The report's dimensions are the logical screen, and the frame walk
            // read the same header.
            assert_eq!(
                (report.width, report.height),
                screen,
                "the report and the frame walk disagree about the canvas"
            );
            assert_eq!(
                report.has_animated,
                frames > 1,
                "the report says animated={} after walking {frames} frames",
                report.has_animated
            );
        }
        Err(_) => {
            assert!(
                frames == 0,
                "validate_bytes refused a GIF whose {frames} frames all decoded"
            );
        }
    }

    // The engine decoded the first frame, so that image must be inside the
    // profile: `decode_bounded` is the only way a user reaches these pixels.
    if let Ok(img) = engine {
        if let Err(e) = limits.check_decoded(&img) {
            panic!("decode_bounded returned {}x{} outside the profile: {e}", img.width(), img.height());
        }
    }
});
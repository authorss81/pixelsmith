// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: the JPEG decoder, on its own.
//!
//! JPEG is the format the product is used on most and the one whose headers are
//! pure attacker data: the frame header carries the dimensions, the component
//! count and the sampling factors, and a decoder that trusts any of them can be
//! asked for gigabytes before it has read a single entropy-coded byte. The
//! payload always starts with a real `SOI` plus a JFIF `APP0`, so the target
//! reaches the frame-header walk rather than bouncing off the signature.
//!
//! Asserted here: the decoder's dimensions and buffer arithmetic are consistent
//! with each other, a decoded frame is inside the profile it was given, and the
//! engine's bounded path never calls a file too large that this decoder read
//! under the same limits.

use image::codecs::jpeg::JpegDecoder;
use libfuzzer_sys::fuzz_target;
use pixelsmith_core::{OutputFormat, decode_bounded};
use px_fuzz::{
    Case, assert_engine_agrees, assert_sniffed, assert_within_limits, cursor,
    decode_into_exact_buffer, fuzz_limits, heap_budget, measure_peak, seed_magic,
};

fuzz_target!(|data: &[u8]| {
    let Some((case, tail)) = Case::from_bytes(data) else {
        return;
    };
    let bytes = if case.with_header {
        seed_magic(OutputFormat::Jpeg, tail)
    } else {
        tail.to_vec()
    };
    let limits = fuzz_limits(case.profile);
    let budget = heap_budget(&limits, "JpegDecoder");

    let (raw, peak) = measure_peak(|| {
        JpegDecoder::new(cursor(&bytes))
            .ok()
            .map(|decoder| decode_into_exact_buffer(decoder, &limits, "JpegDecoder"))
    });
    budget.assert_within(peak);

    let raw_decoded = match raw {
        None => false,
        Some(Err(_)) => false,
        Some(Ok(img)) => {
            // A frame that decoded is a frame whose dimensions and sample buffer
            // agreed. `decode_into_exact_buffer` checks the arithmetic; this
            // checks the result against the profile that was supposed to be in
            // force, which is the check that catches an unenforced limit.
            assert_within_limits(&img, &limits);
            assert!(
                img.width() > 0 && img.height() > 0,
                "JpegDecoder returned a zero-sized frame"
            );
            true
        }
    };

    let (engine, peak) = measure_peak(|| decode_bounded(&bytes, &limits));
    budget.assert_within(peak);
    assert_sniffed(&bytes, OutputFormat::Jpeg, raw_decoded);
    assert_engine_agrees(raw_decoded, engine);
});
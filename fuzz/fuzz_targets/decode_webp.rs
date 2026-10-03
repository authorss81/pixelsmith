// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: the WebP decoder, on its own.
//!
//! WebP is three formats in a container: `VP8 ` (lossy), `VP8L` (lossless) and
//! `VP8X` (extended, which adds a 24-bit canvas, an alpha plane and an
//! animation chunk). The container is a RIFF, so the chunk length fields are
//! little-endian and are frequently out of step with the data behind them.
//!
//! The payload always starts with a real `RIFF`/`WEBP` header and a chunk tag the
//! fuzzer picks, which is how all three sub-formats stay one byte apart instead
//! of the fuzzer having to discover each tag separately.
//!
//! The sniffer cross-check is absent here on purpose: the decoder accepts a bare
//! VP8 bitstream with no RIFF wrapper at all, so "the decoder read it" does not
//! imply "the sniffer calls it WebP".

use image::codecs::webp::WebPDecoder;
use libfuzzer_sys::fuzz_target;
use pixelsmith_core::{OutputFormat, decode_bounded};
use px_fuzz::{
    Case, assert_engine_agrees, assert_within_limits, cursor, decode_into_exact_buffer, fuzz_limits,
    heap_budget, measure_peak, seed_magic,
};

fuzz_target!(|data: &[u8]| {
    let Some((case, tail)) = Case::from_bytes(data) else {
        return;
    };
    let bytes = if case.with_header { seed_magic(OutputFormat::WebP, tail) } else { tail.to_vec() };
    let limits = fuzz_limits(case.profile);
    let budget = heap_budget(&limits, "WebPDecoder");

    let (raw, peak) = measure_peak(|| {
        WebPDecoder::new(cursor(&bytes))
            .ok()
            .map(|decoder| decode_into_exact_buffer(decoder, &limits, "WebPDecoder"))
    });
    budget.assert_within(peak);

    let raw_decoded = match raw {
        None | Some(Err(_)) => false,
        Some(Ok(img)) => {
            assert_within_limits(&img, &limits);
            // The extended format's canvas is 24 bits wide per side, so it can
            // describe an image four times wider than a JPEG's 16-bit frame
            // header. That is precisely the case a per-side limit has to catch,
            // and `check_decoded` is what catches it.
            assert!(
                u64::from(img.width()) * u64::from(img.height()) <= limits.max_pixels,
                "WebP decoded {}x{} over the {} pixel budget",
                img.width(),
                img.height(),
                limits.max_pixels
            );
            true
        }
    };

    let (engine, peak) = measure_peak(|| decode_bounded(&bytes, &limits));
    budget.assert_within(peak);
    assert_engine_agrees(raw_decoded, engine);
});
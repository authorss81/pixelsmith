// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: the BMP decoder, on its own.
//!
//! BMP has no compression and no chunk structure: fourteen bytes of file header,
//! forty of DIB header, then raw pixels. Which makes it the format where the
//! arithmetic is the whole attack surface — the width and height are *signed*
//! 32-bit fields, so a file can ask for a negative extent, and the row stride is
//! `width * bytes-per-pixel` with padding to a four-byte boundary.
//!
//! The payload is therefore always laid down with a complete 40-byte
//! `BITMAPINFOHEADER` whose fields the fuzzer writes, so a negative dimension or
//! a row-stride overflow is reachable in one step instead of after discovering
//! the container.
//!
//! Asserted here: the decoder's buffer arithmetic, that a decoded image is inside
//! the profile it was given, that a BMP that decoded was sniffed as a BMP, and
//! that the engine's bounded path never calls such a file too large.

use image::codecs::bmp::BmpDecoder;
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
    let bytes = if case.with_header { seed_magic(OutputFormat::Bmp, tail) } else { tail.to_vec() };
    let limits = fuzz_limits(case.profile);
    let budget = heap_budget(&limits, "BmpDecoder");

    let (raw, peak) = measure_peak(|| {
        BmpDecoder::new(cursor(&bytes))
            .ok()
            .map(|decoder| decode_into_exact_buffer(decoder, &limits, "BmpDecoder"))
    });
    budget.assert_within(peak);

    let raw_decoded = match raw {
        None | Some(Err(_)) => false,
        Some(Ok(img)) => {
            assert_within_limits(&img, &limits);
            // A negative extent that survived into an image would mean the signed
            // header fields were read as unsigned somewhere, and every later
            // stride calculation would be built on it.
            assert!(img.width() > 0 && img.height() > 0, "BMP decoded a non-positive extent");
            true
        }
    };

    let (engine, peak) = measure_peak(|| decode_bounded(&bytes, &limits));
    budget.assert_within(peak);
    assert_sniffed(&bytes, OutputFormat::Bmp, raw_decoded);
    assert_engine_agrees(raw_decoded, engine);
});
// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: the PNG decoder, on its own.
//!
//! PNG is the format where a dimension check is most obviously necessary and most
//! often wrong: the whole picture is described by a thirteen-byte IHDR, and
//! everything after it — the IDAT stream, the interlacing pass, the expanded
//! palette — can be made to produce far more data than thirteen bytes asked for.
//! The payload is therefore always laid down with a well-formed IHDR whose
//! thirteen payload bytes the fuzzer writes, so the width and height under test
//! are always the ones the decoder will use.
//!
//! Asserted here: the decoder's own buffer arithmetic, that a decoded image is
//! inside the profile it was given, that a PNG that decoded was sniffed as a PNG,
//! and that the engine's bounded path never calls such a file too large.

use image::codecs::png::PngDecoder;
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
    let bytes = if case.with_header { seed_magic(OutputFormat::Png, tail) } else { tail.to_vec() };
    let limits = fuzz_limits(case.profile);
    let budget = heap_budget(&limits, "PngDecoder");

    let (raw, peak) = measure_peak(|| {
        PngDecoder::new(cursor(&bytes))
            .ok()
            .map(|decoder| decode_into_exact_buffer(decoder, &limits, "PngDecoder"))
    });
    budget.assert_within(peak);

    let raw_decoded = match raw {
        None | Some(Err(_)) => false,
        Some(Ok(img)) => {
            assert_within_limits(&img, &limits);
            // PNG is the only format here whose decoder can legally return a
            // paletted image, which `image` expands to RGB. An image that came
            // back with four channels per pixel therefore had an alpha channel,
            // and one that came back with one is a greyscale — neither is a bug,
            // but both have to be a real colour type, which `assert_within_limits`
            // has already established by way of `check_decoded`.
            assert!(
                matches!(img.color().channel_count(), 1 | 2 | 3 | 4),
                "decoded a PNG with {} channels",
                img.color().channel_count()
            );
            true
        }
    };

    let (engine, peak) = measure_peak(|| decode_bounded(&bytes, &limits));
    budget.assert_within(peak);
    assert_sniffed(&bytes, OutputFormat::Png, raw_decoded);
    assert_engine_agrees(raw_decoded, engine);
});
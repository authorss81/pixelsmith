// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: the TIFF decoder, on its own.
//!
//! TIFF is the format with the most ways to lie: the header is four bytes of
//! byte-order mark and an IFD offset, and everything else — how many entries the
//! directory has, where each one's data lives, whether it is tiled or stripped —
//! is a number in the file. The offset can point backwards or past the end, and
//! the entry count is a `u16`, so a four-hundred-byte file can claim a directory
//! of 65535 entries.
//!
//! The payload always starts with a real byte-order mark and an IFD offset the
//! fuzzer chose, so the directory walk happens rather than being skipped.
//!
//! Asserted here: the decoder's buffer arithmetic, that a decoded image is inside
//! the profile it was given, that a TIFF that decoded was sniffed as a TIFF, and
//! that the engine's bounded path never calls such a file too large.

use image::codecs::tiff::TiffDecoder;
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
    let bytes = if case.with_header { seed_magic(OutputFormat::Tiff, tail) } else { tail.to_vec() };
    let limits = fuzz_limits(case.profile);
    let budget = heap_budget(&limits, "TiffDecoder");

    let (raw, peak) = measure_peak(|| {
        TiffDecoder::new(cursor(&bytes))
            .ok()
            .map(|decoder| decode_into_exact_buffer(decoder, &limits, "TiffDecoder"))
    });
    budget.assert_within(peak);

    let raw_decoded = match raw {
        None | Some(Err(_)) => false,
        Some(Ok(img)) => {
            assert_within_limits(&img, &limits);
            // TIFF is the format that carries an ICC profile and an EXIF block,
            // so a decode also runs those parsers. Their output must not change
            // the geometry, and this is the iteration where the byte count of the
            // file and the pixel count of the image have been asked to disagree.
            assert!(
                u64::from(img.width()) * u64::from(img.height()) <= limits.max_pixels,
                "{}x{} decoded over the {} pixel budget",
                img.width(),
                img.height(),
                limits.max_pixels
            );
            true
        }
    };

    let (engine, peak) = measure_peak(|| decode_bounded(&bytes, &limits));
    budget.assert_within(peak);
    assert_sniffed(&bytes, OutputFormat::Tiff, raw_decoded);
    assert_engine_agrees(raw_decoded, engine);
});
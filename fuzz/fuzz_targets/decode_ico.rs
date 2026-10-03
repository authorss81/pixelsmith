// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: the ICO decoder, on its own.
//!
//! ICO is a container of images rather than an image: a six-byte directory whose
//! entry count is a `u16`, then sixteen bytes per entry giving a size, an offset
//! and a width/height pair stored in single bytes where zero means 256. Every one
//! of those numbers is the file's to choose, and the entries are frequently BMPs
//! or PNGs in their own right, so a correct ICO decode means parsing two
//! containers.
//!
//! The payload always starts with a real `ICONDIR` whose entry count the fuzzer
//! chose, so the directory walk happens: the interesting cases are a count of
//! 65535 with no entries behind it, and an entry pointing at a second container
//! that claims to be enormous.
//!
//! The sniffer cross-check is deliberately absent here. `IcoDecoder::new` reads
//! the reserved and type words without checking them, so it accepts bytes the
//! sniffer does not call ICO — asserting otherwise would be asserting a rule the
//! decoder does not follow.

use image::codecs::ico::IcoDecoder;
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
    let bytes = if case.with_header { seed_magic(OutputFormat::Ico, tail) } else { tail.to_vec() };
    let limits = fuzz_limits(case.profile);
    let budget = heap_budget(&limits, "IcoDecoder");

    // The directory itself is attacker-controlled and parsed before any decoding
    // happens, so it is measured separately from the pixel buffer.
    let declared_entries = bytes
        .get(4..6)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .unwrap_or(0);
    let (raw, peak) = measure_peak(|| {
        IcoDecoder::new(cursor(&bytes))
            .ok()
            .map(|decoder| decode_into_exact_buffer(decoder, &limits, "IcoDecoder"))
    });
    budget.assert_within(peak);

    // A directory claiming more entries than the file has bytes for them cannot
    // have been fully read, so the decoder must have refused it rather than
    // inventing entries.
    if declared_entries as usize > bytes.len().saturating_sub(6) / 16 {
        assert!(
            matches!(raw, None | Some(Err(_))),
            "a directory claiming {declared_entries} entries in {} bytes decoded anyway",
            bytes.len()
        );
    }

    let raw_decoded = match raw {
        None | Some(Err(_)) => false,
        Some(Ok(img)) => {
            assert_within_limits(&img, &limits);
            // The single-byte width and height in an entry mean zero is 256, so
            // 257 is not a size any entry can ask for. A decoded image wider than
            // that came from the embedded container ignoring the directory.
            assert!(
                img.width() <= 256 && img.height() <= 256,
                "ICO decoded {}x{}, which no ICONDIR entry can describe",
                img.width(),
                img.height()
            );
            true
        }
    };

    let (engine, peak) = measure_peak(|| decode_bounded(&bytes, &limits));
    budget.assert_within(peak);
    assert_engine_agrees(raw_decoded, engine);
});
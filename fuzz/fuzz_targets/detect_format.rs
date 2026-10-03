// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: `format::detect_format` — the answer the UI is given about a
//! file, derived from magic bytes because a filename cannot be trusted.
//!
//! What a crash here would mean: something other than `Err` escaped, which on a
//! hostile file is a denial of service in the one function every other entry
//! point calls first. The assertions below are about the answer being *usable*:
//! the format the engine reports has to be one it can actually name and act on.

use libfuzzer_sys::fuzz_target;
use pixelsmith_core::{OutputFormat, detect_format};
use px_fuzz::{Case, HeapBudget, measure_peak};

/// True only for bytes `image` really does treat as AVIF: a four-byte box size,
/// then `ftypavif`.
///
/// Spelled out rather than taken from `image` so the check fails if the sniffer's
/// AVIF rule ever widens. Calling a non-AVIF file AVIF would promise a format
/// `capabilities()` reports as unwritable, which is hard rule 10.
fn looks_like_avif(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..12] == b"ftypavif"
}

fuzz_target!(|data: &[u8]| {
    let Some((case, tail)) = Case::from_bytes(data) else {
        return;
    };
    let bytes = case.payload(case.format(), tail);

    // Sniffing reads the container signature and nothing else, so it must not
    // allocate in proportion to the file. Measured, not assumed: if a regression
    // made this read the whole buffer into a decode, the peak moves.
    let budget = HeapBudget::header_only(bytes.len(), "detect_format");
    let ((first, again), peak) = measure_peak(|| (detect_format(&bytes), detect_format(&bytes)));
    budget.assert_within(peak);

    match (first, again) {
        (Ok(fmt), repeat) => {
            // The extension and MIME tables cannot drift away from the enum: the
            // output filename is derived from the extension, so a mismatch would
            // write `photo.jpg` for a PNG.
            assert_eq!(
                OutputFormat::from_extension(fmt.extension()),
                Some(fmt),
                "extension table disagrees with the enum for {fmt:?}"
            );
            assert!(fmt.mime().starts_with("image/"), "{fmt:?} has a non-image MIME type");
            assert_eq!(repeat.unwrap(), fmt, "detect_format is not deterministic");

            if fmt == OutputFormat::Avif {
                assert!(
                    looks_like_avif(&bytes),
                    "reported AVIF for bytes that are not a ftyp/avif box"
                );
            }
        }
        (Err(_), repeat) => {
            assert!(repeat.is_err(), "detect_format disagreed with itself");
        }
    }

    // A signature lives in the first sixteen bytes by definition, so a file that
    // stops early either sniffs as something else or as nothing at all. Neither
    // may be allowed to panic, and both must stay self-consistent.
    for length in [1usize, 2, 3, 4, 8, 12, 16] {
        let prefix = &bytes[..length.min(bytes.len())];
        if let Ok(fmt) = detect_format(prefix) {
            assert_eq!(
                OutputFormat::from_extension(fmt.extension()),
                Some(fmt),
                "{length}-byte prefix reported {fmt:?} with no matching extension"
            );
        }
    }
});
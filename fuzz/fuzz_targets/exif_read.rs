// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: `exif::read` — the metadata reader.
//!
//! EXIF is the part of an image file that is pure data: nested IFDs, variable
//! length rationals, offsets that point backwards, entry counts that are `u16`
//! and therefore free to lie. It is also the part that decides whether the app
//! tells a user their photo carries GPS coordinates, so a wrong answer here is a
//! privacy problem and not only a robustness one.
//!
//! Three properties are asserted:
//!
//! * `read` and `has_exif` agree. They are separate entry points over the same
//!   parser, and the UI shows one and acts on the other.
//! * The report is internally consistent: a GPS flag with no GPS tag, a
//!   sensitive tag list longer than the entry list, an orientation the pipeline
//!   cannot act on.
//! * Parsing memory is bounded. A crafted IFD can claim 65535 entries in a file
//!   of two hundred bytes, so the measured peak is what proves the reader walks
//!   the directory it was given rather than the one it was promised.
//!
//! A panic in this file is the finding, not a mistake in the harness: libFuzzer
//! writes the reproducer to `fuzz/artifacts/exif_read/` and reruns it, which is
//! what turns "something in here can be made to panic" into a test case.

use libfuzzer_sys::fuzz_target;
use pixelsmith_core::exif::{self, ExifInfo};
use px_fuzz::{Case, HeapBudget, measure_peak};

fn check(info: &ExifInfo) {
    // Every reported entry has to be renderable: the UI puts these strings in a
    // list, and an empty tag or an empty value is a row the user cannot read.
    for entry in &info.entries {
        assert!(!entry.tag.is_empty(), "an EXIF entry with an empty tag");
        assert!(!entry.value.is_empty(), "tag {} has an empty value", entry.tag);
    }

    assert!(
        info.sensitive_tags.len() <= info.entries.len(),
        "{} sensitive tags reported from {} entries",
        info.sensitive_tags.len(),
        info.entries.len()
    );
    for tag in &info.sensitive_tags {
        assert!(
            info.entries.iter().any(|e| &e.tag == tag),
            "sensitive tag {tag} is not in the entry list"
        );
    }

    // `has_gps` drives "this file carries your location". It must be exactly the
    // presence of a GPS tag, never a stale default and never a tag the filter
    // would have dropped from the list entirely.
    let tagged_gps = info
        .entries
        .iter()
        .any(|e| e.tag.to_ascii_uppercase().starts_with("GPS"));
    assert_eq!(info.has_gps, tagged_gps, "has_gps disagrees with the GPS tags");

    if let Some(orientation) = info.orientation {
        assert!(
            (1..=8).contains(&orientation),
            "orientation {orientation} is not a value the pipeline can act on"
        );
    }

    // `camera` is accumulated from Make and Model, so it can only be present when
    // at least one of those tags is present.
    if let Some(camera) = &info.camera {
        assert!(!camera.is_empty(), "an empty camera name");
        assert!(
            info.entries
                .iter()
                .any(|e| matches!(e.tag.as_str(), "Make" | "Model")),
            "a camera name with neither Make nor Model behind it"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((case, tail)) = Case::from_bytes(data) else {
        return;
    };
    // Every format's container is tried, because EXIF lives inside JPEG APP1
    // segments, inside PNG eXIf chunks and inside TIFF IFDs, and each of those
    // wrappers is a separate parser with its own idea of where the block starts.
    let bytes = case.payload(case.format(), tail);

    let budget = HeapBudget::header_only(bytes.len(), "exif::read");

    let (parsed, peak) = measure_peak(|| exif::read(&bytes));
    budget.assert_within(peak);
    let info = parsed.expect("exif::read documents no error path, so it must not fail");

    let (has_exif, peak) = measure_peak(|| exif::has_exif(&bytes));
    budget.assert_within(peak);
    // One parser, two entry points. The sound direction is the strict one: no
    // container means no entries. The converse does not hold and is not asserted —
    // a container can parse and yield nothing renderable, which is a legitimate
    // answer and not a finding.
    if !has_exif {
        assert!(
            info.entries.is_empty(),
            "has_exif said false but read returned {} entries",
            info.entries.len()
        );
    }

    check(&info);

    // Prefixes: a truncated EXIF block is what a half-written file looks like, and
    // it must degrade to "no metadata" rather than to a panic.
    for length in [8usize, 32, 128] {
        if length >= bytes.len() {
            continue;
        }
        let (prefix, peak) = measure_peak(|| exif::read(&bytes[..length]));
        budget.assert_within(peak);
        check(&prefix.expect("exif::read documents no error path"));
    }
});
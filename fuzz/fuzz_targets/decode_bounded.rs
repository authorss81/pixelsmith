// libFuzzer supplies `main` from its C++ runtime, so this crate must not
// define one. `cargo fuzz` also needs the `#![no_main]`, which is why a
// fuzz target cannot be built with a plain `cargo build`.
#![no_main]
//! Fuzz target: `decode_bounded` — the one function every image the user opens
//! goes through, and therefore the whole of the engine's attack surface.
//!
//! The engine's claim is threefold and each part is asserted separately:
//!
//! * **Nothing escapes.** Not `Ok`, not `Err` — a panic, an arithmetic overflow
//!   or a thread abort is a crash on a file the user picked off their disk.
//! * **Memory is bounded by `Limits`, not by luck.** The ceiling comes from the
//!   profile under test and the peak is measured, so a header that lies about
//!   its size and gets decoded anyway fails the iteration.
//! * **A decoded image always satisfies the post-decode check.** `decode_bounded`
//!   runs `Limits::check_decoded` after decoding; if that can fail on an image
//!   this function just returned, the last line of defence is not load-bearing.
//!
//! The memory ceiling is what turns "we think the parser is safe" into a build
//! signal: a decoder that ignores the limits stops being a slow test run and
//! becomes a crash with a reproducer attached.

use libfuzzer_sys::fuzz_target;
use pixelsmith_core::{Limits, decode_bounded, validate_bytes};
use px_fuzz::{Case, fuzz_limits, heap_budget, is_resource_error, measure_peak};

fuzz_target!(|data: &[u8]| {
    let Some((case, tail)) = Case::from_bytes(data) else {
        return;
    };
    let bytes = case.payload(case.format(), tail);
    let limits: Limits = fuzz_limits(case.profile);
    let budget = heap_budget(&limits, "decode_bounded");

    let (decoded, peak) = measure_peak(|| decode_bounded(&bytes, &limits));
    // Asserted before anything is done with the result, so a memory finding is
    // reported as a memory finding rather than as a later inconsistency.
    budget.assert_within(peak);

    let img = match decoded {
        Ok(img) => img,
        Err(e) => {
            // `decode_bounded` validates first, so a resource refusal means
            // validation already refused — which is the documented behaviour, not
            // a disagreement. Assert it anyway: if `validate_bytes` ever starts
            // accepting files `decode_bounded` rejects for size, this is where it
            // shows up.
            let validated = validate_bytes(&bytes, &limits);
            if let Ok(report) = validated {
                assert!(
                    !is_resource_error(&e),
                    "validation accepted {}x{} and decode then refused the file \
                     as too large: {e}",
                    report.width,
                    report.height
                );
            }
            return;
        }
    };

    // Every image this function hands back must satisfy the post-decode
    // assertion. If it does not, `decode_bounded` returned something it would
    // have rejected had the caller re-checked it.
    limits
        .check_decoded(&img)
        .unwrap_or_else(|e| panic!("decode_bounded returned an image failing its own check: {e}"));

    assert!(img.width() > 0 && img.height() > 0, "decoded a zero-sized image");
    assert!(
        u64::from(img.width()) * u64::from(img.height()) <= limits.max_pixels,
        "decoded {}x{} over a {} pixel budget",
        img.width(),
        img.height(),
        limits.max_pixels
    );

    // A decoded image is backed by a buffer. Its length has to be exactly the
    // dimensions times a real channel count, which is what catches a decoder
    // that handed back a view over a smaller allocation — the shape of bug that
    // becomes a heap overflow in whatever touches the pixels next.
    let pixels = img.width() as usize * img.height() as usize;
    let raw = img.as_bytes().len();
    let channels = raw / pixels;
    assert!(
        channels == 1 || channels == 3 || channels == 4,
        "{}x{} holds {raw} bytes, which is {channels} bytes per pixel — not a colour type",
        img.width(),
        img.height()
    );
    assert_eq!(
        channels * pixels,
        raw,
        "the sample buffer does not match the dimensions it was decoded with"
    );
});
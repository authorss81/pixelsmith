//! `TargetBytes::encode_with`: the byte-ceiling search, as encoders per fit.
//!
//! ```
//! cargo bench --all-features -- --profile ci
//! ```
//!
//! ## Why the unit is "encoders per successful fit", not seconds
//!
//! A criterion benchmark reports a duration, and for this function a duration is
//! nearly the wrong number. The search does a fixed small amount of *decision
//! work* — halve the interval, encode, compare — and the cost of each decision is
//! one whole encoder pass. So what determines whether a user notices is **how
//! many times the file is encoded**, not how fast any one pass is: seven encodes
//! of a 12 MP photo is seven times the phone's battery, the heat and the wait.
//!
//! So this file measures the whole convergence and asserts the count, and
//! `encode_with`'s own cost per encode is already covered by `benches/encode.rs`.
//! The count is asserted rather than merely reported because it is the one part
//! of this function that is a *design* claim — "converges in ~7 passes" is written
//! into `target::TargetBytes`'s doc comment — and a change that made it 20 would
//! be a 3× regression in a thing no timing assertion could catch.
//!
//! ## What this does NOT measure
//!
//! - **Whether the file actually fits.** `TargetBytes` reports `target_met`, and
//!   the search is only correct if it reports it honestly. That is
//!   `target::tests::real_jpeg_lands_under_the_target`, which asserts the byte
//!   count rather than the clock.
//! - **The impossible case.** A ceiling below what the floor quality produces
//!   skips the search entirely and returns one attempt. `target::tests` covers
//!   that shape; benchmarking it here would measure a different function.
//! - **A pathological size curve.** The search assumes encoded size grows
//!   monotonically with quality. It does for these encoders, and `target::tests`
//!   asserts the property with a synthetic encoder where it can be checked exactly
//!   rather than sampled.

use criterion::{Criterion, Throughput};
use pixelsmith_core::format::{EncodingOptions, OutputFormat};
use pixelsmith_core::target::{TargetBytes, default_encoder};

mod common;

/// Upper bound on encodes per fit, matching `target::tests::converges_in_few_passes`.
///
/// Log2 of the 30..=95 quality range is 7, so a search that never misjudges its
/// interval costs 7. Nine leaves room for the two extra passes the search can
/// make when the first attempt already fits (it then walks *up* to the highest
/// quality that still fits) or when nothing does.
const MAX_ENCODES: usize = 9;

/// Count the encoder passes without changing what they do.
///
/// A `Cell` rather than an `AtomicUsize`: the search is single-threaded, and the
/// counter is read only after `encode_with` returns.
fn counting_encoder(calls: &std::cell::Cell<usize>) -> impl Fn(
    &image::DynamicImage,
    OutputFormat,
    EncodingOptions,
) -> pixelsmith_core::Result<Vec<u8>> + '_ {
    move |img, format, options| {
        calls.set(calls.get() + 1);
        default_encoder(img, format, options)
    }
}

fn search(c: &mut Criterion) {
    let img = common::decoded(common::MP12);
    let target = TargetBytes::new(120_000);

    // Establish the pass count first, so `Throughput::Elements` can carry it and
    // the criterion report reads in encodes rather than in milliseconds.
    let probe = std::cell::Cell::new(0usize);
    let encoder = counting_encoder(&probe);
    let result = target
        .encode_with(img, OutputFormat::Jpeg, EncodingOptions::default(), &encoder)
        .expect("the engine's own decoder output must encode");
    let encodes = probe.get();
    assert!(
        (2..=MAX_ENCODES).contains(&encodes),
        "the search took {encodes} encoder passes for one fit, expected 2..={MAX_ENCODES}; \
         the convergence claim in target::TargetBytes's doc comment no longer holds"
    );

    let mut group = c.benchmark_group("target_bytes");
    // `Elements` is the number of *encoder passes*, which is the unit the user
    // pays for. `Bytes` would be the output size of the final encode alone and
    // would understate the work by the ratio of passes to 1.
    group.throughput(Throughput::Elements(encodes as u64));
    group.bench_function(format!("jpeg_search_{encodes}_encodes_per_fit"), |b| {
        // A fresh counter per benchmark, and a fresh encoder per iteration: the
        // second is what keeps criterion's repeated calls from accumulating into a
        // number nobody reads. Only the search is inside `iter` — the counter is
        // not, because timing a `Cell` write would be timing nothing.
        let calls = std::cell::Cell::new(0usize);
        b.iter(|| {
            let before = calls.get();
            let encoder = counting_encoder(&calls);
            let out = std::hint::black_box(target.encode_with(
                std::hint::black_box(img),
                OutputFormat::Jpeg,
                EncodingOptions::default(),
                &encoder,
            ))
            .expect("the same search already succeeded above");
            // One subtraction and one comparison against a measurement in the
            // hundreds of milliseconds. Deliberately *inside* the timed closure:
            // checked outside it, the assertion would only cover the first
            // iteration and would silently stop covering the rest, and a
            // benchmark that asserts nothing in the loop is the thing this phase
            // exists to avoid.
            assert_eq!(
                calls.get() - before,
                encodes,
                "the search took a different number of passes than the probe measured"
            );
            out
        });
    });
    // Report the outcome the user sees, not only the time: a search that got
    // slower by doing more passes is a different regression from one that got
    // slower per pass, and this line is what tells them apart.
    let (fitted, quality, met) = result;
    eprintln!(
        "target_bytes: {encodes} encoder passes -> {} bytes at q{quality}, target_met={met}",
        fitted.len()
    );
    group.finish();
}

fn main() {
    common::run(|c| {
        search(c);
    });
}

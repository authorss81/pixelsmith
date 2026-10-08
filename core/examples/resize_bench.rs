//! The two resize kernels, timed side by side. This is where the numbers in
//! `docs/BENCHMARKS.md` come from.
//!
//! ```
//! cargo run --release --features simd --example resize_bench
//! ```
//!
//! Release, and `--features simd`: a debug build's `image` kernel is roughly an
//! order of magnitude slower than the one that ships, which is the difference
//! between "the SIMD kernel is slower" and "the SIMD kernel is slower than a
//! debug build of the other one".
//!
//! It is an example rather than a `#[bench]` or a test because both of those
//! are the wrong shape here. `#[bench]` needs nightly. A test would run on every
//! `scripts/verify.sh` push, on whatever machine that happens to be, and a
//! timing assertion in the gate is a flake generator: the gate's job is to prove
//! the two kernels agree, which `resize::tests` does without a clock. Phase-10
//! owns the regression gate, and it will own its own threshold and its own
//! tolerance.
//!
//! The fixture is a *photograph stand-in*, and deliberately not the adversarial
//! one in `resize::tests`: a smooth wave for the low frequencies a real photo is
//! mostly made of, plus a hard 4-pixel comb and a few sharp edges so there is
//! something to ring. The cross-check fixture is small and full of checkerboard,
//! because it is there to make disagreement visible. Benchmarking the adversarial
//! fixture would flatter the reference kernel, whose `f32` accumulation is
//! well-conditioned on the whole plane, and say nothing about a real export.
//!
//! ## Why the gate on `simd` is in core/Cargo.toml and not here
//!
//! This used to open with `#![cfg(feature = "simd")]`, which is the second half
//! of audit finding 2 and fails for a reason specific to examples: an `#[cfg]`
//! that removes every item removes `fn main` as well, and an example is a binary
//! target, so rustc then reports `error[E0601]: main function not found in crate
//! resize_bench`. `cargo test` builds examples, so `cargo test` and
//! `cargo test --features streaming` both failed on a benchmark nobody had asked
//! for.
//!
//! The `[[example]]` entry in core/Cargo.toml carries `required-features =
//! ["simd"]` instead, so cargo declines to build the target at all and says why
//! — `target 'resize_bench' ... requires the features: 'simd'` — rather than
//! handing rustc a crate with no entry point. `required-features` is also the
//! only one of the two mechanisms that `cargo run --example` can explain.

use std::time::{Duration, Instant};

use image::DynamicImage;
use image::imageops::FilterType;
use pixelsmith_core::resize::{resample_reference, simd};

/// How many timed repetitions per case.
///
/// The kernel is deterministic, so this is not about variance in the output —
/// it is about the scheduler: a single 20 ms resize can be preempted, and a
/// median of five is far more honest than a first-iteration stopwatch.
const RUNS: usize = 5;

struct Case {
    label: &'static str,
    source: (u32, u32),
    target: (u32, u32),
    filter: FilterType,
}

/// The four cases the phase prompt names, plus a fifth that is the one a user is
/// most likely to hit and the one that is hardest to speed up: an upscale, where
/// the kernel's cost is dominated by generating destination samples rather than
/// by reading source ones.
const CASES: [Case; 5] = [
    Case {
        label: "24 MP -> 1920 wide",
        source: (6000, 4000),
        target: (1920, 1280),
        filter: FilterType::Lanczos3,
    },
    Case {
        label: "24 MP -> 400 wide",
        source: (6000, 4000),
        target: (400, 267),
        filter: FilterType::Lanczos3,
    },
    Case {
        label: "4000 px -> 400 px thumbnail",
        source: (4000, 3000),
        target: (400, 300),
        filter: FilterType::Triangle,
    },
    Case {
        label: "800 px -> 4000 px upscale",
        source: (800, 600),
        target: (4000, 3000),
        filter: FilterType::Lanczos3,
    },
    Case {
        label: "24 MP -> 1920 wide, Gaussian",
        source: (6000, 4000),
        target: (1920, 1280),
        filter: FilterType::Gaussian,
    },
];

/// A photograph stand-in: low-frequency wave, a hard comb, and edges.
fn photograph(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let fx = f64::from(x) / f64::from(w) * 7.0;
        let fy = f64::from(y) / f64::from(h) * 5.0;
        let wave = fx.sin() * 55.0 + fy.cos() * 40.0 + (fx * fy).sin() * 22.0;
        // Every 128th column is black: a hard vertical edge that every
        // convolution kernel rings against, which is the worst case for
        // agreement between two implementations and the best case for spotting a
        // kernel that is genuinely different rather than differently rounded.
        let edge = x % 128 < 2;
        let (r, g, b) = if edge {
            (12u8, 14, 18)
        } else {
            (
                (118.0 + wave).clamp(0.0, 255.0) as u8,
                (104.0 + wave * 0.7).clamp(0.0, 255.0) as u8,
                (86.0 + wave * 0.4).clamp(0.0, 255.0) as u8,
            )
        };
        image::Rgba([r, g, b, 255])
    })
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// Time one closure, discarding the result through `black_box` so the optimiser
/// cannot delete the work it is being asked to time.
fn time_ms<F: FnMut() -> DynamicImage>(mut run: F) -> f64 {
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let out = run();
        let elapsed = started.elapsed();
        std::hint::black_box(&out);
        samples.push(elapsed);
    }
    median(samples).as_secs_f64() * 1000.0
}

fn cpuinfo_field(field: &str) -> Option<String> {
    std::fs::read_to_string("/proc/cpuinfo")
        .unwrap_or_default()
        .lines()
        .find(|line| line.starts_with(field))
        .and_then(|line| line.split(':').nth(1))
        .map(|value| value.trim().to_string())
}

/// Describe the machine, and specifically which of `fast_image_resize`'s three
/// kernels ran — because that choice is most of why the numbers here do or do not
/// transfer to another box.
///
/// `fast_image_resize` dispatches on runtime CPU features (`cpu_extensions.rs`:
/// AVX2, else SSE4.1, else scalar), so the same binary on a phone's NEON and on
/// this runner's AVX2 take different paths. Reading `/proc/cpuinfo` and applying
/// the same order is a reproduction of that dispatch rather than a query against
/// it — the crate does not expose the choice — so this can be wrong where the
/// crate is right. It is here to be read alongside the numbers, not to be
/// trusted on its own.
fn machine() -> String {
    let model = cpuinfo_field("model name").unwrap_or_else(|| "unknown CPU".to_string());
    let flags = cpuinfo_field("flags").unwrap_or_default();
    let has = |flag: &str| flags.split_whitespace().any(|f| f == flag);
    let kernel = if has("avx2") {
        "AVX2"
    } else if has("sse4_1") {
        "SSE4.1"
    } else {
        "scalar (no AVX2, no SSE4.1)"
    };
    format!(
        "{model} | {} threads | {}-{} | {kernel} | {}",
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        std::env::consts::ARCH,
        std::env::consts::OS,
        if cfg!(debug_assertions) {
            "DEBUG BUILD — the numbers below mean nothing"
        } else {
            "release"
        },
    )
}

/// Worst and mean absolute per-channel difference between the two kernels.
fn difference(want: &image::RgbaImage, got: &image::RgbaImage) -> (u32, f64) {
    let mut max = 0u32;
    let mut total = 0.0f64;
    let mut count = 0.0f64;
    for (x, y, pa) in want.enumerate_pixels() {
        let pb = got.get_pixel(x, y);
        for (a, b) in pa.0.iter().zip(pb.0.iter()) {
            let d = u32::from(*a).abs_diff(u32::from(*b));
            max = max.max(d);
            total += f64::from(d);
            count += 1.0;
        }
    }
    (max, total / count)
}

fn main() {
    if cfg!(debug_assertions) {
        eprintln!(
            "This is a debug build. `image`'s kernel is an order of magnitude \
             slower in debug than the one that ships, so the speedups printed \
             below are not the speedups. Re-run with --release."
        );
    }
    println!("# resize kernels");
    println!();
    println!("machine: {}", machine());
    println!("feature: simd (reference kernel is `image`, SIMD kernel is `fast_image_resize`)");
    println!("runs:    {RUNS} timed, median reported");
    println!();
    println!(
        "{:<34} {:>9} {:>9} {:>9} {:>8} {:>7}",
        "case", "filter", "reference", "simd", "speedup", "maxdiff"
    );

    for case in &CASES {
        let src = DynamicImage::ImageRgba8(photograph(case.source.0, case.source.1));
        let (w, h) = case.target;

        let reference_ms = time_ms(|| resample_reference(&src, w, h, case.filter));
        let simd_ms = time_ms(|| {
            simd::try_resample(&src, w, h, case.filter)
                .expect("Lanczos3 and Gaussian reach the SIMD kernel")
        });

        let want = resample_reference(&src, w, h, case.filter).to_rgba8();
        let got = simd::try_resample(&src, w, h, case.filter)
            .expect("Lanczos3 and Gaussian reach the SIMD kernel")
            .to_rgba8();
        let (max, mean) = difference(&want, &got);

        println!(
            "{:<34} {:>9} {:>8.1}ms {:>8.1}ms {:>7.2}x {:>4} ({mean:.2} mean)",
            case.label,
            format!("{:?}", case.filter),
            reference_ms,
            simd_ms,
            reference_ms / simd_ms,
            max,
        );
    }
}

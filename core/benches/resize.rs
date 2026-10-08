//! Resize: the single resampling pass, at the sizes a user actually asks for.
//!
//! ```
//! cargo bench --all-features -- --profile ci
//! ```
//!
//! Every case calls `pipeline::resize_to`, which is what `Pipeline::apply` calls,
//! which in turn resolves to `resize::resample` — the *one* call site hard rule 5
//! permits. Benchmarking `image::imageops::resize` directly would time a function
//! the product does not reach, and if `simd` is enabled it would time the wrong
//! kernel entirely.
//!
//! ## What this does NOT measure
//!
//! - **Which kernel ran.** `resize::resample` picks `fast_image_resize` when the
//!   `simd` feature is on and falls back to the reference kernel when the SIMD one
//!   declines (and `Nearest` always declines — see `docs/ARCHITECTURE.md` gotcha
//!   21). Nothing here reports which, so **the same benchmark ID means different
//!   things in different feature sets.** `docs/BENCHMARKS.md` states the feature
//!   set every published number was taken under; the two-kernel comparison itself
//!   lives in `core/examples/resize_bench.rs`, which names its kernel per row.
//! - **Crop and orientation.** Those are memory moves (`crop_imm`) and buffer
//!   transposes, microseconds against hundreds of milliseconds here. Timing them
//!   would mean a benchmark whose number is mostly scheduler noise.
//! - **`Cover`.** It resamples then centre-crops, so its cost is the resample plus
//!   a slice. The resample is already covered; the slice is not worth a clock.
//! - **The output pixels.** Whether the answer looks right is `resize::tests`' job,
//!   and it asserts values rather than durations so a slow machine cannot fail it.

use criterion::{BenchmarkId, Criterion, Throughput};
use pixelsmith_core::ResampleFilter;
use pixelsmith_core::pipeline::{FitMode, ResizeSpec, resize_to};

mod common;

struct Case {
    label: &'static str,
    source: (u32, u32),
    spec: ResizeSpec,
}

/// The cases the phase prompt names, with the flags each one needs to actually
/// reach the resampler.
///
/// `no_upscale` matters on the last one: `ResizeSpec::default()` refuses to
/// enlarge, which is the right default for a photo, and a benchmark that wanted
/// an upscale and left it set would be measuring `resize_to`'s early return.
fn cases() -> Vec<Case> {
    let mut v = vec![
        Case {
            label: "24MP_to_1920",
            source: common::MP24,
            spec: ResizeSpec {
                width: Some(1920),
                height: None,
                fit: FitMode::Width,
                filter: ResampleFilter::Lanczos3,
                no_upscale: false,
            },
        },
        Case {
            label: "24MP_to_400",
            source: common::MP24,
            spec: ResizeSpec {
                width: Some(400),
                height: None,
                fit: FitMode::Width,
                filter: ResampleFilter::Lanczos3,
                no_upscale: false,
            },
        },
        Case {
            label: "24MP_to_64_extreme",
            source: common::MP24,
            spec: ResizeSpec {
                width: Some(64),
                height: None,
                fit: FitMode::Width,
                filter: ResampleFilter::Lanczos3,
                no_upscale: false,
            },
        },
    ];

    // 2x upscale of the 2 MP fixture. `FilterType` is Lanczos3 and it is NOT
    // demoted to Triangle: `resize_to`'s extreme-reduction branch only fires when
    // the *target* is under a third of the source's long side, and here the target
    // is larger, so the user's requested filter is what runs.
    v.push(Case {
        label: "2MP_upscale_2x",
        source: common::MP2,
        spec: ResizeSpec {
            width: Some(common::MP2.0 * 2),
            height: None,
            fit: FitMode::Width,
            filter: ResampleFilter::Lanczos3,
            no_upscale: false,
        },
    });
    v
}

fn resize(c: &mut Criterion) {
    let mut group = c.benchmark_group("resize");
    for case in cases() {
        let src = common::decoded(case.source);
        // Throughput in destination pixels, because that is the work the kernel
        // does per output sample. Source pixels are reported by the label.
        let (w, h) = case
            .spec
            .resolve(src.width(), src.height())
            .expect("static case");
        group.throughput(Throughput::Elements(u64::from(w) * u64::from(h)));
        group.bench_function(BenchmarkId::from_parameter(case.label), |b| {
            b.iter(|| {
                std::hint::black_box(resize_to(std::hint::black_box(src), case.spec))
                    .expect("a static resize spec over a decoded fixture cannot fail")
            })
        });
    }
    group.finish();
}

fn main() {
    common::run(|c| {
        resize(c);
    });
}

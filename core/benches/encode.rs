//! Encode: what the user waits for after they press Export.
//!
//! ```
//! cargo bench --all-features -- --profile ci
//! ```
//!
//! Every case calls `lib::encode_fixed`, which is `format::encode` with no
//! search — the path a fixed-quality export takes. `worker::process_one` calls the
//! same function, so a number here is a number the UI can show.
//!
//! JPEG is benchmarked at q85 and q95 because those are the two ends of what the
//! engine offers by default (`EncodingOptions::default()` is 85; `TargetBytes`
//! searches 30..=95). The gap between them is the interesting one: it is where
//! q90 sits, and `image`'s old JPEG encoder switched chroma sampling there, which
//! is half of why `jpeg-encoder` replaced it (`docs/JPEG.md`).
//!
//! WebP is at q80, and only `#[cfg]`d on `webp-lossy`: in a build without that
//! feature the only WebP encoder compiled in is lossless and ignores quality, so a
//! row labelled "lossy WebP at q80" would be a number produced by a different
//! encoder than the label claims.
//!
//! ## What this does NOT measure
//!
//! - **AVIF.** It is in the default build and it is by far the slowest encoder
//!   here — measured at ~3.2 s for a 1600×1200 photo against 20 ms for a q85 JPEG
//!   (`docs/ARCHITECTURE.md`). At 12 MP it would dominate the whole suite's wall
//!   clock, and the `--profile ci` sample count is what keeps it from. It is left
//!   out on purpose and named here rather than silently dropped; the number
//!   already recorded stands in for it.
//! - **The resize that normally precedes it.** A real export is decode, resize,
//!   strip, encode. These are the encode alone, from an already-resampled image.
//! - **The batch path's parallelism.** `worker::process_batch` fans out across
//!   files with rayon. Every number here is single-threaded, which is the
//!   pessimistic half and the one that matters on a phone.
//! - **PNG at a size anyone would export.** PNG is `deflate` at `CompressionType::Best`
//!   on RGBA, so it is slow in a way that has nothing to do with the transform.
//!   It is measured at 2 MP rather than 12 MP for exactly that reason, and
//!   `docs/BENCHMARKS.md` says so next to the number.

use criterion::{BenchmarkId, Criterion, Throughput};
use pixelsmith_core::format::{EncodingOptions, OutputFormat};

mod common;

fn jpeg(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode/jpeg");
    // 12 MP: the ordinary phone photo, and the same source the resize group uses.
    let src = common::decoded(common::MP12);
    group.throughput(Throughput::Elements(
        u64::from(src.width()) * u64::from(src.height()),
    ));
    for quality in [85u8, 95u8] {
        group.bench_function(BenchmarkId::from_parameter(format!("q{quality}")), |b| {
            let options = EncodingOptions::default().with_quality(quality);
            b.iter(|| {
                std::hint::black_box(pixelsmith_core::encode_fixed(
                    std::hint::black_box(src),
                    OutputFormat::Jpeg,
                    options,
                ))
                .expect("the engine's own decoder output must encode")
            })
        });
    }
    group.finish();
}

#[cfg(feature = "webp-lossy")]
fn webp(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode/webp_lossy");
    let src = common::decoded(common::MP12);
    group.throughput(Throughput::Elements(
        u64::from(src.width()) * u64::from(src.height()),
    ));
    group.bench_function("q80", |b| {
        let options = EncodingOptions::default().with_quality(80);
        b.iter(|| {
            std::hint::black_box(pixelsmith_core::encode_fixed(
                std::hint::black_box(src),
                OutputFormat::WebP,
                options,
            ))
            .expect("the engine's own decoder output must encode")
        })
    });
    group.finish();
}

/// Lossless WebP, for the build without `webp-lossy`.
///
/// Not a substitute for the row above: it is a different encoder with a different
/// cost, and `OutputFormat::is_lossless(WebP)` is true here. It exists so the
/// benchmark file still measures *something* about WebP in that configuration
/// rather than failing to compile.
#[cfg(not(feature = "webp-lossy"))]
fn webp(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode/webp_lossless");
    let src = common::decoded(common::MP2);
    group.throughput(Throughput::Elements(
        u64::from(src.width()) * u64::from(src.height()),
    ));
    group.bench_function("q80_ignored", |b| {
        let options = EncodingOptions::default().with_quality(80);
        b.iter(|| {
            std::hint::black_box(pixelsmith_core::encode_fixed(
                std::hint::black_box(src),
                OutputFormat::WebP,
                options,
            ))
            .expect("the engine's own decoder output must encode")
        })
    });
    group.finish();
}

fn png(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode/png");
    // 2 MP, not 12 MP, and the reason is above: PNG here is deflate at Best on
    // four channels, so it costs roughly linearly in pixels and is the slowest
    // thing in the suite. Shrinking the *input* would be cheating, so the input
    // is stated in the benchmark's own name and in docs/BENCHMARKS.md rather than
    // left to look like a 12 MP number.
    let src = common::decoded(common::MP2);
    group.throughput(Throughput::Elements(
        u64::from(src.width()) * u64::from(src.height()),
    ));
    group.bench_function("2MP_lossless_best", |b| {
        b.iter(|| {
            std::hint::black_box(pixelsmith_core::encode_fixed(
                std::hint::black_box(src),
                OutputFormat::Png,
                EncodingOptions::default(),
            ))
            .expect("the engine's own decoder output must encode")
        })
    });
    group.finish();
}

fn main() {
    common::run(|c| {
        jpeg(c);
        webp(c);
        png(c);
    });
}

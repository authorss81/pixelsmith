//! Peak-allocation measurement for the streaming decode path.
//!
//! # Why this is its own test binary
//!
//! The measurement is a counting [`GlobalAlloc`], and a counter that other threads
//! allocate through is not a measurement: `cargo test` runs a binary's tests in
//! parallel, so a sibling test's fixture would land in this one. This file
//! therefore contains exactly one `#[test]`, which is the only way the number can
//! mean what the docs claim.
//!
//! # What it asserts and what it prints
//!
//! The assertion is the phase's acceptance criterion: a 120-megapixel source
//! resized to 1000 px wide must stay under a stated ceiling while the same job
//! through the in-memory path needs four figures more. The printed table is what
//! docs/BENCHMARKS.md quotes for 24, 60 and 120 MP; run it with `--nocapture` to
//! re-measure on a different machine.
//!
//! Run time is dominated by generating the fixtures, which is why they are
//! generated rather than committed: a 120 MP PNG of compressible content is a few
//! megabytes on disk, and a 120 MP fixture in the repository would be a licence
//! question nobody needs.

// The whole file sits behind the same gate as the module it measures
// (`#[cfg(feature = "streaming")] pub mod stream`, lib.rs:24). An integration
// test can only reach the crate's *public* API, and `stream` is not part of that
// API without the feature, so an ungated file here is
// `error[E0433]: cannot find 'stream' in pixelsmith_core` in every configuration
// that does not enable `streaming` — including plain `cargo test`, which is the
// command README.md's build section tells a contributor to run. That was the
// first half of audit finding 2, and it was invisible for as long as the gate ran
// `--all-features` and nothing else.
//
// `#![cfg]` and `required-features` are both available here and the difference
// matters. `required-features` in core/Cargo.toml tells cargo not to build this
// target at all, so the binary simply does not appear in `cargo test --no-run`'s
// output; `#![cfg]` builds it and leaves every item removed, which for a target
// compiled with `--test` means libtest's injected `main` is still there and the
// result is a harness that reports `running 0 tests`. The second is the honest
// form for this file: a test that compiles to nothing says so, rather than being
// absent from a build and looking like a target nobody has written yet.
//
// This is the whole of the fix — nothing above is gated and nothing below is
// weakened. The one test still asserts what phase-11 made it assert.
#![cfg(feature = "streaming")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use pixelsmith_core::{FitMode, Limits, Pipeline, ResizeSpec};

/// Live and peak bytes, in the order the allocator sees them.
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: forwarding the layout unchanged to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: the pointer and layout came from `System.alloc` above.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        record(new_size);
        // SAFETY: the pointer and layout came from `System.alloc` above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

fn record(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

/// Peak live bytes since the last [`reset`].
fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// Live bytes right now, which is what a fresh [`reset`] starts from.
fn reset() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The ceiling this test asserts, in bytes, for the 120 MP case.
///
/// 24 MB is roughly `working_set_bytes`'s 12 MB plus slack for the destination
/// image's own bookkeeping and the fixture bytes, and it is a quarter of what a
/// phone with `Limits::mobile()` can promise: a mid-range Android device gives an
/// app process a few hundred MB before the OS kills it, and the whole point of
/// this phase is that a 120 MP panorama no longer spends what it used to.
const CEILING_BYTES: usize = 24 * 1024 * 1024;

/// Compressible, photograph-ish content: a multi-octave wave, so a PNG of it is a
/// few megabytes rather than 480, and the fixture does not dominate the disk.
fn fixture(w: u32, h: u32) -> Vec<u8> {
    let raw: Vec<u8> = (0..(w as usize * h as usize * 4))
        .map(|i| {
            let p = i / 4;
            let (x, y, c) = (p % w as usize, p / w as usize, i % 4);
            if c == 3 {
                return 255;
            }
            let fx = x as f64 / w as f64 * 6.0;
            let fy = y as f64 / h as f64 * 4.0;
            let wave = fx.sin() * 80.0 + fy.cos() * 55.0 + (fx * 7.3).sin() * 9.0;
            let v = [128.0 + wave, 100.0 + wave * 0.6, 150.0 - wave * 0.5][c];
            v.clamp(0.0, 255.0) as u8
        })
        .collect();
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&raw).expect("png data");
    }
    out
}

fn megabytes(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[test]
fn a_120_megapixel_source_resizes_to_1000_wide_inside_a_stated_ceiling() {
    let pipeline = Pipeline::new().with_resize(ResizeSpec {
        width: Some(1000),
        height: None,
        fit: FitMode::Width,
        ..Default::default()
    });
    let mobile = Limits::mobile();

    // The in-memory column, and the two smaller sources, cost about three minutes
    // of the suite's wall clock in a debug build (a 120 MP Lanczos3 resize at
    // `opt-level = 0`), so they are opt-in. The numbers docs/BENCHMARKS.md quotes
    // were taken with it on:
    //
    //     PX_MEASURE_BEFORE=1 cargo test --features streaming \
    //         --test streaming_peak -- --nocapture
    //
    // The ceiling below is asserted either way, because it is about the streaming
    // path and costs nothing to check.
    let measure_before = std::env::var_os("PX_MEASURE_BEFORE").is_some();
    // Only the 120 MP row runs by default, because it is the one the acceptance
    // criterion is about and it is the slowest; the 24 and 60 MP rows come with
    // the in-memory column so the whole table in docs/BENCHMARKS.md comes from one
    // command.
    let sizes: &[(u32, u32)] = if measure_before {
        &[(6000, 4000), (12000, 5000), (12000, 10_000)]
    } else {
        &[(12000, 10_000)]
    };
    println!(
        "peak MB | source MP | in-memory | streaming | streaming working set\n\
         --------|----------|-----------|-----------|-----------------------"
    );

    for (w, h) in sizes.iter().copied() {
        let megapixels = (u64::from(w) * u64::from(h)) / 1_000_000;
        // The engine's own size resolution rather than a formula here, so the
        // test cannot disagree with the pipeline about what 1000 px wide means.
        let (dst_w, dst_h) = pipeline.output_dimensions(w, h).expect("a size");
        let filter = pixelsmith_core::pipeline::filter_for(
            pipeline.resize.as_ref().expect("a resize"),
            w,
            h,
        );
        let bytes = fixture(w, h);

        // --- the in-memory path: decode, then transform. ---
        let mut in_memory = 0usize;
        if measure_before {
            reset();
            let mark = peak();
            let decoded = pixelsmith_core::decode_bounded(&bytes, &Limits::default())
                .expect("in-memory decode");
            let resized = pipeline.apply(&decoded).expect("in-memory resize");
            in_memory = peak().saturating_sub(mark);
            assert_eq!(
                (resized.width(), resized.height()),
                (dst_w, dst_h),
                "the pipeline produced a different size"
            );
            drop(decoded);
            drop(resized);
        }

        // --- the streaming path. The fixture bytes stay alive in both cases, so
        // they cancel out of the delta; `working_set_bytes` is what the engine
        // itself accounts for.
        reset();
        let before = peak();
        let streamed =
            pixelsmith_core::stream::decode_resized(&bytes, &mobile, (dst_w, dst_h), filter)
                .expect("streamed decode and resize");
        let streaming = peak().saturating_sub(before);
        let accounted = pixelsmith_core::stream::working_set_bytes(w, h, dst_w, dst_h, filter);
        assert_eq!(
            (streamed.width(), streamed.height()),
            (dst_w, dst_h),
            "the streaming path produced a different size from the in-memory one"
        );

        println!(
            "{:>9.1} | {:>8} | {:>9.1} | {:>9.1} | {:>9.1} MB",
            streaming >> 20,
            megapixels,
            in_memory >> 20,
            streaming >> 20,
            megabytes(accounted as usize)
        );

        // Only the flagship case carries a ceiling, and only the streaming path is
        // allowed near it.
        assert!(
            streaming < CEILING_BYTES,
            "a {megapixels} MP source resized to {dst_w} wide peaked at \
             {:.1} MB, over the {:.1} MB ceiling this test states",
            megabytes(streaming),
            megabytes(CEILING_BYTES)
        );
        if measure_before && megapixels >= 120 {
            assert!(
                streaming * 8 < in_memory,
                "streaming peaked at {:.1} MB against {:.1} MB in memory, so the \
                 saving is not worth the second code path",
                megabytes(streaming),
                megabytes(in_memory)
            );
        }
    }
}

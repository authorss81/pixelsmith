//! EXIF: reading it, and stripping it.
//!
//! ```
//! cargo bench --all-features -- --profile ci
//! ```
//!
//! Two functions, and they are on opposite sides of a decision. `exif::read` is
//! what the UI calls to decide whether to warn a user that a photo carries GPS;
//! `exif::strip` is what runs on **every** export that does not explicitly keep
//! metadata, which is every export the product performs.
//!
//! The fixture is `common::jpeg_with_exif`, which carries a full phone-sized tag
//! set — orientation, make, model, timestamps, lens, and every sensitive class
//! `exif::write_back` filters. That is deliberate: both functions' cost is
//! proportional to the number of tags, so a three-field fixture would have made
//! the numbers a quarter of what they are, and it would have left a regression in
//! the filter list invisible.
//!
//! ## What this does NOT measure
//!
//! - **`exif::write_back`.** It is the only path that puts metadata back, it runs
//!   only on explicit opt-in, and the UI does not offer the flag — so it is not a
//!   path a user waits for. It is covered by tests.
//! - **A HEIF's metadata.** It lives in a separate container item referenced by a
//!   `cdsc` relation, not in a JPEG APP1 segment, so `exif::read` cannot see one
//!   and this benchmark does not represent it. `docs/ARCHITECTURE.md`'s metadata
//!   section says what that means for an iPhone photo.
//! - **Stripping as a *file* operation.** `exif::strip` takes and returns decoded
//!   pixels: it rebuilds an RGBA buffer from raw samples, which is how a maker note
//!   or an embedded thumbnail cannot survive. The bytes come off because the
//!   encoder never sees the original container. So this is a memory copy, and the
//!   cost of not carrying metadata is one linear pass over the pixel buffer.
//! - **Parsing a malformed block.** The hostile half of EXIF handling is
//!   `core/tests/hostile.rs`, and it is not something a clock should be asked about.

use criterion::{Criterion, Throughput};
use pixelsmith_core::exif;

mod common;

fn read(c: &mut Criterion) {
    // 12 MP: the ordinary phone photo. EXIF cost does not scale with pixels, so
    // the size here is about the cost of *reaching* the APP1 segment, which is a
    // scan of the bytes before it.
    let bytes = common::jpeg_with_exif(common::MP12);
    let mut group = c.benchmark_group("exif/read");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.bench_function("full_tag_set", |b| {
        b.iter(|| {
            std::hint::black_box(exif::read(std::hint::black_box(bytes)))
                .expect("reading EXIF out of a fixture this engine wrote cannot fail")
        })
    });
    group.finish();
}

fn strip(c: &mut Criterion) {
    let img = common::decoded(common::MP12);
    let mut group = c.benchmark_group("exif/strip");
    group.throughput(Throughput::Elements(
        u64::from(img.width()) * u64::from(img.height()),
    ));
    group.bench_function("rebuild_rgba_buffer_12MP", |b| {
        b.iter(|| {
            std::hint::black_box(exif::strip(std::hint::black_box(img)))
                .expect("rebuilding an RGBA buffer from an RGBA buffer cannot fail")
        })
    });
    group.finish();
}

fn main() {
    common::run(|c| {
        read(c);
        strip(c);
    });
}

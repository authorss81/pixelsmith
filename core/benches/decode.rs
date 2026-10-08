//! Decode: how long it takes to get pixels out of a file.
//!
//! ```
//! cargo bench --all-features -- --profile ci
//! ```
//!
//! Every case goes through `lib::decode_bounded`, which is the path
//! `worker::process_one` uses for an ordinary file. That matters more than it
//! sounds: the alternative is `image::load_from_memory`, which skips
//! `validate::validate_bytes` and `Limits::apply_to_decoder`, and a benchmark of
//! the skipped path is a benchmark of code the product does not run.
//!
//! ## What this does NOT measure
//!
//! - **Filesystem and I/O.** The bytes are already in memory. A 24 MP JPEG off an
//!   SD card is a different question, and this suite cannot answer it.
//! - **The real entropy-decoding load.** The fixture is compressible (see
//!   `benches/common/mod.rs`), so the Huffman stage does less work than it would
//!   on a camera original. These numbers are therefore a **floor**, and a JPEG
//!   decode on a real photo is meaningfully slower.
//! - **The sandbox.** `sandbox::decode_sandboxed` costs a process spawn, which is
//!   the whole point of it and is not a decode measurement.
//! - **A hostile file.** Nothing here is over `Limits`; refusing a 60000×60000
//!   header takes microseconds and is a correctness property, not a latency one.
//!
//! The fixture is JPEG-decoded for [`common::decoded`], so the encode benchmarks
//! all start from the same pixels regardless of which format the decode group is
//! currently timing.

use criterion::{BenchmarkId, Criterion, Throughput};

mod common;

/// Formats whose decode this group covers. WebP needs `webp-lossy` for nothing
/// here — decoding is `image`'s job in every build — so it is unconditional.
/// A fixture generator: size in, encoded bytes out.
type Fixture = fn((u32, u32)) -> &'static [u8];

fn cases() -> Vec<(&'static str, Fixture)> {
    vec![
        ("jpeg", common::jpeg_size_probe),
        ("png", common::png),
        ("webp", common::webp_decode_fixture),
    ]
}

fn decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode");
    for (name, fixture) in cases() {
        for (size_name, size) in common::SIZES {
            let bytes = fixture(size);
            group.throughput(Throughput::Bytes(bytes.len() as u64));
            group.bench_with_input(
                BenchmarkId::from_parameter(format!("{name}/{size_name}")),
                &bytes,
                |b, bytes| {
                    // `Limits::default()` is the profile a desktop build uses.
                    // The mobile profile is the same code with smaller numbers,
                    // so the work is identical and timing it twice would be noise.
                    let limits = pixelsmith_core::Limits::default();
                    b.iter(|| {
                        std::hint::black_box(
                            pixelsmith_core::decode_bounded(std::hint::black_box(bytes), &limits)
                                .expect("a fixture this engine encoded must decode"),
                        )
                    });
                },
            );
        }
    }
    group.finish();
}

fn main() {
    common::run(|c| {
        decode(c);
    });
}

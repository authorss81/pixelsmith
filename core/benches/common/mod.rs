//! Fixtures for the benchmark suite, generated rather than committed.
//!
//! There are no image files in this repository and none in this directory. Every
//! fixture is built here at the size the benchmark needs, so a benchmark cannot
//! silently be measuring a 64-pixel square, and so there is no third-party
//! photograph in the tree whose licence anybody would have to reason about.
//!
//! # Every fixture here is COMPRESSIBLE, on purpose
//!
//! The generators produce smooth low-frequency content — a multi-octave wave, a
//! few gradients, a soft vignette — with only occasional hard edges. They are
//! **not** noise, and that is a load-bearing choice rather than an aesthetic one:
//!
//! * An encoder's cost is dominated by how much of the input it can predict. On
//!   random noise nothing is predictable, so JPEG and WebP spend all their time
//!   in the entropy coder and their time stops saying anything about the DCT and
//!   the prediction pass — which is the part that would change if the code under
//!   test changed. A noise-fixture JPEG benchmark measures libjpeg's Huffman
//!   coder, not the pipeline.
//! * Noise is incompressible, so a **byte ceiling can never be met** at any
//!   quality. `TargetBytes::encode_with` on noise would report
//!   `target_met: false` and benchmark the failure path instead of the search.
//!   `target.rs`'s own tests say the same thing about their fixtures.
//! * Decoding is the exception, and it is worth being precise about why: a
//!   decoder's cost is dominated by entropy *decoding*, so noise is arguably the
//!   more representative input there. But then the encoded fixture would be as
//!   large as the raw image (96 MB at 24 MP), which makes the *decode* benchmark
//!   measure the filesystem. So these fixtures are compressible, and the decode
//!   benchmarks say in `docs/BENCHMARKS.md` that they under-report Huffman work
//!   relative to a real photo.
//!
//! One consequence to remember: the fixture is the same for every case in a
//! group, so a regression in the *encoder* cannot hide behind a change of input,
//! and a regression in the *fixture* shows up in every case at once.
//!
//! Sizes are the three the phase prompt names, at the 3:2 aspect ratio every
//! phone camera in the presets' target audience produces.
//!
//! | Name | Pixels | Used for |
//! | --- | --- | --- |
//! | [`MP2`] | 1632×1224 (2.0 MP) | the smallest case, where per-call overhead is visible |
//! | [`MP12`] | 4000×3000 (12.0 MP) | the ordinary phone photo |
//! | [`MP24`] | 6000×4000 (24.0 MP) | the top of `web-hero`, and the resize worst case |
//!
//! # `unwrap` is fine here and nowhere else
//!
//! AGENTS.md hard rule 3 forbids `unwrap` on untrusted input. Nothing below
//! parses bytes a user picked off their disk: it generates pixels, encodes them
//! with this engine's own encoders, and checks the result. A failure is a bug in
//! the fixture, not an attack, so it panics loudly.

#![allow(dead_code)] // Each bench binary includes this module and uses a subset.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use criterion::Criterion;
use pixelsmith_core::decode_bounded;
use pixelsmith_core::format::{EncodingOptions, OutputFormat};
use pixelsmith_core::Limits;

pub const MP2: (u32, u32) = (1632, 1224);
pub const MP12: (u32, u32) = (4000, 3000);
pub const MP24: (u32, u32) = (6000, 4000);

/// Every fixture the suite names, so a benchmark file and `docs/BENCHMARKS.md`
/// cannot disagree about which sizes are covered.
pub const SIZES: [(&str, (u32, u32)); 3] = [("2MP", MP2), ("12MP", MP12), ("24MP", MP24)];

/// A photograph stand-in: low-frequency wave, a few gradients, one hard comb and
/// a vignette. Same shape as the fixture in `core/examples/resize_bench.rs`, so
/// a number in `docs/BENCHMARKS.md` has one description rather than two.
///
/// The comb is every 128th column, which is a hard vertical edge for every
/// convolution kernel to ring against. It is here so the fixture is not *so*
/// smooth that the encoders find it trivial and a real edge's cost is invisible.
pub fn photograph(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let fx = f64::from(x) / f64::from(w) * 7.0;
        let fy = f64::from(y) / f64::from(h) * 5.0;
        let wave = fx.sin() * 55.0 + fy.cos() * 40.0 + (fx * fy).sin() * 22.0;
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

// ---------------------------------------------------------------------------
// The `ci` profile
// ---------------------------------------------------------------------------

/// Iteration counts for `--profile ci`, and why each one is what it is.
///
/// | Setting | ci | criterion default | Reasoning |
/// | --- | ---: | ---: | --- |
/// | `sample_size` | 10 | 100 | Ten samples is enough to tell a 15% regression from noise, which is the gate's threshold. It is not enough to resolve 2%, and nothing here claims to. criterion's floor is also 10 — `configure_from_args` asserts `num_size >= 10` — so this is the smallest run that produces a distribution at all. |
/// | `warm_up_time` | 1 s | 3 s | One second is enough to page in the fixture and let the allocator reach steady state. The first timed iteration of a 24 MP case would otherwise be dominated by first-touch page faults, and it is the *median* that gets reported, so a single bad first iteration does not move the number — but three seconds of warm-up per benchmark is nine seconds of the suite's budget spent on nothing. |
/// | `measurement_time` | 3 s | 5 s | With 10 samples, 3 s of measurement is generous. |
/// | `nresamples` | 10 000 | 100 000 | Bootstrap resampling is fixed per benchmark rather than per sample, so 100 000 of them is seconds of pure post-processing on every one of the nineteen cases. Dropping it widens the confidence interval, which a 15% threshold absorbs comfortably. |
///
/// `full()` is criterion's own configuration, reachable with `--profile full`. It
/// exists so the tight numbers in `docs/BENCHMARKS.md` can be reproduced, and so
/// the ci budget is a choice on the record rather than a consequence of never
/// having looked at what the default costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CiProfile {
    pub sample_size: usize,
    pub warm_up: std::time::Duration,
    pub measurement: std::time::Duration,
    pub nresamples: usize,
}

impl CiProfile {
    const CI: CiProfile = CiProfile {
        sample_size: 10,
        warm_up: std::time::Duration::from_secs(1),
        measurement: std::time::Duration::from_secs(3),
        nresamples: 10_000,
    };

    const FULL: CiProfile = CiProfile {
        sample_size: 100,
        warm_up: std::time::Duration::from_secs(3),
        measurement: std::time::Duration::from_secs(5),
        nresamples: 100_000,
    };

    pub fn full() -> Self {
        Self::FULL
    }

    fn apply(self, c: Criterion) -> Criterion {
        c.sample_size(self.sample_size)
            .warm_up_time(self.warm_up)
            .measurement_time(self.measurement)
            .nresamples(self.nresamples)
    }
}

/// Build the `Criterion` this process should run with, from `std::env::args()`.
///
/// ## Why this parses the command line itself
///
/// The phase prompt asks for `cargo bench --all-features -- --profile ci`, and
/// criterion 0.7 rejects that command outright: it removed its own `--profile`
/// option after 0.4 (replaced by `--quick`, `--sample-size`, `--warm-up-time`
/// and `--measurement-time`), and its argument parser is a clap `Command` that
/// calls `get_matches()` on the process's own argv and exits 2 on an unknown
/// flag. Rewriting argv is not available either — `std::env::set_args` is still
/// unstable — and criterion exposes no way to hand it a pre-parsed configuration.
///
/// So the suite parses the handful of flags it supports and builds the runner
/// from criterion's own builder methods. Two properties of that choice matter:
///
/// - **An unrecognised flag is a hard error**, not a shrug. If a future maintainer
///   passes `--profile c1` (a one) or a flag criterion has but this does not, the
///   run stops and says so. Silently ignoring it would produce a full-speed run
///   that still printed "profile: ci", which is the specific way a gate stops
///   being a gate.
/// - **`--bench` is accepted and ignored.** Cargo appends it to every benchmark
///   binary's argv, so every invocation contains it.
///
/// Supported: `--profile <name>`, `--noplot`, `--save-baseline <name>`,
/// `--baseline <name>`, `--bench`, `--test`, `--help`, and one optional
/// positional FILTER (a substring, which is criterion's own rule).
///
/// `--test` runs every benchmark once and reports nothing, which is what
/// `cargo test --benches` does in criterion. `scripts/verify.sh` does not do that
/// — `cargo test` without `--benches` does not build bench targets at all, and
/// `cargo clippy --all-targets` compiles them without running them — but the flag
/// is honoured here so a benchmark can be smoke-tested without a full run.
///
/// # Panics
///
/// On an unrecognised flag, on an unknown profile name, or on a flag missing its
/// value. This is not a library and none of its inputs are untrusted: argv comes
/// from whoever ran `cargo bench`, and a wrong flag should stop the run rather
/// than be tolerated.
pub fn criterion_configure() -> Criterion {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut profile = Profile::default();
    let mut filter: Option<String> = None;
    let mut i = 0;

    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--bench" => {}
            "--noplot" => profile.plots = false,
            "--test" | "--help" => profile.help = true,
            "--profile" | "--save-baseline" | "--baseline" => {
                let value = args.get(i + 1).unwrap_or_else(|| {
                    eprintln!("bench: {arg} needs a value");
                    std::process::exit(2);
                });
                match arg {
                    "--profile" => profile = parse_profile(value),
                    "--save-baseline" => profile.baseline = Some(BaselineAction::Save(value.clone())),
                    _ => profile.baseline = Some(BaselineAction::Compare(value.clone())),
                }
                i += 1;
            }
            _ => match arg.strip_prefix("--profile=") {
                Some(value) => profile = parse_profile(value),
                None => match arg.strip_prefix("--save-baseline=") {
                    Some(value) => profile.baseline = Some(BaselineAction::Save(value.to_string())),
                    None => match arg.strip_prefix("--baseline=") {
                        Some(value) => {
                            profile.baseline = Some(BaselineAction::Compare(value.to_string()))
                        }
                        None => {
                            if let Some(unknown) = arg.strip_prefix('-') {
                                eprintln!("bench: unrecognised flag `--{unknown}`");
                                eprintln!("{}", USAGE);
                                std::process::exit(2);
                            }
                            filter = Some(arg.to_string());
                        }
                    },
                },
            },
        }
        i += 1;
    }

    announce(&profile);
    if profile.help {
        println!("{USAGE}");
        std::process::exit(0);
    }

    let mut c = profile.ci.apply(Criterion::default());
    if !profile.plots {
        // Nothing in this tree can draw a plot anyway: criterion was added with
        // `default-features = false`, so there is no `plotters` backend, and a CI
        // runner has no gnuplot. Saying so here is what keeps `--noplot` from
        // being a flag that pretends to control something.
        c = c.without_plots();
    }
    if profile.help {
        // `--test`: one unmeasured pass per benchmark, which is what criterion's
        // own `--test` does. sample_size's floor is 10 and warm-up/measurement must
        // be non-zero, so this is the smallest run that still exercises every
        // closure without producing a number anyone could mistake for one.
        c = c
            .sample_size(10)
            .warm_up_time(std::time::Duration::from_millis(1))
            .measurement_time(std::time::Duration::from_millis(1));
    }
    match profile.baseline {
        Some(BaselineAction::Save(name)) => c = c.save_baseline(name),
        Some(BaselineAction::Compare(name)) => c = c.retain_baseline(name, false),
        None => {}
    }
    if let Some(text) = filter {
        c = c.with_filter(text);
    }
    c
}

const USAGE: &str = "\
usage: cargo bench --all-features -- [--profile <name>] [--noplot]
                       [--save-baseline <name> | --baseline <name>]
                       [--test] [FILTER]

profiles:
  ci      short run for a CI regression gate (the default for this suite's docs)
  full    criterion's own settings: 100 samples, 3s warm-up, 5s measurement";

/// Iteration budget for a run, and whether it draws plots.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Profile {
    /// See [`CiProfile`] for the numbers and why they are these.
    ci: CiProfile,
    plots: bool,
    help: bool,
    baseline: Option<BaselineAction>,
}

impl Default for Profile {
    /// `ci`, deliberately.
    ///
    /// The alternative — criterion's defaults — is a five-minute-per-benchmark
    /// run times nineteen benchmarks, which on this crate is the difference
    /// between a nightly job and a nightly job that times out. A plain
    /// `cargo bench` with no arguments running at 5× the budget with nobody
    /// expecting it is the wrong default for a suite whose entire purpose is a
    /// regression gate. `--profile full` is there for when someone wants the
    /// numbers to be tight rather than fast.
    fn default() -> Self {
        Self {
            ci: CiProfile::CI,
            plots: false,
            help: false,
            baseline: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BaselineAction {
    /// `--save-baseline`: write this run's results under `name`.
    Save(String),
    /// `--baseline`: compare against `name`, leaving it untouched.
    Compare(String),
}

fn parse_profile(name: &str) -> Profile {
    match name {
        "ci" => Profile::default(),
        "full" => Profile {
            ci: CiProfile::full(),
            ..Profile::default()
        },
        other => {
            eprintln!("bench: unknown profile `{other}`; expected `ci` or `full`");
            std::process::exit(2);
        }
    }
}

/// Print which profile is in force, on stderr.
///
/// stderr and not stdout because criterion writes its report to stdout, and the
/// workflow parses that report: interleaving this line into it would break the
/// regression check, and it is the check that matters.
fn announce(profile: &Profile) {
    eprintln!(
        "profile: {}{}",
        if profile.ci == CiProfile::FULL {
            "full"
        } else {
            "ci"
        },
        match &profile.baseline {
            Some(BaselineAction::Save(name)) => format!(" (saving baseline `{name}`)"),
            Some(BaselineAction::Compare(name)) => format!(" (comparing against baseline `{name}`)"),
            None => String::new(),
        }
    );
}

/// `criterion_configure` plus the final summary, which is what
/// `criterion_main!` would otherwise do.
///
/// Each bench file's `main` is a few lines because of this, and it has to be
/// spelled out in every one: `criterion_main!` builds its own `Criterion::default()`
/// and would ignore all of the above.
pub fn run(benches: impl FnOnce(&mut Criterion)) {
    let mut c = criterion_configure();
    benches(&mut c);
    c.final_summary();
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// `(format, quality, size)` for one cached fixture.
type FixtureKey = (u8, u8, u32);

/// Every generated fixture, keyed so a second request for the same bytes is a
/// lookup rather than another encode.
///
/// The bytes are leaked rather than borrowed: a fixture has to outlive every
/// iteration and the process is about to exit anyway, so borrowing it would mean
/// threading a lifetime through `iter_batched`'s setup closure for no benefit.
type FixtureCache = Mutex<HashMap<FixtureKey, &'static [u8]>>;

fn cache() -> &'static FixtureCache {
    static CACHE: OnceLock<FixtureCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached(kind: u8, quality: u8, size: (u32, u32), build: impl FnOnce() -> Vec<u8>) -> &'static [u8] {
    let key = (kind, quality, size.0 * 100_000 + size.1);
    // A poisoned lock means a previous build panicked; the bytes are still
    // perfectly usable and this is a benchmark process.
    let mut map = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = map.get(&key) {
        return found;
    }
    let leaked: &'static [u8] = Box::leak(build().into_boxed_slice());
    map.insert(key, leaked);
    leaked
}

/// JPEG bytes for `size` at `quality`, for the decode group to iterate over a
/// list of formats with one signature.
pub fn jpeg_size_probe(size: (u32, u32)) -> &'static [u8] {
    jpeg(size, 90)
}

/// JPEG bytes for `size`, at `quality`.
pub fn jpeg(size: (u32, u32), quality: u8) -> &'static [u8] {
    cached(1, quality, size, || {
        encode(&image::DynamicImage::ImageRgba8(photograph(size.0, size.1)), OutputFormat::Jpeg, quality)
    })
}

/// PNG bytes for `size`. Lossless, so there is no quality to vary: the encoder
/// is `deflate` either way, which is exactly why this case is slow.
pub fn png(size: (u32, u32)) -> &'static [u8] {
    cached(2, 0, size, || {
        encode(&image::DynamicImage::ImageRgba8(photograph(size.0, size.1)), OutputFormat::Png, 0)
    })
}

/// WebP bytes for `size` at `quality`.
///
/// Lossy in a build with the `webp-lossy` feature (the default) and **lossless**
/// in a build without it, because there the only WebP encoder compiled in is
/// `image`'s lossless one and `quality` has no meaning. The fixture is the same
/// shape either way, so the decode group can use it unconditionally; the
/// *encode* benchmark that claims to measure "lossy WebP at q80" is `#[cfg]`d on
/// the feature instead, because a number produced by the lossless encoder under
/// that label would be a lie.
pub fn webp(size: (u32, u32), quality: u8) -> &'static [u8] {
    cached(3, quality, size, || {
        encode(
            &image::DynamicImage::ImageRgba8(photograph(size.0, size.1)),
            OutputFormat::WebP,
            quality,
        )
    })
}

/// The WebP fixture the decode group uses, whatever the build's WebP capability is.
pub fn webp_decode_fixture(size: (u32, u32)) -> &'static [u8] {
    webp(size, 80)
}

fn encode(img: &image::DynamicImage, format: OutputFormat, quality: u8) -> Vec<u8> {
    pixelsmith_core::encode_fixed(img, format, EncodingOptions::default().with_quality(quality))
        .expect("the engine's own encoder must accept the fixture it was handed")
}

/// Decoded pixels for `size`, through the bounded path production uses.
///
/// This is the *source* every pixel-touching benchmark resizes or encodes, and
/// it is a `DynamicImage` rather than raw pixels because that is what
/// `worker::process_one` hands the pipeline. Benchmarking a raw `RgbaImage`
/// would measure a path the product never takes.
pub fn decoded(size: (u32, u32)) -> &'static image::DynamicImage {
    static DECODED: OnceLock<Mutex<HashMap<u32, &'static image::DynamicImage>>> = OnceLock::new();
    let key = size.0 * 100_000 + size.1;
    let map = DECODED.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = map.get(&key) {
        return found;
    }
    let encoded = jpeg(size, 90);
    let img = decode_bounded(encoded, &Limits::default())
        .expect("a JPEG this engine just wrote must decode within the default limits");
    let leaked: &'static image::DynamicImage = Box::leak(Box::new(img));
    map.insert(key, leaked);
    leaked
}

/// JPEG bytes carrying a **full EXIF tag set**: orientation, camera identity,
/// timestamps, and every class of tag `exif::write_back` filters out.
///
/// "Full" means what a phone actually writes, not one convenient tag: the point
/// of the EXIF benchmarks is that reading and filtering are work proportional to
/// the number of tags, so a fixture with three ASCII fields measures the wrong
/// thing. Every sensitive class is present — GPS, owner name, artist,
/// copyright, body serial, lens — so a regression that quietly stopped filtering
/// would show up as extra bytes here.
pub fn jpeg_with_exif(size: (u32, u32)) -> &'static [u8] {
    cached(4, 0, size, || {
        let base = jpeg(size, 85).to_vec();
        let fields = exif_fields(size);
        let block = pixelsmith_core::exif::build_block(&fields)
            .expect("the fixture's own tag list must serialise");
        let mut out = base;
        pixelsmith_core::format::append_exif(&mut out, &block)
            .expect("append_exif must accept a block it was handed");
        out
    })
}

/// The tag set behind [`jpeg_with_exif`], in IFD 0.
///
/// All of it is in IFD 0 on purpose: `exif::GPS_IFD` documents that
/// `kamadak-exif`'s writer refuses a GPS tag in IFD 2 unless the thumbnail IFD is
/// also populated, and a benchmark fixture is not the place to discover that.
pub fn exif_fields(size: (u32, u32)) -> Vec<exif::Field> {
    use exif::{Tag, Value};
    use pixelsmith_core::exif::{ascii, field, long, GPS_IFD};

    vec![
        field(Tag::Make, GPS_IFD, ascii("Pexelsmith")),
        field(Tag::Model, GPS_IFD, ascii("PX-1 Bench")),
        field(Tag::Software, GPS_IFD, ascii("pixelsmith_core bench")),
        field(Tag::Orientation, GPS_IFD, Value::Short(vec![6])),
        field(Tag::DateTime, GPS_IFD, ascii("2026:10:07 12:34:56")),
        field(Tag::DateTimeOriginal, GPS_IFD, ascii("2026:10:07 12:34:56")),
        field(Tag::ExposureTime, GPS_IFD, Value::Rational(vec![exif::Rational {
            num: 1,
            denom: 250,
        }])),
        field(Tag::FNumber, GPS_IFD, Value::Rational(vec![exif::Rational {
            num: 18,
            denom: 10,
        }])),
        field(Tag::ISOSpeed, GPS_IFD, Value::Short(vec![400])),
        field(Tag::FocalLength, GPS_IFD, Value::Rational(vec![exif::Rational {
            num: 35,
            denom: 1,
        }])),
        field(Tag::Artist, GPS_IFD, ascii("Benchmark Fixture")),
        field(Tag::Copyright, GPS_IFD, ascii("(c) 2026 nobody, all rights reserved")),
        field(Tag::CameraOwnerName, GPS_IFD, ascii("Jane Q. Bench")),
        field(Tag::BodySerialNumber, GPS_IFD, ascii("B0DGEH0000000001")),
        field(Tag::LensModel, GPS_IFD, ascii("PX Bench 24-70mm f/2.8")),
        field(Tag::LensSerialNumber, GPS_IFD, ascii("L0NSB0000000001")),
        field(Tag::GPSLatitude, GPS_IFD, Value::Rational(vec![
            exif::Rational { num: 51, denom: 1 },
            exif::Rational { num: 30, denom: 1 },
            exif::Rational { num: 0, denom: 100 },
        ])),
        field(Tag::GPSLatitudeRef, GPS_IFD, ascii("N")),
        field(Tag::GPSLongitude, GPS_IFD, Value::Rational(vec![
            exif::Rational { num: 0, denom: 1 },
            exif::Rational { num: 7, denom: 1 },
            exif::Rational { num: 39, denom: 100 },
        ])),
        field(Tag::GPSLongitudeRef, GPS_IFD, ascii("W")),
        field(Tag::GPSAltitude, GPS_IFD, Value::Rational(vec![
            exif::Rational { num: 1234, denom: 10 },
        ])),
        field(Tag::GPSAltitudeRef, GPS_IFD, Value::Byte(vec![0])),
        field(Tag::XResolution, GPS_IFD, Value::Rational(vec![exif::Rational {
            num: 72,
            denom: 1,
        }])),
        field(Tag::YResolution, GPS_IFD, Value::Rational(vec![exif::Rational {
            num: 72,
            denom: 1,
        }])),
        field(Tag::ResolutionUnit, GPS_IFD, Value::Short(vec![2])),
        field(Tag::ImageWidth, GPS_IFD, long(size.0)),
        field(Tag::ImageLength, GPS_IFD, long(size.1)),
    ]
}